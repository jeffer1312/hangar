//! Etiquetas de máquina e conta: gravadas no servidor da máquina, iguais em todos os clientes.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

struct Fixture {
    _home: tempfile::TempDir,
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
        Self { _home: home, url: format!("http://{address}/api/tags"), task }
    }

    async fn send(&self, method: reqwest::Method, body: Option<&str>) -> (u16, Value) {
        let mut request = reqwest::Client::new().request(method, &self.url).bearer_auth("fixture-owner");
        if let Some(body) = body { request = request.header("content-type", "application/json").body(body.to_owned()); }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or(Value::Null))
    }

    async fn put(&self, body: Value) -> (u16, Value) { self.send(reqwest::Method::PUT, Some(&body.to_string())).await }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

fn code(body: &Value) -> &str { body["detail"]["code"].as_str().unwrap_or_default() }

#[tokio::test]
async fn machine_and_account_tags_round_trip_and_partial_puts_keep_the_rest() {
    let fixture = Fixture::start().await;
    assert_eq!(fixture.send(reqwest::Method::GET, None).await, (200, json!({"machine": null, "accounts": {}})));
    let machine = json!({"name": "Casa", "icon": "house", "color": "green"});
    let (status, doc) = fixture.put(json!({"machine": machine})).await;
    assert_eq!((status, &doc["machine"]), (200, &machine));
    let (status, doc) = fixture.put(json!({"account": {"id": "claude:/x/.claude", "tag": {"icon": "wallet", "color": null}}})).await;
    assert_eq!(status, 200);
    assert_eq!(doc, json!({"machine": machine, "accounts": {"claude:/x/.claude": {"icon": "wallet", "color": null}}}), "a máquina fica");
    let (_, doc) = fixture.put(json!({"account": {"id": "claude:/x/.claude", "tag": null}})).await;
    assert_eq!(doc["accounts"], json!({}), "tag null remove a conta");
    let (_, doc) = fixture.put(json!({"machine": null})).await;
    assert_eq!(doc["machine"], Value::Null, "machine null remove");
    assert_eq!(fixture.send(reqwest::Method::GET, None).await.1, json!({"machine": null, "accounts": {}}));
}

#[tokio::test]
async fn invalid_bodies_are_422_and_change_nothing() {
    let fixture = Fixture::start().await;
    let long = "x".repeat(25);
    for body in [
        json!({"machine": {"name": "a", "icon": "rocket", "color": null}}),
        json!({"machine": {"name": "a", "icon": "house", "color": "red"}}),
        json!({"machine": {"name": long, "icon": "house", "color": null}}),
        json!({"machine": {"name": "a", "icon": "house", "color": null}, "extra": 1}),
        json!({"machine": {"name": "a", "icon": "house", "color": null, "extra": 1}}),
        json!({"account": {"id": "a", "tag": {"icon": "house", "color": null}}}),
        json!({"account": {"id": "", "tag": null}}),
        json!({"account": {"id": "x".repeat(513), "tag": null}}),
        json!({"account": {"id": "a"}}),
    ] {
        let (status, reply) = fixture.put(body.clone()).await;
        assert_eq!((status, code(&reply)), (422, "erro_corpo_invalido"), "{body}");
    }
    let (status, _) = fixture.put(json!({"machine": {"name": "x".repeat(24), "icon": "cloud", "color": "violet"}})).await;
    assert_eq!(status, 200, "24 caracteres cabem");
    let big = format!(r#"{{"machine":{{"name":"{}","icon":"house","color":null}}}}"#, "x".repeat(17 * 1024));
    assert_eq!(fixture.send(reqwest::Method::PUT, Some(&big)).await.0, 422, "corpo acima de 16 KB");
}

#[tokio::test]
async fn more_than_200_accounts_is_409_but_editing_an_existing_one_still_works() {
    let fixture = Fixture::start().await;
    for i in 0..200 {
        assert_eq!(fixture.put(json!({"account": {"id": format!("claude:/a{i}"), "tag": {"icon": "ticket", "color": null}}})).await.0, 200);
    }
    let (status, reply) = fixture.put(json!({"account": {"id": "claude:/new", "tag": {"icon": "ticket", "color": null}}})).await;
    assert_eq!((status, code(&reply)), (409, "tags_full"));
    assert_eq!(fixture.put(json!({"account": {"id": "claude:/a7", "tag": {"icon": "id-card", "color": "pink"}}})).await.0, 200);
}

#[tokio::test]
async fn tags_require_owner_credentials() {
    let fixture = Fixture::start().await;
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let response = reqwest::Client::new().request(method, &fixture.url)
            .header("content-type", "application/json").body(r#"{"machine":null}"#).send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert_eq!((status, code(&body)), (403, "tags_owner_only"));
    }
}
