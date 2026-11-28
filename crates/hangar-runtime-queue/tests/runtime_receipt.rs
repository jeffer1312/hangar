use hangar_runtime_queue::ReceiptIndex;
use serde_json::json;
use std::collections::BTreeMap;

include!("fixtures/merged_inputs.rs");

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
fn merged_user_message_confirms_one_entry_per_text_block() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path, "").unwrap();
    let mut index = ReceiptIndex::new("claude", "sid");
    let cursor = index.capture(&path).unwrap();
    std::fs::write(&path, format!("{}\n", merged_inputs_line("sid","2026-10-13T13:50:29.778Z"))).unwrap();
    index.scan(&path).unwrap();
    let mut used = BTreeMap::new();
    for (n,text) in MERGED_INPUTS.into_iter().enumerate() {
        let row = json!({"text":text});
        let proof = index.match_after(&path,&cursor,&row,&used).unwrap();
        let proof = proof.unwrap_or_else(||panic!("entrada {n} sem prova na mensagem juntada: {text}"));
        assert!(proof.validates(&cursor,&row));
        assert!(used.insert(proof.occurrence.id.clone(),json!({"operation_id":n})).is_none(),"um bloco provou duas entradas");
    }
    // Cada bloco vale uma vez: uma quarta entrada com o mesmo texto da primeira não acha prova.
    assert!(index.match_after(&path,&cursor,&json!({"text":MERGED_INPUTS[0]}),&used).unwrap().is_none());
}

#[test]
fn merged_user_message_shows_every_block_and_absorbs_each_entry_in_history() {
    use hangar_transcript::{merged_history, HistoryRequest, Provider, TAIL_WINDOW};
    let dir = tempfile::tempdir().unwrap();
    let (jsonl, queue) = (dir.path().join("chat.jsonl"), dir.path().join("fila.jsonl"));
    // A conversa começa antes do envio: entrada anterior ao começo seria de outra conversa e sairia por isso.
    let opening = json!({"type":"user","uuid":"u-0","timestamp":"2026-10-13T13:50:00.000Z","message":{"content":"inicio"}});
    std::fs::write(&jsonl, format!("{opening}\n{}\n", merged_inputs_line("sid","2026-10-13T13:50:29.778Z"))).unwrap();
    // Entregues e ainda sem a baixa do recibo: o /history as esconde pela linha que cada uma tem na mensagem juntada.
    let sent = hangar_transcript::ts_of_iso("2026-10-13T13:50:20.000Z").unwrap();
    let rows: String = MERGED_INPUTS.iter().enumerate()
        .map(|(n,text)| format!("{}\n", json!({"id":format!("m{n}"),"text":text,"ts":sent + n as f64,"delivered":true})))
        .collect();
    std::fs::write(&queue, rows).unwrap();
    let events = merged_history(&HistoryRequest { provider: Provider::Claude, jsonl, queue: Some(queue), limit: None, tail_window: TAIL_WINDOW }).unwrap();
    let texts: Vec<(&str,&str)> = events.iter().map(|e|(e.id.as_str(),e.text.as_deref().unwrap_or(""))).collect();
    let expected = MERGED_INPUTS.iter().map(|t|t.trim()).collect::<Vec<_>>().join("\n");
    assert_eq!(texts, [("u-0", "inicio"), ("17729883-e589-4998-b1ee-45d3e73dcd56", expected.as_str())],
        "uma mensagem com os três blocos e nenhuma bolha da fila");
    assert_eq!(events[1].image_count, Some(3));
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
fn removal_absorbed_mid_turn_is_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path,"").unwrap();
    let mut index = ReceiptIndex::new("claude","sid");
    let cursor = index.capture(&path).unwrap();
    std::fs::write(&path,"{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"Olá\"}\n\
        {\"type\":\"queue-operation\",\"operation\":\"remove\",\"content\":\"Olá\",\"reason\":\"absorbed_mid_turn\"}\n").unwrap();
    index.scan(&path).unwrap();
    assert!(index.match_after(&path,&cursor,&json!({"text":"Olá"}),&BTreeMap::new()).unwrap().is_some(),
        "a mensagem absorvida no meio do turno saiu da fila interna: é a prova do consumo");
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

#[test]
fn absent_cursor_enqueue_needs_current_conversation_and_new_timestamp() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("chat.jsonl");
    let mut index=ReceiptIndex::new("claude","sid"); let cursor=index.capture(&path).unwrap();
    let row=json!({"text":"Olá"});
    for (sid,ts) in [("sid","2020-01-01T00:00:00Z".to_string()),("other",chrono::Utc::now().to_rfc3339())] {
        std::fs::write(&path,format!("{}\n",json!({"type":"queue-operation","operation":"enqueue","sessionId":sid,"timestamp":ts,"content":"Olá"}))).unwrap();
        index.scan(&path).unwrap(); assert!(!index.enqueued_after(&path,&cursor,&row).unwrap(),"{sid}");
    }
    std::fs::write(&path,format!("{}\n",json!({"type":"queue-operation","operation":"enqueue","sessionId":"sid","timestamp":chrono::Utc::now().to_rfc3339(),"content":"Olá"}))).unwrap();
    index.scan(&path).unwrap(); assert!(index.enqueued_after(&path,&cursor,&row).unwrap());
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
fn codex_identified_thread_without_rollout_is_proven_absence_confirmed_by_content() {
    // Thread da TUI sem turno: o caminho do rollout ainda não existe; nada é aberto e o cursor vale como anterior ao arquivo.
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("rollout.jsonl");
    let empty=std::path::Path::new("");
    let mut index=ReceiptIndex::new("codex","thread-1");
    let cursor=index.capture(empty).unwrap();
    assert!(cursor.file_identity.is_none() && cursor.offset == 0 && cursor.absent_since.is_some());
    assert!(index.scan(empty).unwrap().is_empty());
    assert!(index.match_after(empty,&cursor,&json!({"text":"um"}),&BTreeMap::new()).unwrap().is_none());
    codex_rollout(&path,Some("thread-1"));
    index.scan(&path).unwrap();
    let row=json!({"text":"um"});
    let proof=index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().expect("rollout que surgiu confirma pelo conteúdo");
    assert!(proof.validates(&cursor,&row));
}

#[test]
fn codex_invalid_rollout_path_is_an_error_not_absence() {
    let dir=tempfile::tempdir().unwrap();
    assert!(ReceiptIndex::new("codex","thread-1").capture(dir.path()).is_err());
}

#[test]
fn codex_rebound_conversation_drops_old_cursors() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("rollout.jsonl");
    let mut index=ReceiptIndex::new("codex","thread-1"); let cursor=index.capture(&path).unwrap();
    codex_rollout(&path,Some("thread-1"));
    index.scan(&path).unwrap();
    index.rebind("thread-2");
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

/// O fim de turno só vale enquanto nada do turno vem depois dele: outra resposta do assistente ou o resultado de uma
/// ferramenta (a CLI seguiu, por exemplo por um hook de parada que bloqueou) o desfazem; o subagente não.
#[test]
fn a_turn_end_is_undone_by_a_later_assistant_or_tool_result_of_the_main_thread() {
    let end = json!({"type":"assistant","isSidechain":false,"message":{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}});
    let user = json!({"type":"user","uuid":"u1","message":{"content":"oi"}});
    let cases = [
        (json!({"type":"assistant","isSidechain":false,"message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1"}]}}),false),
        (json!({"type":"user","isSidechain":false,"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}}),false),
        (json!({"type":"assistant","isSidechain":true,"message":{"stop_reason":"tool_use","content":[]}}),true),
        (json!({"type":"system","subtype":"stop_hook_summary","isSidechain":false}),true),
    ];
    for (after,ended) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.jsonl");
        std::fs::write(&path,format!("{user}\n{end}\n")).unwrap();
        let mut index = ReceiptIndex::new("claude","sid");
        index.scan(&path).unwrap();
        assert!(index.turn_ended().is_some());
        use std::io::Write;
        writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(),"{after}").unwrap();
        index.scan(&path).unwrap();
        assert_eq!(index.turn_ended().is_some(),ended,"{after}");
    }
}
