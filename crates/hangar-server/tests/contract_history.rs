// crates/hangar-server/tests/contract_history.rs
mod common;

use std::path::Path;

use common::{canon, contract, golden};
use hangar_server::transcript::{
    history_etag, merged_history, HistoryRequest, InternalInfo, LineParser, Provider, TAIL_WINDOW,
};

const VARIANTS: [(&str, Option<usize>, u64); 6] = [
    ("full", None, TAIL_WINDOW),
    ("limit2", Some(2), TAIL_WINDOW),
    ("limit200", Some(200), TAIL_WINDOW),
    ("limit2_w512", Some(2), 512),
    ("limit9_w512", Some(9), 512),
    ("limit200_w512", Some(200), 512),
];

fn check_history(fixture: &str, provider: Provider, queue: &str) {
    let g = golden(&format!("{}.history.json", fixture.trim_end_matches(".jsonl")));
    for (name, limit, window) in VARIANTS {
        for (suffix, session) in [("", "sem-queue"), ("+queue", queue)] {
            let req = HistoryRequest {
                provider,
                jsonl: contract().join("transcripts").join(fixture),
                queue: Some(contract().join("queue").join(format!("{session}.jsonl"))),
                limit,
                tail_window: window,
            };
            let key = format!("{name}{suffix}");
            let got: Vec<String> =
                merged_history(&req).unwrap().iter().map(|ev| canon(&serde_json::to_value(ev).unwrap())).collect();
            let want: Vec<String> =
                g[&key].as_array().unwrap_or_else(|| panic!("golden sem {key}")).iter().map(canon).collect();
            for (i, (a, b)) in got.iter().zip(&want).enumerate() {
                assert_eq!(a, b, "{fixture} {key}: evento {i}");
            }
            assert_eq!(got.len(), want.len(), "{fixture} {key}: quantidade");
        }
    }
}

#[test]
fn claude_matches_python() {
    check_history("claude.jsonl", Provider::Claude, "claude-fixture");
}

#[test]
fn codex_matches_python() {
    check_history("codex.jsonl", Provider::Codex, "codex-fixture");
}

fn request(dir: &Path, limit: Option<usize>) -> HistoryRequest {
    HistoryRequest {
        provider: Provider::Claude,
        jsonl: dir.join("s.jsonl"),
        queue: Some(dir.join("queue.jsonl")),
        limit,
        tail_window: TAIL_WINDOW,
    }
}

#[test]
fn no_transcript_means_empty_history_and_no_etag() {
    let dir = tempfile::tempdir().unwrap();
    assert!(merged_history(&request(dir.path(), None)).unwrap().is_empty());
    assert_eq!(history_etag(&request(dir.path(), None)), None);
}

#[cfg(unix)]
#[test]
fn unreadable_transcript_is_an_error_not_empty_history() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.jsonl");
    std::fs::write(&p, "{\"type\": \"user\"}\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&p).is_ok() {
        return; // root lê mesmo sem permissão
    }
    for limit in [None, Some(2)] {
        assert!(merged_history(&request(dir.path(), limit)).is_err());
    }
}

#[test]
fn etag_changes_with_queue_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("s.jsonl"), "{\"type\": \"user\"}\n").unwrap();
    let before = history_etag(&request(dir.path(), None)).unwrap();
    assert!(before.starts_with('"') && before.ends_with('"'));
    assert_eq!(history_etag(&request(dir.path(), None)).unwrap(), before);
    std::fs::write(dir.path().join("queue.jsonl"), "{\"id\": \"a\", \"text\": \"oi\"}\n").unwrap();
    let with_queue = history_etag(&request(dir.path(), None)).unwrap();
    assert_ne!(with_queue, before);
    assert_ne!(history_etag(&request(dir.path(), Some(30))).unwrap(), with_queue);
}

#[test]
fn internal_info_becomes_history_request() {
    let info: InternalInfo = serde_json::from_value(serde_json::json!({
        "provider": "claude-headless", "jsonl": "/x/a.jsonl", "session_key": "a",
        "history": {"queue": "/q/s.jsonl"},
    }))
    .unwrap();
    let req = info.history_request(Some(0)).unwrap();
    assert_eq!(req.provider, Provider::ClaudeHeadless);
    assert_eq!(req.jsonl, Path::new("/x/a.jsonl"));
    assert_eq!(req.queue.as_deref(), Some(Path::new("/q/s.jsonl")));
    assert_eq!((req.limit, req.tail_window), (None, TAIL_WINDOW));
    assert_eq!(info.history_request(Some(30)).unwrap().limit, Some(30));
    let pi: InternalInfo =
        serde_json::from_value(serde_json::json!({"provider": "pi", "jsonl": "/x/a.jsonl", "session_key": "a", "history": {}}))
            .unwrap();
    assert!(pi.history_request(None).is_none());
    let fresh: InternalInfo =
        serde_json::from_value(serde_json::json!({"provider": "codex", "jsonl": null, "session_key": "", "history": {}}))
            .unwrap();
    assert!(fresh.history_request(None).is_none());
}

// A Task 12 manda os dois para `spawn_blocking`: se deixarem de ser Send, quebra aqui e não lá.
#[test]
fn parser_and_request_cross_threads() {
    fn _assert_send<T: Send + 'static>() {}
    _assert_send::<LineParser>();
    _assert_send::<HistoryRequest>();
}
