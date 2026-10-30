#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_plain_http_socket_keeps_the_application_lock_until_completion() {
    use tokio::io::AsyncWriteExt;
    let _test = TESTS.lock().await;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/apply-http.json")).unwrap();
    let original: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/apply-socket-disconnect.json")).unwrap();
    let payload = base64::engine::general_purpose::STANDARD.decode(fixture["payload_base64"].as_str().unwrap()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let app = axum::Router::new().route("/api/config-sync/apply",axum::routing::any(handle)).with_state(state(home.path()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener,app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    drop(APPLYING.lock().await);
    let writing = crate::config_sync::runtime::hold_writes();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("POST /api/config-sync/apply?items=hangar_prefs HTTP/1.1\r\nHost: fixture\r\nAuthorization: Bearer fixture-owner\r\nContent-Length: {}\r\n\r\n",payload.len()).as_bytes()).await.unwrap();
    socket.write_all(&payload).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            if APPLYING.try_lock().is_err() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    socket.shutdown().await.unwrap();
    drop(socket);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let held_after_disconnect = APPLYING.try_lock().is_err();
    drop(writing);
    let path = home.path().join(".claude/runtime-config.json");
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        while !path.exists() || APPLYING.try_lock().is_err() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    server.abort();
    let _ = server.await;
    assert_eq!(json!(held_after_disconnect),original["result"]["locked_after_disconnect"]);
    let data: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(data["editor"],"vim");
}
