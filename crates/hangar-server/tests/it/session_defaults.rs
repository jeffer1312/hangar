//! O desktop publica o padrão; outro cliente o lê sem depender da conta Claude selecionada.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

struct Fixture {
    home: tempfile::TempDir,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_port(false).await
    }

    async fn start_port(private: bool) -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, upstream: crate::refused_address(), internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into()));
        let state = Arc::new(state);
        let router = if private { hangar_server::routes::terminal_router(state) } else { hangar_server::routes::router(state) };
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}/api/session-defaults"), task }
    }

    async fn request(&self, method: reqwest::Method, query: &str, body: Option<Value>) -> reqwest::Response {
        let client = reqwest::Client::new();
        let mut request = client.request(method, format!("{}?{query}", self.base)).bearer_auth("fixture-owner");
        if let Some(body) = body { request = request.header("content-type", "application/json").body(body.to_string()); }
        request.send().await.unwrap()
    }

    async fn stop(&mut self) { self.task.abort(); let _ = (&mut self.task).await; }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

#[tokio::test]
async fn connect_uses_the_same_defaults_through_the_private_forwarder() {
    let mut fixture = Fixture::start_port(true).await;
    let url = fixture.base.replace("/api/session-defaults", "/__hangar_server/forwarded?provider=claude");
    let response = reqwest::Client::new().put(url).header("x-hangar-internal", "fixture-internal")
        .header("x-hangar-path", "/api/session-defaults").header("content-type", "application/json")
        .body(r#"{"defaults":{"model":"opus","effort":"high","permission":"bypassPermissions"}}"#)
        .send().await.unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(value["defaults"]["permission"], "bypassPermissions");
    fixture.stop().await;
}

#[tokio::test]
async fn clearing_a_default_prevents_stale_desktop_import_and_engines_are_separate() {
    let mut fixture = Fixture::start().await;
    let default = json!({"model":"sol", "effort":"high", "permission":"bypassPermissions"});
    let put = |value: Value, import| json!({"defaults": value, "if_absent": import});
    assert_eq!(fixture.request(reqwest::Method::PUT, "provider=claude&engine=proxy&engine_account=one", Some(put(default.clone(), false))).await.status(), 200);
    assert_eq!(fixture.request(reqwest::Method::PUT, "provider=claude", Some(put(Value::Null, false))).await.status(), 200);
    let response = fixture.request(reqwest::Method::PUT, "provider=claude", Some(put(default, true))).await;
    let result: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(result["defaults"], Value::Null);
    assert_eq!(result["configured"], true);
    let other: Value = serde_json::from_slice(&fixture.request(reqwest::Method::GET, "provider=claude&engine=proxy&engine_account=two", None).await.bytes().await.unwrap()).unwrap();
    assert_eq!(other["defaults"], Value::Null);
    fixture.stop().await;
}

#[tokio::test]
async fn invalid_permission_and_unreadable_storage_never_become_an_empty_default() {
    let mut fixture = Fixture::start().await;
    let invalid = fixture.request(reqwest::Method::PUT, "provider=claude", Some(json!({
        "defaults":{"model":"opus", "effort":"high", "permission":"unknown"}
    }))).await;
    assert_eq!(invalid.status(), 400);
    let path = fixture.home.path().join(".hangar/session-defaults.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    assert_eq!(fixture.request(reqwest::Method::GET, "provider=claude", None).await.status(), 503);
    assert_eq!(fixture.request(reqwest::Method::PUT, "provider=claude", Some(json!({"defaults":null}))).await.status(), 503);
    assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    fixture.stop().await;
}

/// Decisão 5 e R5-09: quem não é o dono leva o 401 contado do Rust, com o corpo do `require_auth`.
#[tokio::test]
async fn defaults_require_owner_credentials() {
    let mut fixture = Fixture::start().await;
    let client = reqwest::Client::new();
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let response = client.request(method, format!("{}?provider=claude", fixture.base))
            .header("content-type", "application/json").body(r#"{"defaults":null}"#).send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(serde_json::from_slice::<Value>(&response.bytes().await.unwrap()).unwrap(),
            json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}}));
    }
    fixture.stop().await;
}

#[tokio::test]
async fn desktop_defaults_are_readable_by_mobile_and_isolated_by_harness() {
    let mut fixture = Fixture::start().await;
    let response = fixture.request(reqwest::Method::PUT, "provider=claude", Some(json!({
        "defaults": {"model":"opus", "effort":"high", "permission":"bypassPermissions"}
    }))).await;
    assert_eq!(response.status(), 200, "o servidor deve guardar o padrão do desktop");
    let saved: Value = serde_json::from_slice(&fixture.request(reqwest::Method::GET, "provider=claude", None).await.bytes().await.unwrap()).unwrap();
    assert_eq!(saved["defaults"], json!({"model":"opus", "effort":"high", "permission":"bypassPermissions"}));
    let codex: Value = serde_json::from_slice(&fixture.request(reqwest::Method::GET, "provider=codex", None).await.bytes().await.unwrap()).unwrap();
    assert_eq!(codex["defaults"], Value::Null);
    assert_eq!(codex["configured"], false);
    fixture.stop().await;
}
