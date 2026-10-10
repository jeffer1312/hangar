// crates/hangar-server/tests/contract_tail.rs
use crate::common;

use common::{canon, contract, golden};
use hangar_server::transcript::{LineParser, Provider};

/// (offset, evento) de cada linha, como o leitor ao vivo vai alimentar o parser.
fn tail(fixture: &str, provider: Provider) -> Vec<(u64, String)> {
    let bytes = std::fs::read(contract().join("transcripts").join(fixture)).expect("fixture");
    let mut parser = LineParser::new(provider).with_peer_resolver(|_| None);
    let mut out = Vec::new();
    let mut start = 0u64;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        for ev in parser.feed(line, start) {
            assert_eq!(ev.offset, Some(start));
            out.push((start, canon(&serde_json::to_value(&ev).unwrap())));
        }
        start += line.len() as u64;
    }
    out
}

fn want(name: &str) -> Vec<(u64, String)> {
    golden(name)
        .as_array()
        .expect("lista")
        .iter()
        .map(|r| (r["offset"].as_u64().expect("offset"), canon(&r["event"])))
        .collect()
}

fn assert_same(got: Vec<(u64, String)>, want: Vec<(u64, String)>, ctx: &str) {
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "{ctx}: evento {i}");
    }
    assert_eq!(got.len(), want.len(), "{ctx}: quantidade de eventos");
}

#[test]
fn claude_with_and_without_terminal_matches_python() {
    for provider in [Provider::Claude, Provider::ClaudeHeadless] {
        assert_same(tail("claude.jsonl", provider), want("claude.tail.json"), provider.as_str());
    }
}

#[test]
fn codex_matches_python() {
    assert_same(tail("codex.jsonl", Provider::Codex), want("codex.tail.json"), "codex");
}

#[test]
fn lone_surrogate_with_timestamp_keeps_reading() {
    // No Python esta linha estoura o md5 do RewriteFilter; o golden traz o que o parse_obj daria.
    assert_same(
        tail("claude_rewrite_surrogate.jsonl", Provider::Claude),
        want("claude_rewrite_surrogate.tail.json"),
        "reescrita com surrogate",
    );
}

#[test]
fn pasted_content_needs_the_same_id_to_close() {
    let text_of = |content: &str| {
        let line = serde_json::json!({"type": "user", "uuid": "u", "message": {"role": "user", "content": content}});
        LineParser::new(Provider::Claude).feed(line.to_string().as_bytes(), 0).pop().and_then(|ev| ev.text)
    };
    let pasted = "antes <pasted_content id=\"a\">\noi\n</pasted_content id=\"a\"> depois";
    assert_eq!(text_of(pasted).as_deref(), Some("antes oi depois"));
    let quoted = "<pasted_content id=\"a\">oi</pasted_content id=\"b\">";
    assert_eq!(text_of(quoted).as_deref(), Some(quoted));
}

#[test]
fn provider_from_python_name() {
    assert_eq!(Provider::parse("claude"), Some(Provider::Claude));
    assert_eq!(Provider::parse("claude-headless"), Some(Provider::ClaudeHeadless));
    assert_eq!(Provider::parse("codex"), Some(Provider::Codex));
    assert_eq!(Provider::parse("pi"), None);
}

#[test]
fn native_peer_message_is_labeled_with_the_tmux_name() {
    let feed = |line: serde_json::Value| {
        let mut parser =
            LineParser::new(Provider::Claude).with_peer_resolver(|pid| (pid == 123).then(|| "alvo".to_string()));
        parser.feed(line.to_string().as_bytes(), 0).pop().and_then(|ev| ev.text)
    };
    let origin = serde_json::json!({"type": "user", "uuid": "u", "message": {"role": "user", "content": "x"},
        "origin": {"kind": "peer", "name": "Título da sessão", "verifiedPeerPid": 123, "body": "oi"}});
    assert_eq!(feed(origin).as_deref(), Some("[de: alvo] oi"));
    let wrapped = serde_json::json!({"type": "queue-operation", "operation": "remove", "timestamp": "t", "content":
        "<cross-session-message from=\"/run/cc-socks/123.sock\" from-name=\"Título\">\noi\n</cross-session-message>"});
    assert_eq!(feed(wrapped).as_deref(), Some("[de: alvo] oi"));
    // sem o pid resolvido, o título do recado segue valendo
    let unknown = serde_json::json!({"type": "user", "uuid": "u", "message": {"role": "user", "content": "x"},
        "origin": {"kind": "peer", "name": "Título", "verifiedPeerPid": 7, "body": "oi"}});
    assert_eq!(feed(unknown).as_deref(), Some("[de: Título] oi"));
}
