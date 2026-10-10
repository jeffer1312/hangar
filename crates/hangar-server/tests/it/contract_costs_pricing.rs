use hangar_server::costs::pricing::{Pricing, canonizar_provedor, custo};
use hangar_server::costs::py::{LocalTs, char_len, parse_obj, py_int};
use serde_json::{Value, json};
use std::path::Path;

fn contract() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract")
}

fn rate_json(r: Option<hangar_server::costs::pricing::Rate>) -> Value {
    r.map_or(Value::Null, |r| json!({"input": r.input, "output": r.output, "cache_read": r.cache_read,
        "cache_write": r.cache_write, "provider": r.provider, "origin": r.origin, "cache_estimado": r.cache_estimado}))
}

#[test]
fn pricing_matches_python() {
    let p = Pricing::load(&contract().join("costs/pricing"));
    let rows: Value = serde_json::from_slice(&std::fs::read(contract().join("golden/costs_pricing.json")).unwrap()).unwrap();
    for row in rows.as_array().unwrap() {
        let m = row["model"].as_str().unwrap();
        assert_eq!(p.canonizar(m), row["canon"].as_str().unwrap(), "canon {m}");
        let r = p.rate_for(m);
        assert_eq!(rate_json(r.clone()), row["rate"], "rate {m}");
        assert_eq!(rate_json(r.as_ref().map(|r| p.rate_fast(r, m))), row["fast"], "fast {m}");
        assert_eq!(rate_json(r.as_ref().map(|r| p.rate_codex(r, m, true))), row["codex_long"], "codex {m}");
    }
}

#[test]
fn snapshot_is_used_without_cache_dir() {
    let dir = tempfile::tempdir().unwrap();
    let p = Pricing::load(&dir.path().join("missing"));
    assert_eq!(p.rate_for("claude-opus-5").unwrap().origin, "snapshot");
}

#[test]
fn py_int_follows_python_int_or_zero() {
    for (v, want) in [(json!(null), 0), (json!(false), 0), (json!(true), 1), (json!(7), 7), (json!(7.9), 7),
                      (json!(-2.5), -2), (json!(" 12 "), 12), (json!("-3"), -3), (json!("3.5"), 0), (json!(""), 0),
                      (json!([1]), 0), (json!({"a": 1}), 0), (json!(u64::MAX), i64::MAX),
                      (json!("9999999999999999999999999"), i64::MAX),
                      (json!("-9999999999999999999999999"), i64::MIN),
                      (json!("1_000"), 1000), (json!("１２"), 12), (json!("-١٢"), -12),
                      (json!("1__0"), 0), (json!("1_"), 0), (json!("_1"), 0),
                      (json!("9999999999999999999999999x"), 0)] {
        assert_eq!(py_int(Some(&v)), want, "{v}");
    }
    assert_eq!(py_int(None), 0);
}

#[test]
fn local_time_matches_python_isoformat() {
    let t = LocalTs::from_iso("2026-09-30T02:30:00.120Z").unwrap();
    assert_eq!(t.iso(), "2026-09-29T23:30:00.120000-03:00");
    assert_eq!(t.day(), "2026-09-29");
    let encoded = serde_json::to_value(t).unwrap();
    assert_eq!(serde_json::from_value::<LocalTs>(encoded).unwrap(), t);
    assert_eq!(LocalTs::from_iso("2026-09-30T10:00:00Z").unwrap().iso(), "2026-09-30T07:00:00-03:00");
    assert_eq!(LocalTs::from_millis_f64(1_790_935_200_123.4).iso(), "2026-10-02T07:00:00.123400-03:00");
    assert!(LocalTs::from_iso("ontem").is_none());
    for raw in ["2026-09-30T10:00:00.000001", "20260930 100000.000001"] {
        assert_eq!(LocalTs::from_iso(raw).unwrap().iso(), "2026-09-30T10:00:00.000001-03:00");
    }
    assert_eq!(LocalTs::from_iso("2026-09-30").unwrap().iso(), "2026-09-30T00:00:00-03:00");
    assert_eq!(LocalTs::from_iso("2026-09-30T10:00:00+01:30:00.000001").unwrap().iso(), "2026-09-30T05:29:59.999999-03:00");
    for (ms, micros) in [(0.0005, 0), (0.0015, 2), (-0.0005, 0), (-0.0015, -2),
                         (-9.9935, -9993), (-9.9905, -9991)] {
        assert_eq!(LocalTs::from_millis_f64(ms).0, micros);
    }
}

#[test]
fn rate_text_numbers_accept_python_float_syntax() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"),
        r#"{"modelos":{"numeric":{"input":" 1_0.5 ","output":"２","cache_read":"٠.١","cache_write":false}}}"#).unwrap();
    let p = Pricing::load(dir.path());
    let rate = p.rate_for("numeric").unwrap();
    assert_eq!(rate.input, 10.5);
    assert_eq!(rate.output, 2.0);
    assert_eq!(rate.cache_read, 0.1);
    assert_eq!(rate.cache_write, 0.0);
    assert!(!rate.cache_estimado);
}

#[test]
fn local_time_accepts_python_iso_week_dates() {
    for (raw, expected) in [
        ("2026-W40-5", "2026-10-02T00:00:00-03:00"),
        ("2026W405", "2026-10-02T00:00:00-03:00"),
        ("2026-W40", "2026-09-28T00:00:00-03:00"),
        ("2026W40", "2026-09-28T00:00:00-03:00"),
        ("2026-W40-5T10:00:00+00:00", "2026-10-02T07:00:00-03:00"),
        ("2026W405T10:00:00Z", "2026-10-02T07:00:00-03:00"),
        ("2026W405T10:00:00", "2026-10-02T10:00:00-03:00"),
        ("2026-W40T10:00:00.000001", "2026-09-28T10:00:00.000001-03:00"),
        ("2026W40T10:00:00.000001-03:00", "2026-09-28T10:00:00.000001-03:00"),
        ("2020-W53-7", "2021-01-03T00:00:00-03:00"),
        ("2020W537", "2021-01-03T00:00:00-03:00"),
        ("0001-W01-1", "0001-01-01T00:00:00-03:00"),
    ] {
        assert_eq!(LocalTs::from_iso(raw).map(|t| t.iso()).as_deref(), Some(expected), "{raw}");
    }
    for raw in ["2021-W53-1", "2026-W00-1", "2026-W54-1", "2026-W40-0", "2026-W40-8",
                "2026W400", "2026W408", "2026-W401", "2026W40-1", "9999-W52-7"] {
        assert!(LocalTs::from_iso(raw).is_none(), "{raw}");
    }
}

#[test]
fn number_whitespace_matches_python_conversions() {
    let dir = tempfile::tempdir().unwrap();
    let mut models = serde_json::Map::new();
    for code in 0x1c..=0x1f {
        let control = char::from_u32(code).unwrap();
        let raw = format!("{control}12{control}");
        assert_eq!(py_int(Some(&json!(raw))), 0, "controle {code:#x}");
        models.insert(format!("invalid-{code}"), json!({"input": raw, "output": 2}));
    }
    for (index, raw) in ["\t12\r\n", "\u{a0}12\u{a0}", "\u{2003}１２\u{2003}", "\u{202f}1_2\u{202f}"].into_iter().enumerate() {
        assert_eq!(py_int(Some(&json!(raw))), 12, "{raw:?}");
        models.insert(format!("valid-{index}"), json!({"input": raw, "output": 2}));
    }
    std::fs::write(dir.path().join("models.dev.json"), serde_json::to_vec(&json!({"modelos": models})).unwrap()).unwrap();
    let p = Pricing::load(dir.path());
    for code in 0x1c..=0x1f {
        assert!(p.rate_for(&format!("invalid-{code}")).is_none(), "controle float {code:#x}");
    }
    for index in 0..4 {
        assert_eq!(p.rate_for(&format!("valid-{index}")).unwrap().input, 12.0);
    }
}

#[test]
fn iso_week_separators_follow_python_disambiguation() {
    for (raw, expected) in [
        ("2026W40810:00", "2026-09-28T10:00:00-03:00"),
        ("2026W40810", "2026-09-28T10:00:00-03:00"),
        ("2026W40810:00Z", "2026-09-28T07:00:00-03:00"),
        ("2026W405810:00", "2026-10-02T10:00:00-03:00"),
        ("2026W4010000", "2026-09-28T00:00:00-03:00"),
        ("2026W405100000", "2026-09-28T10:00:00-03:00"),
        ("2026-W40-1000", "2026-09-28T10:00:00-03:00"),
        ("2026-W40-10:00", "2026-09-28T10:00:00-03:00"),
        ("2026W40水10:00", "2026-09-28T10:00:00-03:00"),
        ("2026W405水10:00", "2026-10-02T10:00:00-03:00"),
        ("2026-W40水10:00", "2026-09-28T10:00:00-03:00"),
        ("2026-W40-5水10:00", "2026-10-02T10:00:00-03:00"),
    ] {
        assert_eq!(LocalTs::from_iso(raw).map(|t| t.iso()).as_deref(), Some(expected), "{raw}");
    }
    for raw in ["2026W408", "2026W4010", "2026-W40-", "2026-W40-510:00"] {
        assert!(LocalTs::from_iso(raw).is_none(), "{raw}");
    }
}

#[test]
fn line_objects_preserve_python_character_counts() {
    let obj = parse_obj(b" \t{\"text\":\"a\\ud800\\ud83d\\ude00\"}\r\n").unwrap();
    assert_eq!(char_len(obj["text"].as_str().unwrap()), 3);
    assert_eq!(char_len("á水𐀀"), 3);
    assert_eq!(parse_obj(b"{\"text\":\"\xff\"}").unwrap()["text"], "�");
    assert_eq!(parse_obj(b"\x0b{\"x\":1}\x0b").unwrap()["x"], 1);
    assert!(parse_obj("\u{a0}{\"x\":1}\u{a0}".as_bytes()).is_none());
    for raw in [b"".as_slice(), b"[]", b"null", b"{\"x\":"] {
        assert!(parse_obj(raw).is_none());
    }
}

#[test]
fn pricing_reload_invalidates_both_memos_and_keeps_alias_order() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = dir.path().join("models.dev.json");
    let overrides = dir.path().join("overrides.json");
    std::fs::write(&catalog, r#"{"modelos":{"MiXeD":{"input":"1","output":"2","provider":"openai"},"mixed":{"input":9,"output":9}}}"#).unwrap();
    let mut p = Pricing::load(dir.path());
    assert_eq!(p.canonizar("MIXED"), "MiXeD");
    let initial = p.rate_for("MIXED").unwrap();
    assert_eq!(initial.cache_write, 0.0);
    assert!(initial.cache_estimado);
    assert_eq!(p.provider_for("MIXED").as_deref(), Some("openai"));
    assert_eq!(custo(&initial, 1_000_000, 2_000_000, 3_000_000, 4_000_000), [1.0, 4.0, 0.0, 4.0]);
    let generation = p.generation();
    assert!(!p.reload_if_changed());
    std::fs::write(&overrides, r#"{"MiXeD":{"input":"4","output":"8","provider":"custom"}}"#).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(p.generation(), generation + 1);
    assert_eq!(p.rate_for("MIXED").unwrap().input, 4.0);
    assert_eq!(p.rate_for("MIXED").unwrap().origin, "override");
    std::fs::write(&catalog, r#"{"modelos":{"mixed":{"input":3,"output":6,"provider":"openai","cache_read":0.3}}}"#).unwrap();
    // No Windows mudar a data exige o arquivo aberto para escrita; só leitura dá "Access is denied".
    std::fs::OpenOptions::new().write(true).open(&catalog).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(p.canonizar("MIXED"), "mixed");
    assert_eq!(p.rate_for("MIXED").unwrap().input, 3.0);
    std::fs::remove_file(&catalog).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(p.rate_for("claude-opus-5").unwrap().origin, "snapshot");
    assert_eq!(canonizar_provedor(" openai-codex "), "openai");
    assert_eq!(canonizar_provedor("moonshot"), "moonshotai");
    assert_eq!(canonizar_provedor("anthropic:abc"), "anthropic:abc");
}
