use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use hangar_dream::config::Config as DreamConfig;
use serde_json::{json, Value};

use crate::accounts::{environment::AccountEnvironment, AccountService};
use crate::auth::TrustedHosts;
use crate::config::Config;
use crate::routes::AppState;

static TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn state(home: &std::path::Path) -> Arc<AppState> {
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: address,
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.to_string_lossy().into_owned()), ("USERPROFILE".into(), home.to_string_lossy().into_owned())].into(),
    ));
    state.hangar_dir = home.join(".hangar");
    Arc::new(state)
}

async fn call(st: &Arc<AppState>, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer fixture-owner")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = super::routes::route(st.clone(), "127.0.0.1:1".parse().unwrap(), req).await;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn dreamer_is(st: &AppState, machine: &str) {
    DreamConfig { machine: machine.into(), ..DreamConfig::default() }.save(&super::Paths::of(st).config()).unwrap();
}

#[tokio::test]
async fn only_the_registered_dreamer_writes_memory_and_only_over_what_it_read() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    dreamer_is(&st, "vps");
    let empty = hangar_dream::memory::digest(&Default::default());
    let put = |from: &str, digest: &str| json!({"root": "/srv/proj", "files": {"MEMORY.md": "- [a](a.md) — x\n", "a.md": "fato"}, "expect_digest": digest, "from_machine": from});

    let (status, body) = call(&st, "PUT", "/api/dream/memory", put("notebook", &empty)).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["detail"]["code"], "dream_not_dreamer", "{body}");

    let (status, body) = call(&st, "PUT", "/api/dream/memory", put("vps", "velho")).await;
    assert_eq!(status, 409, "{body}");

    let (status, body) = call(&st, "PUT", "/api/dream/memory", put("vps", &empty)).await;
    assert_eq!(status, 200, "{body}");
    let dir = hangar_dream::project::memory_dir(&home.path().join(".claude"), std::path::Path::new("/srv/proj"));
    assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), "fato");

    let (status, body) = call(&st, "GET", "/api/dream/memory?root=%2Fsrv%2Fproj", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(body["files"]["a.md"], "fato");
}

#[tokio::test]
async fn config_masks_the_key_and_keeps_it_when_the_mask_comes_back() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    let (status, body) = call(&st, "PUT", "/api/dream/config", json!({"api_key": "chave-de-teste-segredo-1234", "enabled": true})).await;
    assert_eq!(status, 200, "{body}");
    let masked = body["api_key"].as_str().unwrap().to_string();
    assert!(!masked.contains("segredo"));
    let (status, _) = call(&st, "PUT", "/api/dream/config", json!({"api_key": masked, "hour": "02:30"})).await;
    assert_eq!(status, 200);
    let saved = DreamConfig::load(&super::Paths::of(&st).config()).unwrap();
    assert_eq!(saved.api_key, "chave-de-teste-segredo-1234");
    assert_eq!(saved.hour, "02:30");
    let (status, body) = call(&st, "PUT", "/api/dream/config", json!({"hour": "25:00"})).await;
    assert_eq!(status, 422, "{body}");
}

#[tokio::test]
async fn run_now_is_refused_off_the_dreamer() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    dreamer_is(&st, "vps");
    let (status, body) = call(&st, "POST", "/api/dream/run", json!({})).await;
    assert_eq!(status, 409, "{body}");
}
