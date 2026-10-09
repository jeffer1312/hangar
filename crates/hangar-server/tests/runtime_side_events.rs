mod fake;

use fake::*;
use serde_json::json;

#[tokio::test]
async fn history_projection_error_not_304() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let router = axum::Router::new().fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE });
    let task = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
    let server = spawn_server(config(upstream, "127.0.0.1")).await;
    let response = client().get(format!("http://{server}/api/sessions/s/history?token={OWNER}"))
        .header("if-none-match", "old").send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    task.abort();
}

#[tokio::test]
async fn two_clients_one_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.jsonl");
    append_lines(&path, 0..1);
    let (python, upstream) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &path));
    let server = spawn_server(config(upstream, "127.0.0.1")).await;
    let mut a = sse(open_events(server, "s", "", &[]).await);
    let mut b = sse(open_events(server, "s", "", &[]).await);
    next_any(&mut a).await;
    next_any(&mut b).await;
    wait_until(|| python.side_conns() == 1).await;
    // O estado do Claude sem terminal é do feed do Rust; `stats` segue vindo da conexão interna.
    let stats = json!({"turns":1});
    python.push_side("stats", &stats.to_string());
    assert_eq!(next_named(&mut a, "stats").await.data, stats.to_string());
    assert_eq!(next_named(&mut b, "stats").await.data, stats.to_string());
    assert_eq!(python.side_conns(), 1);
}

#[tokio::test]
async fn provider_switch_no_double_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.jsonl");
    append_lines(&path, 0..1);
    let (python, upstream) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &path));
    let server = spawn_server(config(upstream, "127.0.0.1")).await;
    let mut events = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(id_of(&messages(&mut events, 1).await[0]), "u0");
    wait_until(|| python.side_conns() == 1).await;
    let info = info_json("codex", &path);
    python.set_info(info.clone());
    python.push_side("info", &info.to_string());
    next_named(&mut events, "reset").await;
    python.push_side("state", &json!({"session":"s", "state":"idle", "headless":true}).to_string());
    let state = next_named(&mut events, "state").await;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&state.data).unwrap()["state"], "idle");
    assert_eq!(python.side_conns(), 1);
}
