use hangar_server::costs::{claude::ClaudeFold, codex, index::Fold, py::parse_obj};
use serde_json::{Value, json};
use std::path::Path;

fn output_value(output: hangar_server::costs::rows::FoldOutput) -> Value {
    json!({"costs":output.costs,"usage":output.usage,"areas":output.areas})
}

fn complete_json(raw: &[u8]) -> Vec<u8> {
    parse_obj(raw).map(|obj| serde_json::to_vec(&obj).unwrap()).unwrap_or_default()
}

fn escaped_kind(raw: &[u8]) -> Vec<u8> {
    let mut text = String::from_utf8(complete_json(raw)).unwrap();
    if let Some(Value::String(kind)) = parse_obj(raw).and_then(|obj| obj.get("type").cloned()) {
        if let Some(first) = kind.chars().next() {
            let canonical = serde_json::to_string(&kind).unwrap();
            let escaped = format!("\"\\u{:04x}{}\"", first as u32, &kind[first.len_utf8()..]);
            // O empréstimo de &str recusa escapes e força o leitor completo de produção.
            text = text.replace(&canonical, &escaped);
        }
    }
    text.into_bytes()
}

fn codex_output(lines: &[Vec<u8>], force_complete: bool) -> Value {
    let mut fold = codex::new_fold(Path::new("rollout-synthetic.jsonl"));
    for raw in lines {
        if force_complete { fold.line(&escaped_kind(raw)); } else { fold.line(raw); }
    }
    output_value(fold.close())
}

fn record(kind: &str, timestamp: Option<&str>, payload: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"type":kind,"timestamp":timestamp,"payload":payload})).unwrap()
}

fn context() -> Vec<Vec<u8>> {
    vec![record("session_meta", Some("2026-10-03T02:59:59Z"),
                json!({"id":"synthetic","cwd":"/repo/ação","model_provider":"openai"})),
         record("turn_context", None, json!({"turn_id":"turn-a","model":"gpt-5"}))]
}

fn tool() -> Vec<u8> {
    record("response_item", None, json!({"type":"function_call","name":"mcp__ação","arguments":"{}"}))
}

#[test]
fn ignored_codex_events_update_local_day_before_a_consumed_event_without_timestamp() {
    for payload in [json!({"type":"agent_message","message":"ação"}), json!({}), Value::Null] {
        let mut lines = context();
        lines.push(record("event_msg", Some("2026-10-03T03:00:00Z"), payload.clone()));
        lines.push(tool());
        let got = codex_output(&lines, false);
        assert_eq!(got, codex_output(&lines, true), "{payload}");
        assert_eq!(got["usage"][0]["dia"], if payload.is_object() { "2026-10-03" } else { "2026-10-02" });
        assert_eq!(got["usage"][0]["nome"], "mcp__ação");
        assert_eq!(got["usage"][0]["chamadas"], 1);
    }
}

#[test]
fn ambiguous_codex_events_keep_full_parser_semantics_and_real_tool_counters() {
    let mut invalid_utf8 = br#"{"type":"response_item","payload":{"type":"function_call","name":"mcp__"# .to_vec();
    invalid_utf8.push(0xff);
    invalid_utf8.extend_from_slice(br#"","arguments":"{}"}}"#);
    let cases = vec![
        br#"{"type":"ignored","type":"response_item","payload":{"type":"ignored","type":"function_call","name":"mcp__duplicate"}}"#.to_vec(),
        br#"{"type":"response_item","type":"ignored","timestamp":"2026-10-03T03:00:00Z","payload":{"type":"function_call","name":"mcp__ignored"}}"#.to_vec(),
        br#"{"type":"response_item","payload":{"type":"function_\u0063all","name":"mcp__escape"}}"#.to_vec(),
        br#"{"type":"response_item","payload":{"type":7,"type":"function_call","name":"mcp__typed"}}"#.to_vec(),
        br#"{"type":"response_item","payload":["function_call",{"name":"mcp__array"}]}"#.to_vec(),
        br#"["response_item",null,{"type":"function_call","name":"mcp__outer_array"}]"#.to_vec(),
        invalid_utf8,
    ];
    for raw in cases {
        let mut lines = context();
        lines.push(raw.clone());
        lines.push(tool());
        assert_eq!(codex_output(&lines, false), codex_output(&lines, true), "{raw:?}");
    }
}

#[test]
fn consumed_codex_events_preserve_tokens_compaction_and_tool_output() {
    let mut lines = context();
    lines.extend([
        record("response_item", None, json!({"type":"custom_tool_call","name":"exec","call_id":"call-a",
            "input":"await tools.exec_command({\"cmd\":\"cat arquivo.txt\"})"})),
        record("response_item", None, json!({"type":"custom_tool_call_output","call_id":"call-a","output":"ação"})),
        record("event_msg", Some("2026-10-03T03:00:00Z"), json!({"type":"token_count","info":{
            "total_token_usage":{"input_tokens":100,"output_tokens":9},"last_token_usage":{"input_tokens":100}}})),
        record("token_usage_record", Some("2026-10-03T03:00:01Z"), json!({"thread_id":"synthetic","turn_id":"turn-b",
            "response_id":"response-a","usage":{"input_tokens":20,"cached_input_tokens":3,"output_tokens":2}})),
        record("compacted", Some("2026-10-03T03:00:02Z"), json!({})),
        tool(),
    ]);
    let got = codex_output(&lines, false);
    assert_eq!(got, codex_output(&lines, true));
    assert_eq!(got["costs"].as_array().unwrap().len(), 1);
    assert_eq!(got["costs"][0]["input"], 117);
    assert_eq!(got["costs"][0]["output"], 11);
    assert_eq!(got["costs"][0]["cache_read"], 3);
    assert!(got["usage"].as_array().unwrap().iter().any(|row| row["ctx_chars"] == 4));
}

#[test]
fn claude_ambiguous_json_matches_full_json_interpretation_including_utf8() {
    let mut invalid_utf8 = br#"{"type":"user","timestamp":"2026-10-03T03:00:00Z","message":{"content":"/"# .to_vec();
    invalid_utf8.push(0xff);
    invalid_utf8.extend_from_slice(br#""}}"#);
    let cases = vec![
        br#"{"type":"ignored","type":"user","message":{"content":"/acao"}}"#.to_vec(),
        br#"{"type":"assistant","timestamp":"2026-10-03T03:00:00Z","message":{"model":"alpha","usage":{},"usage":{"input_tokens":7}}}"#.to_vec(),
        br#"["user",null,null,null,null,null,null,{"content":"/acao"}]"#.to_vec(),
        invalid_utf8,
    ];
    for raw in cases {
        let mut optimized = ClaudeFold::new("synthetic".into(), false, false);
        let mut complete = optimized.clone();
        optimized.line(&raw);
        complete.line(&complete_json(&raw));
        assert_eq!(output_value(optimized.close()), output_value(complete.close()), "{raw:?}");
    }
}

#[test]
fn claude_escaped_relevant_key_preserves_python_pre_filter_and_full_parser_fallback() {
    let raw = br#"{"type":"assistant","timestamp":"2026-10-03T03:00:00Z","message":{"model":"alpha","\u0075sage":{"input_tokens":7}}}"#;
    let mut optimized = ClaudeFold::new("synthetic".into(), false, false);
    let mut complete = optimized.clone();
    optimized.line(raw);
    complete.line(&complete_json(raw));
    let skipped = output_value(optimized.close());
    assert!(skipped["costs"].as_array().unwrap().is_empty());
    assert!(skipped["usage"].as_array().unwrap().is_empty());
    assert!(skipped["areas"]["turns"].as_array().unwrap().is_empty());
    let expected = output_value(complete.close());
    assert_eq!(expected["costs"][0]["input"], 7);
    let accepted = br#"{"hint":"usage","type":"assistant","timestamp":"2026-10-03T03:00:00Z","message":{"model":"alpha","\u0075sage":{"input_tokens":7}}}"#;
    let mut fallback = ClaudeFold::new("synthetic".into(), false, false);
    fallback.line(accepted);
    assert_eq!(output_value(fallback.close()), expected);
}

#[test]
fn codex_fast_and_ambiguous_events_resume_from_the_real_index_without_duplicate_rows() {
    use hangar_server::costs::{areas::AreaMap, index::Index};
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-synthetic.jsonl");
    let areas = AreaMap::load(&dir.path().join("missing.json"));
    let resumed = Index::open(&dir.path().join("resumed")).unwrap();
    let full = Index::open(&dir.path().join("full")).unwrap();
    let mut lines = context();
    lines.extend([
        record("event_msg", Some("2026-10-03T03:00:00Z"), json!({"type":"agent_message","message":"ação"})),
        tool(),
        br#"{"type":"ignored","type":"response_item","payload":{"type":"ignored","type":"function_call","name":"mcp__duplicate"}}"#.to_vec(),
        record("token_usage_record", Some("2026-10-03T03:00:01Z"), json!({"thread_id":"synthetic","turn_id":"turn-a",
            "response_id":"response-a","usage":{"input_tokens":20,"cached_input_tokens":3,"output_tokens":2}})),
    ]);
    let encode = |rows: &[Vec<u8>]| rows.iter().flat_map(|row| row.iter().copied().chain([b'\n'])).collect::<Vec<_>>();
    std::fs::write(&path, encode(&lines[..3])).unwrap();
    let starts = AtomicUsize::new(0);
    let tracked = |path: &Path| { starts.fetch_add(1, Ordering::SeqCst); codex::new_fold(path) };
    let sync = |index: &Index| index.try_sync_file(&path, &tracked, codex::VERSION, "scope",
        areas.signature(), &|entries| areas.area_lines(entries)).unwrap();
    sync(&resumed);
    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&encode(&lines[3..])).unwrap();
    drop(file);
    sync(&resumed);
    assert_eq!(starts.load(Ordering::SeqCst), 1, "a retomada precisa usar o acumulador salvo");
    sync(&full);
    assert_eq!(resumed.read_costs(None, None, None).unwrap(), full.read_costs(None, None, None).unwrap());
    assert_eq!(resumed.read_usage("scope", None).unwrap(), full.read_usage("scope", None).unwrap());
    assert_eq!(serde_json::to_value(resumed.read_costs(None, None, None).unwrap()).unwrap(), codex_output(&lines, true)["costs"]);
    let rows = resumed.read_usage("scope", None).unwrap();
    assert_eq!(rows.iter().filter(|row| row.tipo == "tool").map(|row| (&row.dia, &row.nome, row.chamadas)).collect::<Vec<_>>(),
        vec![(&"2026-10-03".to_owned(), &"mcp__ação".to_owned(), 1), (&"2026-10-03".to_owned(), &"mcp__duplicate".to_owned(), 1)]);
}
