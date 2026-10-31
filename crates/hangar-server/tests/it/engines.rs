//! Contratos ENG-001–015: catálogo e escrita independem do Python.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn engines_catalog_without_python() {
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen: address, upstream: crate::refused_address(), internal_secret: "fixture-internal".into(), auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.accounts = AccountService::new(AccountEnvironment::from_map([("HOME".into(), home.path().to_string_lossy().into_owned()), ("USERPROFILE".into(), home.path().to_string_lossy().into_owned())].into()));
    let router = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let response = reqwest::Client::new().get(format!("http://{address}/api/engines")).bearer_auth("fixture-owner").send().await.unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    task.abort();
    assert_eq!(status, 200, "catálogo próprio com o Python fora: {body}");
    assert_eq!(body, json!({"motores": {}, "arquivo_corrompido": false, "arquivo_caminho": home.path().join(".claude/engines.json")}));
}

proptest::proptest! {
    #[test]
    fn engines_reject_misplaced_numeric_separators(digits in "[0-9]{1,40}") {
        for malformed in [format!("_{digits}"),format!("{digits}_"),format!("1__{digits}")] {
            let result = hangar_server::engines::store::positive("context_window", &json!(malformed));
            proptest::prop_assert!(result.is_err());
        }
    }
}
