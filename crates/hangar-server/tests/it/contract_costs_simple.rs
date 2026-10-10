use crate::common;
use common::costs::*;
use hangar_server::costs;
use hangar_server::costs::index::{Fold, Index};
use hangar_server::costs::simple;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

fn sync_simple(ix: &Index, base: &Path) {
    let pi = base.join("pi");
    let files = costs::collect::list_files(&pi, |n| n.ends_with(".jsonl"));
    ix.sync(&format!("pi:{}", pi.display()), &files, &simple::new_pi_fold(&pi, "pi"),
        simple::PI_VERSION, "", &|_| vec![], &progress()).unwrap();
    let kimi = base.join("kimi/sessions");
    let files = costs::collect::list_files(&kimi, |n| n == "wire.jsonl");
    ix.sync(&format!("kimi:{}", kimi.display()), &files, &simple::new_kimi_fold,
        simple::KIMI_VERSION, "", &|_| vec![], &progress()).unwrap();
}

/// Id de sessão do Pi é caminho relativo: `str(Path)` no Python, `MAIN_SEPARATOR` no Rust; o golden
/// foi gravado com `/`.
fn native_separators(value: &Value) -> Value {
    match value {
        Value::String(text) if text.starts_with("--repo--/") => Value::String(text.replace('/', std::path::MAIN_SEPARATOR_STR)),
        Value::Array(values) => Value::Array(values.iter().map(native_separators).collect()),
        Value::Object(values) => Value::Object(values.iter().map(|(k, v)| (k.clone(), native_separators(v))).collect()),
        other => other.clone(),
    }
}

fn assert_golden(ix: &Index, base: &Path, golden: &Value) {
    for prefix in ["pi/", "kimi/"] {
        let want = golden.as_object().unwrap().iter().filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), native_separators(value))).collect::<Map<_, _>>();
        assert_close(&dump(ix, base, prefix), &Value::Object(want), prefix);
    }
}

fn feed(fold: &mut impl Fold, value: Value) {
    fold.line(&serde_json::to_vec(&value).unwrap());
}

fn pi_start(source: &str) -> simple::PiFold {
    let mut fold = simple::new_pi_fold(Path::new("sessions"), source)(Path::new("sessions/é/t/run-1/session.jsonl"));
    feed(&mut fold, json!({"type":"session","cwd":"/repo/ação","timestamp":"2026-10-01T12:00:00"}));
    feed(&mut fold, json!({"type":"model_change","provider":"openai-codex","modelId":"gpt"}));
    fold
}

fn kimi_usage(input: i64, time: f64, model: &str) -> Value {
    json!({"type":"usage.record","time":time,"model":model,
        "usage":{"inputOther":input,"output":2,"inputCacheRead":3,"inputCacheCreation":4}})
}

#[test]
fn pi_and_kimi_index_match_python() {
    let (_dir, base) = fixtures_copy();
    let ix = Index::open(&base.join("../idx")).unwrap();
    sync_simple(&ix, &base);
    assert_golden(&ix, &base, &golden_index());
}

#[test]
fn pi_and_kimi_resumed_in_two_halves_match_python() {
    let (_dir, base) = fixtures_copy();
    let ix = Index::open(&base.join("../idx")).unwrap();
    let tails = halve_all(&base);
    sync_simple(&ix, &base);
    for (path, tail) in tails { append(&path, &tail); }
    sync_simple(&ix, &base);
    assert_golden(&ix, &base, &golden_index()["__resumed__"]);
}

#[test]
fn omp_preserves_relative_id_final_model_and_all_message_deltas() {
    let mut fold = pi_start("omp");
    feed(&mut fold, json!({"type":"message","message":{"usage":{"input":9,"output":true,"cacheRead":"１２","cacheWrite":2.9,"cost":{"total":999}}}}));
    feed(&mut fold, json!({"type":"model_change","model":"kimi-coding/novo/modelo"}));
    feed(&mut fold, json!({"type":"message","message":{"usage":{"input":"-3","output":4,"cacheRead":1,"cacheWrite":"invalid"}}}));
    feed(&mut fold, json!({"type":"session","cwd":"","timestamp":"invalid"}));
    let snapshot = serde_json::to_vec(&fold).unwrap();
    let out = fold.close();
    assert_eq!(snapshot, serde_json::to_vec(&fold).unwrap());
    assert_eq!(out.costs, fold.close().costs);
    assert!(out.usage.is_empty() && out.areas.is_none());
    let row = &out.costs[0];
    #[cfg(windows)]
    assert_eq!(row.session_id, r"é\t\run-1\session");
    #[cfg(not(windows))]
    assert_eq!(row.session_id, "é/t/run-1/session");
    assert_eq!((row.source.as_str(), row.provider.as_str(), row.model.as_str()), ("omp", "moonshotai", "novo/modelo"));
    assert_eq!((row.input, row.output, row.cache_read, row.cache_write), (6, 5, 13, 2));
    assert_eq!(row.ts.iso(), "2026-10-01T12:00:00-03:00");
    assert_eq!(row.project, "/repo/ação");
    assert!(!row.subagente && row.account_id.is_none() && !row.codex_long_context && !row.fast);
    assert_eq!((row.cache_write_1h, row.regravado, row.regravado_1h), (0, 0, 0));
}

#[test]
fn pi_snapshot_resume_preserves_metadata_and_utf8_replacement() {
    let mut fold = pi_start("pi");
    feed(&mut fold, json!({"type":"message","message":{"usage":{"input":10}}}));
    let mut resumed: simple::PiFold = serde_json::from_slice(&serde_json::to_vec(&fold).unwrap()).unwrap();
    let raw = b"{\"type\":\"session\",\"cwd\":\"/repo/\xff\",\"timestamp\":\"2026-10-02T03:00:00Z\"}";
    fold.line(raw);
    resumed.line(raw);
    let tail = json!({"type":"message","message":{"usage":{"input":20,"output":3}}});
    feed(&mut fold, tail.clone());
    feed(&mut resumed, tail);
    assert_eq!(fold.close().costs, resumed.close().costs);
    let row = &resumed.close().costs[0];
    assert_eq!((row.input, row.output), (30, 3));
    assert_eq!(row.project, "/repo/�");
    assert_eq!(row.ts.iso(), "2026-10-02T00:00:00-03:00");
}

#[test]
fn simple_folds_skip_missing_usage_or_timestamp_and_keep_zero_usage() {
    let mut pi = simple::new_pi_fold(Path::new("sessions"), "pi")(Path::new("sessions/a.jsonl"));
    for raw in [b"[]".as_slice(), b"null", b"broken", b"{\"type\":\"message\",\"message\":[]}"] { pi.line(raw); }
    feed(&mut pi, json!({"type":"message","message":{"usage":{"input":7}}}));
    assert!(pi.close().costs.is_empty());
    pi = simple::new_pi_fold(Path::new("sessions"), "pi")(Path::new("sessions/a.jsonl"));
    feed(&mut pi, json!({"type":"session","timestamp":"2026-10-01T12:00:00Z"}));
    assert!(pi.close().costs.is_empty());
    feed(&mut pi, json!({"type":"message","message":{"usage":{}}}));
    let row = pi.close().costs.remove(0);
    assert_eq!((row.provider.as_str(), row.model.as_str(), row.project.as_str()), ("?", "?", "desconhecido"));
    let mut kimi = simple::new_kimi_fold(Path::new("wd/session_k/agents/main/wire.jsonl"));
    feed(&mut kimi, json!({"type":"usage.record","time":"1790769600123","usage":{"inputOther":10}}));
    assert!(kimi.close().costs.is_empty());
    feed(&mut kimi, json!({"type":"usage.record","time":true,"usage":{}}));
    assert_eq!(kimi.close().costs[0].ts, costs::py::LocalTs::from_millis_f64(1.0));
}

#[test]
fn kimi_keeps_last_numeric_timestamp_model_alias_and_subagent_rule() {
    let mut fold = simple::new_kimi_fold(Path::new("wd/session_k/agents/agent-1/wire.jsonl"));
    for value in [json!({"type":"message","text":"usage.record","usage":{"inputOther":999}}),
        json!({"type":"usage.record","time":1790769600123.5,"model":"moonshot/k3","usage":{"inputOther":"１２","output":true}}),
        json!({"type":"usage.record","time":"invalid","model":"","usage":{"inputOther":2.9,"inputCacheRead":5,"inputCacheCreation":6}})] {
        feed(&mut fold, value);
    }
    let snapshot = serde_json::to_vec(&fold).unwrap();
    let out = fold.close();
    assert_eq!(snapshot, serde_json::to_vec(&fold).unwrap());
    assert_eq!(out.costs, fold.close().costs);
    assert!(out.usage.is_empty() && out.areas.is_none());
    let row = &out.costs[0];
    assert_eq!(row.ts, costs::py::LocalTs::from_millis_f64(1790769600123.5));
    assert_eq!((row.input, row.output, row.cache_read, row.cache_write), (14, 1, 5, 6));
    assert_eq!((row.provider.as_str(), row.model.as_str(), row.project.as_str(), row.session_id.as_str()), ("moonshotai", "moonshot/k3", "desconhecido", "session_k"));
    assert!(row.subagente);
    for (path, subagent) in [("wd/session_k/agents/main/wire.jsonl", false),
        ("wd/session_k/other/agent-1/wire.jsonl", false), ("wd/session_k/agents/agent-1/other.jsonl", false)] {
        let mut other = simple::new_kimi_fold(Path::new(path));
        feed(&mut other, kimi_usage(1, 1790769600123.0, "unmapped/model"));
        assert_eq!(other.close().costs[0].subagente, subagent);
        assert_eq!(other.close().costs[0].provider, "unmapped");
    }
}

#[test]
fn kimi_subagent_effective_resume_accumulates_two_events_without_recreating_fold() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wd/session_k/agents/agent-1/wire.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let first = serde_json::to_vec(&kimi_usage(10, 1790769600123.0, "apikey/k3")).unwrap();
    std::fs::write(&path, [first.as_slice(), b"\n"].concat()).unwrap();
    let ix = Index::open(&dir.path().join("idx")).unwrap();
    let creations = AtomicUsize::new(0);
    let tracked = |p: &Path| { creations.fetch_add(1, Ordering::Relaxed); simple::new_kimi_fold(p) };
    let sync = || ix.sync("kimi:synthetic", &[path.clone()], &tracked, simple::KIMI_VERSION,
        "", &|_| vec![], &progress()).unwrap();
    sync();
    let before = ix.read_costs(Some("kimi:synthetic"), None, None).unwrap();
    assert_eq!(before[0].input, 10);
    let second = serde_json::to_vec(&kimi_usage(20, 1790769601123.0, "openai-codex/model" )).unwrap();
    append(&path, &[second.as_slice(), b"\n"].concat());
    sync();
    assert_eq!(creations.load(Ordering::Relaxed), 1, "a retomada deve consumir a cauda sobre o estado salvo");
    let row = ix.read_costs(Some("kimi:synthetic"), None, None).unwrap().remove(0);
    assert_eq!((row.input, row.output, row.cache_read, row.cache_write), (30, 4, 6, 8));
    assert_eq!(row.provider, "openai");
    assert_eq!(row.ts, costs::py::LocalTs::from_millis_f64(1790769601123.0));
    assert!(row.subagente);
    assert_eq!(row.project, "desconhecido");
}

#[test]
fn pi_index_resume_does_not_double_count_complete_fragment() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("pi");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    let prefix = b"{\"type\":\"session\",\"timestamp\":\"2026-10-01T12:00:00Z\"}\n{\"type\":\"message\",\"message\":{\"usage\":{\"input\":10}}}\n";
    let fragment = b"{\"type\":\"message\",\"message\":{\"usage\":{\"input\":20}}}";
    std::fs::write(&path, [prefix.as_slice(), fragment].concat()).unwrap();
    let ix = Index::open(&dir.path().join("idx")).unwrap();
    let factory = simple::new_pi_fold(&root, "pi");
    let creations = AtomicUsize::new(0);
    let tracked = |p: &Path| { creations.fetch_add(1, Ordering::Relaxed); factory(p) };
    let sync = || ix.sync("pi:synthetic", &[path.clone()], &tracked, simple::PI_VERSION,
        "", &|_| vec![], &progress()).unwrap();
    sync();
    assert_eq!(ix.read_costs(Some("pi:synthetic"), None, None).unwrap()[0].input, 30);
    append(&path, b"\n{\"type\":\"message\",\"message\":{\"usage\":{\"input\":3}}}\n");
    sync();
    assert_eq!(creations.load(Ordering::Relaxed), 1);
    assert_eq!(ix.read_costs(Some("pi:synthetic"), None, None).unwrap()[0].input, 33);
}

#[test]
fn kimi_snapshot_resume_keeps_empty_alias_fallback_and_utf8_replacement() {
    let mut fold = simple::new_kimi_fold(Path::new("wd/session_k/agents/main/wire.jsonl"));
    feed(&mut fold, kimi_usage(10, 1790769600123.0, "plain"));
    assert_eq!(fold.close().costs[0].provider, "?");
    let mut resumed: simple::KimiFold = serde_json::from_slice(&serde_json::to_vec(&fold).unwrap()).unwrap();
    let raw = b"{\"type\":\"usage.record\",\"model\":\"/k\xff\",\"usage\":{\"inputOther\":5}}";
    fold.line(raw);
    resumed.line(raw);
    assert_eq!(fold.close().costs, resumed.close().costs);
    let row = resumed.close().costs.remove(0);
    assert_eq!((row.input, row.model.as_str(), row.provider.as_str()), (15, "/k�", "?"));
}

#[test]
fn pi_and_omp_relative_id_uses_native_separator_and_removes_only_last_extension() {
    let root = Path::new("sessions");
    let path = root.join("é").join("t").join("run-1").join("session.tar.jsonl");
    #[cfg(windows)]
    let expected = r"é\t\run-1\session.tar";
    #[cfg(not(windows))]
    let expected = "é/t/run-1/session.tar";
    for source in ["pi", "omp"] {
        let mut fold = simple::new_pi_fold(root, source)(&path);
        feed(&mut fold, json!({"type":"session","timestamp":"2026-10-01T12:00:00Z"}));
        feed(&mut fold, json!({"type":"message","message":{"usage":{"input":1}}}));
        let mut resumed: simple::PiFold = serde_json::from_slice(&serde_json::to_vec(&fold).unwrap()).unwrap();
        let row = fold.close().costs.remove(0);
        assert_eq!(row.session_id, expected);
        assert_eq!(row.source, source);
        assert_eq!(resumed.close().costs[0].session_id, expected);
    }
}

#[test]
fn kimi_projects_skips_invalid_lines_and_later_entries_replace_projects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session_index.jsonl");
    std::fs::write(&path, b"broken\n[]\n{\"sessionId\":\"\"}\n{\"sessionId\":\"one\",\"workDir\":\"/repo/old\"}\n{\"sessionId\":\"two\"}\n{\"sessionId\":\"one\",\"workDir\":\"/repo/a\"}\n").unwrap();
    let projects = simple::kimi_projects(&path);
    assert_eq!(projects.len(), 2);
    assert_eq!(projects["one"], "/repo/a");
    assert_eq!(projects["two"], "");
    assert!(simple::kimi_projects(&dir.path().join("missing")).is_empty());
    assert_eq!(simple::kimi_projects(&contract().join("costs/kimi/session_index.jsonl"))["session_k1"], "/repo/k");
}
