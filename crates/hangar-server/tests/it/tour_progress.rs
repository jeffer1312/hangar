//! O que um computador já viu do tour vale para outro ligado ao mesmo servidor, e nunca regride.
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
        Self { home, base: format!("http://{address}/api/tour-progress"), task }
    }

    async fn send(&self, method: reqwest::Method, body: Option<&str>) -> reqwest::Response {
        let mut request = reqwest::Client::new().request(method, &self.base).bearer_auth("fixture-owner");
        if let Some(body) = body { request = request.header("content-type", "application/json").body(body.to_owned()); }
        request.send().await.unwrap()
    }

    async fn put(&self, body: Value) -> Value {
        let response = self.send(reqwest::Method::PUT, Some(&body.to_string())).await;
        assert_eq!(response.status(), 200, "PUT {body}");
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
    }

    async fn get(&self) -> Value {
        let response = self.send(reqwest::Method::GET, None).await;
        assert_eq!(response.status(), 200);
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
    }

    fn file(&self) -> std::path::PathBuf { self.home.path().join(".hangar/tour-progress.json") }

    async fn stop(&mut self) { self.task.abort(); let _ = (&mut self.task).await; }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

#[tokio::test]
async fn empty_server_answers_nothing_seen() {
    let mut fixture = Fixture::start().await;
    assert_eq!(fixture.get().await, json!({"tour": null, "tips": [], "tips_off": null}));
    assert!(!fixture.file().exists(), "GET não cria o arquivo");
    fixture.stop().await;
}

#[tokio::test]
async fn tour_state_never_downgrades() {
    let mut fixture = Fixture::start().await;
    assert_eq!(fixture.put(json!({"tour": "skipped"})).await["tour"], "skipped");
    assert_eq!(fixture.put(json!({"tour": null})).await["tour"], "skipped");
    assert_eq!(fixture.put(json!({})).await["tour"], "skipped");
    assert_eq!(fixture.get().await["tour"], "skipped");
    assert_eq!(fixture.put(json!({"tour": "done"})).await["tour"], "done");
    assert_eq!(fixture.put(json!({"tour": "skipped"})).await["tour"], "done");
    assert_eq!(fixture.get().await["tour"], "done");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(fixture.file()).unwrap().permissions().mode() & 0o777, 0o600);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn tips_are_united_and_the_newest_switch_wins() {
    let mut fixture = Fixture::start().await;
    fixture.put(json!({"tips": ["composer", "board"], "tips_off": {"on": true, "at": "2026-10-30T12:00:00Z"}})).await;
    let merged = fixture.put(json!({"tips": ["board", "archive"], "tips_off": {"on": false, "at": "2026-10-30T11:00:00Z"}})).await;
    assert_eq!(merged["tips"], json!(["archive", "board", "composer"]));
    assert_eq!(merged["tips_off"], json!({"on": true, "at": "2026-10-30T12:00:00Z"}));
    let newer = fixture.put(json!({"tips_off": {"on": false, "at": "2026-10-30T13:00:00Z"}})).await;
    assert_eq!(newer["tips_off"], json!({"on": false, "at": "2026-10-30T13:00:00Z"}));
    assert_eq!(newer["tips"], json!(["archive", "board", "composer"]));
    assert_eq!(fixture.get().await, newer);
    fixture.stop().await;
}

#[tokio::test]
async fn unreadable_file_is_503_and_stays_untouched() {
    let mut fixture = Fixture::start().await;
    let path = fixture.file();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for broken in [b"broken".as_slice(), br#"{"tour":null,"tips":[],"tips_off":{"on":true,"at":"ontem"}}"#.as_slice()] {
        std::fs::write(&path, broken).unwrap();
        for response in [fixture.send(reqwest::Method::GET, None).await, fixture.send(reqwest::Method::PUT, Some(r#"{"tour":"done"}"#)).await] {
            assert_eq!(response.status(), 503);
            let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
            assert_eq!(body["detail"]["code"], "tour_progress_unreadable");
        }
        assert_eq!(std::fs::read(&path).unwrap(), broken);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn malformed_body_is_422_and_saves_nothing() {
    let mut fixture = Fixture::start().await;
    for body in ["not json", r#"{"tour":"visto"}"#, r#"{"tips":"composer"}"#,
        r#"{"tips_off":{"on":true,"at":"ontem"}}"#, r#"{"tips_off":{"on":true}}"#] {
        let response = fixture.send(reqwest::Method::PUT, Some(body)).await;
        assert_eq!(response.status(), 422, "{body}");
        let value: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert_eq!(value["detail"]["code"], "erro_corpo_invalido", "{body}");
    }
    assert!(!fixture.file().exists());
    fixture.stop().await;
}

#[tokio::test]
async fn a_field_from_a_newer_app_is_ignored() {
    let mut fixture = Fixture::start().await;
    let merged = fixture.put(json!({"tour": "done", "extra": 1, "tips_off": {"on": true, "at": "2026-10-30T12:00:00Z", "by": "x"}})).await;
    assert_eq!(merged, json!({"tour": "done", "tips": [], "tips_off": {"on": true, "at": "2026-10-30T12:00:00Z"}}));
    assert_eq!(fixture.get().await, merged);
    fixture.stop().await;
}

#[tokio::test]
async fn tour_progress_requires_owner_credentials() {
    let mut fixture = Fixture::start().await;
    let client = reqwest::Client::new();
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let response = client.request(method, &fixture.base)
            .header("content-type", "application/json").body(r#"{"tour":"done"}"#).send().await.unwrap();
        assert_eq!(response.status(), 403);
    }
    assert!(!fixture.file().exists());
    fixture.stop().await;
}
