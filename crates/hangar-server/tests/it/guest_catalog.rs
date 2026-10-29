//! A identidade do login continua sendo convidado nos catálogos compartilhados.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment},
    login_guests::{Service, Settings}, routes::AppState};
use serde_json::Value;
use std::sync::Arc;
use tower_service::Service as TowerService;

#[tokio::test]
async fn guest_catalog_login_guests_preserves_authorized_global_reads_and_denies_writes() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/http.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let service = Service::new(Settings {
        guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
        runtime_config: home.path().join("runtime-config.json"), sync_enabled: false,
        bootstrap: String::new(), secret: b"fixture".to_vec(), secure: false,
        rate_max: 10.into(), rate_window: 300.into(),
    });
    service.store.create("Ana", home.path().to_str().unwrap(), false, true,
        "ana".into(), "guest-token".into()).unwrap();
    let mut state = AppState::new(crate::fake::config(crate::refused_address(), ""));
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(), home.path().to_string_lossy().into_owned()),
        ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ("PATH".into(), String::new()),
    ].into()));
    let owner = state.cfg.auth_token.clone();
    state.share = None;
    state.login_guests = Arc::new(service);
    let mut router = hangar_server::routes::router(Arc::new(state));
    let mut differences = Vec::new();
    for path in ["/api/engines", "/api/providers", "/api/model-options", "/api/harness/codex/opcoes"] {
        assert!(fixture["gate_rows"].as_array().unwrap().iter().any(|row|
            row["method"] == "GET" && row["path"] == path && row["allowed"] == true),
            "a permissão deve vir da captura Python: {path}");
        let uri = if path == "/api/model-options" {format!("{path}?engine=missing")} else {path.into()};
        let mut responses = Vec::new();
        for token in [&owner, "guest-token"] {
            let mut request = axum::http::Request::get(&uri)
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty()).unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                "127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap()));
            let response = router.call(request).await.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            responses.push((status, body));
        }
        if responses[0] != responses[1] {
            differences.push(format!("{path}: dono {}, convidado {}", responses[0].0, responses[1].0));
        }
        let mut request = axum::http::Request::put(path)
            .header("authorization", "Bearer guest-token")
            .body(axum::body::Body::from("{}")).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap()));
        assert_eq!(router.call(request).await.unwrap().status(), 403,
            "leitura não concede escrita: {path}");
    }
    assert!(differences.is_empty(), "GUE-031: {differences:?}");
}
