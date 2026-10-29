use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},
    routes::AppState};
use serde_json::{Value,json};
use std::sync::Arc;
use tower_service::Service;

#[tokio::test]
async fn runtime_http_preserves_captured_responses_and_configuration_files() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/config-http.json")).unwrap();
    let mut differences = Vec::new();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".hangar/current");
        std::fs::create_dir_all(&root).unwrap();
        hangar_server::boot_version::publish(&root, hangar_server::boot_version::Snapshot {
            describe:"fixture-running".into(),readable:None,
        });
        let mut state = AppState::new(crate::fake::config(crate::refused_address(), ""));
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(),home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(),home.path().to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(),String::new()),
            ("PATH".into(),String::new()),
        ].into()));
        state.share = None;
        let mut request = axum::http::Request::builder()
            .method(case["method"].as_str().unwrap()).uri("/api/config")
            .header("authorization",format!("Bearer {}",state.cfg.auth_token))
            .header("content-type","application/json")
            .body(axum::body::Body::from(case["raw"].as_str().unwrap().to_owned())).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap()));
        let response = hangar_server::routes::router(Arc::new(state)).call(request).await.unwrap();
        let status = response.status().as_u16();
        let allow = response.headers().get("allow").map(|v|v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(),usize::MAX).await.unwrap();
        let raw = String::from_utf8(bytes.to_vec()).unwrap().replace(home.path().to_str().unwrap(),"⟦HOME⟧");
        let body = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
        let expected = if case["status"] == 400 && case["raw"].as_str().unwrap().contains("\\ud800") {
            json!({"detail":"runtime-config.json contém surrogate Unicode isolado; arquivo preservado"})
        } else {case["body"].clone()};
        if status != case["status"].as_u64().unwrap() as u16 || body != expected || json!(allow) != case["allow"] {
            differences.push(format!("{index}: resposta {status}, esperada {}; corpo igual={}; leitura={:?}",case["status"],body==expected,body.get("somente_leitura")));
        }
        for file in [".claude/runtime-config.json",".claude/settings.json"] {
            let actual = std::fs::read_to_string(home.path().join(file)).ok();
            if actual.as_deref() != case["files"].get(file).and_then(Value::as_str) {
                differences.push(format!("{index}: arquivo {file}"));
            }
        }
    }
    assert!(differences.is_empty(),"configuração HTTP: {differences:?}");
}
