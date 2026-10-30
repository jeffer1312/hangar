use hangar_server::login_guests::{Service, Settings};
use axum::{body::{Body, to_bytes}, extract::Request};
use serde_json::Value;
use std::sync::Arc;

#[tokio::test]
async fn sync_enabled_preserves_persisted_truth_and_environment_default() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/sync-enabled.json")).unwrap();
    let mut differences = Vec::new();
    for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("runtime-config.json");
        std::fs::write(&path, row["document"].as_str().unwrap()).unwrap();
        let service = Arc::new(Service::new(Settings {
            guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
            runtime_config: path, sync_enabled: row["default"].as_bool().unwrap(), bootstrap: String::new(),
            secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into(),
        }));
        let response = service.respond(true, "127.0.0.1".into(), false,
            Request::builder().uri("/api/sync/setup").body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), 200, "caso {index}");
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        if body != row["response"] { differences.push(format!("{index}: {body} versus {}", row["response"])); }
    }
    assert!(differences.is_empty(), "{differences:?}");
}
