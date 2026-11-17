//! A escuta das threads principais que a TUI abre durante a vida do pane. Só olha: quem chama
//! `thread/start` é a TUI (thread de outro cliente sem turno não tem rollout e o `codex resume` a recusa).
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

use super::session::Session;
use super::{Pane, Screen};

/// Quanto esperar o app-server para começar a escutar; estourar não é fatal, só perde o vínculo.
const WAIT: Duration = Duration::from_secs(60);

/// A conversa de `thread/started` que é a principal desta sessão: mesma pasta, aberta pela TUI, sem pai.
pub(crate) fn primary(message: &Value, cwd: &str) -> Option<(String, String, Value)> {
    if message.get("method").and_then(Value::as_str) != Some("thread/started") {
        return None;
    }
    let thread = message.get("params").and_then(|params| params.get("thread")).filter(|thread| thread.is_object())?;
    if !crate::runtime::codex::primary_thread(thread, Some(cwd)) {
        return None;
    }
    Some((thread["id"].as_str()?.to_owned(), thread["path"].as_str()?.to_owned(), thread.clone()))
}

pub(crate) async fn listen(session: Arc<Session>, cwd: String, gone: Arc<AtomicBool>) {
    let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let deadline = Instant::now() + WAIT;
    let mut socket = None;
    while socket.is_none() && Instant::now() < deadline {
        // Quem avisa da morte do app-server é a espera da porta, uma vez só.
        if gone.load(Ordering::Acquire) {
            return;
        }
        match tokio::time::timeout(Duration::from_secs(1), tokio_tungstenite::connect_async_with_config(session.endpoint.as_str(), Some(config), false)).await {
            Ok(Ok((ws, _))) => socket = Some(ws),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let Some(mut ws) = socket else {
        Pane.say(&format!("app-server nao respondeu em {}", session.endpoint));
        return;
    };
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"clientInfo": crate::codex_setup::reload::client_info(), "capabilities": null}});
    if ws.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    // Socket fechado: o servidor foi embora (a TUI saiu e a saída o derrubou), nada a gravar.
    while let Some(Ok(message)) = ws.next().await {
        if !message.is_text() && !message.is_binary() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&message.into_data()) else { continue };
        if let Some((id, path, thread)) = primary(&value, &cwd) {
            session.save_thread(&id, &path, Some(&thread), &Pane);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_primary_thread_of_this_folder_is_taken() {
        let started = |thread: Value| json!({"method": "thread/started", "params": {"thread": thread}});
        let base = json!({"id": "t1", "path": "/r/t1", "cwd": "/w", "source": "cli", "threadSource": "user"});
        assert_eq!(primary(&started(base.clone()), "/w").map(|(id, path, _)| (id, path)), Some(("t1".into(), "/r/t1".into())));
        let with = |field: &str, value: Value| { let mut thread = base.clone(); thread[field] = value; started(thread) };
        for refused in [with("cwd", json!("/outra")), with("source", json!("exec")), with("parentThreadId", json!("t0")),
            with("threadSource", json!("subagent")), with("canAcceptDirectInput", json!(false)), with("path", json!("")),
            json!({"method": "thread/updated", "params": {"thread": base}}), json!({"method": "thread/started", "params": {"thread": "x"}}),
            json!([1])] {
            assert!(primary(&refused, "/w").is_none(), "{refused}");
        }
        assert!(primary(&with("source", json!("vscode")), "/w").is_some());
    }
}
