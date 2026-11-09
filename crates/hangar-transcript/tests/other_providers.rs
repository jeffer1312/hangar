use hangar_transcript::{LineParser, Provider};
use serde_json::{Value, json};
use proptest::prelude::*;

fn parse(provider: &str, value: Value) -> Vec<hangar_api::chat::ChatEvent> {
    let parsed = Provider::parse(provider);
    assert!(parsed.is_some(), "provedor suportado pelo histórico: {provider}");
    let provider = parsed.unwrap();
    LineParser::new(provider).feed(value.to_string().as_bytes(), 23)
}

#[test]
fn h003_pi_assistant_preserves_cache_and_timestamp() {
    let ev = parse("pi", json!({"type":"message","id":"n1","message":{"role":"assistant",
        "timestamp":1786453187986u64,"usage":{"cacheRead":8704},"content":[{"type":"text","text":"ola"}]}}));
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].text.as_deref(), Some("ola"));
    assert_eq!(ev[0].cache_read, Some(8704));
    assert_eq!(ev[0].ts, Some(1786453187.986));
    assert_eq!(ev[0].offset, Some(23));
}

#[test]
fn h017_kimi_results_keep_distinct_ids_without_uuid() {
    let a = parse("kimi", json!({"type":"context.append_loop_event","event":{
        "type":"tool.result","parentUuid":"pai-tool_1","toolCallId":"tool_1","result":{"output":"conteudo"}}}));
    let b = parse("kimi", json!({"type":"context.append_loop_event","event":{
        "type":"tool.result","parentUuid":"pai-tool_2","toolCallId":"tool_2","result":{"output":"ok"}}}));
    assert_eq!(a[0].id, "res:tool_1");
    assert_eq!(b[0].id, "res:tool_2");
    assert_eq!(a[0].tool_use_id.as_deref(), Some("tool_1"));
    assert_eq!(a[0].result.as_deref(), Some("conteudo"));
    assert_eq!(a[0].is_error, Some(false));
    for (event, expected_id) in [
        (json!({"type":"tool.result","parentUuid":"pai"}),"res:pai"),
        (json!({"type":"tool.result","uuid":"u4"}),"res:u4"),
        (json!({"type":"tool.result"}),""),
    ] {
        let events = parse("kimi",json!({"type":"context.append_loop_event","event":event}));
        assert_eq!(events[0].id,expected_id);
    }
}

#[test]
fn h001_pi_metadata_and_torn_writes_are_ignored() {
    let provider = Provider::parse("pi").expect("Pi suportado");
    let mut parser = LineParser::new(provider);
    for raw in ["", "{\"type\":\"mess", "[]", "null", "{\"type\":\"session\"}"] {
        assert!(parser.feed(raw.as_bytes(), 0).is_empty());
    }
}

#[test]
fn h004_pi_block_ids_keep_original_indices() {
    let evs = parse("omp", json!({"type":"message","id":"n1","message":{"role":"assistant","content":[
        {"type":"text","text":"ola"}, {"type":"toolCall","name":"bash","id":"call_1","arguments":{"cmd":"ls"}}
    ]}}));
    assert_eq!(evs.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["n1", "n1:1"]);
    assert_eq!(evs[1].tool_input, Some(json!({"cmd":"ls"}).as_object().unwrap().clone()));
    assert_eq!(evs[1].tool_use_id.as_deref(), Some("call_1"));
}

#[test]
fn h005_pi_thinking_requires_nonempty_string() {
    for text in [json!("hmm"), json!("  "), json!(42), json!(null)] {
        let evs = parse("pi", json!({"type":"message","id":"n1","message":{"role":"assistant","content":[{"type":"thinking","thinking":text}]}}));
        if text == "hmm" { assert_eq!(evs[0].kind.as_str(), "thinking"); assert_eq!(evs[0].text.as_deref(), Some("hmm")); }
        else { assert!(evs.is_empty()); }
    }
}

#[test]
fn h006_pi_tools_normalize_arguments() {
    for args in [json!({"cmd":"ls"}), json!([]), json!(null)] {
        let evs = parse("pi", json!({"type":"message","id":"n1","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","id":"call_1","arguments":args}]}}));
        assert_eq!(evs[0].tool_name.as_deref(), Some("bash"));
        assert_eq!(evs[0].tool_input.as_ref().unwrap().len(), usize::from(args.is_object()));
    }
}

#[test]
fn h007_h008_pi_result_links_errors_and_summarizes_images() {
    let evs = parse("pi", json!({"type":"message","id":"r1","message":{"role":"toolResult","toolCallId":"call_1","isError":true,"content":[
        {"type":"text","text":"saida"},{"type":"image","mimeType":"image/png","data":"AAAA"}
    ]}}));
    assert_eq!(evs[0].result.as_deref(), Some("saida\n[imagem image/png]"));
    assert_eq!(evs[0].tool_use_id.as_deref(), Some("call_1"));
    assert_eq!(evs[0].is_error, Some(true));
    assert_eq!(evs[0].tool_name, None);
}

#[test]
fn h009_pi_cleans_ansi_and_lone_surrogates() {
    let provider = Provider::parse("pi").expect("Pi suportado");
    let evs = LineParser::new(provider).feed(br#"{"type":"message","id":"n1","message":{"role":"assistant","content":[{"type":"text","text":"\u001b[0mmeio \ud83d fim"}]}}"#, 0);
    assert_eq!(evs[0].text.as_deref(), Some("meio \u{fffd} fim"));
    assert!(serde_json::to_string(&evs).is_ok());
}

#[test]
fn h013_h014_kimi_only_user_origin_and_distinct_blocks() {
    for origin in ["user", "injection", "steer"] {
        let evs = parse("kimi", json!({"type":"context.append_message","time":1786453187986u64,"message":{
            "id":"msg_1","role":"user","origin":{"kind":origin},"content":[{"type":"text","text":"oi"},{"type":"text","text":"outra"}]}}));
        if origin == "user" {
            assert_eq!(evs.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["msg_1", "msg_1:1"]);
            assert_eq!(evs[0].ts, Some(1786453187.986));
        } else { assert!(evs.is_empty()); }
    }
}

#[test]
fn h015_kimi_text_and_thinking_have_separate_kinds() {
    for (part, kind, text) in [(json!({"type":"text","text":"ok"}), "assistant_msg", "ok"), (json!({"type":"think","think":"hmm"}), "thinking", "hmm")] {
        let evs = parse("kimi", json!({"type":"context.append_loop_event","event":{"type":"content.part","uuid":"u1","part":part}}));
        assert_eq!(evs[0].kind.as_str(), kind);
        assert_eq!(evs[0].text.as_deref(), Some(text));
        assert_eq!(evs[0].id, "u1");
    }
}

#[test]
fn h016_kimi_tools_preserve_full_call() {
    let evs = parse("kimi", json!({"type":"context.append_loop_event","event":{"type":"tool.call","uuid":"u3","toolCallId":"tool_1","name":"Read","args":{"path":"/tmp/x"}}}));
    assert_eq!(evs[0].id, "u3");
    assert_eq!(evs[0].tool_name.as_deref(), Some("Read"));
    assert_eq!(evs[0].tool_use_id.as_deref(), Some("tool_1"));
    assert_eq!(evs[0].tool_input.as_ref().unwrap()["path"], "/tmp/x");
}

#[test]
fn h018_kimi_result_string_and_object_keep_unicode() {
    for (result, expected) in [(json!("conteúdo"), "conteúdo"), (json!({"output":"conteúdo"}), "conteúdo"), (json!({"outro":"ação"}), "{\"outro\": \"ação\"}")] {
        let evs = parse("kimi", json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"t","result":result}}));
        assert_eq!(evs[0].result.as_deref(), Some(expected));
    }
}

#[test]
fn h019_kimi_metadata_and_empty_thinking_are_ignored() {
    for value in [json!({"type":"metadata"}), json!({"type":"turn.prompt"}), json!({"type":"config.update"}), json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":42}}})] {
        assert!(parse("kimi", value).is_empty());
    }
}

fn pi_user(text: &str) -> Value { json!({"type":"message","id":"n1","message":{"role":"user","timestamp":1785160179000u64,"content":[{"type":"text","text":text}]}}) }
fn hook(parent: &str, context: &str) -> Value { json!({"type":"custom_message","customType":"claude-hook-context","parentId":parent,"content":context}) }

#[test]
fn h002_pi_user_has_original_id_and_no_empty_bubbles() {
    let provider = Provider::parse("pi"); assert!(provider.is_some());
    let mut parser = LineParser::new(provider.unwrap());
    assert!(parser.feed(pi_user("oi").to_string().as_bytes(), 31).is_empty());
    let events = parser.flush();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some("oi"));
    assert_eq!(events[0].id, "n1");
    assert!(parser.feed(pi_user("  ").to_string().as_bytes(), 50).is_empty());
    assert!(parser.flush().is_empty());
}

#[test]
fn h010_h034_hook_pair_strips_context_preserving_original_line_offset() {
    let provider = Provider::parse("pi"); assert!(provider.is_some());
    let context = "[skill-suggester] Prompt casa com skills instaladas que o usuario costuma esquecer:\n- polir layout: refactoring-ui";
    let prompt = "Ué o layout não parece ter seguido os padrões?";
    let mut parser = LineParser::new(provider.unwrap());
    assert!(parser.feed(pi_user(&format!("{context}\n\n{prompt}")).to_string().as_bytes(), 31).is_empty());
    assert!(parser.has_pending());
    let events = parser.feed(hook("n1",context).to_string().as_bytes(), 251);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some(prompt));
    assert_eq!(events[0].offset, Some(31));
    assert!(!parser.has_pending());
    assert!(parser.flush().is_empty());
    parser.feed(pi_user(context).to_string().as_bytes(), 350);
    assert!(parser.feed(hook("n1",context).to_string().as_bytes(), 500).is_empty());
}

#[test]
fn h011_unmatched_hook_and_batch_flush_keep_person_text() {
    let provider = Provider::parse("omp"); assert!(provider.is_some());
    let mut parser = LineParser::new(provider.unwrap());
    parser.feed(pi_user("contexto e mais coisa").to_string().as_bytes(), 10);
    let events = parser.feed(hook("outra", "contexto").to_string().as_bytes(), 100);
    assert_eq!(events[0].text.as_deref(), Some("contexto e mais coisa"));
    assert_eq!(events[0].offset, Some(10));
    parser.feed(pi_user("oi").to_string().as_bytes(), 150);
    assert_eq!(parser.flush()[0].text.as_deref(), Some("oi"));
    parser.feed(pi_user("contexto e mais coisa").to_string().as_bytes(), 200);
    assert_eq!(parser.flush()[0].text.as_deref(), Some("contexto e mais coisa"));
    parser.feed(pi_user("oi").to_string().as_bytes(), 250);
    let assistant = json!({"type":"message","id":"n2","message":{"role":"assistant","content":[{"type":"text","text":"ola"}]}});
    let events = parser.feed(assistant.to_string().as_bytes(), 350);
    assert_eq!(events.iter().map(|event| event.text.as_deref().unwrap()).collect::<Vec<_>>(), ["oi","ola"]);
    assert_eq!(events.iter().map(|event| event.offset).collect::<Vec<_>>(), [Some(250),Some(350)]);
}

#[test]
fn h055_history_keeps_queue_entry_between_held_user_and_reply() {
    use hangar_transcript::{HistoryRequest, merged_history, TAIL_WINDOW};
    use std::io::Write;
    let provider = Provider::parse("pi"); assert!(provider.is_some());
    let mut transcript = tempfile::NamedTempFile::new().unwrap();
    writeln!(transcript,"{}",pi_user("primeiro")).unwrap();
    writeln!(transcript,"{}",json!({"type":"message","id":"n2","message":{"role":"assistant","timestamp":1785160184000u64,"content":[{"type":"text","text":"resposta"}]}})).unwrap();
    let mut queue = tempfile::NamedTempFile::new().unwrap();
    writeln!(queue,"{}",json!({"id":"q1","text":"segundo","ts":1785160180.0})).unwrap();
    let request = HistoryRequest { provider:provider.unwrap(), jsonl:transcript.path().to_owned(), queue:Some(queue.path().to_owned()), limit:None, tail_window:TAIL_WINDOW };
    let events = merged_history(&request).unwrap();
    assert_eq!(events.iter().filter_map(|e| e.text.as_deref()).collect::<Vec<_>>(), ["primeiro","segundo","resposta"]);
}

#[test]
fn h056_real_pi_corpus_keeps_full_tools_and_cleans_ansi() {
    let provider = Provider::parse("pi"); assert!(provider.is_some());
    let mut parser = LineParser::new(provider.unwrap());
    let input = include_bytes!("fixtures/pi_session.jsonl");
    assert!(input.windows(7).any(|s| s == br"\u001b["));
    let mut events = Vec::new();
    let mut offset = 0;
    for line in input.split(|b| *b == b'\n') {
        events.extend(parser.feed(line, offset));
        offset += line.len() as u64 + 1;
    }
    events.extend(parser.flush());
    assert_eq!(events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["n1","n2","n2:1","n2:2","n3","n4","n4:1","n5","n6","n7","n8","n9"]);
    assert_eq!(events.iter().filter(|e| e.kind.as_str() == "tool_result").map(|e| e.result.as_deref().unwrap()).collect::<Vec<_>>(),
        ["README.md\nmain.py\n", "[imagem image/png]", "permission denied"]);
    let uses = events.iter().filter(|e| e.kind.as_str() == "tool_use").collect::<Vec<_>>();
    assert_eq!(uses.len(), 3);
    assert_eq!(uses[0].tool_input.as_ref().unwrap()["command"], "ls /work/demo");
    for result in events.iter().filter(|e| e.kind.as_str() == "tool_result") {
        assert!(uses.iter().any(|call| call.tool_use_id == result.tool_use_id));
    }
    assert!(events.iter().any(|e| e.kind.as_str() == "user_msg"));
    assert!(events.iter().all(|e| !e.text.as_deref().unwrap_or("").contains('\x1b') && !e.result.as_deref().unwrap_or("").contains('\x1b')));
}

fn external_json() -> impl Strategy<Value = Value> {
    let scalar = prop_oneof![Just(Value::Null), any::<bool>().prop_map(Value::Bool), any::<i64>().prop_map(|n| json!(n)), ".{0,80}".prop_map(Value::String)];
    scalar.prop_recursive(5, 128, 8, |inner| prop_oneof![
        prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
        prop::collection::vec((prop_oneof![Just("type".to_owned()), Just("message".to_owned()), Just("content".to_owned()), Just("role".to_owned()), Just("event".to_owned()), ".{0,16}"], inner), 0..8).prop_map(|pairs| Value::Object(pairs.into_iter().collect()))
    ])
}

proptest! {
    #[test]
    fn external_jsonl_never_panics(value in external_json(), raw in prop::collection::vec(any::<u8>(), 0..512)) {
        for name in ["pi","omp","kimi"] {
            if let Some(provider) = Provider::parse(name) {
                let mut parser = LineParser::new(provider);
                let mut events = parser.feed(value.to_string().as_bytes(), 0);
                events.extend(parser.feed(&raw, 64));
                events.extend(parser.flush());
                prop_assert!(serde_json::to_vec(&events).is_ok());
            }
        }
    }
}
