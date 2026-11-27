//! Painel do dono: `/panel` mostra a entrada ou o painel conforme a sessão; `/panel/data` é o resumo em JSON.
//! Contas por convite (`accounts`); sessão num cookie `__Host-` que o JavaScript não lê.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tokio::sync::Semaphore;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use sha2::Digest;

use crate::{accounts, store};

const DASHBOARD: &str = include_str!("panel/dashboard.html");
const LOGIN: &str = include_str!("panel/login.html");
const JOIN: &str = include_str!("panel/join.html");
const STYLE: &str = include_str!("panel/panel.css");
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
}

/// Só os scripts da própria página rodam, conferidos pelo hash; o resto do conteúdo vem só desta origem.
fn csp(html: &str) -> String {
    let hashes: Vec<String> = html
        .split("<script>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</script>").map(|(body, _)| body))
        .map(|body| format!("'sha256-{}'", STANDARD.encode(sha2::Sha256::digest(body.as_bytes()))))
        .collect();
    format!(
        "default-src 'none'; script-src {}; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; \
         form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        hashes.join(" ")
    )
}

static CSP: LazyLock<[String; 3]> = LazyLock::new(|| [csp(DASHBOARD), csp(LOGIN), csp(JOIN)]);

#[derive(Deserialize)]
struct Period {
    days: Option<i32>,
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

pub fn router(pool: PgPool) -> Router {
    accounts::warm();
    let state = Panel { pool, limiter: Arc::new(Limiter::new()) };
    Router::new()
        .route("/panel", get(home))
        .route("/panel/join", get(|| async { page(JOIN, 2) }))
        .route("/panel/assets/panel.css", get(|| async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE) }))
        .route("/panel/data", get(data))
        .route("/panel/api/me", get(me))
        .route("/panel/api/login", post(login))
        .route("/panel/api/logout", post(logout))
        .route("/panel/api/invites", post(invite))
        .route("/panel/api/join", post(join))
        .with_state(state)
}

fn page(html: &'static str, csp: usize) -> Response {
    // A página não roda dentro de outro site (clickjacking) e não manda o caminho a links de fora.
    let mut response = Html(html).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&CSP[csp]).expect("CSP em ASCII"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

async fn home(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    match user(&panel.pool, &headers).await {
        Ok(Some(_)) => page(DASHBOARD, 0),
        Ok(None) => page(LOGIN, 1),
        Err(response) => response,
    }
}

async fn me(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    match user(&panel.pool, &headers).await {
        Ok(Some((_, email))) => axum::Json(json!({"email": email})).into_response(),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => response,
    }
}

async fn data(State(panel): State<Panel>, headers: HeaderMap, Query(period): Query<Period>) -> Response {
    let pool = &panel.pool;
    match user(pool, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    }
    let days = period.days.unwrap_or(DEFAULT_DAYS).clamp(1, 400);
    match store::summary(pool, days).await {
        Ok(summary) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(summary)).into_response(),
        Err(e) => unavailable("summary", &e),
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
        Ok(token) => axum::Json(json!({"path": format!("/panel/join#{token}"), "days": accounts::INVITE_DAYS})).into_response(),
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
    fn csp_allows_only_the_page_scripts_by_hash() {
        let policy = csp("<p>x</p><script>alert(1)</script>");
        assert!(policy.contains("script-src 'sha256-"), "{policy}");
        assert!(policy.contains("frame-ancestors 'none'") && !policy.contains("unsafe-eval"));
        assert_eq!(csp(DASHBOARD).matches("'sha256-").count(), DASHBOARD.matches("<script>").count());
    }

    #[test]
    fn writes_only_from_the_same_origin() {
        assert!(same_origin(&headers(&[("host", "telemetry.hangar.dev.br")])));
        assert!(same_origin(&headers(&[("host", "telemetry.hangar.dev.br"), ("origin", "https://telemetry.hangar.dev.br")])));
        assert!(!same_origin(&headers(&[("host", "telemetry.hangar.dev.br"), ("origin", "https://evil.example")])));
    }
}
