//! Avisos da CLI ignorados na máquina: só o dono lê e grava, e arquivo estragado nunca vira lista vazia.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

struct Fixture {
    home: tempfile::TempDir,
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
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
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, url: format!("http://{address}/api/notice-ignores"), task }
    }

    async fn send(&self, method: reqwest::Method, token: &str, body: Option<&str>) -> (u16, Value) {
        let mut request = reqwest::Client::new().request(method, &self.url).bearer_auth(token);
        if let Some(body) = body { request = request.header("content-type", "application/json").body(body.to_owned()); }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or(Value::Null))
    }

    async fn get(&self) -> (u16, Value) { self.send(reqwest::Method::GET, "fixture-owner", None).await }
    async fn put(&self, body: Value) -> (u16, Value) { self.send(reqwest::Method::PUT, "fixture-owner", Some(&body.to_string())).await }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

fn code(body: &Value) -> &str { body["detail"]["code"].as_str().unwrap_or_default() }

#[tokio::test]
async fn notice_ignores_put_then_get() {
    let fixture = Fixture::start().await;
    assert_eq!(fixture.get().await, (200, json!({"keys": []})));
    assert_eq!(fixture.put(json!({"key": "hook:SessionStart:compact", "ignored": true})).await, (200, json!({"keys": ["hook:SessionStart:compact"]})));
    assert_eq!(fixture.put(json!({"key": "note:retry", "ignored": true})).await.1, json!({"keys": ["hook:SessionStart:compact", "note:retry"]}));
    assert_eq!(fixture.get().await.1, json!({"keys": ["hook:SessionStart:compact", "note:retry"]}));
    assert_eq!(fixture.put(json!({"key": "hook:SessionStart:compact", "ignored": false})).await.1, json!({"keys": ["note:retry"]}));
}

#[tokio::test]
async fn notice_ignores_invalid_bodies_are_422_and_change_nothing() {
    let fixture = Fixture::start().await;
    let long = format!("hook:{}", "a".repeat(196));
    for body in [
        json!({"key": "", "ignored": true}),
        json!({"key": "x:y", "ignored": true}),
        json!({"key": "hook:", "ignored": true}),
        json!({"key": long, "ignored": true}),
        json!({"key": "hook:Stop"}),
        json!({"key": "hook:Stop", "ignored": "sim"}),
        json!({"key": "hook:Stop", "ignored": true, "extra": 1}),
    ] {
        let (status, reply) = fixture.put(body.clone()).await;
        assert_eq!((status, code(&reply)), (422, "erro_corpo_invalido"), "{body}");
    }
    let big = json!({"key": "hook:Stop", "ignored": true, "pad": "x".repeat(8 * 1024)}).to_string();
    assert_eq!(fixture.send(reqwest::Method::PUT, "fixture-owner", Some(&big)).await.0, 422, "corpo acima de 4 KB");
    assert_eq!(fixture.get().await.1, json!({"keys": []}));
}

#[tokio::test]
async fn notice_ignores_unreadable_file_is_an_error_and_stays_as_it_was() {
    let fixture = Fixture::start().await;
    let path = fixture.home.path().join(".hangar").join("notice-ignores.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{\"keys\": [").unwrap();
    let (status, reply) = fixture.get().await;
    assert_eq!((status, code(&reply)), (500, "notice_ignores_unreadable"));
    let (status, reply) = fixture.put(json!({"key": "hook:Stop", "ignored": true})).await;
    assert_eq!((status, code(&reply)), (500, "notice_ignores_unreadable"));
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"keys\": [", "a gravação não passa por cima do arquivo estragado");
}

#[tokio::test]
async fn notice_ignores_full_list_is_409() {
    let fixture = Fixture::start().await;
    let keys: Vec<String> = (0..200).map(|i| format!("hook:h{i:03}")).collect();
    let path = fixture.home.path().join(".hangar").join("notice-ignores.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, json!({"keys": keys}).to_string()).unwrap();
    let (status, reply) = fixture.put(json!({"key": "hook:a-mais", "ignored": true})).await;
    assert_eq!((status, code(&reply)), (409, "notice_ignores_full"));
    assert_eq!(fixture.put(json!({"key": "hook:h000", "ignored": false})).await.0, 200);
    assert_eq!(fixture.put(json!({"key": "hook:a-mais", "ignored": true})).await.0, 200);
}

#[tokio::test]
async fn notice_ignores_require_owner_credentials() {
    let fixture = Fixture::start().await;
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let response = reqwest::Client::new().request(method, &fixture.url)
            .header("content-type", "application/json").body(r#"{"key":"hook:Stop","ignored":true}"#).send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert_eq!((status, code(&body)), (403, "notice_ignores_owner_only"));
    }
    assert!(!fixture.home.path().join(".hangar").join("notice-ignores.json").exists());
}
