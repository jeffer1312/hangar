use super::*;
use crate::{accounts::{AccountService,environment::AccountEnvironment}, auth::TrustedHosts, config::Config};
use base64::Engine;
static TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

include!("apply_socket_tests.rs");

fn state(home: &std::path::Path) -> Arc<AppState> {
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut state = AppState::new(Config {
        listen: address, upstream: address, internal_secret: "fixture-internal".into(), auth_token: "fixture-owner".into(),
        log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(),home.to_string_lossy().into_owned()),
        ("USERPROFILE".into(),home.to_string_lossy().into_owned()),
    ].into()));
    Arc::new(state)
}

#[tokio::test]
async fn busy_refuses_before_reading_or_unpacking_any_body() {
    let _test = TESTS.lock().await;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/apply-http.json")).unwrap();
    let payload = base64::engine::general_purpose::STANDARD.decode(fixture["payload_base64"].as_str().unwrap()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path());
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let _guard = APPLYING.lock().await;
    let mut compared = 0;
    for row in fixture["cases"].as_array().unwrap().iter().filter(|row|row["busy"] == true) {
        let input = match row["input"].as_str().unwrap() {
            "valid" => payload.clone(), "empty" => vec![], _ => b"invalid".to_vec(),
        };
        let request = Request::builder().method("POST").uri(format!("/api/config-sync/apply{}",row["query"].as_str().unwrap()))
            .header("authorization","Bearer fixture-owner").body(Body::from(input)).unwrap();
        let response = handle(State(state.clone()),ConnectInfo(address),request).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16);
        let body = to_bytes(response.into_body(),usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),row["body"]);
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(),0);
        compared += 1;
    }
    assert_eq!(compared,3);
}

#[tokio::test]
async fn closing_progress_response_keeps_the_application_alive() {
    let _test = TESTS.lock().await;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/apply-http.json")).unwrap();
    let original: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/apply-disconnect.json")).unwrap();
    assert_eq!(original["result"]["events"],json!(["started","closed","finished"]));
    let payload = base64::engine::general_purpose::STANDARD.decode(fixture["payload_base64"].as_str().unwrap()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path());
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    // Espera outro teste de exclusão terminar antes de abrir uma nova aplicação.
    drop(APPLYING.lock().await);
    let writing = crate::config_sync::runtime::hold_writes();
    let request = Request::builder().method("POST").uri("/api/config-sync/apply?items=hangar_prefs&stream=true")
        .header("authorization","Bearer fixture-owner").body(Body::from(payload)).unwrap();
    let response = handle(State(state),ConnectInfo(address),request).await;
    assert_eq!(response.status(),StatusCode::OK);
    let mut lines = response.into_body().into_data_stream();
    use futures_util::StreamExt;
    let first = lines.next().await.unwrap().unwrap();
    let first: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(first["type"],original["result"]["first"]["type"]);
    assert_eq!(first["phase"],original["result"]["first"]["phase"]);
    let path = home.path().join(".claude/runtime-config.json");
    assert!(!path.exists(),"a escrita permanece retida até o consumidor fechar");
    drop(lines);
    drop(writing);
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        while !path.exists() { tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
    }).await.unwrap();
    let data: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(data["editor"],"vim");
}
