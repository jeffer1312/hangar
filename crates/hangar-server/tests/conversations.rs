// crates/hangar-server/tests/conversations.rs
//! Histórico e chat ao vivo servidos pelo hangar-server, com o Python falso no lugar do backend.
mod fake;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::transcript::{InternalInfo, merged_history};
use serde_json::{Value, json};

async fn setup(lines: std::ops::Range<usize>, stem: &str) -> (tempfile::TempDir, PathBuf, Vec<u64>, Arc<Fake>, SocketAddr) {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join(format!("{stem}.jsonl"));
    let offs = append_lines(&jsonl, lines);
    let (fake, up) = spawn_fake().await;
    fake.set_info(info_json("claude", &jsonl));
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    (dir, jsonl, offs, fake, srv)
}

#[tokio::test]
async fn events_open_with_ping_headers_and_backfill_of_200() {
    let (_dir, jsonl, offs, _fake, srv) = setup(0..250, "sess-a").await;

    let mut raw = open_events(srv, "s", "", &[("origin", "http://outra")]).await;
    assert_eq!(raw.headers()["content-type"], "text/event-stream; charset=utf-8");
    assert_eq!(raw.headers()["cache-control"], "no-store");
    assert_eq!(raw.headers()["x-accel-buffering"], "no");
    assert_eq!(raw.headers()["access-control-allow-origin"], "*");
    let first = tokio::time::timeout(Duration::from_secs(5), raw.chunk()).await.unwrap().unwrap().unwrap();
    assert!(first.starts_with(b"event: ping\r\ndata: {}\r\n\r\n"), "{:?}", first);
    assert!(!String::from_utf8_lossy(&first).contains("retry:"));
    drop(raw);

    let mut es = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(next_any(&mut es).await.event, "ping");
    let msgs = messages(&mut es, 200).await;
    assert_eq!(msgs[0].id, format!("sess-a:{}", offs[50]));
    assert_eq!(id_of(&msgs[0]), "u50");
    assert_eq!(id_of(&msgs[199]), "u249");

    let more = append_lines(&jsonl, 250..251);
    let live = messages(&mut es, 1).await;
    assert_eq!(live[0].id, format!("sess-a:{}", more[0]));
    assert_eq!(id_of(&live[0]), "u250");
}

#[tokio::test]
async fn events_resume_query_beats_header_and_bad_cursor_falls_back_to_tail() {
    let (_dir, _jsonl, offs, _fake, srv) = setup(0..250, "sess-b").await;

    let q = format!("last_event_id=sess-b:{}", offs[240]);
    let header = format!("sess-b:{}", offs[10]);
    let mut es = sse(open_events(srv, "s", &q, &[("last-event-id", header.as_str())]).await);
    let msgs = messages(&mut es, 10).await;
    assert_eq!(id_of(&msgs[0]), "u240");
    assert_eq!(id_of(&msgs[9]), "u249");

    let mut so_header = sse(open_events(srv, "s", "", &[("last-event-id", header.as_str())]).await);
    assert_eq!(id_of(&messages(&mut so_header, 1).await[0]), "u10");

    let mut outro = sse(open_events(srv, "s", "last_event_id=outro:0", &[]).await);
    assert_eq!(id_of(&messages(&mut outro, 1).await[0]), "u50");

    let mut alem = sse(open_events(srv, "s", "last_event_id=sess-b:999999999", &[]).await);
    assert_eq!(id_of(&messages(&mut alem, 1).await[0]), "u50");
}

#[tokio::test]
async fn partial_line_is_delivered_once_when_completed() {
    let (_dir, jsonl, _offs, _fake, srv) = setup(0..3, "sess-c").await;
    let off3 = std::fs::metadata(&jsonl).unwrap().len();
    let line3 = claude_line(3);
    let (head, tail) = line3.split_at(20);
    append_raw(&jsonl, head);

    let mut a = sse(open_events(srv, "s", "", &[]).await);
    let got = messages(&mut a, 3).await;
    assert_eq!(id_of(&got[2]), "u2");

    // Reconecta no meio da gravação, retomando da última mensagem recebida.
    let mut b = sse(open_events(srv, "s", &format!("last_event_id={}", got[2].id), &[]).await);
    assert_eq!(id_of(&messages(&mut b, 1).await[0]), "u2");

    append_raw(&jsonl, tail);
    for es in [&mut a, &mut b] {
        let m = messages(es, 1).await;
        assert_eq!(id_of(&m[0]), "u3");
        assert_eq!(m[0].id, format!("sess-c:{off3}"));
    }
    append_lines(&jsonl, 4..5);
    assert_eq!(id_of(&messages(&mut a, 1).await[0]), "u4", "a linha 3 não pode vir de novo");
}

#[tokio::test]
async fn devices_share_one_internal_connection_and_late_one_gets_snapshot() {
    let (_dir, _jsonl, _offs, fake, srv) = setup(0..1, "sess-d").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 1).await;
    wait_until(|| fake.side_conns() == 1).await;

    let state = r#"{"session":"s","state":"idle"}"#;
    let queued = r#"{"kind":"user_msg","id":"queued-7","text":"na fila"}"#;
    fake.push_side("state", state);
    fake.push_side("message", queued);
    assert_eq!(next_named(&mut a, "state").await.data, state);
    assert_eq!(id_of(&next_named(&mut a, "message").await), "queued-7");

    let mut b = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(id_of(&messages(&mut b, 1).await[0]), "u0");
    assert_eq!(next_named(&mut b, "state").await.data, state);
    assert_eq!(id_of(&next_named(&mut b, "message").await), "queued-7");

    assert_eq!(fake.side_conns(), 1);
    assert_eq!(fake.side_apps(), vec!["1".to_string()]);
}

/// Lê até o quadro marcador e falha se no caminho vier outro `reset`.
async fn no_reset_until(es: &mut Events, event: &str, data: &str) {
    loop {
        let ev = next_any(es).await;
        assert_ne!(ev.event, "reset", "um reset só por troca");
        if ev.event == event && ev.data == data {
            return;
        }
    }
}

#[tokio::test]
async fn new_info_resets_every_device_and_follows_new_file() {
    let (dir, _a, _offs, fake, srv) = setup(0..2, "sess-e").await;
    let b_path = dir.path().join("sess-e2.jsonl");
    append_lines(&b_path, 100..102);

    let mut devices = Vec::new();
    for _ in 0..3 {
        let mut es = sse(open_events(srv, "s", "", &[]).await);
        messages(&mut es, 2).await;
        devices.push(es);
    }
    wait_until(|| fake.side_conns() == 1).await;

    let novo = info_json("claude", &b_path);
    fake.set_info(novo.clone());
    fake.push_side("info", &novo.to_string());
    for es in &mut devices {
        assert_eq!(next_non_ping(es).await.event, "reset");
        let m = messages(es, 2).await;
        assert_eq!(id_of(&m[0]), "u100");
        assert!(m[0].id.starts_with("sess-e2:"));
    }
    let marker = r#"{"session":"s","state":"marcador"}"#;
    fake.push_side("state", marker);
    for es in &mut devices {
        no_reset_until(es, "state", marker).await;
    }
    assert_eq!(fake.side_conns(), 1, "a troca vem pela conexão que já existe");
}

#[tokio::test]
async fn provider_outside_rust_resets_and_next_connection_goes_to_python() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..1, "sess-f").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 1).await;
    wait_until(|| fake.side_conns() == 1).await;

    let pi = json!({"provider": "pi", "jsonl": jsonl, "session_key": "sess-f", "history": {}});
    fake.set_info(pi.clone());
    fake.push_side("info", &pi.to_string());
    assert_eq!(next_non_ping(&mut a).await.event, "reset");
    assert!(stream_ends(&mut a).await);

    let r = open_events(srv, "s", "", &[]).await;
    assert_eq!(r.text().await.unwrap(), "from-python");
}

#[tokio::test]
async fn truncated_transcript_resets_and_rereads_from_start() {
    let (_dir, jsonl, _offs, _fake, srv) = setup(0..3, "sess-g").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 3).await;

    std::fs::write(&jsonl, claude_line(9)).unwrap();
    assert_eq!(next_non_ping(&mut a).await.event, "reset");
    let m = messages(&mut a, 1).await;
    assert_eq!(id_of(&m[0]), "u9");
    assert_eq!(m[0].id, "sess-g:0");
}

#[tokio::test]
async fn same_name_with_new_transcript_never_serves_the_dead_one() {
    let (dir, _a, _offs, fake, srv) = setup(0..1, "sess-morta").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(id_of(&messages(&mut a, 1).await[0]), "u0");
    wait_until(|| fake.side_conns() == 1).await;

    // A sessão morreu e nasceu outra com o mesmo nome; o cache de info já venceu.
    let nova = dir.path().join("sess-nova.jsonl");
    append_lines(&nova, 100..101);
    fake.set_info(info_json("claude", &nova));
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let mut b = sse(open_events(srv, "s", "", &[]).await);
    let first = next_non_ping(&mut b).await;
    assert_eq!(first.event, "message");
    assert_eq!(id_of(&first), "u100");
    assert!(first.id.starts_with("sess-nova:"));

    assert_eq!(next_non_ping(&mut a).await.event, "reset");
    assert_eq!(id_of(&messages(&mut a, 1).await[0]), "u100");
    wait_until(|| fake.side_conns() == 2).await;

    // O primeiro `info` da conexão religada confirma a troca: nenhum reset a mais, nenhuma
    // religação a mais.
    let marker = r#"{"session":"s","state":"marcador"}"#;
    fake.push_side("state", marker);
    no_reset_until(&mut a, "state", marker).await;
    no_reset_until(&mut b, "state", marker).await;
    assert_eq!(fake.side_conns(), 2);
}

#[tokio::test]
async fn events_without_owner_token_goes_to_python() {
    let (_dir, _jsonl, _offs, fake, srv) = setup(0..1, "sess-h").await;
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/events?token=errado"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 0);

    // Controle: o token do dono abre o stream no Rust, que consulta a rota interna.
    let r = open_events(srv, "s", "", &[]).await;
    assert_eq!(r.headers()["content-type"], "text/event-stream; charset=utf-8");
    assert_eq!(fake.info_calls(), 1);
}

#[tokio::test]
async fn history_is_served_by_rust_with_etag_limit_gzip_and_cors() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..40, "sess-i").await;
    let info: InternalInfo = serde_json::from_value(info_json("claude", &jsonl)).unwrap();
    let req = info.history_request(None).expect("history_field tem de casar com o formato da Task 9");
    let expected = serde_json::to_value(merged_history(&req).unwrap()).unwrap();
    let url = format!("http://{srv}/api/sessions/s/history");

    let r = client().get(&url).bearer_auth(OWNER).header("origin", "http://outra").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/json");
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert_eq!(r.headers()["access-control-expose-headers"], "ETag");
    let etag = r.headers().get("etag").expect("etag").to_str().unwrap().to_owned();
    let got: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(got, expected);
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0, "não passou pelo Python");

    let r304 = client().get(&url).bearer_auth(OWNER).header("if-none-match", &etag).send().await.unwrap();
    assert_eq!(r304.status(), 304);
    assert_eq!(r304.headers()["etag"], etag.as_str());

    let rz = client().get(&url).bearer_auth(OWNER).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(rz.headers()["content-encoding"], "gzip");

    let rl = client().get(format!("{url}?limit=5")).bearer_auth(OWNER).send().await.unwrap();
    let tail: Value = serde_json::from_str(&rl.text().await.unwrap()).unwrap();
    let all = expected.as_array().unwrap();
    assert_eq!(tail.as_array().unwrap().as_slice(), &all[all.len() - 5..]);
}

#[tokio::test]
async fn history_negative_limit_is_the_whole_history_like_python() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..40, "sess-k").await;
    let info: InternalInfo = serde_json::from_value(info_json("claude", &jsonl)).unwrap();
    let expected = serde_json::to_value(merged_history(&info.history_request(None).unwrap()).unwrap()).unwrap();
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history?limit=-5"))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let got: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(got, expected);
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0, "não passou pelo Python");
}

#[tokio::test]
async fn history_without_owner_or_supported_provider_goes_to_python() {
    let (_dir, jsonl, _offs, fake, srv) = setup(0..2, "sess-j").await;
    let url = format!("http://{srv}/api/sessions/s/history");

    let r = client().get(&url).bearer_auth("convidado").send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 0);

    let r = client().get(format!("{url}?limit=abc")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");

    fake.set_info(json!({"provider": "pi", "jsonl": jsonl, "session_key": "sess-j", "history": {}}));
    let r = client().get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(fake.info_calls(), 1, "o dono vindo do loopback consulta a rota interna");
}

#[tokio::test]
async fn answered_pane_question_is_not_replayed_to_late_devices() {
    let (_dir, _jsonl, _offs, fake, srv) = setup(0..1, "sess-q").await;
    let mut a = sse(open_events(srv, "s", "", &[]).await);
    messages(&mut a, 1).await;
    wait_until(|| fake.side_conns() == 1).await;

    let awaiting = r#"{"session":"s","state":"awaiting_input"}"#;
    let question = r#"{"questions":[{"question":"qual?"}]}"#;
    fake.push_side("state", awaiting);
    fake.push_side("ask_question", question);
    assert_eq!(next_named(&mut a, "ask_question").await.data, question);

    // Chegou durante a pergunta: recebe.
    let mut b = sse(open_events(srv, "s", "", &[]).await);
    assert_eq!(next_named(&mut b, "ask_question").await.data, question);

    let idle = r#"{"session":"s","state":"idle"}"#;
    fake.push_side("state", idle);
    assert_eq!(next_named(&mut a, "state").await.data, idle);

    // Chegou depois da resposta: estado atual, sem a pergunta velha.
    let mut c = sse(open_events(srv, "s", "", &[]).await);
    let marker = r#"{"session":"s","state":"marcador"}"#;
    fake.push_side("state", marker);
    loop {
        let ev = next_any(&mut c).await;
        assert_ne!(ev.event, "ask_question", "pergunta já respondida");
        if ev.event == "state" && ev.data == marker {
            break;
        }
    }
}

#[tokio::test]
async fn history_never_serves_a_dead_transcript_from_the_info_cache() {
    let (dir, _jsonl, _offs, fake, srv) = setup(0..2, "sess-velha").await;
    let url = format!("http://{srv}/api/sessions/s/history");
    let ids = |v: Value| v.as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();

    let r = client().get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(ids(serde_json::from_str(&r.text().await.unwrap()).unwrap()), ["u0", "u1"]);

    // Fechada e recriada com o mesmo nome dentro do TTL do cache.
    let nova = dir.path().join("sess-nova2.jsonl");
    append_lines(&nova, 100..101);
    fake.set_info(info_json("claude", &nova));
    let r = client().get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(ids(serde_json::from_str(&r.text().await.unwrap()).unwrap()), ["u100"]);
    assert_eq!(fake.info_calls(), 2);
}
