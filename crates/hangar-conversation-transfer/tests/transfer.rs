use std::path::Path;

use hangar_conversation_transfer::{
    ImportBoundary, Store, TransferPhase, TransferRecord, convert_bytes, convert_snapshot, digest,
    serialized_batches, verify_source,
};
use serde_json::{Value, json};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn text(items: &[Value]) -> String {
    items
        .iter()
        .flat_map(|i| i["content"].as_array().into_iter().flatten())
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn transfer_conversion_preserves_full_outputs_order_media_and_history() {
    let mixed = convert_snapshot(&fixture("mixed_blocks.jsonl")).unwrap();
    assert_eq!(
        mixed
            .items
            .iter()
            .map(|i| i["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "message",
            "function_call",
            "function_call_output",
            "message"
        ]
    );
    let output = (0..12000)
        .map(|n| format!("line-{n:05}|\n"))
        .collect::<String>();
    assert_eq!(mixed.items[2]["output"], output);
    assert_eq!(mixed.max_output_bytes, output.len());
    assert_eq!(mixed.items[0]["content"][0]["text"], "antes");
    assert_eq!(mixed.items[3]["content"][0]["text"], "depois");
    assert_eq!(
        mixed.source_digest,
        digest(&std::fs::read(fixture("mixed_blocks.jsonl")).unwrap())
    );
    assert_eq!(
        mixed,
        convert_snapshot(&fixture("mixed_blocks.jsonl")).unwrap()
    );
    let media = convert_snapshot(&fixture("media_and_thinking.jsonl")).unwrap();
    assert_eq!(media.items[0]["content"][0]["type"], "input_image");
    let output = &media
        .items
        .iter()
        .find(|i| i["type"] == "function_call_output")
        .unwrap()["output"];
    assert_eq!(output[0]["text"], "[Claude tool_result: is_error=true]\n");
    assert_eq!(output[1]["text"], "texto completo");
    assert_eq!(output[2]["type"], "input_image");
    assert!(text(&media.items).contains("assinatura-original"));
    assert_eq!(media.source_limitations.len(), 1);
    assert!(media.items.iter().all(|i| i["type"] != "reasoning"));
}

#[test]
fn transfer_conversion_follows_compaction_and_does_not_invent_tool_pairs() {
    let compact = convert_snapshot(&fixture("compacted_branch.jsonl")).unwrap();
    let history = text(&compact.items);
    assert!(history.contains("regra hist") && history.contains("meta preservada"));
    assert!(
        !history.contains("resumo que não entra")
            && !history.contains("fork rejeitado")
            && !history.contains("duplicata posterior")
    );
    assert!(compact.selected_uuids.contains("instructions"));
    let orphan = convert_snapshot(&fixture("unpaired_tools.jsonl")).unwrap();
    assert!(orphan.items.iter().all(|i| i["type"] == "message"));
    assert!(text(&orphan.items).contains("resultado desconhecido"));
    assert!(text(&orphan.items).contains("órfão missing-call"));
    for (file, code) in [
        (
            "missing_parent.jsonl",
            "session_transfer_source_parent_missing",
        ),
        ("cycle.jsonl", "session_transfer_source_cycle"),
        (
            "unknown_context.jsonl",
            "session_transfer_unsupported_context",
        ),
    ] {
        assert_eq!(convert_snapshot(&fixture(file)).unwrap_err().code, code);
    }
}

#[test]
fn transfer_conversion_rejects_partial_invalid_and_duplicate_json_without_splitting_unicode() {
    for (raw, code) in [
        (b"{}".as_slice(), "session_transfer_source_partial_line"),
        (b"{broken}\n", "session_transfer_invalid_source_json"),
        (
            b"{\"type\":\"user\",\"type\":\"assistant\"}\n",
            "session_transfer_invalid_source_json",
        ),
        (
            b"{\"nested\":{\"x\":1,\"x\":2}}\n",
            "session_transfer_invalid_source_json",
        ),
        (b"[]\n", "session_transfer_invalid_source_schema"),
    ] {
        assert_eq!(convert_bytes(raw).unwrap_err().code, code);
    }
    for separator in ['\u{2028}', '\u{2029}', '\u{85}'] {
        let content = format!("antes{separator}depois");
        let raw = format!(
            "{}\r\n",
            json!({"type":"user","uuid":"u","parentUuid":null,"message":{"role":"user","content":content}})
        );
        assert_eq!(
            convert_bytes(raw.as_bytes()).unwrap().items[0]["content"][0]["text"],
            content
        );
    }
}

#[test]
fn transfer_batches_keep_items_and_honor_exact_escaped_envelope() {
    use hangar_core::pycompat::pyjson;
    let item = json!({"type":"message","role":"user","content":[{"type":"input_text","text":"ação ".repeat(80)}]});
    let envelope = json!({"jsonrpc":"2.0","id":u64::MAX,"method":"thread/inject_items","params":{"threadId":"f".repeat(36),"items":[item.clone()]}});
    let exact = pyjson::dumps(&envelope, false).len() + 1;
    assert_eq!(
        serialized_batches(&[item.clone()], exact).unwrap(),
        vec![vec![item.clone()]]
    );
    assert_eq!(
        serialized_batches(&[item.clone()], exact - 1)
            .unwrap_err()
            .code,
        "session_transfer_source_item_too_large"
    );
    let items = vec![item; 8];
    let batches = serialized_batches(&items, exact).unwrap();
    assert_eq!(batches.len(), 8);
    assert_eq!(batches.concat(), items);
    assert!(serialized_batches(&[], 2048).unwrap().is_empty());
}

fn record() -> TransferRecord {
    TransferRecord {
        id: "00000000-0000-4000-8000-000000000001".into(),
        name: "session".into(),
        source_life: "k:old".into(),
        phase: TransferPhase::SourceStopped,
        source: None,
        origin_meta: json!({"key":"old","session_id":"original","jsonl":"/tmp/original.jsonl","headless":true}),
        destination_meta: Some(
            json!({"thread_id":"thread","codex_home":"/tmp/account","lifecycle_id":"k:old","key":"old"}),
        ),
        boundary: Some(ImportBoundary {
            thread_id: "thread".into(),
            rollout_path: "/tmp/rollout".into(),
            min_offset: 452,
            imported_item_ids: vec!["i0".into()],
            prefix_digest: None,
        }),
        error_code: None,
    }
}

#[test]
fn transfer_store_keeps_snapshot_private_and_rejects_corruption_or_stale_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    let original = dir.path().join("original.jsonl");
    let raw = b"original\r\n";
    std::fs::write(&original, raw).unwrap();
    let r = record();
    store.save(&r).unwrap();
    let saved = store
        .capture_snapshot(&r, &original, vec!["u0".into()])
        .unwrap();
    let source = saved.source.as_ref().unwrap();
    assert_eq!(std::fs::read(&source.path).unwrap(), raw);
    verify_source(source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&source.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(store.load(&r.id).unwrap(), Some(saved.clone()));
    let mut changed = saved.clone();
    changed.source_life = "k:new".into();
    assert!(store.save(&changed).is_err());
    changed = saved.clone();
    changed.origin_meta["api_key"] = json!("secret");
    assert!(store.save(&changed).is_err());
    changed = saved.clone();
    changed.phase = TransferPhase::Imported;
    store.save(&changed).unwrap();
    assert!(store.capture_snapshot(&saved, &original, vec![]).is_err());
    std::fs::write(&source.path, b"changed").unwrap();
    assert!(verify_source(source).is_err());
    std::fs::write(store.base().join(format!("{}.json", r.id)), b"null").unwrap();
    assert!(store.load(&r.id).is_err());
}

#[test]
fn transfer_store_association_survives_close_but_not_name_reuse_or_clear() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store"));
    let mut r = record();
    store.save(&r).unwrap();
    assert!(
        store
            .for_session(&r.name, Some("k:new"), None, None, false)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .for_session(&r.name, None, None, None, true)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .for_session(
                &r.name,
                Some("k:old"),
                None,
                Some(Path::new("/tmp/different.jsonl")),
                false
            )
            .unwrap()
            .is_none()
    );
    r.phase = TransferPhase::Complete;
    store.save(&r).unwrap();
    assert!(store.list_incomplete().unwrap().is_empty());
    assert_eq!(
        store.for_thread("/tmp/account", "thread").unwrap(),
        Some(r.clone())
    );
    assert!(store.for_thread("/tmp/other", "thread").unwrap().is_none());
    let meta = json!({"provider":"codex","thread_id":"new-thread","codex_home":"/tmp/account","transfer_id":r.id});
    assert!(
        store
            .for_session(&r.name, Some("k:old"), Some(&meta), None, false)
            .unwrap()
            .is_none()
    );
}

proptest::proptest! {
    #[test]
    fn transfer_arbitrary_input_is_a_result_not_a_panic(raw in proptest::collection::vec(proptest::prelude::any::<u8>(),0..4096)) {let _=convert_bytes(&raw);}
}
