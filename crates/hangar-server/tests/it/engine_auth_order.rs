//! Casos capturados após a integração das rotas e do diagnóstico.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use serde_json::Value;
use std::sync::Arc;

#[tokio::test]
async fn engine_auth_order_matches_original_http() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/auth-order.json"
    ))
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_pairs([
        ("HOME", root.path().to_string_lossy().into_owned()),
        ("USERPROFILE", root.path().to_string_lossy().into_owned()),
    ]));
    let app = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let bytes = case["hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let mut request = client
            .request(
                reqwest::Method::from_bytes(case["method"].as_str().unwrap().as_bytes()).unwrap(),
                format!("http://{address}{}", case["path"].as_str().unwrap()),
            )
            .header("origin", "https://fixture.invalid")
            .body(bytes);
        if let Some(value) = case["content_type"].as_str() {
            request = request.header("content-type", value);
        }
        if let Some(value) = case["origin"].as_str() {
            request = request.header("origin", value);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap();
        let expected = case["headers"].as_object().unwrap();
        let header_mismatch = expected
            .iter()
            .any(|(key, value)| headers.get(key).and_then(|v| v.to_str().ok()) != value.as_str())
            || headers.keys().any(|key| {
                key.as_str().starts_with("access-control-") && !expected.contains_key(key.as_str())
            });
        if status as u64 != case["status"].as_u64().unwrap()
            || body != case["body"].as_str().unwrap()
            || header_mismatch
        {
            failures.push(format!(
                "{} {:?} {:?}: status={}, body={}, headers={:?}; esperado {}",
                case["name"], case["content_type"], case["origin"], status, body, headers, case
            ));
        }
    }
    task.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
