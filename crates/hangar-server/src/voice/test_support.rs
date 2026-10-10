//! App-server falso para os testes da chamada.
use super::call::Spawn;
use super::rpc::Rpc;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Responde o necessário, devolve o que recebeu e aceita notificações empurradas pelo teste. Cada `thread/realtime/start`
/// devolve a resposta SDP `v=0 answer <n>`.
pub fn fake_app_server() -> (Spawn, tokio::sync::mpsc::UnboundedReceiver<Value>, tokio::sync::mpsc::UnboundedSender<Value>) {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let (seen_tx, seen) = tokio::sync::mpsc::unbounded_channel();
    let (push, mut pushed) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let (read, write) = tokio::io::split(theirs);
        let write = std::sync::Arc::new(tokio::sync::Mutex::new(write));
        let w = write.clone();
        tokio::spawn(async move { while let Some(v) = pushed.recv().await { let _ = w.lock().await.write_all(format!("{v}\n").as_bytes()).await; } });
        let mut lines = BufReader::new(read).lines();
        let mut starts = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: Value = serde_json::from_str(&line).unwrap();
            let _ = seen_tx.send(msg.clone());
            let result = match msg["method"].as_str() {
                Some("initialize") => json!({}),
                Some("config/read") => json!({"config": {"model": "gpt-test"}}),
                Some("thread/start") => json!({"thread": {"id": "t1"}, "model": "gpt-test"}),
                Some("thread/realtime/start") => {
                    starts += 1;
                    let sdp = format!("v=0 answer {starts}");
                    let w = write.clone();
                    tokio::spawn(async move { let _ = w.lock().await.write_all(format!("{}\n", json!({"method": "thread/realtime/sdp", "params": {"threadId": "t1", "sdp": sdp}})).as_bytes()).await; });
                    json!({})
                }
                Some(_) => json!({}),
                None => continue,
            };
            if msg.get("id").is_some() { let _ = write.lock().await.write_all(format!("{}\n", json!({"id": msg["id"], "result": result})).as_bytes()).await; }
        }
    });
    let (r, w) = tokio::io::split(ours);
    let spawn: Spawn = Box::new(move || Box::pin(async move { Ok(Rpc::over_lines(r, w)) }));
    (spawn, seen, push)
}

/// Fala do usuário no turno `turn`, como a voz delega: libera as ferramentas desse turno (`SpokenTurns`).
pub fn push_speech(push: &tokio::sync::mpsc::UnboundedSender<Value>, turn: &str, text: &str) {
    let delegation = format!("<realtime_delegation>\n<input>{text}</input>\n<transcript_delta>user: {text}</transcript_delta>\n</realtime_delegation>");
    let _ = push.send(json!({"method": "item/started", "params": {"threadId": "t1", "turnId": turn,
        "item": {"type": "userMessage", "id": format!("item-{turn}-{}", text.len()), "content": [{"type": "text", "text": delegation}]}}}));
}

/// Chamada de ferramenta do organizador; a resposta aparece no `seen` com o mesmo `id` e `result`.
pub fn push_tool(push: &tokio::sync::mpsc::UnboundedSender<Value>, id: &str, name: &str, args: Value, turn: &str) {
    let _ = push.send(json!({"id": id, "method": "item/tool/call", "params": {"tool": name, "arguments": args, "turnId": turn, "threadId": "t1"}}));
}
