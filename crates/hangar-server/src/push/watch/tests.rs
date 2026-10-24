//! Casos tirados do Python por `backend/tests/fixtures/contract/push/gen_push.py`.
use super::*;
use crate::push::send::{Notice, Outgoing};
use crate::push::store;
use serde_json::json;

fn cases() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/push/cases.json");
    serde_json::from_slice(&std::fs::read(path).expect("rode gen_push.py")).unwrap()
}

#[test]
fn summary_matches_python() {
    for c in cases()["summary"].as_array().unwrap() {
        assert_eq!(summary(c["text"].as_str()).map(Value::from).unwrap_or(Value::Null), c["out"], "{}", c["text"]);
    }
}

#[test]
fn quiet_hours_match_python() {
    for c in cases()["quiet"].as_array().unwrap() {
        let now = store::parse_time(c["now"].as_str().unwrap()).unwrap();
        assert_eq!(Value::Bool(store::in_quiet_hours(&c["prefs"], now)), c["out"], "{c}");
    }
}

#[test]
fn reset_delay_matches_python() {
    for c in cases()["reset"].as_array().unwrap() {
        let now = chrono::NaiveDateTime::parse_from_str(c["now"].as_str().unwrap(), "%Y-%m-%dT%H:%M:%S").unwrap();
        assert_eq!(reset_delay(c["reset"].as_str().unwrap(), now).map(Value::from).unwrap_or(Value::Null), c["out"], "{c}");
    }
}

fn outgoing(case: &str) -> Outgoing {
    match case {
        "awaiting_question" => Outgoing::single("s", Notice::Awaiting(Some("Qual ambiente?".into()))),
        "awaiting_default" => Outgoing::single("s", Notice::Awaiting(None)),
        "awaiting_coalesced" => Outgoing::coalesced(&["a".to_owned(), "b".to_owned()].into()),
        "finished_reply" => Outgoing::single("s", Notice::Finished(Some("Tudo pronto".into()))),
        "finished_plain" => Outgoing::single("s", Notice::Finished(None)),
        "dead" => Outgoing::single("s", Notice::Dead),
        "stalled" => Outgoing::single("s", Notice::Stalled),
        "limited_reset" => Outgoing::single("s", Notice::Limited(Some("3pm".into()))),
        "limited_plain" => Outgoing::single("s", Notice::Limited(None)),
        other => panic!("caso novo no golden: {other}"),
    }
}

#[test]
fn payloads_match_python_for_every_subscription_and_expo() {
    let subs = [json!({"subscription": {"endpoint": "e-pt"}, "label": "Casa", "serverId": "s1", "locale": "pt"}),
        json!({"subscription": {"endpoint": "e-en"}, "label": "Work", "serverId": "s2", "locale": "en"}),
        json!({"subscription": {"endpoint": "e-old"}, "label": "", "serverId": "s3"})];
    for c in cases()["payloads"].as_array().unwrap() {
        let out = outgoing(c["case"].as_str().unwrap());
        let web: Vec<Value> = subs.iter().map(|s| json!({"endpoint": s["subscription"]["endpoint"], "payload": out.payload(s)})).collect();
        assert_eq!(Value::Array(web), c["web"], "{}", c["case"]);
        let expo = json!([{"session": out.session, "title": {"pt": out.title_suffix("pt"), "en": out.title_suffix("en")},
            "body": {"pt": (out.body)("pt"), "en": (out.body)("en")}, "list": out.url.as_deref() == Some("/")}]);
        assert_eq!(expo, c["expo"], "{}", c["case"]);
    }
}

#[test]
fn background_scan_matches_python() {
    for c in cases()["scan"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let lines: Vec<&str> = c["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
        std::fs::write(&path, if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" }).unwrap();
        let s = scan(&path, c["now"].as_f64().unwrap()).unwrap();
        assert_eq!((json!(s.open), s.last_reply.map(Value::from).unwrap_or(Value::Null)), (c["open"].clone(), c["last_reply"].clone()), "{}", c["case"]);
    }
}

fn row(spec: &str) -> (String, Option<String>, bool, Option<Option<String>>, bool) {
    let (name, flags) = spec.split_once(':').unwrap_or((spec, ""));
    let limited = flags.starts_with("limited").then(|| flags.split_once('=').map(|(_, r)| r.to_owned()));
    (name.to_owned(), Some(name.to_owned()), flags == "stalled", limited, flags == "orq")
}

fn as_python(name: &str, notice: &Notice) -> Value {
    match notice {
        Notice::Dead => json!(["notify_dead", name]),
        Notice::Stalled => json!(["notify_stalled", name]),
        Notice::Limited(reset) => json!(["notify_limited", name, reset]),
        other => panic!("aviso fora da varredura: {other:?}"),
    }
}

#[test]
fn awaiting_and_finished_only_for_claude_and_codex_but_stall_for_every_row() {
    assert!(turn_notices("claude") && turn_notices("codex"));
    for other in ["kimi", "pi", "omp", "orq"] { assert!(!turn_notices(other), "{other} já é avisado pelo Python"); }
    // A varredura não olha o provedor: Kimi travada ainda sai daqui.
    let mut m = Memory::default();
    let notices = m.sweep(&[("kimi-1".into(), Some("k1".into()), true, None)], true);
    assert!(matches!(notices.as_slice(), [(n, Notice::Stalled)] if n == "kimi-1"));
}

#[test]
fn sweep_matches_python_stall_watch() {
    for c in cases()["sweep"].as_array().unwrap() {
        let mut m = Memory::default();
        let mut got = Vec::new();
        for step in c["steps"].as_array().unwrap() {
            let rows: Vec<_> = step.as_array().unwrap().iter().map(|s| row(s.as_str().unwrap()))
                .filter(|r| !r.4).map(|(n, k, s, l, _)| (n, k, s, l)).collect();
            let notices = m.sweep(&rows, c["notify_dead"].as_bool().unwrap());
            got.push(Value::Array(notices.iter().map(|(n, x)| as_python(n, x)).collect()));
        }
        assert_eq!(Value::Array(got), c["notices"], "{}", c["case"]);
    }
}

#[test]
fn turn_series_matches_python() {
    for c in cases()["series"].as_array().unwrap() {
        let mut m = Memory::default();
        let mut got = Vec::new();
        for step in c["steps"].as_array().unwrap() {
            let (state, ts) = (step[0].as_str().unwrap(), step[1].as_f64().unwrap());
            match state {
                "working" => m.working("sid", ts),
                "idle" => { m.last_idle.insert("sid".into(), ts); }
                _ => m.forget("sid"),
            }
            got.push(m.started.get("sid").map(|v| json!(v)).unwrap_or(Value::Null));
        }
        let want: Vec<Value> = c["started"].as_array().unwrap().iter().map(|v| v.as_f64().map(|f| json!(f)).unwrap_or(Value::Null)).collect();
        assert_eq!(got, want, "{}", c["case"]);
    }
}

#[test]
fn memory_survives_a_restart_so_nothing_is_lost_or_repeated() {
    let dir = tempfile::tempdir().unwrap();
    let path = Memory::path(dir.path());
    let mut m = Memory::default();
    m.working("sid", 100.0);
    assert!(m.sweep(&[("a".into(), Some("sid".into()), true, None), ("b".into(), None, false, None)], true)
        .contains(&("a".into(), Notice::Stalled)));
    m.save(&path).unwrap();
    let mut back = Memory::load(&path);
    assert_eq!(back.started.get("sid"), Some(&100.0), "o início da série sobrevive: o 'terminou' sai depois do reinício");
    // Ainda travada depois do reinício: não repete; `b` sumiu enquanto o servidor estava fora: avisa.
    let again = back.sweep(&[("a".into(), Some("sid".into()), true, None)], true);
    assert_eq!(again, vec![("b".to_owned(), Notice::Dead)]);
}

#[test]
fn store_upserts_by_endpoint_and_validates_quiet_hours() {
    let dir = tempfile::tempdir().unwrap();
    store::add(dir.path(), json!({"endpoint": "e1", "keys": {}}), "Casa", "s1", "pt").unwrap();
    store::add(dir.path(), json!({"endpoint": "e1", "keys": {}}), "Casa2", "s1", "en").unwrap();
    assert_eq!(store::subs(dir.path()).len(), 1);
    assert_eq!(store::subs(dir.path())[0]["label"], "Casa2");
    assert_eq!(store::add(dir.path(), json!({}), "x", "s", "pt"), Err("subscription sem endpoint"));
    store::prune(dir.path(), &["e1".to_owned()]).unwrap();
    assert!(store::subs(dir.path()).is_empty());
    assert_eq!(store::set_quiet_hours(dir.path(), Some("25:00"), Some("07:00")), Err("horario invalido (use HH:MM)"));
    store::set_quiet_hours(dir.path(), Some("22:00"), Some("07:00")).unwrap();
    store::set_muted(dir.path(), "s", true).unwrap();
    assert_eq!(store::settings(dir.path()), json!({"muted": ["s"], "quiet_hours": {"start": "22:00", "end": "07:00"}}));
    store::set_quiet_hours(dir.path(), None, Some("07:00")).unwrap();
    store::set_muted(dir.path(), "s", false).unwrap();
    assert_eq!(store::settings(dir.path()), json!({"muted": [], "quiet_hours": null}));
}

proptest::proptest! {
    #[test]
    fn summary_and_reset_never_panic(text in ".{0,300}", reset in ".{0,20}") {
        let _ = summary(Some(&text));
        let _ = reset_delay(&reset, chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap());
    }
}
