//! Regressões de leitura coerente, topologia compacta e identidade dos eventos importados.
use super::*;
use serde_json::json;
use std::{fs::OpenOptions, io::Write};

fn sha(bytes: &[u8]) -> String {
    hex_digest(ring::digest::digest(&ring::digest::SHA256, bytes))
}

fn encoded(rows: &[Value]) -> Vec<u8> {
    rows.iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn append_during_read_retries_and_returns_only_a_stable_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("active.jsonl");
    std::fs::write(&path, b"primeira\n").unwrap();
    let mut attempts = 0;
    let bytes = stable_read(&path, |reader, length| {
        attempts += 1;
        let mut bytes = Vec::new();
        reader.take(length).read_to_end(&mut bytes)?;
        if attempts == 1 {
            OpenOptions::new()
                .append(true)
                .open(&path)?
                .write_all(b"segunda\n")?;
        }
        Ok(bytes)
    })
    .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(bytes, b"primeira\nsegunda\n");
}

#[test]
fn continuously_changing_source_stops_after_three_attempts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("active.jsonl");
    std::fs::write(&path, b"inicio\n").unwrap();
    let mut attempts = 0;
    let result = stable_read(&path, |reader, length| {
        attempts += 1;
        let n = io::copy(&mut reader.take(length), &mut io::sink())?;
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"novo\n")?;
        Ok(n)
    });
    assert_eq!(attempts, 3);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn stable_parse_and_io_failures_are_not_hidden_by_retries() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.jsonl");
    std::fs::write(&path, b"invalido\n").unwrap();
    let mut attempts = 0;
    let result: io::Result<()> = stable_read(&path, |_, _| {
        attempts += 1;
        Err(invalid("Falha de parsing estável."))
    });
    assert_eq!(attempts, 1);
    assert_eq!(result.unwrap_err().to_string(), "Falha de parsing estável.");
    std::fs::remove_file(&path).unwrap();
    let result = stable_read(&path, |_, _| Ok(()));
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
}

#[test]
fn queue_change_rebuilds_the_commit_projection_before_returning() {
    let directory = tempfile::tempdir().unwrap();
    let queue = directory.path().join("queue.jsonl");
    std::fs::write(&queue, b"{\"id\":\"first\",\"text\":\"primeira\"}\n").unwrap();
    let info =
        serde_json::from_value(json!({"provider":"codex","history":{"queue":queue}})).unwrap();
    let mut attempts = 0;
    let entries = with_stable_queue(&info, || {
        attempts += 1;
        let entries = super::super::load_queue(&queue)?;
        if attempts == 1 {
            OpenOptions::new()
                .append(true)
                .open(&queue)?
                .write_all(b"{\"id\":\"second\",\"text\":\"segunda\"}\n")?;
        }
        Ok(entries)
    })
    .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1]["id"], "second");
}

#[test]
fn source_index_retains_topology_without_large_discarded_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.jsonl");
    let rows = [
        json!({"type":"user","uuid":"root","parentUuid":null,"message":{"role":"user","content":"origem"}}),
        json!({"type":"assistant","uuid":"fork","parentUuid":"root","isSidechain":true,
            "message":{"role":"assistant","content":"x".repeat(8 * 1024 * 1024)}}),
        json!({"type":"assistant","uuid":"leaf","parentUuid":"root","message":{"role":"assistant","content":"fim"}}),
    ];
    let bytes = encoded(&rows);
    std::fs::write(&path, &bytes).unwrap();
    let (metadata, offsets) = stable_read(&path, |reader, length| {
        source_index(reader, length, &sha(&bytes))
    })
    .unwrap();
    assert_eq!(metadata.len(), rows.len());
    assert!(metadata.iter().all(|row| row.get("message").is_none()));
    assert!(serde_json::to_vec(&metadata).unwrap().len() < 1024);
    assert_eq!(chain(&metadata).unwrap(), vec![0, 2]);
    assert_eq!(offsets[2], encoded(&rows[..2]).len() as u64);
    let source = json!({"path":path,"digest":sha(&bytes),"selected_uuids":["root","leaf"]});
    let (events, _, _) = source_events(&source, "fixture", 30).unwrap();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        ["origem", "fim"]
    );
}

#[test]
fn imported_held_and_delivery_ids_keep_first_occurrence_before_prefixing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.jsonl");
    let held =
        "UserPromptSubmit operation blocked by hook: bloqueio\nOriginal prompt: mensagem barrada";
    let rows = [
        json!({"type":"user","uuid":"root","parentUuid":null,"message":{"role":"user","content":"repetida"}}),
        json!({"type":"system","uuid":"held-first","parentUuid":"root","timestamp":"2026-01-01T00:00:01Z","content":held}),
        json!({"type":"system","uuid":"held-retry","parentUuid":"held-first","timestamp":"2026-01-01T00:00:02Z","content":held}),
        json!({"type":"attachment","uuid":"attached","parentUuid":"held-retry","timestamp":"2026-01-01T00:00:03Z",
            "attachment":{"type":"queued_command","delivery_id":"same","prompt":[{"type":"text","text":"primeira entrega"}]}}),
        json!({"type":"queue-operation","uuid":"removed","parentUuid":"attached","timestamp":"2026-01-01T00:00:04Z",
            "operation":"remove","deliveryId":"same","content":"segunda entrega"}),
        json!({"type":"user","uuid":"leaf","parentUuid":"removed","message":{"role":"user","content":"repetida"}}),
    ];
    let bytes = encoded(&rows);
    std::fs::write(&path, &bytes).unwrap();
    let source = json!({"path":path,"digest":sha(&bytes),"selected_uuids":rows.iter().map(|row|row["uuid"].clone()).collect::<Vec<_>>()});
    let (events, _, _) = source_events(&source, "fixture", 30).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        [
            "repetida",
            "mensagem barrada",
            "primeira entrega",
            "repetida"
        ]
    );
    assert!(events[1].id.starts_with("transfer:fixture:claude:held:"));
    assert_eq!(events[1].ts, Some(1767225601.));
    assert_eq!(events[2].id, "transfer:fixture:claude:delivery:same");
    assert_eq!(events[2].ts, Some(1767225603.));
    assert_ne!(events[0].id, events[3].id);
}

fn expand(value: &Value, home: &Path) -> Value {
    match value {
        Value::String(value) => {
            let file = match value.as_str() {
                "@SOURCE@" => Some("snapshot.jsonl"),
                "@ROLLOUT@" => Some("rollout.jsonl"),
                "@CODEX@" => Some("codex"),
                "@HOME@" => Some(""),
                "@ORIGIN@" => Some("original.jsonl"),
                _ => None,
            };
            file.map(|file| json!(home.join(file)))
                .unwrap_or_else(|| json!(value))
        }
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), expand(value, home)))
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.iter().map(|value| expand(value, home)).collect())
        }
        value => value.clone(),
    }
}

fn setup(case: &Value, home: &Path) -> (SessionRow, InternalInfo) {
    std::fs::write(
        home.join("snapshot.jsonl"),
        case["source_bytes"].as_str().unwrap(),
    )
    .unwrap();
    std::fs::write(
        home.join("rollout.jsonl"),
        case["rollout_bytes"].as_str().unwrap(),
    )
    .unwrap();
    let record = expand(&case["record"], home);
    let id = record["id"].as_str().unwrap();
    let records = home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join(format!("{id}.json")), record.to_string()).unwrap();
    let row =
        serde_json::from_value(json!({"name":"s","provider":"codex","transfer_id":id})).unwrap();
    let info = serde_json::from_value(
        json!({"provider":"codex","jsonl":home.join("rollout.jsonl"),
        "history":{"queue":home.join("queue.jsonl")}}),
    )
    .unwrap();
    (row, info)
}

#[test]
fn absent_last_prompt_matches_the_independent_original_python_capture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/speech/http_transfer_history_review.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let (row, info) = setup(case, directory.path());
        let events = read(&row, directory.path(), &info, 30).unwrap().unwrap();
        let actual: Vec<_> = events
            .iter()
            .map(|event| json!({"kind":event.kind,"text":event.text,"ts":event.ts}))
            .collect();
        assert_eq!(json!(actual), case["events"], "{}", case["name"]);
    }
}

#[test]
fn streamed_active_tail_keeps_old_queue_commits_and_timestamp_order() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/speech/http_transfer_history_review.json"
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path();
    let (row, info) = setup(&fixture["cases"][0], home);
    let mut rollout = OpenOptions::new()
        .append(true)
        .open(home.join("rollout.jsonl"))
        .unwrap();
    for index in (0..120).rev() {
        let text = if index == 0 {
            "confirmação antiga".to_owned()
        } else {
            format!("nova-{index}")
        };
        let value = json!({"timestamp":format!("2026-02-02T00:{:02}:{:02}Z",index / 60,index % 60),
            "type":"response_item","payload":{"type":"message","id":format!("active-{index}"),"role":"user",
            "content":[{"type":"input_text","text":text}]}});
        writeln!(rollout, "{value}").unwrap();
    }
    drop(rollout);
    std::fs::write(
        home.join("queue.jsonl"),
        format!(
            "{}\n",
            json!({"id":"q-old","text":"confirmação antiga","ts":1769990399.,"delivered":true})
        ),
    )
    .unwrap();
    let events = read(&row, home, &info, 30).unwrap().unwrap();
    assert_eq!(events.len(), 30);
    assert_eq!(events.first().unwrap().text.as_deref(), Some("nova-90"));
    assert_eq!(events.last().unwrap().text.as_deref(), Some("nova-119"));
    assert!(events.iter().all(|event| event.id.starts_with("transfer:")));
    assert!(events.windows(2).all(|pair| pair[0].ts <= pair[1].ts));
}

#[test]
fn prefix_digest_is_streamed_without_consuming_the_active_suffix() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rollout.jsonl");
    let header = json!({"type":"session_meta","payload":{"id":"thread"}});
    let mut bytes = encoded(&[header.clone()]);
    bytes.extend_from_slice(&vec![b'x'; 1024 * 1024]);
    bytes.push(b'\n');
    let offset = bytes.len();
    let expected = sha(&bytes);
    bytes.extend_from_slice(b"ativo\n");
    std::fs::write(&path, bytes).unwrap();
    stable_read(&path, |reader, _| {
        assert_eq!(prefix(reader, offset as u64, &expected)?, header);
        assert_eq!(reader.stream_position()?, offset as u64);
        let mut suffix = String::new();
        reader.read_to_string(&mut suffix)?;
        assert_eq!(suffix, "ativo\n");
        Ok(())
    })
    .unwrap();
}
