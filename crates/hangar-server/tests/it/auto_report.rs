//! `GET`/`PUT /api/auto-report`: o consentimento do relatório automático da atualização. Só o dono lê
//! ou muda; ausente é `null` (ainda não respondido).
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

const OWNER: &str = "auto-owner";

struct Fixture { home: tempfile::TempDir, base: String, task: tokio::task::JoinHandle<()> }

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

impl Fixture {
    async fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, upstream: crate::refused_address(), internal_secret: "auto-internal".into(),
            auth_token: OWNER.into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into()));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}"), task }
    }

    async fn call(&self, owner: bool, headers: &[(&str, &str)], body: Option<&str>) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("{}/api/auto-report", self.base);
        let mut req = match body {
            Some(body) => client.put(url).header("content-type", "application/json").body(body.to_owned()),
            None => client.get(url),
        };
        if owner { req = req.bearer_auth(OWNER); }
        for (k, v) in headers { req = req.header(*k, *v); }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        (status, serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap_or(Value::Null))
    }
}

fn code_of(body: &Value) -> &Value { &body["detail"]["code"] }

#[tokio::test(flavor = "multi_thread")]
async fn unanswered_is_null_and_put_is_kept() {
    let f = Fixture::start().await;
    assert_eq!(f.call(true, &[], None).await, (200, json!({"auto_report": null})));
    assert_eq!(f.call(true, &[], Some(r#"{"auto_report": true}"#)).await, (200, json!({"auto_report": true})));
    assert_eq!(f.call(true, &[], None).await, (200, json!({"auto_report": true})));
    let saved: Value = serde_json::from_slice(&std::fs::read(f.home.path().join(".hangar/auto-report.json")).unwrap()).unwrap();
    assert_eq!(saved, json!({"auto_report": true}));
    assert_eq!(f.call(true, &[], Some(r#"{"auto_report": false}"#)).await, (200, json!({"auto_report": false})));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_reads_or_changes_it() {
    let f = Fixture::start().await;
    let (status, body) = f.call(false, &[], None).await;
    assert_eq!((status, code_of(&body)), (403, &json!("auto_report_owner_only")));
    let (status, body) = f.call(true, &[("x-hangar-guest", "1")], Some(r#"{"auto_report": true}"#)).await;
    assert_eq!((status, code_of(&body)), (403, &json!("auto_report_owner_only")));
    assert!(!f.home.path().join(".hangar/auto-report.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn body_outside_the_contract_is_400() {
    let f = Fixture::start().await;
    for body in [r#"{"auto_report": "sim"}"#, r#"{"auto_report": null}"#, r#"{"auto_report": true, "x": 1}"#, "{}", "nada"] {
        let (status, reply) = f.call(true, &[], Some(body)).await;
        assert_eq!((status, code_of(&reply)), (400, &json!("auto_report_invalid")), "{body}");
    }
    assert_eq!(f.call(true, &[], None).await, (200, json!({"auto_report": null})));
}
