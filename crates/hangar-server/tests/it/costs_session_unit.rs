use hangar_server::costs::{pricing::Pricing, py::LocalTs, rows::UsageRow};
use serde_json::json;
use hangar_server::costs::session_cost::estimate;
use hangar_server::costs::{areas::AreaMap, codex, index::{Index, IndexError, Progress, FILE_NAME}};
use std::io::Write;
use std::path::Path;

fn rollout_record(kind: &str, second: u32, payload: serde_json::Value) -> String {
    format!("{}\n", json!({"type": kind, "timestamp": format!("2026-10-01T12:00:{second:02}Z"), "payload": payload}))
}

fn rollout(path: &Path) {
    std::fs::write(path, [
        rollout_record("session_meta", 0, json!({"id": "synthetic", "model_provider": "openai-codex"})),
        rollout_record("turn_context", 1, json!({"turn_id": "first", "model": "gpt-6-sol"})),
        rollout_record("token_usage_record", 2, json!({"thread_id": "synthetic", "response_id": "a", "usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 5}})),
        rollout_record("turn_context", 3, json!({"turn_id": "second", "model": "tiny"})),
        rollout_record("token_usage_record", 4, json!({"thread_id": "synthetic", "response_id": "b", "usage": {"input_tokens": 200, "output_tokens": 6}})),
    ].concat()).unwrap();
}

fn row(model: &str) -> UsageRow {
    UsageRow {
        ts: LocalTs::from_iso("2026-10-03T12:00:00-03:00").unwrap(),
        source: "codex".into(), provider: "codex:conta".into(), model: model.into(),
        project: "projeto".into(), session_id: "sessão".into(), input: 1_000_000,
        output: 0, cache_write: 0, cache_read: 0, subagente: false, account_id: None,
        codex_long_context: false, cache_write_1h: 0, fast: false, regravado: 0,
        regravado_1h: 0,
    }
}

fn pricing() -> (tempfile::TempDir, Pricing) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"), json!({"modelos": {
        "gpt-6-sol": {"input": 2, "output": 8, "cache_write": 0, "cache_read": 0.2, "provider": "openai"},
        "claude-opus-5": {"input": 5, "output": 25, "cache_write": 6.25, "cache_read": 0.5, "provider": "anthropic"},
        "wide": {"input": 1e16, "output": 1, "cache_write": 1, "cache_read": 1, "provider": "openai"},
        "tiny": {"input": 1, "output": 1, "cache_write": 1, "cache_read": 1, "provider": "openai"},
        "free": {"input": 0, "output": 0, "cache_write": 0, "cache_read": 0, "provider": "openai"}
    }}).to_string()).unwrap();
    let pricing = Pricing::load(dir.path());
    (dir, pricing)
}

#[test]
fn empty_usage_serializes_null_and_python_field_order() {
    let (_dir, pricing) = pricing();
    assert_eq!(serde_json::to_string(&estimate(vec![], &pricing)).unwrap(),
        r#"{"cost_usd":null,"missing_models":[],"has_usage":false}"#);
}

#[test]
fn rows_without_primary_tokens_do_not_count_as_usage_or_missing_rates() {
    let (_dir, pricing) = pricing();
    let mut zero = row("missing");
    zero.input = 0;
    zero.cache_write_1h = 900;
    zero.regravado = 800;
    zero.regravado_1h = 700;
    assert_eq!(serde_json::to_value(estimate(vec![zero], &pricing)).unwrap(),
        json!({"cost_usd": null, "missing_models": [], "has_usage": false}));
}

#[test]
fn session_cost_uses_each_primary_token_kind() {
    let (_dir, pricing) = pricing();
    let cases = [("input", 2.0), ("output", 8.0), ("cache_write", 0.0), ("cache_read", 0.2)];
    for (kind, expected) in cases {
        let mut usage = row("gpt-6-sol");
        usage.input = 0;
        match kind {
            "input" => usage.input = 1_000_000,
            "output" => usage.output = 1_000_000,
            "cache_write" => usage.cache_write = 1_000_000,
            "cache_read" => usage.cache_read = 1_000_000,
            _ => unreachable!(),
        }
        let result = estimate(vec![usage], &pricing);
        assert_eq!(result.cost_usd, Some(expected), "{kind}");
        assert!(result.has_usage, "{kind}");
        assert!(result.missing_models.is_empty(), "{kind}");
    }
}

#[test]
fn long_context_has_its_own_group_and_adjusted_tariff() {
    let (_dir, pricing) = pricing();
    let mut normal = row("gpt-6-sol");
    normal.output = 1_000_000;
    normal.cache_read = 1_000_000;
    let mut long = normal.clone();
    long.codex_long_context = true;
    let result = estimate(vec![normal, long], &pricing);
    assert_eq!(result.cost_usd, Some(26.599999999999998));
    assert!(result.has_usage);
    assert!(result.missing_models.is_empty());
}

#[test]
fn same_model_across_days_is_grouped_before_float_conversion() {
    let (_dir, pricing) = pricing();
    let wide = row("wide");
    let mut first = row("tiny");
    first.ts = LocalTs::from_iso("2026-10-01T12:00:00-03:00").unwrap();
    let mut second = first.clone();
    second.ts = LocalTs::from_iso("2026-10-02T12:00:00-03:00").unwrap();
    let result = estimate(vec![wide, first, second, row("tiny")], &pricing);
    assert_eq!(result.cost_usd, Some(10_000_000_000_000_004.0));
}

#[test]
fn raw_model_groups_and_first_insertion_order_survive_canonicalization() {
    let (_dir, pricing) = pricing();
    let result = estimate(vec![row("wide"), row("tiny"), row("openai/tiny"), row("apikey/tiny")], &pricing);
    assert_eq!(result.cost_usd, Some(10_000_000_000_000_000.0));
    let result = estimate(vec![row("tiny"), row("openai/tiny"), row("apikey/tiny"), row("wide")], &pricing);
    assert_eq!(result.cost_usd, Some(10_000_000_000_000_004.0));
}

#[test]
fn each_group_uses_cpython_compensated_sum() {
    let (_dir, pricing) = pricing();
    let mut wide = row("wide");
    wide.output = 1_000_000;
    wide.cache_write = 1_000_000;
    wide.cache_read = 1_000_000;
    assert_eq!(estimate(vec![wide], &pricing).cost_usd, Some(10_000_000_000_000_004.0));
}

#[test]
fn regrouping_keeps_non_primary_fields_from_the_first_row() {
    let (_dir, pricing) = pricing();
    let mut first = row("claude-opus-5");
    first.input = 0;
    first.cache_write = 1_000_000;
    first.cache_write_1h = 100_000;
    let mut second = first.clone();
    second.fast = true;
    second.cache_write_1h = 900_000;
    assert_eq!(estimate(vec![first, second], &pricing).cost_usd, Some(12.875));
}

#[test]
fn missing_tariffs_return_null_with_sorted_deduplicated_canonical_models() {
    let (_dir, pricing) = pricing();
    let rows = vec![row("openai/z"), row("cx/a"), row("z"), row("gpt-6-sol"),
        row("unknown"), row("<synthetic>"), row("")];
    assert_eq!(serde_json::to_value(estimate(rows, &pricing)).unwrap(),
        json!({"cost_usd": null, "missing_models": ["", "<synthetic>", "a", "unknown", "z"], "has_usage": true}));
}

#[test]
fn free_tariff_is_usage_with_a_numeric_zero_cost() {
    let (_dir, pricing) = pricing();
    assert_eq!(serde_json::to_value(estimate(vec![row("free")], &pricing)).unwrap(),
        json!({"cost_usd": 0.0, "missing_models": [], "has_usage": true}));
}

#[test]
fn nonzero_counters_are_usage_even_when_their_sum_is_zero() {
    let (_dir, pricing) = pricing();
    let mut usage = row("tiny");
    usage.input = -1_000_000;
    usage.output = 1_000_000;
    let result = estimate(vec![usage], &pricing);
    assert_eq!(result.cost_usd, Some(0.0));
    assert!(result.has_usage);
}

#[test]
fn long_context_respects_local_override_without_scaling() {
    let (dir, mut pricing) = pricing();
    std::fs::write(dir.path().join("overrides.json"), json!({"gpt-6-sol": {
        "input": 1, "output": 2, "cache_write": 3, "cache_read": 4, "provider": "openai"
    }}).to_string()).unwrap();
    assert!(pricing.reload_if_changed());
    let mut usage = row("gpt-6-sol");
    usage.codex_long_context = true;
    usage.output = 1_000_000;
    usage.cache_write = 1_000_000;
    usage.cache_read = 1_000_000;
    assert_eq!(estimate(vec![usage], &pricing).cost_usd, Some(10.0));
}

#[test]
fn checked_single_rollout_keeps_row_order_generation_and_existing_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-synthetic.jsonl");
    rollout(&path);
    let areas = AreaMap::load(&dir.path().join("missing-areas.json"));
    let index = Index::open(&dir.path().join("index")).unwrap();
    let before = index.generation();
    let rows = codex::try_session_rows(&index, &path, &areas).unwrap();
    assert_eq!(rows.iter().map(|r| (r.model.as_str(), r.input, r.output, r.cache_read)).collect::<Vec<_>>(),
        [("gpt-6-sol", 80, 5, 20), ("tiny", 200, 6, 0)]);
    assert!(index.generation() > before);
    let unchanged = index.generation();
    assert_eq!(codex::session_rows(&index, &path, &areas).unwrap(), rows);
    assert_eq!(codex::try_session_rows(&index, &path, &areas).unwrap(), rows);
    assert_eq!(index.generation(), unchanged);
    // A varredura grava pelo caminho canônico (`rollout_owners`), como o custo avulso.
    index.sync("codex:synthetic", &[std::fs::canonicalize(&path).unwrap()], &codex::new_fold, codex::VERSION,
        areas.signature(), &|entries| areas.area_lines(entries), &Progress::default()).unwrap();
    assert!(index.read_costs(Some("codex:avulso"), None, None).unwrap().is_empty());
    let before_growth = index.generation();
    let tail = rollout_record("token_usage_record", 5, json!({"thread_id": "synthetic", "response_id": "c", "usage": {"input_tokens": 30, "output_tokens": 7}}));
    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(tail.as_bytes()).unwrap();
    let grown = codex::try_session_rows(&index, &path, &areas).unwrap();
    assert_eq!(grown.iter().map(|r| (r.model.as_str(), r.input, r.output, r.cache_read)).collect::<Vec<_>>(),
        [("gpt-6-sol", 80, 5, 20), ("tiny", 230, 13, 0)]);
    assert!(index.generation() > before_growth);
    assert_eq!(index.read_costs(Some("codex:synthetic"), None, None).unwrap(), grown);
    assert!(index.read_costs(Some("codex:avulso"), None, None).unwrap().is_empty());
}

#[test]
fn checked_single_rollout_returns_disk_error_when_index_directory_disappears() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-synthetic.jsonl");
    rollout(&path);
    let areas = AreaMap::load(&dir.path().join("missing-areas.json"));
    let index_dir = dir.path().join("index");
    let index = Index::open(&index_dir).unwrap();
    std::fs::remove_dir_all(&index_dir).unwrap();
    std::fs::write(&index_dir, b"bloqueio").unwrap();
    assert!(matches!(codex::try_session_rows(&index, &path, &areas), Err(IndexError::NoDisk)));
    assert!(codex::session_rows(&index, &path, &areas).is_none());
}

#[test]
fn checked_single_rollout_returns_error_when_rollout_disappears_after_indexing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-synthetic.jsonl");
    rollout(&path);
    let areas = AreaMap::load(&dir.path().join("missing-areas.json"));
    let index = Index::open(&dir.path().join("index")).unwrap();
    assert!(!codex::try_session_rows(&index, &path, &areas).unwrap().is_empty());
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(codex::try_session_rows(&index, &path, &areas), Err(IndexError::NoDisk)));
    assert!(codex::session_rows(&index, &path, &areas).is_none());
}

#[test]
fn checked_single_rollout_preserves_reader_panic_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-malformed.jsonl");
    std::fs::write(&path, rollout_record("session_meta", 0, json!({"id": true}))).unwrap();
    let areas = AreaMap::load(&dir.path().join("missing-areas.json"));
    let index = Index::open(&dir.path().join("index")).unwrap();
    let result = codex::try_session_rows(&index, &path, &areas);
    assert!(matches!(result, Err(IndexError::ReaderPanic)), "{result:?}");
    assert!(codex::session_rows(&index, &path, &areas).is_none());
}

#[test]
fn checked_single_rollout_preserves_index_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-synthetic.jsonl");
    rollout(&path);
    let areas = AreaMap::load(&dir.path().join("missing-areas.json"));
    let index_dir = dir.path().join("index");
    let index = Index::open(&index_dir).unwrap();
    assert!(!codex::try_session_rows(&index, &path, &areas).unwrap().is_empty());
    let conn = rusqlite::Connection::open(index_dir.join(FILE_NAME)).unwrap();
    conn.execute("UPDATE custo SET ts='invalid'", []).unwrap();
    drop(conn);
    let result = codex::try_session_rows(&index, &path, &areas);
    assert!(matches!(result, Err(IndexError::Sqlite(rusqlite::Error::InvalidQuery))), "{result:?}");
    assert!(codex::session_rows(&index, &path, &areas).is_none());
}
