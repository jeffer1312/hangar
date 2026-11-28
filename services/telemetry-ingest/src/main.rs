//! Receptor da telemetria: `POST /v1/batch` grava o pacote diário de cada máquina no Postgres;
//! `POST /v1/declined` conta o aviso único de quem recusou, sem identificação.
//! Não lê IP nem cabeçalho de origem; o limite de pedidos fica com a Cloudflare.
mod accounts;
mod panel;
mod store;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use hangar_telemetry::wire::{Batch, Refusal};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::signal::unix::{SignalKind, signal};

// Os 200 registros de falha de até 40 campos e os 5000 passos do dia cabem com folga junto dos contadores.
const MAX_BODY: usize = 2 * 1024 * 1024;
/// O aviso de recusa são quatro campos curtos.
const MAX_REFUSAL_BODY: usize = 1024;

/// Aviso em processo de pacote gravado, para o painel aberto reler; e o sinal de desligar, que fecha esses canais.
pub struct Live {
    notify: tokio::sync::broadcast::Sender<()>,
    stop: tokio::sync::watch::Sender<bool>,
}

impl Live {
    fn new() -> Live {
        Live { notify: tokio::sync::broadcast::channel(16).0, stop: tokio::sync::watch::channel(false).0 }
    }
}

#[derive(Clone)]
struct Ingest {
    pool: PgPool,
    live: Arc<Live>,
}

fn app(pool: PgPool, live: Arc<Live>) -> Router {
    Router::new()
        .route("/v1/batch", post(post_batch))
        .route("/v1/declined", post(post_declined).layer(DefaultBodyLimit::max(MAX_REFUSAL_BODY)))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(Ingest { pool, live })
}

fn app_with_panel(pool: PgPool, live: Arc<Live>) -> Router {
    app(pool.clone(), live.clone()).merge(panel::router(pool, live))
}

fn reply(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn refused(status: StatusCode, field: &str) -> Response {
    reply(status, format!(r#"{{"ok":false,"error":"{field}"}}"#))
}

/// O corpo vem como bytes para que pacote fora da forma seja 400, e não o 422 do `Json` do axum.
async fn post_batch(State(ingest): State<Ingest>, body: Bytes) -> Response {
    let Ok(batch) = serde_json::from_slice::<Batch>(&body) else {
        return refused(StatusCode::BAD_REQUEST, "json");
    };
    if let Err(field) = batch.validate() {
        return refused(StatusCode::BAD_REQUEST, field);
    }
    match store::insert(&ingest.pool, &batch).await {
        Ok(store::Insert::Accepted) => {
            // Sem painel aberto não há quem ouça, e o envio só falha por isso.
            let _ = ingest.live.notify.send(());
            reply(StatusCode::OK, r#"{"ok":true}"#.to_owned())
        }
        Ok(store::Insert::DayAhead) => refused(StatusCode::BAD_REQUEST, "day"),
        Err(error) if store::is_bad_value(&error) => refused(StatusCode::BAD_REQUEST, "value"),
        Err(error) => {
            eprintln!("telemetry-ingest: insert: {error}");
            refused(StatusCode::SERVICE_UNAVAILABLE, "store")
        }
    }
}

/// Campo a mais (um identificador) ou tipo fora da lista é 400, como no pacote.
async fn post_declined(State(ingest): State<Ingest>, body: Bytes) -> Response {
    let Ok(refusal) = serde_json::from_slice::<Refusal>(&body) else {
        return refused(StatusCode::BAD_REQUEST, "json");
    };
    if let Err(field) = refusal.validate() {
        return refused(StatusCode::BAD_REQUEST, field);
    }
    match store::count_refusal(&ingest.pool, &refusal).await {
        Ok(()) => {
            // O cartão de aceite do painel aberto relê na hora.
            let _ = ingest.live.notify.send(());
            reply(StatusCode::OK, r#"{"ok":true}"#.to_owned())
        }
        Err(error) => {
            eprintln!("telemetry-ingest: refusal: {error}");
            refused(StatusCode::SERVICE_UNAVAILABLE, "store")
        }
    }
}

async fn retention(pool: PgPool) {
    let mut daily = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    loop {
        daily.tick().await;
        match store::prune(&pool).await {
            Ok(removed) => eprintln!("telemetry-ingest: retention removed {removed} batches"),
            Err(error) => eprintln!("telemetry-ingest: retention: {error}"),
        }
        if let Err(error) = store::prune_refusals(&pool).await {
            eprintln!("telemetry-ingest: retention (refusals): {error}");
        }
        if let Err(error) = accounts::prune(&pool).await {
            eprintln!("telemetry-ingest: retention (panel): {error}");
        }
    }
}

/// `telemetry-ingest set-user <email>` com a senha na entrada padrão: cria a conta ou troca a senha dela.
/// A senha nunca vai na linha de comando, que aparece na lista de processos.
async fn set_user(pool: &PgPool, email: &str) -> Result<(), Box<dyn std::error::Error>> {
    let email = accounts::normalize_email(email).ok_or("e-mail inválido")?;
    let mut password = String::new();
    std::io::stdin().read_line(&mut password)?;
    let password = password.trim_end_matches(['\r', '\n']).to_owned();
    if !accounts::password_fits(&password) {
        return Err(format!("a senha precisa ter de {} a {} caracteres", accounts::MIN_PASSWORD, accounts::MAX_PASSWORD).into());
    }
    accounts::set_user(pool, &email, &accounts::hash_off_thread(password).await).await?;
    eprintln!("telemetry-ingest: conta {email} pronta");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let listen = std::env::var("LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let pool = PgPoolOptions::new().max_connections(5).connect(&database_url).await?;
    store::migrate(&pool).await?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, email] if command == "set-user" => return set_user(&pool, email).await,
        [command, ..] if command == "set-user" => return Err("uso: telemetry-ingest set-user <email> (senha na entrada padrão)".into()),
        [] => {}
        _ => return Err(format!("argumento desconhecido: {}", args.join(" ")).into()),
    }
    tokio::spawn(retention(pool.clone()));
    // Instalado antes de ouvir: falha aqui derruba a subida, não o desligamento.
    let mut terminate = signal(SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("telemetry-ingest: listening on {listen}");
    // O pedido em curso termina de gravar antes de o processo sair.
    let live = Arc::new(Live::new());
    let closing = live.clone();
    // Os canais ao vivo do painel ficariam abertos para sempre: o desligamento os fecha antes de esperar os pedidos.
    let stop = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        // `send_replace` grava mesmo sem ninguém ouvindo: um canal que assina depois já nasce fechado.
        closing.stop.send_replace(true);
    };
    axum::serve(listener, app_with_panel(pool, live)).with_graceful_shutdown(stop).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::hash::{BuildHasher, RandomState};

    use axum::body::Body;
    use axum::http::{HeaderMap, Request};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    fn app(pool: PgPool) -> Router { super::app(pool, Arc::new(Live::new())) }

    fn app_with_panel(pool: PgPool) -> Router { super::app_with_panel(pool, Arc::new(Live::new())) }

    /// Pool que nunca conecta: os pedidos recusados antes do banco não precisam dele.
    fn offline() -> PgPool {
        PgPoolOptions::new().connect_lazy("postgres://telemetry@127.0.0.1:1/unused").unwrap()
    }

    async fn online() -> PgPool {
        let url = std::env::var("TELEMETRY_TEST_DATABASE_URL").expect("TELEMETRY_TEST_DATABASE_URL");
        let pool = PgPoolOptions::new().connect(&url).await.unwrap();
        store::migrate(&pool).await.unwrap();
        pool
    }

    /// Hoje em UTC mais `offset` dias, pelo relógio do banco que confere o dia.
    async fn utc_day(pool: &PgPool, offset: i32) -> String {
        sqlx::query_scalar("SELECT to_char((now() AT TIME ZONE 'utc')::date + $1, 'YYYY-MM-DD')")
            .bind(offset)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Id aleatório por chamada, para os testes dividirem o mesmo banco entre rodadas sem colidir.
    fn fresh_id() -> String {
        let random = || RandomState::new().hash_one(0u8);
        format!("{:016x}{:016x}", random(), random())
    }

    fn batch_body(batch_id: &str, day: &str) -> Value {
        json!({
            "schema": 1,
            "batch_id": batch_id,
            "machine_id": fresh_id(),
            "owner_id": fresh_id(),
            "day": day,
            "env": {"version": "0.42.0", "commit": "dev", "os": "linux", "arch": "x86_64", "channel": "latest"},
            "counters": [
                {"event": "future_event", "dims": {"new_dim": "new_value"}, "bucket": "lt3s", "count": 7},
                {"event": "peers", "count": 2}
            ]
        })
    }

    async fn send(pool: PgPool, body: impl Into<Body>) -> (StatusCode, String) {
        let request = Request::post("/v1/batch").header(header::CONTENT_TYPE, "application/json").body(body.into()).unwrap();
        let response = app(pool).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn post_json(pool: PgPool, body: &Value) -> (StatusCode, String) {
        send(pool, serde_json::to_vec(body).unwrap()).await
    }

    async fn call(router: Router, method: &str, path: &str, cookie: Option<&str>, body: Option<Value>) -> (StatusCode, HeaderMap, String) {
        let mut request = Request::builder().method(method).uri(path).header(header::HOST, "telemetry.test");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let request = match body {
            Some(body) => request.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        };
        let response = router.oneshot(request.unwrap()).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// O par `nome=valor` do Set-Cookie, como o navegador o devolve.
    fn session_of(headers: &HeaderMap) -> String {
        headers[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned()
    }

    #[tokio::test]
    async fn without_a_session_the_panel_serves_the_app_and_hides_the_data() {
        let (status, _, page) = call(app_with_panel(offline()), "GET", "/", None, None).await;
        assert_eq!(status, StatusCode::OK);
        // O app pergunta a sessão em /panel/api/me e mostra a entrada; a página em si não traz dado nenhum.
        assert!(page.contains(r#"<div id="root">"#) && !page.contains("/panel/data"), "{page}");
        assert_eq!(call(app_with_panel(offline()), "GET", "/panel/data", None, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(app_with_panel(offline()), "GET", "/panel/events", None, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(app_with_panel(offline()), "POST", "/panel/api/invites", None, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(app_with_panel(offline()), "GET", "/panel/trail?machine=x&day=y", None, None).await.0, StatusCode::UNAUTHORIZED);
        // Consulta que nem desserializa também é 401 sem sessão.
        assert_eq!(call(app_with_panel(offline()), "GET", "/panel/trail?machine=a&machine=b", None, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(app_with_panel(offline()), "GET", "/panel/data?days=abc", None, None).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_panel_installs_as_an_app_without_caching_pages() {
        let (status, headers, body) = call(app_with_panel(offline()), "GET", "/panel/manifest.webmanifest", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "application/manifest+json");
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
        let manifest: Value = serde_json::from_str(&body).unwrap();
        assert_eq!((manifest["start_url"].as_str(), manifest["scope"].as_str(), manifest["display"].as_str()), (Some("/"), Some("/"), Some("standalone")));
        // O id antigo mantém o app já instalado como o mesmo app.
        assert_eq!(manifest["id"].as_str(), Some("/panel"));
        let icons = manifest["icons"].as_array().unwrap();
        for size in ["192x192", "512x512"] {
            for purpose in ["any", "maskable"] {
                assert!(icons.iter().any(|i| i["sizes"] == size && i["purpose"] == purpose), "{size} {purpose}");
            }
        }
        for src in icons.iter().map(|i| i["src"].as_str().unwrap()).chain(["/panel/apple-touch-icon.png"]) {
            let (status, headers, _) = call(app_with_panel(offline()), "GET", src, None, None).await;
            assert!(status == StatusCode::OK && headers[header::CONTENT_TYPE] == "image/png", "{src}");
        }

        let (status, headers, body) = call(app_with_panel(offline()), "GET", "/panel/sw.js", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
        assert_eq!(headers["service-worker-allowed"], "/");
        assert!(body.contains("fetch(e.request)"), "{body}");

        for path in ["/", "/join"] {
            let (status, headers, page) = call(app_with_panel(offline()), "GET", path, None, None).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(headers[header::CACHE_CONTROL], "no-store", "a página nunca fica guardada: {path}");
            assert!(page.contains("/panel/manifest.webmanifest"), "{path}");
            let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(policy.contains("script-src 'self';") && policy.contains("worker-src 'self'") && !policy.contains("sha256-"), "{policy}");
            assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        }
    }

    /// Link e app instalados com o endereço antigo abrem o painel na raiz.
    #[tokio::test]
    async fn old_panel_addresses_go_to_the_root() {
        for (from, to) in [("/panel", "/"), ("/panel/", "/"), ("/panel/join", "/join"), ("/panel/join?invite=abc", "/join?invite=abc")] {
            let (status, headers, _) = call(app_with_panel(offline()), "GET", from, None, None).await;
            assert_eq!(status, StatusCode::PERMANENT_REDIRECT, "{from}");
            assert_eq!(headers[header::LOCATION], to, "{from}");
        }
    }

    #[tokio::test]
    async fn hashed_assets_stay_cached_and_unknown_files_are_404() {
        let (_, _, page) = call(app_with_panel(offline()), "GET", "/", None, None).await;
        let script = page.split(r#"src=""#).nth(1).and_then(|rest| rest.split('"').next()).unwrap().to_owned();
        assert!(script.starts_with("/panel/assets/") && script.ends_with(".js"), "{script}");
        let (status, headers, body) = call(app_with_panel(offline()), "GET", &script, None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        // O app registra o service worker que só repassa à rede.
        assert!(body.contains("/panel/sw.js"), "o app registra o service worker");
        for missing in ["/panel/assets/nada.js", "/panel/assets/../index.html", "/panel/nada.png"] {
            assert_eq!(call(app_with_panel(offline()), "GET", missing, None, None).await.0, StatusCode::NOT_FOUND, "{missing}");
        }
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn account_login_invite_join_and_logout() {
        let pool = online().await;
        let owner = format!("dono-{}@example.test", fresh_id());
        accounts::set_user(&pool, &owner, &accounts::hash_password("senha-do-dono-1")).await.unwrap();
        let app = || app_with_panel(pool.clone());

        let wrong = json!({"email": owner, "password": "senha-errada-99"});
        assert_eq!(call(app(), "POST", "/panel/api/login", None, Some(wrong)).await.0, StatusCode::UNAUTHORIZED);
        let unknown = json!({"email": "ninguem@example.test", "password": "senha-do-dono-1"});
        assert_eq!(call(app(), "POST", "/panel/api/login", None, Some(unknown)).await.0, StatusCode::UNAUTHORIZED);
        let right = json!({"email": owner.to_uppercase(), "password": "senha-do-dono-1"});
        let (status, headers, _) = call(app(), "POST", "/panel/api/login", None, Some(right)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let cookie = session_of(&headers);
        assert!(headers[header::SET_COOKIE].to_str().unwrap().contains("HttpOnly; Secure; SameSite=Strict"));

        assert_eq!(call(app(), "GET", "/panel/api/me", Some(&cookie), None).await.0, StatusCode::OK, "com sessão o app abre o painel");
        assert_eq!(call(app(), "GET", "/panel/data?days=7", Some(&cookie), None).await.0, StatusCode::OK);

        let (status, _, body) = call(app(), "POST", "/panel/api/invites", Some(&cookie), None).await;
        assert_eq!(status, StatusCode::OK);
        let invite = serde_json::from_str::<Value>(&body).unwrap()["path"].as_str().unwrap().split('#').nth(1).unwrap().to_owned();
        let guest = format!("convidado-{}@example.test", fresh_id());
        let short = json!({"invite": invite, "email": guest, "password": "curta"});
        assert_eq!(call(app(), "POST", "/panel/api/join", None, Some(short)).await.0, StatusCode::BAD_REQUEST);
        let taken = json!({"invite": invite, "email": owner, "password": "senha-do-convidado"});
        assert_eq!(call(app(), "POST", "/panel/api/join", None, Some(taken)).await.0, StatusCode::CONFLICT);
        let join = json!({"invite": invite, "email": guest, "password": "senha-do-convidado"});
        let (status, headers, _) = call(app(), "POST", "/panel/api/join", None, Some(join.clone())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, _, me) = call(app(), "GET", "/panel/api/me", Some(&session_of(&headers)), None).await;
        assert!(me.contains(&guest), "{me}");
        let again = json!({"invite": invite, "email": format!("outro-{}@example.test", fresh_id()), "password": "senha-do-convidado"});
        assert_eq!(call(app(), "POST", "/panel/api/join", None, Some(again)).await.0, StatusCode::BAD_REQUEST, "convite é de uso único");

        assert_eq!(call(app(), "POST", "/panel/api/logout", Some(&cookie), None).await.0, StatusCode::NO_CONTENT);
        assert_eq!(call(app(), "GET", "/panel/data", Some(&cookie), None).await.0, StatusCode::UNAUTHORIZED, "sair encerra a sessão");
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn a_password_changed_during_login_opens_no_session() {
        let pool = online().await;
        let email = format!("troca-{}@example.test", fresh_id());
        accounts::set_user(&pool, &email, &accounts::hash_password("senha-antiga-1")).await.unwrap();
        let (id, old_hash) = accounts::login(&pool, &email, "senha-antiga-1").await.unwrap().unwrap();
        accounts::set_user(&pool, &email, &accounts::hash_password("senha-nova-22")).await.unwrap();
        assert_eq!(accounts::create_session(&pool, id, &old_hash).await.unwrap(), None);
        let (id, hash) = accounts::login(&pool, &email, "senha-nova-22").await.unwrap().unwrap();
        assert!(accounts::create_session(&pool, id, &hash).await.unwrap().is_some());
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn panel_data_sums_the_counters_of_the_period() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        let (first, second) = (batch_body(&fresh_id(), &today), batch_body(&fresh_id(), &today));
        for body in [&first, &second] {
            assert_eq!(post_json(pool.clone(), body).await.0, StatusCode::OK);
        }
        let email = format!("dados-{}@example.test", fresh_id());
        accounts::set_user(&pool, &email, &accounts::hash_password("senha-dos-dados")).await.unwrap();
        let login = json!({"email": email, "password": "senha-dos-dados"});
        let (_, headers, _) = call(app_with_panel(pool.clone()), "POST", "/panel/api/login", None, Some(login)).await;
        let (status, _, text) = call(app_with_panel(pool), "GET", "/panel/data?days=7", Some(&session_of(&headers)), None).await;
        assert_eq!(status, StatusCode::OK);
        let data: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(data["days"], 7);
        let future = data["by_dims"].as_array().unwrap().iter().find(|c| c["event"] == "future_event" && c["dims"]["new_dim"] == "new_value").unwrap();
        // Outras rodadas no mesmo banco somam junto: confere o mínimo destas duas máquinas.
        assert!(future["count"].as_i64().unwrap() >= 14 && future["machines"].as_i64().unwrap() >= 2, "{future}");
        assert!(data["active"]["machines_7d"].as_i64().unwrap() >= 2);
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn panel_data_gives_the_speed_buckets_by_system() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        let mut body = batch_body(&fresh_id(), &today);
        body["env"]["os"] = json!("windows");
        let dims = json!({"provider": "claude", "surface": "terminal", "opened_by": "person"});
        body["counters"] = json!([
            {"event": "session_ready", "dims": dims, "bucket": "lt3s", "count": 3},
            {"event": "session_opened", "dims": dims, "bucket": "lt3s", "count": 3},
        ]);
        assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
        let email = format!("velocidade-{}@example.test", fresh_id());
        accounts::set_user(&pool, &email, &accounts::hash_password("senha-da-velocidade")).await.unwrap();
        let login = json!({"email": email, "password": "senha-da-velocidade"});
        let (_, headers, _) = call(app_with_panel(pool.clone()), "POST", "/panel/api/login", None, Some(login)).await;
        let (status, _, text) = call(app_with_panel(pool), "GET", "/panel/data?days=7", Some(&session_of(&headers)), None).await;
        assert_eq!(status, StatusCode::OK);
        let data: Value = serde_json::from_str(&text).unwrap();
        let speed = data["speed"].as_array().unwrap();
        let ready = speed.iter().find(|c| c["event"] == "session_ready" && c["os"] == "windows" && c["dims"] == dims && c["bucket"] == "lt3s").unwrap();
        assert!(ready["count"].as_i64().unwrap() >= 3, "{ready}");
        assert!(speed.iter().all(|c| c["event"] != "session_opened"), "só as medidas do bloco Velocidade");
    }

    async fn declined(pool: PgPool, body: &Value) -> (StatusCode, String) {
        let request = Request::post("/v1/declined").header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap())).unwrap();
        let response = app(pool).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn declined_accepts_only_the_four_closed_fields() {
        let ok = json!({"kind": "declined", "version": "0.42.0", "os": "linux", "channel": "latest"});
        let mut with_id = ok.clone();
        with_id["machine_id"] = json!(fresh_id());
        let mut bad_kind = ok.clone();
        bad_kind["kind"] = json!("later");
        let mut bad_os = ok.clone();
        bad_os["os"] = json!("Linux Mint");
        let mut missing = ok.clone();
        missing.as_object_mut().unwrap().remove("channel");
        for (body, error) in [(with_id, "json"), (bad_kind, "json"), (missing, "json"), (bad_os, "os")] {
            assert_eq!(declined(offline(), &body).await, (StatusCode::BAD_REQUEST, format!(r#"{{"ok":false,"error":"{error}"}}"#)), "{body}");
        }
        let mut big = ok.clone();
        big["version"] = json!("x".repeat(MAX_REFUSAL_BODY));
        assert_eq!(declined(offline(), &big).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn declined_counts_each_ping_and_the_summary_splits_by_kind() {
        let pool = online().await;
        let before = store::summary(&pool, 7).await.unwrap()["refusals"].clone();
        // Versão desta rodada: o banco é dividido entre rodadas.
        let version = format!("t-{}", fresh_id());
        for kind in ["declined", "declined", "revoked"] {
            let body = json!({"kind": kind, "version": version, "os": "linux", "channel": "latest"});
            assert_eq!(declined(pool.clone(), &body).await, (StatusCode::OK, r#"{"ok":true}"#.to_owned()));
        }
        let rows: Vec<(String, i64)> = sqlx::query_as("SELECT kind, count FROM refusals WHERE version = $1 ORDER BY kind")
            .bind(&version).fetch_all(&pool).await.unwrap();
        assert_eq!(rows, vec![("declined".to_owned(), 2), ("revoked".to_owned(), 1)]);
        let after = store::summary(&pool, 7).await.unwrap()["refusals"].clone();
        let grew = |kind: &str| after[kind].as_i64().unwrap() - before[kind].as_i64().unwrap();
        assert!(grew("declined") >= 2 && grew("revoked") >= 1, "{before} {after}");
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn the_open_panel_hears_each_new_batch() {
        use futures_util::StreamExt;
        let pool = online().await;
        let cookie = logged_in(&pool).await;
        let live = Arc::new(Live::new());
        let request = Request::get("/panel/events").header(header::HOST, "telemetry.test").header(header::COOKIE, &cookie).body(Body::empty()).unwrap();
        let response = super::app_with_panel(pool.clone(), live.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-accel-buffering"], "no", "proxy no meio não junta os avisos");
        let mut events = response.into_body().into_data_stream();
        let id = fresh_id();
        let batch = batch_body(&id, &utc_day(&pool, 0).await);
        let request = Request::post("/v1/batch").header(header::CONTENT_TYPE, "application/json").body(Body::from(batch.to_string())).unwrap();
        assert_eq!(super::app(pool.clone(), live.clone()).oneshot(request).await.unwrap().status(), StatusCode::OK);
        let chunk = tokio::time::timeout(Duration::from_secs(5), events.next()).await.expect("aviso do pacote").unwrap().unwrap();
        assert!(String::from_utf8_lossy(&chunk).contains("event: batch"), "{chunk:?}");
        forget(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn body_over_the_limit_is_413() {
        let (status, _) = send(offline(), vec![b' '; MAX_BODY + 1]).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        // No limite ainda passa pelo leitor: espaço só não é JSON.
        let (status, _) = send(offline(), vec![b' '; MAX_BODY]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_machine_id_is_400() {
        let mut body = batch_body(&fresh_id(), "2026-11-26");
        body.as_object_mut().unwrap().remove("machine_id");
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(text, r#"{"ok":false,"error":"json"}"#);
    }

    #[tokio::test]
    async fn malformed_fields_are_400() {
        let mut body = batch_body("NOT-HEX", "2026-11-26");
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"batch_id"}"#));
        body["batch_id"] = json!(fresh_id());
        body["counters"][0]["count"] = json!(u64::MAX);
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"count"}"#));
        body["counters"][0]["count"] = json!(1);
        body["logs"] = json!([{"level": "error", "target": "hangar_server::list", "message": "x", "count": u64::MAX}]);
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"log_count"}"#));
        body["logs"][0]["count"] = json!(hangar_telemetry::wire::MAX_COUNT + 1);
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"log_count"}"#));
        body["logs"][0]["count"] = json!(1);
        body["counters"][0]["count"] = json!(hangar_telemetry::wire::MAX_COUNT + 1);
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"count"}"#));
        body["counters"][0]["count"] = json!(1);
        body["logs"][0]["level"] = json!("info");
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"log_level"}"#));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn huge_counts_already_stored_do_not_break_the_summary() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        // Gravado direto, sem o `validate`: como um pacote de antes do teto.
        let (event, message) = (format!("huge_{}", fresh_id()), format!("enorme {}", fresh_id()));
        for _ in 0..2 {
            let mut body = batch_body(&fresh_id(), &today);
            body["counters"] = json!([{"event": event, "bucket": "lt1s", "count": i64::MAX}]);
            body["logs"] = json!([{"level": "error", "target": "hangar_server::list", "message": message, "count": i64::MAX}]);
            let batch: Batch = serde_json::from_value(body).unwrap();
            assert!(matches!(store::insert(&pool, &batch).await.unwrap(), store::Insert::Accepted));
        }
        let data = store::summary(&pool, 7).await.unwrap();
        let find = |key: &str, field: &str, value: &str| data[key].as_array().unwrap().iter().find(|row| row[field] == json!(value)).unwrap()["count"].clone();
        assert_eq!(find("counters", "event", &event), json!(i64::MAX));
        assert_eq!(find("by_dims", "event", &event), json!(i64::MAX));
        assert_eq!(find("by_bucket", "event", &event), json!(i64::MAX));
        assert_eq!(find("logs", "message", &message), json!(i64::MAX));
        assert_eq!(find("trend", "event", &event), json!(i64::MAX));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn log_lines_of_two_machines_become_one_aggregated_row() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        // Mensagem desta rodada: o banco é dividido entre rodadas.
        let message = format!("lista não lida {}", fresh_id());
        for (version, count) in [("0.42.0", 3), ("0.43.0", 2)] {
            let mut body = batch_body(&fresh_id(), &today);
            body["env"]["version"] = json!(version);
            body["logs"] = json!([
                {"level": "error", "target": "hangar_server::list", "message": message, "count": count},
                {"level": "warn", "target": "hangar_server::list", "message": format!("{message} aviso"), "count": 1},
            ]);
            assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
        }
        let data = store::summary(&pool, 7).await.unwrap();
        let row = data["logs"].as_array().unwrap().iter().find(|l| l["message"] == json!(message)).unwrap().clone();
        assert_eq!(row, json!({
            "level": "error", "target": "hangar_server::list", "message": message, "count": 5, "machines": 2,
            "first_day": today, "last_day": today, "versions": ["0.42.0", "0.43.0"],
        }));
        let warn = data["logs"].as_array().unwrap().iter().find(|l| l["message"] == json!(format!("{message} aviso"))).unwrap();
        assert_eq!((&warn["level"], &warn["count"], &warn["machines"]), (&json!("warn"), &json!(2), &json!(2)));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn unknown_event_is_stored_as_sent() {
        let pool = online().await;
        let id = fresh_id();
        let (status, text) = post_json(pool.clone(), &batch_body(&id, &utc_day(&pool, -1).await)).await;
        assert_eq!((status, text.as_str()), (StatusCode::OK, r#"{"ok":true}"#));
        let rows: Vec<(String, Option<String>, Option<String>, i64)> = sqlx::query_as(
            "SELECT event, dims->>'new_dim', bucket, count FROM counters WHERE batch_id = $1 ORDER BY event",
        )
        .bind(&id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows, vec![
            ("future_event".to_owned(), Some("new_value".to_owned()), Some("lt3s".to_owned()), 7),
            ("peers".to_owned(), None, None, 2),
        ]);
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn repeated_batch_id_is_200_and_not_stored_twice() {
        let pool = online().await;
        let id = fresh_id();
        let body = batch_body(&id, &utc_day(&pool, -1).await);
        for _ in 0..2 {
            let (status, text) = post_json(pool.clone(), &body).await;
            assert_eq!((status, text.as_str()), (StatusCode::OK, r#"{"ok":true}"#));
        }
        let batches: i64 = sqlx::query_scalar("SELECT count(*) FROM batches WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        let counters: i64 = sqlx::query_scalar("SELECT count(*) FROM counters WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!((batches, counters), (1, 2));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn day_too_far_ahead_is_400() {
        let pool = online().await;
        let (status, _) = post_json(pool.clone(), &batch_body(&fresh_id(), &utc_day(&pool, 2).await)).await;
        assert_eq!(status, StatusCode::OK);
        let id = fresh_id();
        let (status, text) = post_json(pool.clone(), &batch_body(&id, &utc_day(&pool, 3).await)).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"day"}"#));
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM batches WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(stored, 0);
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn impossible_calendar_day_is_400() {
        let (status, text) = post_json(online().await, &batch_body(&fresh_id(), "2026-02-31")).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"value"}"#));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn retention_drops_batches_older_than_400_days() {
        let pool = online().await;
        let yesterday = utc_day(&pool, -1).await;
        let (old, recent) = (fresh_id(), fresh_id());
        for (id, day) in [(&old, "2000-01-01"), (&recent, yesterday.as_str())] {
            assert_eq!(post_json(pool.clone(), &batch_body(id, day)).await.0, StatusCode::OK);
        }
        store::prune(&pool).await.unwrap();
        let left: Vec<String> = sqlx::query_scalar("SELECT batch_id FROM batches WHERE batch_id = $1 OR batch_id = $2")
            .bind(&old)
            .bind(&recent)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, vec![recent]);
        let orphans: i64 = sqlx::query_scalar("SELECT count(*) FROM counters WHERE batch_id = $1").bind(&old).fetch_one(&pool).await.unwrap();
        assert_eq!(orphans, 0);
        let version = format!("t-{}", fresh_id());
        sqlx::query("INSERT INTO refusals (day, kind, version, os, channel) VALUES ('2000-01-01', 'declined', $1, 'linux', 'latest'), ($2::date, 'declined', $1, 'linux', 'latest')")
            .bind(&version).bind(&yesterday).execute(&pool).await.unwrap();
        store::prune_refusals(&pool).await.unwrap();
        let days: Vec<String> = sqlx::query_scalar("SELECT to_char(day, 'YYYY-MM-DD') FROM refusals WHERE version = $1").bind(&version).fetch_all(&pool).await.unwrap();
        assert_eq!(days, vec![yesterday]);
    }

    /// Entra com uma conta nova e devolve o cookie da sessão.
    async fn logged_in(pool: &PgPool) -> String {
        let email = format!("painel-{}@example.test", fresh_id());
        accounts::set_user(pool, &email, &accounts::hash_password("senha-do-painel")).await.unwrap();
        let login = json!({"email": email, "password": "senha-do-painel"});
        let (_, headers, _) = call(app_with_panel(pool.clone()), "POST", "/panel/api/login", None, Some(login)).await;
        session_of(&headers)
    }

    /// Apaga os pacotes do teste: os cortes do resumo (300 assinaturas, 50 passagens) são do banco inteiro.
    async fn forget(pool: &PgPool, ids: &[String]) {
        sqlx::query("DELETE FROM batches WHERE batch_id = ANY($1)").bind(ids).execute(pool).await.unwrap();
    }

    fn record(event: &str, level: &str, codigo: &str, count: u64, first: &str, last: &str, detalhe: &str) -> Value {
        json!({"level": level, "event": event, "count": count, "first": first, "last": last,
               "fields": {"codigo": codigo, "etapa": "abrir", "detalhe": detalhe}, "trail": ["app.visivel", "sessao.nova"]})
    }

    fn step(t: &str, event: &str, level: &str) -> Value { json!({"t": t, "event": event, "level": level, "screen": "chat", "count": 1}) }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn the_same_batch_id_again_replaces_what_the_batch_had() {
        let pool = online().await;
        let id = fresh_id();
        let event = format!("falha.{}", &fresh_id()[..8]);
        let stored = |pool: PgPool, id: String| async move {
            let one = |sql: &'static str| { let (pool, id) = (pool.clone(), id.clone()); async move {
                sqlx::query_scalar::<_, i64>(sql).bind(id).fetch_one(&pool).await.unwrap() } };
            (
                one("SELECT coalesce(sum(count), 0)::bigint FROM counters WHERE batch_id = $1").await,
                one("SELECT count(*) FROM logs WHERE batch_id = $1").await,
                one("SELECT coalesce(sum(count), 0)::bigint FROM records WHERE batch_id = $1").await,
                one("SELECT count(*) FROM trail_steps WHERE batch_id = $1").await,
                one("SELECT count(*) FILTER (WHERE partial) FROM batches WHERE batch_id = $1").await,
            )
        };
        let mut body = batch_body(&id, &utc_day(&pool, 0).await);
        body["partial"] = json!(true);
        body["counters"] = json!([{"event": "peers", "count": 3}]);
        body["logs"] = json!([{"level": "error", "target": "hangar_server::list", "message": "x", "count": 1}]);
        body["records"] = json!([record(&event, "error", "erro_pasta", 1, "10:00:00", "10:00:00", "a")]);
        body["trail"] = json!([step("10:00:00", "app.visivel", "ok"), step("10:00:05", &event, "error")]);
        assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
        let delayed = body.clone();

        body["counters"][0]["count"] = json!(4);
        body["records"][0]["count"] = json!(3);
        body["trail"] = json!([step("11:00:00", &event, "error")]);
        assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
        assert_eq!(stored(pool.clone(), id.clone()).await, (4, 1, 3, 1, 1), "outro parcial também troca, não soma");

        // O final omite partial, como os lotes antigos.
        body.as_object_mut().unwrap().remove("partial");
        body["counters"][0]["count"] = json!(5);
        body["logs"] = json!([]);
        body["records"] = json!([record(&event, "error", "erro_pasta", 4, "10:00:00", "12:00:00", "b")]);
        body["trail"] = json!([step("10:00:00", "app.visivel", "ok"), step("10:00:05", &event, "error"), step("12:00:00", "app.oculto", "ok")]);
        assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
        assert_eq!(stored(pool.clone(), id.clone()).await, (5, 0, 4, 3, 0), "o final troca o retrato, não soma");

        assert_eq!(post_json(pool.clone(), &delayed).await.0, StatusCode::OK);
        assert_eq!(stored(pool.clone(), id.clone()).await, (5, 0, 4, 3, 0));

        // Reenvio do final por uma versão nova: 200, e o final guardado fica como estava.
        body["env"]["version"] = json!("0.43.0");
        body["counters"][0]["count"] = json!(9);
        assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
        assert_eq!(stored(pool.clone(), id.clone()).await, (5, 0, 4, 3, 0));
        let version: String = sqlx::query_scalar("SELECT version FROM batches WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(version, "0.42.0");
        forget(&pool, &[id]).await;
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn failure_records_of_two_machines_become_one_row_per_signature() {
        let pool = online().await;
        let (yesterday, today) = (utc_day(&pool, -1).await, utc_day(&pool, 0).await);
        let (linux, windows) = (fresh_id(), fresh_id());
        let event = format!("sessao.abrir_{}", &fresh_id()[..8]);
        let mut ids = Vec::new();
        for (day, machine, version, os, level, count, first, last, detail) in [
            (&yesterday, &linux, "0.42.0", "linux", "warn", 2, "10:00:00", "11:00:00", "antes"),
            (&today, &linux, "0.42.0", "linux", "error", 2, "08:00:00", "13:00:00", "ultima_ocorrencia"),
            (&today, &windows, "0.43.0", "windows", "warn", 3, "09:30:00", "10:00:00", "amostra"),
        ] {
            let id = fresh_id();
            let mut body = batch_body(&id, day);
            body["machine_id"] = json!(machine);
            body["env"]["version"] = json!(version);
            body["env"]["os"] = json!(os);
            let mut legacy = record(&event, level, "erro_pasta", count, first, last, detail);
            if machine == &windows {
                legacy["trail"] = json!(["app.visivel", "busca.abrir"]);
            }
            let mut open = record(&event, level, "erro_pasta", count, first, last, "amostra da abertura");
            open["fields"]["action"] = json!("browser_open");
            let mut close = record(&event, level, "erro_pasta", count + 2, first, last, "amostra do fechamento");
            close["fields"]["action"] = json!("browser_close");
            // A mesma ferramenta por duas causas: duas linhas, cada uma com a amostra da sua causa.
            let [closed, gone] = ["desktop_app_closed", "cdp_closed"].map(|cause| {
                let mut row = record(&event, level, "erro_pasta", 1, first, last, cause);
                row["fields"]["action"] = json!("browser");
                row["fields"]["causa"] = json!(cause);
                row
            });
            // Todas as assinaturas têm duas máquinas para disputar o corte com os outros testes.
            body["records"] = json!([legacy, open, close, closed, gone, record(&event, "warn", "erro_outro", 1, first, last, "x")]);
            assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
            ids.push(id);
        }
        let data = store::summary(&pool, 7).await.unwrap();
        let rows: Vec<&Value> = data["records"].as_array().unwrap().iter().filter(|r| r["event"] == json!(event)).collect();
        assert_eq!(rows.len(), 6, "{rows:?}");
        let row = rows.iter().find(|r| r["codigo"] == json!("erro_pasta") && r["action"].is_null()).unwrap();
        assert_eq!(**row, json!({
            "level": "error", "event": event, "codigo": "erro_pasta", "etapa": "abrir", "erro_tipo": null, "action": null,
            "count": 7, "machines": 2, "versions": ["0.42.0", "0.43.0"], "os": ["linux", "windows"],
            "first": format!("{yesterday} 10:00:00"), "last": format!("{today} 13:00:00"),
            "sample_at": format!("{today} 09:30:00"),
            "fields": {"codigo": "erro_pasta", "etapa": "abrir", "detalhe": "amostra"}, "trail": ["app.visivel", "busca.abrir"],
            "daily": [{"day": yesterday, "count": 2}, {"day": today, "count": 5}],
            // Pacote antigo, sem retrato nem ambiente: os campos novos vêm vazios.
            "trail_last": [], "ctx_first": {}, "ctx_last": {}, "after": null, "capped": false,
            "batch": {"version": "0.43.0", "commit": "dev", "install": null, "os": "windows", "clock_skew_s": null, "machine": {}},
        }));
        for (action, count, detail) in [("browser_open", 7, "amostra da abertura"), ("browser_close", 13, "amostra do fechamento")] {
            let row = rows.iter().find(|r| r["action"] == json!(action)).unwrap();
            assert_eq!(row["count"], json!(count));
            assert_eq!(row["fields"]["action"], json!(action));
            assert_eq!(row["fields"]["detalhe"], json!(detail));
        }
        for cause in ["desktop_app_closed", "cdp_closed"] {
            let row = rows.iter().find(|r| r["fields"]["causa"] == json!(cause)).unwrap();
            assert_eq!((&row["count"], &row["machines"], &row["action"]), (&json!(3), &json!(2), &json!("browser")), "{cause}");
            assert_eq!(row["daily"], json!([{"day": yesterday, "count": 1}, {"day": today, "count": 2}]), "{cause}");
        }
        assert!(data["records_total"].as_i64().unwrap() >= 6);
        forget(&pool, &ids).await;
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn a_failure_brings_its_context_its_machine_and_what_came_after() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        // Enviado dois minutos antes de chegar: a máquina está com o relógio atrasado.
        let sent_at: String = sqlx::query_scalar(r#"SELECT to_char(now() AT TIME ZONE 'utc' - interval '120 seconds', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#)
            .fetch_one(&pool).await.unwrap();
        let id = fresh_id();
        let event = format!("health.state_{}", &fresh_id()[..8]);
        let mut body = batch_body(&id, &today);
        body["env"]["install"] = json!("checkout");
        body["sent_at"] = json!(sent_at);
        body["machine"] = json!({"os": "linux", "os_name": "cachyos", "ram_total": "16-32g", "cli.claude": "2.1.195"});
        body["counters"][0]["capped"] = json!(true);
        let mut failure = record(&event, "warn", "terminal_observacao_falhou", 3, "10:00:00", "10:06:00", "x");
        failure["fields"]["causa"] = json!("capture_timeout");
        failure["ctx_first"] = json!({"mem_free": "ge40", "provider": "claude"});
        failure["ctx_last"] = json!({"mem_free": "lt5", "provider": "claude"});
        failure["trail_last"] = json!(["tela.ver:chat", "browser"]);
        failure["after"] = json!("recovered");
        failure["capped"] = json!(true);
        body["records"] = json!([failure]);
        body["trail"] = json!([{"t": "10:00:00", "event": "tela.ver", "level": "ok", "screen": "chat", "count": 5, "capped": true}]);
        assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);

        let data = store::summary(&pool, 7).await.unwrap();
        let row = data["records"].as_array().unwrap().iter().find(|r| r["event"] == json!(event)).unwrap().clone();
        assert_eq!(row["ctx_first"], json!({"mem_free": "ge40", "provider": "claude"}));
        assert_eq!(row["ctx_last"], json!({"mem_free": "lt5", "provider": "claude"}));
        assert_eq!(row["trail_last"], json!(["tela.ver:chat", "browser"]));
        assert_eq!(row["after"], json!("recovered"));
        assert_eq!(row["capped"], json!(true));
        assert_eq!(row["fields"]["causa"], json!("capture_timeout"));
        let skew = row["batch"]["clock_skew_s"].as_i64().unwrap();
        assert!((115..=180).contains(&skew), "{skew}");
        assert_eq!(row["batch"], json!({"version": "0.42.0", "commit": "dev", "install": "checkout", "os": "linux", "clock_skew_s": skew,
            "machine": {"os": "linux", "os_name": "cachyos", "ram_total": "16-32g", "cli.claude": "2.1.195"}}));
        let capped: (bool, bool) = sqlx::query_as(
            "SELECT (SELECT bool_and(capped) FROM counters WHERE batch_id = $1 AND event = 'future_event'),
                    (SELECT bool_and(capped) FROM trail_steps WHERE batch_id = $1)")
            .bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(capped, (true, true));
        forget(&pool, &[id]).await;
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn failure_records_stop_at_300_and_say_the_real_total() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = fresh_id();
            let mut body = batch_body(&id, &today);
            body["records"] = json!((0..151).map(|_| record(&format!("corte.{}", &fresh_id()[..12]), "warn", "c", 1, "01:00:00", "01:00:00", "x")).collect::<Vec<_>>());
            assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
            ids.push(id);
        }
        let data = store::summary(&pool, 7).await.unwrap();
        assert_eq!(data["records"].as_array().unwrap().len(), 300);
        // Toda assinatura devolvida está no período, então tem ao menos um dia na minibarra (campos nulos casam também).
        assert!(data["records"].as_array().unwrap().iter().all(|r| !r["daily"].as_array().unwrap().is_empty()));
        assert!(data["records_total"].as_i64().unwrap() >= 302, "{}", data["records_total"]);
        forget(&pool, &ids).await;
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn the_trail_gives_common_transitions_and_the_day_of_one_machine() {
        let pool = online().await;
        let today = utc_day(&pool, 0).await;
        let tag = &fresh_id()[..8];
        let (a, b, c) = (format!("a.{tag}"), format!("b.{tag}"), format!("c.{tag}"));
        // Muitas voltas a → b: a passagem fica entre as 50 mais comuns mesmo com outros dados no banco.
        let mut first: Vec<Value> = (0..400).map(|i| step(&format!("10:{:02}:{:02}", i / 60 % 60, i % 60), if i % 2 == 0 { &a } else { &b }, "ok")).collect();
        first.push(json!({"t": "11:00:00", "event": c, "level": "error", "count": 2}));
        let mut ids = Vec::new();
        let mut machine = String::new();
        for trail in [first, vec![step("09:00:00", &a, "ok"), step("09:00:01", &b, "warn")]] {
            let id = fresh_id();
            let mut body = batch_body(&id, &today);
            body["trail"] = json!(trail);
            if machine.is_empty() {
                machine = body["machine_id"].as_str().unwrap().to_owned();
                body["partial"] = json!(true);
            }
            assert_eq!(post_json(pool.clone(), &body).await.0, StatusCode::OK);
            ids.push(id);
        }
        let data = store::summary(&pool, 7).await.unwrap();
        let find = |from: &str, to: &str| data["transitions"].as_array().unwrap().iter().find(|t| t["from"] == json!(from) && t["to"] == json!(to)).cloned();
        assert_eq!(find(&a, &b), Some(json!({"from": a, "to": b, "count": 201, "machines": 2})));
        assert_eq!(find(&b, &c).map(|t| t["count"].clone()), Some(json!(1)));
        let day = data["trail_days"].as_array().unwrap().iter().find(|d| d["machine"] == json!(machine)).unwrap();
        assert_eq!(*day, json!({"machine": machine, "label": &machine[..8], "day": today, "os": "linux", "version": "0.42.0", "steps": 401, "problems": 1}));
        assert!(data["period"]["partial_batches"].as_i64().unwrap() >= 1);
        assert!(data["transitions_total"].as_i64().unwrap() >= 3);
        assert!(data["trail_days_total"].as_i64().unwrap() >= 2);

        // Novo lote da mesma máquina após novo aceite; o relógio recua no segundo passo.
        let id = fresh_id();
        let mut restarted = batch_body(&id, &today);
        restarted["machine_id"] = json!(machine);
        restarted["trail"] = json!([step("12:00:00", "demo.lote_novo", "ok"), step("08:00:00", "demo.relogio_recuou", "ok")]);
        assert_eq!(post_json(pool.clone(), &restarted).await.0, StatusCode::OK);
        ids.push(id);

        let cookie = logged_in(&pool).await;
        let app = || app_with_panel(pool.clone());
        let path = format!("/panel/trail?machine={machine}&day={today}");
        assert_eq!(call(app(), "GET", &path, None, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(app(), "GET", "/panel/trail?machine=x", None, None).await.0, StatusCode::UNAUTHORIZED, "sem sessão nem diz o que está torto");
        let (status, _, text) = call(app(), "GET", &path, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::OK);
        let trail: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(trail["partial"], json!(true));
        let steps = trail["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 403);
        assert_eq!(steps[0], json!({"t": "10:00:00", "event": a, "level": "ok", "screen": "chat", "count": 1}));
        assert_eq!(steps[400], json!({"t": "11:00:00", "event": c, "level": "error", "screen": null, "count": 2}));
        assert_eq!(steps[401], step("12:00:00", "demo.lote_novo", "ok"));
        assert_eq!(steps[402], step("08:00:00", "demo.relogio_recuou", "ok"));
        for bad in ["/panel/trail?machine=NOT-HEX&day=2026-11-27", "/panel/trail?day=2026-11-27", &format!("/panel/trail?machine={machine}&day=2026-02-31")] {
            assert_eq!(call(app(), "GET", bad, Some(&cookie), None).await.0, StatusCode::BAD_REQUEST, "{bad}");
        }
        let (_, _, empty) = call(app(), "GET", &format!("/panel/trail?machine={}&day={today}", fresh_id()), Some(&cookie), None).await;
        let empty: Value = serde_json::from_str(&empty).unwrap();
        assert_eq!(empty["steps"], json!([]));
        assert_eq!(empty["partial"], json!(false));
        forget(&pool, &ids).await;
    }
}
