//! Painel do dono: `/panel` é o app React (`panel/`, montado pelo Vite e embutido no executável), que
//! pergunta a sessão em `/panel/api/me`; `/panel/data` é o resumo em JSON e `/panel/events` avisa de pacote novo.
//! Contas por convite (`accounts`); sessão num cookie `__Host-` que o JavaScript não lê.
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use futures_util::Stream;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tokio::sync::broadcast::error::RecvError;

use crate::{Live, accounts, store};

/// O build do Vite: `index.html`, os arquivos de `public/` e `assets/` com o hash do conteúdo no nome.
#[derive(RustEmbed)]
#[folder = "panel/dist"]
// Na depuração a pasta pode faltar, e o teste da página diz como montar; no release (imagem) ela é obrigatória.
#[cfg_attr(debug_assertions, allow_missing = true)]
struct Dist;

/// Arquivos da raiz do app, com endereço fixo: revalidados a cada uso.
const ROOT_FILES: [&str; 6] = ["manifest.webmanifest", "sw.js", "icon.svg", "icon-192.png", "icon-512.png", "apple-touch-icon.png"];
/// Um canal ao vivo dura no máximo isto; o navegador reconecta sozinho e a sessão é conferida de novo.
const EVENTS_MAX_LIFE: Duration = Duration::from_secs(15 * 60);
/// Intervalo do `ping` nomeado (e não comentário): o painel vê o silêncio e refaz a conexão meio aberta.
const EVENTS_KEEP_ALIVE: Duration = Duration::from_secs(20);
const COOKIE: &str = "__Host-panel";
const DEFAULT_DAYS: i32 = 30;
/// Espera de toda entrada recusada: tentar senha em série fica lento sem atrasar quem acerta.
const REFUSAL_DELAY: Duration = Duration::from_millis(400);
/// argon2 ao mesmo tempo no servidor; acima disso o pedido espera até `ARGON_WAIT` e então volta 429.
const ARGON_SLOTS: usize = 4;
const ARGON_WAIT: Duration = Duration::from_secs(5);
/// Erros seguidos que travam um e-mail, e por quanto tempo.
const MAX_FAILURES: u32 = 10;
const LOCK_SECS: u64 = 15 * 60;

/// Freio da força bruta: poucos argon2 de cada vez e trava por e-mail depois de erros seguidos.
/// Por IP não serve: a origem é alcançável sem a Cloudflare, e o cabeçalho de IP seria de quem manda.
struct Limiter {
    argon: Semaphore,
    failures: Mutex<HashMap<String, (u32, u64)>>,
    started: Instant,
}

impl Limiter {
    fn new() -> Limiter { Limiter { argon: Semaphore::new(ARGON_SLOTS), failures: Mutex::default(), started: Instant::now() } }

    fn now(&self) -> u64 { self.started.elapsed().as_secs() }

    fn locked(&self, email: &str, now: u64) -> bool {
        let failures = self.failures.lock().unwrap();
        failures.get(email).is_some_and(|(count, last)| *count >= MAX_FAILURES && now.saturating_sub(*last) < LOCK_SECS)
    }

    fn failed(&self, email: &str, now: u64) {
        let mut failures = self.failures.lock().unwrap();
        failures.retain(|_, (_, last)| now.saturating_sub(*last) < LOCK_SECS);
        let entry = failures.entry(email.to_owned()).or_insert((0, now));
        *entry = (entry.0 + 1, now);
    }

    fn succeeded(&self, email: &str) { self.failures.lock().unwrap().remove(email); }
}

#[derive(Clone)]
struct Panel {
    pool: PgPool,
    limiter: Arc<Limiter>,
    live: Arc<Live>,
}

/// Só roda script servido por esta origem, nunca inline. O estilo inline fica: o gráfico escreve as
/// cores das séries num `<style>` e os componentes usam `style`.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; \
    connect-src 'self'; manifest-src 'self'; worker-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

#[derive(Deserialize)]
struct Period {
    days: Option<i32>,
}

/// Opcionais para a sessão ser conferida antes: sem login é 401 mesmo com o pedido torto.
#[derive(Deserialize)]
struct TrailQuery {
    machine: Option<String>,
    day: Option<String>,
}

#[derive(Deserialize)]
struct Login {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct Join {
    invite: String,
    email: String,
    password: String,
}

pub fn router(pool: PgPool, live: Arc<Live>) -> Router {
    accounts::warm();
    let state = Panel { pool, limiter: Arc::new(Limiter::new()), live };
    let mut router = Router::new()
        .route("/", get(page))
        .route("/join", get(page))
        .route("/panel", get(to_root))
        .route("/panel/", get(to_root))
        .route("/panel/join", get(to_join))
        .route("/panel/assets/{*file}", get(hashed_asset));
    for name in ROOT_FILES {
        router = router.route(&format!("/panel/{name}"), get(move || std::future::ready(root_file(name))));
    }
    router
        .route("/panel/events", get(events))
        .route("/panel/data", get(data))
        .route("/panel/trail", get(trail))
        .route("/panel/api/me", get(me))
        .route("/panel/api/login", post(login))
        .route("/panel/api/logout", post(logout))
        .route("/panel/api/invites", post(invite))
        .route("/panel/api/join", post(join))
        .with_state(state)
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("webmanifest") => "application/manifest+json",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn embedded(path: &str, cache: &'static str) -> Response {
    match Dist::get(path) {
        Some(file) => ([(header::CONTENT_TYPE, content_type(path)), (header::CACHE_CONTROL, cache)], file.data).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// O app para `/` e `/join`, com ou sem sessão: quem decide a tela é `/panel/api/me`, e os dados
/// continuam atrás da sessão. Nunca guardado, para a versão nova entrar no recarregar.
async fn page() -> Response {
    let mut response = embedded("index.html", "no-store");
    // A página não roda dentro de outro site (clickjacking) e não manda o caminho a links de fora.
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

/// Endereço antigo (link guardado, app instalado com `start_url` /panel): o painel mora na raiz.
async fn to_root() -> Redirect { Redirect::permanent("/") }

/// O convite leva o token no fragmento, que o navegador mantém no redirecionamento; a consulta vai junto.
async fn to_join(RawQuery(query): RawQuery) -> Redirect {
    Redirect::permanent(&query.map_or_else(|| "/join".to_owned(), |q| format!("/join?{q}")))
}

/// Nome com o hash do conteúdo: o mesmo endereço nunca muda, então fica guardado de vez.
async fn hashed_asset(Path(file): Path<String>) -> Response {
    // Sem subir de pasta: no build de depuração o rust-embed lê do disco.
    if file.split('/').any(|part| part.is_empty() || part == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    embedded(&format!("assets/{file}"), "public, max-age=31536000, immutable")
}

fn root_file(name: &'static str) -> Response {
    let mut response = embedded(name, "no-cache");
    // O painel mora na raiz, fora do diretório do script.
    if name == "sw.js" {
        response.headers_mut().insert("service-worker-allowed", HeaderValue::from_static("/"));
    }
    response
}

/// Um aviso `batch` por pacote gravado; sem dado nenhum, o painel relê `/panel/data`. Acaba no desligamento ou
/// depois de `max_life`, e o navegador reconecta conferindo a sessão de novo.
fn batch_events(live: &Live, max_life: Duration) -> impl Stream<Item = Result<Event, Infallible>> + use<> {
    let state = (live.notify.subscribe(), live.stop.subscribe(), tokio::time::Instant::now() + max_life);
    futures_util::stream::unfold(state, |(mut batches, mut stop, end)| async move {
        let arrived = tokio::select! {
            // Atrasado também é "chegou coisa nova": um aviso basta para reler.
            got = batches.recv() => !matches!(got, Err(RecvError::Closed)),
            _ = stop.wait_for(|stopping| *stopping) => false,
            () = tokio::time::sleep_until(end) => false,
        };
        arrived.then(|| (Ok(Event::default().event("batch").data("{}")), (batches, stop, end)))
    })
}

async fn events(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    match user(&panel.pool, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    }
    let stream = batch_events(&panel.live, EVENTS_MAX_LIFE);
    // Proxy no meio (Cloudflare, Traefik) não pode juntar os avisos esperando o corpo encher.
    let mut response = Sse::new(stream).keep_alive(KeepAlive::new().interval(EVENTS_KEEP_ALIVE).event(Event::default().event("ping"))).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn error(status: StatusCode, code: &str) -> Response { (status, axum::Json(json!({"error": code}))).into_response() }

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE && value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then(|| value.to_owned())
    })
}

fn with_session(mut response: Response, token: &str) -> Response {
    let max_age = accounts::SESSION_DAYS * 24 * 60 * 60;
    let value = format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}");
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&value).expect("cookie em ASCII"));
    response
}

fn without_session(mut response: Response) -> Response {
    let value = format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0");
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&value).expect("cookie em ASCII"));
    response
}

/// Pedido que muda estado só vale vindo desta mesma origem (o `SameSite=Strict` já barra o cookie; isto barra o resto).
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
    origin.split_once("://").is_some_and(|(_, rest)| rest == host)
}

async fn user(pool: &PgPool, headers: &HeaderMap) -> Result<Option<(i64, String)>, Response> {
    let Some(token) = cookie_token(headers) else { return Ok(None) };
    accounts::session_user(pool, &token).await.map_err(|e| unavailable("session", &e))
}

fn unavailable(what: &str, cause: &sqlx::Error) -> Response {
    eprintln!("telemetry-ingest: panel {what}: {cause}");
    error(StatusCode::SERVICE_UNAVAILABLE, "store")
}

async fn me(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    match user(&panel.pool, &headers).await {
        Ok(Some((_, email))) => axum::Json(json!({"email": email})).into_response(),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => response,
    }
}

// A consulta torta só é recusada depois da sessão: sem ela, 401 não diz o formato da rota.
async fn data(State(panel): State<Panel>, headers: HeaderMap, period: Result<Query<Period>, QueryRejection>) -> Response {
    let pool = &panel.pool;
    match user(pool, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    }
    let Ok(Query(period)) = period else { return error(StatusCode::BAD_REQUEST, "query") };
    let days = period.days.unwrap_or(DEFAULT_DAYS).clamp(1, 400);
    match store::summary(pool, days).await {
        Ok(summary) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(summary)).into_response(),
        Err(e) => unavailable("summary", &e),
    }
}

async fn trail(State(panel): State<Panel>, headers: HeaderMap, query: Result<Query<TrailQuery>, QueryRejection>) -> Response {
    let pool = &panel.pool;
    match user(pool, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    }
    let Ok(Query(query)) = query else { return error(StatusCode::BAD_REQUEST, "query") };
    let is_hex_id = |v: &str| v.len() == 32 && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let is_day = |v: &str| v.len() == 10 && v.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    let (Some(machine), Some(day)) = (query.machine.filter(|m| is_hex_id(m)), query.day.filter(|d| is_day(d))) else {
        return error(StatusCode::BAD_REQUEST, "query");
    };
    match store::trail(pool, &machine, &day).await {
        Ok(steps) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(steps)).into_response(),
        Err(e) if store::is_bad_value(&e) => error(StatusCode::BAD_REQUEST, "day"),
        Err(e) => unavailable("trail", &e),
    }
}

async fn login(State(panel): State<Panel>, headers: HeaderMap, body: axum::Json<Login>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    let (Some(email), true) = (accounts::normalize_email(&body.email), body.password.chars().count() <= accounts::MAX_PASSWORD) else {
        tokio::time::sleep(REFUSAL_DELAY).await;
        return error(StatusCode::UNAUTHORIZED, "login");
    };
    let limiter = &panel.limiter;
    if limiter.locked(&email, limiter.now()) {
        return error(StatusCode::TOO_MANY_REQUESTS, "too_many");
    }
    let Ok(Ok(_slot)) = tokio::time::timeout(ARGON_WAIT, limiter.argon.acquire()).await else {
        return error(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    // De novo depois da fila: outro pedido do mesmo e-mail pode ter completado os erros enquanto este esperava.
    if limiter.locked(&email, limiter.now()) {
        return error(StatusCode::TOO_MANY_REQUESTS, "too_many");
    }
    match accounts::login(&panel.pool, &email, &body.password).await {
        Ok(Some((id, hash))) => {
            limiter.succeeded(&email);
            match accounts::create_session(&panel.pool, id, &hash).await {
                Ok(Some(token)) => with_session(StatusCode::NO_CONTENT.into_response(), &token),
                Ok(None) => error(StatusCode::UNAUTHORIZED, "login"),
                Err(e) => unavailable("session", &e),
            }
        }
        Ok(None) => {
            limiter.failed(&email, limiter.now());
            tokio::time::sleep(REFUSAL_DELAY).await;
            error(StatusCode::UNAUTHORIZED, "login")
        }
        Err(e) => unavailable("login", &e),
    }
}

async fn logout(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    if let Some(token) = cookie_token(&headers)
        && let Err(e) = accounts::end_session(&panel.pool, &token).await
    {
        return unavailable("logout", &e);
    }
    without_session(StatusCode::NO_CONTENT.into_response())
}

async fn invite(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    let id = match user(&panel.pool, &headers).await {
        Ok(Some((id, _))) => id,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    };
    match accounts::create_invite(&panel.pool, id).await {
        Ok(token) => axum::Json(json!({"path": format!("/join#{token}"), "days": accounts::INVITE_DAYS})).into_response(),
        Err(e) => unavailable("invite", &e),
    }
}

async fn join(State(panel): State<Panel>, headers: HeaderMap, body: axum::Json<Join>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    let Some(email) = accounts::normalize_email(&body.email) else { return error(StatusCode::BAD_REQUEST, "email") };
    if !accounts::password_fits(&body.password) {
        return error(StatusCode::BAD_REQUEST, "password");
    }
    let invite = body.invite.trim();
    match accounts::invite_open(&panel.pool, invite).await {
        Ok(true) => {}
        Ok(false) => {
            tokio::time::sleep(REFUSAL_DELAY).await;
            return error(StatusCode::BAD_REQUEST, "invite");
        }
        Err(e) => return unavailable("invite", &e),
    }
    let hash = {
        let Ok(Ok(_slot)) = tokio::time::timeout(ARGON_WAIT, panel.limiter.argon.acquire()).await else {
            return error(StatusCode::TOO_MANY_REQUESTS, "busy");
        };
        accounts::hash_off_thread(body.password.clone()).await
    };
    match accounts::join(&panel.pool, invite, &email, &hash).await {
        Ok(accounts::Join::Created(id)) => match accounts::create_session(&panel.pool, id, &hash).await {
            Ok(Some(token)) => with_session(StatusCode::NO_CONTENT.into_response(), &token),
            Ok(None) => error(StatusCode::CONFLICT, "changed"),
            Err(e) => unavailable("session", &e),
        },
        // Gasto entre a conferência e a gravação por outro pedido com o mesmo convite.
        Ok(accounts::Join::BadInvite) => error(StatusCode::BAD_REQUEST, "invite"),
        Ok(accounts::Join::EmailTaken) => error(StatusCode::CONFLICT, "email_taken"),
        Err(e) => unavailable("join", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        map
    }

    #[test]
    fn session_cookie_is_read_only_in_its_own_shape() {
        let token = "a".repeat(64);
        assert_eq!(cookie_token(&headers(&[("cookie", &format!("x=1; {COOKIE}={token}"))])), Some(token.clone()));
        assert_eq!(cookie_token(&headers(&[("cookie", &format!("{COOKIE}=curto"))])), None);
        assert_eq!(cookie_token(&headers(&[("cookie", &format!("outro={token}"))])), None);
        assert_eq!(cookie_token(&headers(&[])), None);
    }

    #[test]
    fn ten_failures_lock_the_email_for_fifteen_minutes() {
        let limiter = Limiter::new();
        for _ in 0..MAX_FAILURES - 1 {
            limiter.failed("a@x.com", 100);
        }
        assert!(!limiter.locked("a@x.com", 100));
        limiter.failed("a@x.com", 100);
        assert!(limiter.locked("a@x.com", 100));
        assert!(!limiter.locked("b@x.com", 100), "a trava é por e-mail");
        assert!(limiter.locked("a@x.com", 100 + LOCK_SECS - 1));
        assert!(!limiter.locked("a@x.com", 100 + LOCK_SECS), "a trava vence sozinha");
        limiter.failed("c@x.com", 5);
        limiter.succeeded("c@x.com");
        assert!(limiter.failures.lock().unwrap().get("c@x.com").is_none(), "acerto zera a conta");
    }

    #[test]
    fn the_app_page_runs_only_its_own_scripts() {
        let html = String::from_utf8(Dist::get("index.html").expect("panel/dist: rode npm run build em panel/").data.into_owned()).unwrap();
        assert!(html.contains(r#"<div id="root">"#) && html.contains(r#"src="/panel/assets/"#), "{html}");
        assert!(!html.contains("<script>"), "nenhum script inline");
        assert!(html.contains(r#"<link rel="manifest" href="/panel/manifest.webmanifest">"#) && html.contains("viewport-fit=cover"));
        let script = CSP.split(';').find(|d| d.trim_start().starts_with("script-src")).unwrap();
        assert_eq!(script.trim(), "script-src 'self'");
        assert!(CSP.contains("frame-ancestors 'none'") && CSP.contains("font-src 'self'") && !CSP.contains("unsafe-eval"));
        for name in ROOT_FILES {
            assert!(Dist::get(name).is_some(), "{name} no build");
        }
        let sw = String::from_utf8(Dist::get("sw.js").unwrap().data.into_owned()).unwrap();
        assert!(sw.contains("fetch(e.request)") && !sw.contains("caches"), "o service worker não guarda nada");
    }

    #[tokio::test]
    async fn each_batch_is_one_event_and_the_stream_ends_on_shutdown() {
        use futures_util::StreamExt;
        let live = Live::new();
        let mut stream = std::pin::pin!(batch_events(&live, Duration::from_secs(60)));
        live.notify.send(()).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap();
        assert!(matches!(event, Some(Ok(_))), "pacote novo vira um aviso");
        live.stop.send(true).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().is_none(), "desligar encerra o canal");

        let fresh = Live::new();
        let mut short = std::pin::pin!(batch_events(&fresh, Duration::ZERO));
        assert!(tokio::time::timeout(Duration::from_secs(5), short.next()).await.unwrap().is_none(), "o canal tem prazo");
    }

    #[test]
    fn writes_only_from_the_same_origin() {
        assert!(same_origin(&headers(&[("host", "telemetry.hangar.dev.br")])));
        assert!(same_origin(&headers(&[("host", "telemetry.hangar.dev.br"), ("origin", "https://telemetry.hangar.dev.br")])));
        assert!(!same_origin(&headers(&[("host", "telemetry.hangar.dev.br"), ("origin", "https://evil.example")])));
    }
}
