//! Resposta do agy ainda a caminho: recado de prioridade alta não lido na caixa da conversa, ou o
//! `SYSTEM_MESSAGE` que acordou o agente sendo o último passo, sem resposta do modelo depois dele.
//! Lido do disco porque o ACP não avisa: entre o fim de uma tarefa de fundo e a resposta do modelo passam segundos.
use std::path::Path;

/// Arquivo maior que isto não é recado nem recibo: é tratado como ausente.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_MESSAGES: usize = 256;
const TAIL_BYTES: u64 = 256 * 1024;

const HIGH: &str = "MESSAGE_PRIORITY_HIGH";
const LOW: &str = "MESSAGE_PRIORITY_LOW";

/// Falha de leitura, atalho ou arquivo grande demais conta como nada pendente: o turno segue para o fechamento normal.
pub fn pending_reply(brain_dir: &Path, conversation: &str) -> bool {
    if !crate::paths::valid_conversation_id(conversation) { return false; }
    let dir = brain_dir.join(conversation).join(".system_generated");
    unread_high_message(&dir.join("messages")) || trailing_wake(&dir.join("logs").join("transcript.jsonl"))
}

fn small_file(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    (meta.is_file() && meta.len() <= MAX_FILE_BYTES).then(|| std::fs::read_to_string(path).ok()).flatten()
}

fn unread_high_message(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    let names: Vec<String> = entries.filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".json") && name != "read.json").collect();
    if names.is_empty() || names.len() > MAX_MESSAGES { return false; }
    let receipts: serde_json::Value = match small_file(&dir.join("read.json")) {
        Some(raw) => match serde_json::from_str(&raw) { Ok(value) => value, Err(_) => return false },
        None if dir.join("read.json").exists() => return false,
        None => serde_json::Value::Null,
    };
    let read = |id: &str| receipts[id] == true;
    names.iter().any(|name| {
        if read(&name[..name.len() - 5]) { return false; }
        let Some(message) = small_file(&dir.join(name)).and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok()) else { return false };
        !message["id"].as_str().is_some_and(read) && message["priority"] == HIGH
    })
}

fn trailing_wake(transcript: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(meta) = std::fs::symlink_metadata(transcript) else { return false };
    if !meta.is_file() { return false; }
    let Ok(mut file) = std::fs::File::open(transcript) else { return false };
    let start = meta.len().saturating_sub(TAIL_BYTES);
    let mut tail = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut tail).is_err() { return false; }
    let text = String::from_utf8_lossy(&tail);
    let Some(line) = text.trim_end().rsplit('\n').next() else { return false };
    let Ok(step) = serde_json::from_str::<serde_json::Value>(line) else { return false };
    if step["type"] != "SYSTEM_MESSAGE" { return false; }
    let content = step["content"].as_str().unwrap_or("");
    content.contains(&format!("priority={HIGH}")) || !content.contains(&format!("priority={LOW}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ID: &str = "03f5ea12-b3b0-4fea-85e0-5b4aea8010b9";

    fn conversation(root: &Path) -> std::path::PathBuf {
        let dir = root.join(ID).join(".system_generated");
        fs::create_dir_all(dir.join("messages")).unwrap();
        fs::create_dir_all(dir.join("logs")).unwrap();
        dir
    }

    fn message(dir: &Path, id: &str, priority: &str) {
        let body = format!(r#"{{"id":"{id}","recipient":"{ID}","sender":"x/task-1","priority":"{priority}","content":"fim"}}"#);
        fs::write(dir.join("messages").join(format!("{id}.json")), body).unwrap();
    }

    fn last_step(dir: &Path, kind: &str, content: &str) {
        let lines = format!("{{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"content\":\"a\"}}\n{}\n",
            serde_json::json!({"step_index":2,"type":kind,"content":content}));
        fs::write(dir.join("logs").join("transcript.jsonl"), lines).unwrap();
    }

    #[test]
    fn an_unread_high_priority_message_is_a_reply_on_its_way() {
        let root = tempfile::tempdir().unwrap();
        let dir = conversation(root.path());
        message(&dir, "m1", "MESSAGE_PRIORITY_HIGH");
        assert!(pending_reply(root.path(), ID));
    }

    #[test]
    fn a_read_or_low_priority_message_is_not() {
        let root = tempfile::tempdir().unwrap();
        let dir = conversation(root.path());
        message(&dir, "m1", "MESSAGE_PRIORITY_HIGH");
        message(&dir, "m2", "MESSAGE_PRIORITY_LOW");
        fs::write(dir.join("messages").join("read.json"), r#"{"m1":true}"#).unwrap();
        assert!(!pending_reply(root.path(), ID));
    }

    #[test]
    fn a_trailing_wake_step_without_a_model_reply_is_a_reply_on_its_way() {
        let root = tempfile::tempdir().unwrap();
        let dir = conversation(root.path());
        last_step(&dir, "SYSTEM_MESSAGE", "[Message] sender=x/task-1 priority=MESSAGE_PRIORITY_HIGH content=fim");
        assert!(pending_reply(root.path(), ID));
        last_step(&dir, "SYSTEM_MESSAGE", "[Message] sender=x priority=MESSAGE_PRIORITY_LOW content=nota");
        assert!(!pending_reply(root.path(), ID));
        last_step(&dir, "PLANNER_RESPONSE", "pronto");
        assert!(!pending_reply(root.path(), ID));
    }

    #[test]
    fn a_conversation_id_out_of_format_or_a_missing_folder_reads_as_nothing_pending() {
        let root = tempfile::tempdir().unwrap();
        assert!(!pending_reply(root.path(), ID));
        assert!(!pending_reply(root.path(), "../fora"));
    }
}
