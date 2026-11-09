use hangar_server::{tail::{backfill, read_frames}, transcript::{LineParser, Provider}};

#[tokio::test]
async fn pi_backfill_does_not_wait_when_the_hook_pair_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let user = serde_json::json!({"type":"message","id":"user-1","message":{
        "role":"user","content":[{"type":"text","text":"contexto\n\nMensagem"}]}});
    let hook = serde_json::json!({"type":"custom_message","customType":"claude-hook-context",
        "parentId":"user-1","content":"contexto"});
    std::fs::write(&path, format!("{user}\n{hook}\n")).unwrap();
    let cut = path.metadata().unwrap().len();
    let frames = hangar_server::tail::backfill_guarded(path, "k".into(), Provider::Pi, None, cut, |_| async {
        panic!("par completo não aguarda outra escrita");
    }).await;
    assert_eq!(frames.len(), 1);
}

#[tokio::test]
async fn pi_shared_tail_releases_the_last_user_without_waiting_for_the_heartbeat() {
    use std::{io::Write, time::Duration};
    use hangar_server::{side::Out, tail::{FileTail, Watchers}};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, "").unwrap();
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let tail = FileTail::spawn(path.clone(), "k".into(), Provider::Pi, 1, tx, Watchers::default(), Box::new(|| {}));
    assert_eq!(tail.state.lock().await.cut().unwrap(), 0);
    let user = serde_json::json!({"type":"message","id":"user-1","message":{
        "role":"user","content":[{"type":"text","text":"Mensagem"}]}});
    writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(), "{user}").unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
        .expect("a mensagem retida sai depois da espera de 200 ms, antes do heartbeat de 5 s").unwrap();
    let Out::Tail(1, frame) = frame else { panic!("evento da mesma vida") };
    let value: serde_json::Value = serde_json::from_str(std::str::from_utf8(&frame).unwrap().split("data: ").nth(1).unwrap().trim()).unwrap();
    assert_eq!(value["text"], "Mensagem");
}

#[tokio::test]
async fn pi_backfill_reads_the_hook_arriving_during_the_grace() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let user = serde_json::json!({"type":"message","id":"user-1","message":{
        "role":"user","content":[{"type":"text","text":"contexto\n\nMensagem"}]}});
    let hook = serde_json::json!({"type":"custom_message","customType":"claude-hook-context",
        "parentId":"user-1","content":"contexto"});
    std::fs::write(&path, format!("{user}\n")).unwrap();
    let cut = path.metadata().unwrap().len();
    let hook_path = path.clone();
    let frames = hangar_server::tail::backfill_guarded(path, "k".into(), Provider::Pi, None, cut, move |_| {
        let mut file = std::fs::OpenOptions::new().append(true).open(hook_path).unwrap();
        writeln!(file, "{hook}").unwrap();
        let future = serde_json::json!({"type":"message","id":"user-2","message":{
            "role":"user","content":[{"type":"text","text":"Mensagem posterior ao corte"}]}});
        writeln!(file, "{future}").unwrap();
        std::future::ready(())
    }).await;
    assert_eq!(frames.len(), 1);
    let frame = std::str::from_utf8(&frames[0]).unwrap();
    assert!(frame.starts_with("id: k:0\r\n"));
    let value: serde_json::Value = serde_json::from_str(frame.split("data: ").nth(1).unwrap().trim()).unwrap();
    assert_eq!(value["text"], "Mensagem");
}

#[tokio::test]
async fn pi_backfill_awaits_the_hook_grace_without_a_blocking_sleep() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let user = serde_json::json!({"type":"message","id":"user-1","message":{
        "role":"user","content":[{"type":"text","text":"Mensagem"}]}});
    std::fs::write(&path, format!("{user}\n")).unwrap();
    let waited = Arc::new(AtomicBool::new(false));
    let observed = waited.clone();
    let cut = path.metadata().unwrap().len();
    let frames = hangar_server::tail::backfill_guarded(path, "k".into(), Provider::Pi, None, cut, move |delay| {
        assert_eq!(delay, std::time::Duration::from_millis(200));
        observed.store(true, Ordering::SeqCst);
        std::future::ready(())
    }).await;
    assert!(waited.load(Ordering::SeqCst), "somente o par retido aguarda o marcador");
    assert_eq!(frames.len(), 1);
}

#[test]
fn pi_backfill_releases_a_user_message_without_a_hook_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let user = serde_json::json!({"type":"message","id":"user-1","message":{
        "role":"user","content":[{"type":"text","text":"Mensagem"}]}});
    std::fs::write(&path, format!("{user}\n")).unwrap();
    let frames = backfill(&path, "k", Provider::Pi, None, path.metadata().unwrap().len());
    assert_eq!(frames.len(), 1, "a mensagem final não depende de um marcador futuro");
    let frame = std::str::from_utf8(&frames[0]).unwrap();
    assert!(frame.starts_with("id: k:0\r\n"));
    let value: serde_json::Value = serde_json::from_str(frame.split("data: ").nth(1).unwrap().trim()).unwrap();
    assert_eq!(value["text"], "Mensagem");
}

#[test]
fn pi_hook_pair_keeps_the_user_line_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let user = serde_json::json!({"type":"message","id":"user-1","message":{
        "role":"user","content":[{"type":"text","text":"contexto\n\nMensagem"}]}});
    let hook = serde_json::json!({"type":"custom_message","customType":"claude-hook-context",
        "parentId":"user-1","content":"contexto"});
    std::fs::write(&path, format!("{user}\n{hook}\n")).unwrap();
    let mut parser = LineParser::new(Provider::Pi);
    let (frames, _) = read_frames(&path, 0, None, &mut parser, "k").unwrap();
    let frame = std::str::from_utf8(frames.first().expect("mensagem com contexto removido")).unwrap();
    assert!(frame.starts_with("id: k:0\r\n"), "{frame}");
    let data = frame.split("data: ").nth(1).unwrap().trim();
    let value: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(value["text"], "Mensagem");
}
