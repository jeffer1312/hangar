use super::WRITES;
use crate::login_guests::{Service, Settings};
use axum::{body::{Body, to_bytes}, extract::Request};
use serde_json::{Value, json};
use std::{sync::{Arc, mpsc}, time::Duration};

#[test]
fn sync_admin_uses_the_runtime_writer_lock_and_preserves_other_preferences() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/sync-lock.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("runtime-config.json");
    std::fs::write(&path, r#"{"sync":true}"#).unwrap();
    let service = Arc::new(Service::new(Settings {
        guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
        runtime_config: path.clone(), sync_enabled: false, bootstrap: String::new(),
        secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into(),
    }));
    let guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let (start_tx, start_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            start_tx.send(()).unwrap();
            let response = service.respond(true, "127.0.0.1".into(), false,
                Request::builder().method("POST").uri("/api/sync/setup/disable").body(Body::empty()).unwrap()).await;
            let status = response.status();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            done_tx.send((status, serde_json::from_slice::<Value>(&body).unwrap())).unwrap();
        });
    });
    start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = done_rx.recv_timeout(Duration::from_millis(100)).ok();
    std::fs::write(&path, r#"{"sync":true,"editor":"vim"}"#).unwrap();
    drop(guard);
    let blocked = early.is_none();
    let (status, response) = early.unwrap_or_else(|| done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    worker.join().unwrap();
    assert_eq!(status, 200);
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(json!({"blocked_by_runtime_writer":blocked,"response":response,"file":disk}), fixture["result"]);
}
