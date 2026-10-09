use hangar_server::runtime::receipt::*;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn one_echo_confirms_only_one_identical_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path, "").unwrap();
    let mut index = ReceiptIndex::new("claude", "sid");
    let cursor = index.capture(&path).unwrap();
    std::fs::write(&path,"{\"type\":\"user\",\"uuid\":\"echo-1\",\"message\":{\"content\":\"Olá\"}}\r\n").unwrap();
    index.scan(&path).unwrap();
    let row = json!({"text":"Olá"});
    let proof = index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().unwrap();
    let used = BTreeMap::from([(proof.occurrence.id,json!({"operation_id":"first"}))]);
    assert!(index.match_after(&path,&cursor,&row,&used).unwrap().is_none());
    let mut restored = ReceiptIndex::new("claude","sid");
    restored.scan(&path).unwrap();
    assert!(restored.match_after(&path,&cursor,&row,&used).unwrap().is_none());
}

#[test]
fn batched_peer_messages_confirm_one_entry_each() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path, "").unwrap();
    let mut index = ReceiptIndex::new("claude", "sid");
    let cursor = index.capture(&path).unwrap();
    let content = "Another Claude session sent a message:\n<cross-session-message from=\"uds:a.sock\" from-name=\"a\">\n[de: a] pronto\nlinha 2\n</cross-session-message>\n<cross-session-message from=\"uds:b.sock\" from-name=\"b\">\n[de: b] feito\n</cross-session-message>\n\nThis came from another Claude session.";
    std::fs::write(&path, format!("{}\n", json!({"type":"user","uuid":"u1","message":{"content":content}}))).unwrap();
    index.scan(&path).unwrap();
    let a = index.match_after(&path,&cursor,&json!({"text":"[de: a] pronto\nlinha 2"}),&BTreeMap::new()).unwrap().unwrap();
    let used = BTreeMap::from([(a.occurrence.id.clone(),json!({"operation_id":"a"}))]);
    let b = index.match_after(&path,&cursor,&json!({"text":"[de: b] feito"}),&used).unwrap().unwrap();
    assert_ne!(a.occurrence.id, b.occurrence.id);
    assert!(a.validates(&cursor,&json!({"text":"[de: a] pronto\nlinha 2"})));
}

#[test]
fn missing_or_partial_transcript_is_no_proof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    assert!(index.scan(&path).unwrap().is_empty());
    std::fs::write(&path,r#"{"type":"user","message":{"content":"Olá"}}"#).unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn enqueue_is_not_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path,"").unwrap();
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    std::fs::write(&path,"{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"Olá\"}\n").unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn cursor_is_dispatch_not_enqueue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path,"").unwrap();
    let mut index = ReceiptIndex::new("claude","sid");
    let first = index.capture(&path).unwrap();
    std::fs::write(&path,"{\"type\":\"user\",\"message\":{\"content\":\"Olá\"}}\n").unwrap();
    let second = index.capture(&path).unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&first,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_some());
    assert!(index.match_after(&path,&second,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn rewritten_file_and_old_conversation_are_no_proof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path,"{\"type\":\"user\",\"message\":{\"content\":\"Anterior\"}}\n").unwrap();
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    std::fs::write(&path,"{\"type\":\"user\",\"message\":{\"content\":\"Reescrito\"}}\n{\"type\":\"user\",\"message\":{\"content\":\"Olá\"}}\n").unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
    let mut other = ReceiptIndex::new("claude","other-sid");
    other.scan(&path).unwrap();
    assert!(other.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn steer_attachment_is_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path,"").unwrap();
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    std::fs::write(&path,"{\"type\":\"attachment\",\"attachment\":{\"type\":\"queued_command\",\"prompt\":[{\"type\":\"text\",\"text\":\"Olá 🌎\"}]}}\n").unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá 🌎"}),&BTreeMap::new()).unwrap().is_some());
}

#[test]
fn rewrite_before_the_read_tail_is_seen_in_the_file_not_in_a_memory_copy() {
    // O índice não guarda o transcript: a âncora do despacho é relida do arquivo.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    let first = "{\"type\":\"user\",\"message\":{\"content\":\"Anterior\"}}\n";
    std::fs::write(&path,first).unwrap();
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    let echo = format!("{{\"type\":\"user\",\"message\":{{\"content\":\"Olá\"}},\"pad\":\"{}\"}}\n","x".repeat(400));
    std::fs::write(&path,format!("{first}{echo}")).unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_some());
    let changed = first.replace("Anterior","Trocado!");
    assert_eq!(changed.len(),first.len());
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    std::io::Write::write_all(&mut file,changed.as_bytes()).unwrap();
    drop(file);
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn first_message_dispatched_before_the_transcript_exists_is_confirmed() {
    // A primeira mensagem de uma sessão nova sai antes de a CLI criar o arquivo: o cursor não tem
    // identidade, e tudo o que o arquivo da mesma conversa traz depois é posterior a ele.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    assert!(cursor.file_identity.is_none());
    // Como o Claude grava: a conversa e o horário vêm em cada linha.
    std::fs::write(&path,format!("{}\n",json!({"type":"user","sessionId":"sid","timestamp":chrono::Utc::now().to_rfc3339(),
        "message":{"content":"um"}}))).unwrap();
    index.scan(&path).unwrap();
    let row = json!({"text":"um"});
    let proof = index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().expect("primeira mensagem confirma");
    assert!(proof.validates(&cursor,&row));
    let mut other = ReceiptIndex::new("claude","outra");
    other.scan(&path).unwrap();
    assert!(other.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn terminal_runtime_absent_cursor_needs_current_conversation_and_new_timestamp() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("chat.jsonl");
    let mut index=ReceiptIndex::new("claude","sid"); let cursor=index.capture(&path).unwrap();
    for (sid,ts) in [("sid","2020-01-01T00:00:00Z".to_string()),("other",chrono::Utc::now().to_rfc3339())] {
        std::fs::write(&path,format!("{}\n",json!({"type":"user","sessionId":sid,"timestamp":ts,"message":{"content":"Olá"}}))).unwrap();
        index.scan(&path).unwrap(); assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_none());
    }
}

/// O Codex grava a conversa só no `session_meta`, não nas linhas da fala.
fn codex_rollout(path:&std::path::Path, conversation:Option<&str>) {
    let now = chrono::Utc::now().to_rfc3339();
    let mut lines = String::new();
    if let Some(id) = conversation { lines += &format!("{}\n",json!({"timestamp":now,"type":"session_meta","payload":{"id":id}})); }
    lines += &format!("{}\n",json!({"timestamp":now,"type":"response_item","payload":{"type":"message","role":"user",
        "content":[{"type":"input_text","text":"um"}]}}));
    std::fs::write(path,lines).unwrap();
}

#[test]
fn codex_first_message_without_rollout_is_confirmed_once() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("rollout.jsonl");
    let mut index=ReceiptIndex::new("codex","thread-1"); let cursor=index.capture(&path).unwrap();
    codex_rollout(&path,Some("thread-1"));
    index.scan(&path).unwrap();
    let row=json!({"text":"um"});
    let proof=index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().expect("primeira mensagem do Codex confirma");
    assert!(proof.validates(&cursor,&row));
    let used=BTreeMap::from([(proof.occurrence.id.clone(),json!({}))]);
    assert!(index.match_after(&path,&cursor,&row,&used).unwrap().is_none());
}

#[test]
fn codex_rollout_of_another_conversation_does_not_confirm() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("rollout.jsonl");
    let mut index=ReceiptIndex::new("codex","thread-1"); let cursor=index.capture(&path).unwrap();
    codex_rollout(&path,Some("thread-2"));
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"um"}),&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn codex_rollout_without_session_meta_falls_back_to_the_cursor_without_file() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("rollout.jsonl");
    let mut index=ReceiptIndex::new("codex","thread-1"); let cursor=index.capture(&path).unwrap();
    codex_rollout(&path,None);
    index.scan(&path).unwrap();
    let proof=index.match_after(&path,&cursor,&json!({"text":"um"}),&BTreeMap::new()).unwrap().expect("sem session_meta vale o cursor sem arquivo");
    assert!(proof.occurrence.identity_unprovable);
}
