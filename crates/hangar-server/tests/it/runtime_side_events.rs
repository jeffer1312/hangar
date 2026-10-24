use crate::fake;

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
    // O estado e a faixa do Claude sem terminal são do Rust: um produtor só para os dois aparelhos.
    let line = json!({"type":"assistant","timestamp":"2026-10-23T12:00:02Z","message":{"id":"m1","usage":{"input_tokens":4,"output_tokens":6}}});
    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()).unwrap();
    let stats = next_named(&mut a, "stats").await.data;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&stats).unwrap()["out_tok"], 6, "{stats}");
    assert_eq!(next_named(&mut b, "stats").await.data, stats);
    // S1: a conta recomeça sem nenhuma chamada (arquivo truncado) e a faixa velha sai da tela.
    let user = json!({"type":"user","timestamp":"2026-10-23T12:00:05Z","message":{"content":"oi"}});
    std::fs::write(&path, format!("{user}\n")).unwrap();
    assert_eq!(next_named(&mut a, "stats").await.data, "null");
}

#[tokio::test]
async fn provider_switch_resets_at_once_without_python() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.jsonl");
    append_lines(&path, 0..1);
    let (python, upstream) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &path));
    let server = spawn_server(config(upstream, "127.0.0.1")).await;
    let mut events = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(id_of(&messages(&mut events, 1).await[0]), "u0");
    python.set_info(info_json("codex", &path));
    readings(&python, 1).await;
    // Provider trocado vale na hora.
    next_named(&mut events, "reset").await;
    assert_eq!(python.hits_to("/internal/sessions/s/side-events"), 0);
}
