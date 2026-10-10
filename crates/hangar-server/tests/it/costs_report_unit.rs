use hangar_server::costs::{self, report_costs};

use costs::pricing::Pricing;
use costs::py::LocalTs;
use costs::rows::UsageRow;
use report_costs::{build, row_cost, since};
use serde_json::json;

fn ts(value: &str) -> LocalTs {
    LocalTs::from_iso(value).unwrap()
}

fn row(day: &str, model: &str, session: &str) -> UsageRow {
    UsageRow {
        ts: ts(&format!("{day}T12:00:00-03:00")), source: "claude".into(),
        provider: "anthropic:conta".into(), model: model.into(), project: "projeto".into(),
        session_id: session.into(), input: 1_000_000, output: 0, cache_write: 0,
        cache_read: 0, subagente: false, account_id: None, codex_long_context: false,
        cache_write_1h: 0, fast: false, regravado: 0, regravado_1h: 0,
    }
}

fn pricing() -> (tempfile::TempDir, Pricing) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"), json!({"modelos": {
        "alpha": {"input": 2, "output": 8, "cache_write": 2.5, "cache_read": 0.2, "provider": "anthropic"},
        "beta": {"input": 2, "output": 4, "cache_write": 2, "cache_read": 1, "provider": "openai"},
        "free-input": {"input": 0, "output": 3, "cache_write": 0, "cache_read": 0, "provider": "openai"},
        "claude-opus-5": {"input": 5, "output": 25, "cache_write": 6.25, "cache_read": 0.5, "provider": "anthropic"},
        "gpt-6-sol": {"input": 2, "output": 8, "cache_write": 0, "cache_read": 0.2, "provider": "openai"}
    }}).to_string()).unwrap();
    let p = Pricing::load(dir.path());
    (dir, p)
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn empty_report_keeps_python_fields_order_nulls_and_kinds() {
    let (_dir, p) = pricing();
    let r = build(vec![], "all", ts("2026-10-03"), &p, &|_| None);
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["totals", "by_day", "by_provider", "by_source", "by_project", "by_model", "by_kind", "rates", "sem_tarifa", "custo_sem_cache", "equivalente_cobrado", "anterior", "applied", "usd_brl", "combos", "sessoes"]);
    assert_eq!(value["totals"].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["key", "label", "sessions", "input", "output", "cache_write", "cache_read", "cost", "cost_input", "cost_output", "cost_cache_write", "cost_cache_read", "cache_write_1h", "regravado", "custo_regravado"]);
    assert!(value["anterior"].is_null());
    assert!(value["usd_brl"].is_null());
    assert!(value["totals"]["label"].is_null());
    assert_eq!(value["applied"], json!({"period": "all"}));
    assert_eq!(value["by_kind"], json!([
        {"kind":"input", "tokens":0, "cost":0.0}, {"kind":"output", "tokens":0, "cost":0.0},
        {"kind":"cache_write", "tokens":0, "cost":0.0}, {"kind":"cache_read", "tokens":0, "cost":0.0}
    ]));
}

#[test]
fn windows_are_local_inclusive_and_previous_needs_one_third_coverage() {
    let (_dir, p) = pricing();
    let now = ts("2026-10-03T01:00:00Z");
    assert_eq!(since("7d", now).as_deref(), Some("2026-09-19"));
    assert_eq!(since("1d", now).as_deref(), Some("2026-10-01"));
    assert_eq!(since("all", now), None);
    assert_eq!(since("nada", now), None);
    let mut rows = vec![row("2026-09-19", "alpha", "a"), row("2026-09-20", "beta", "b")];
    rows.push(row("2026-09-26", "beta", "c"));
    rows.push(row("2026-10-02", "alpha", "d"));
    let r = build(rows.clone(), "7d", now, &p, &|_| None);
    assert_eq!(r.totals.sessions, 2);
    close(r.totals.cost, 4.0);
    assert!(r.anterior.is_none());
    rows.insert(0, row("2026-09-25", "alpha", "e"));
    let r = build(rows, "7d", now, &p, &|_| None);
    let previous = r.anterior.unwrap();
    assert_eq!(previous.key, "anterior");
    assert_eq!(previous.sessions, 3);
    close(previous.cost, 6.0);
    assert_eq!(r.by_day.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["2026-10-02", "2026-09-26"]);
}

#[test]
fn costs_equivalent_cache_and_rewrites_share_the_adjusted_rate() {
    let (_dir, p) = pricing();
    let mut alpha = row("2026-10-03", "alpha", "a");
    alpha.output = 500_000;
    alpha.cache_write = 200_000;
    alpha.cache_read = 300_000;
    alpha.cache_write_1h = 100_000;
    alpha.regravado = 120_000;
    alpha.regravado_1h = 50_000;
    let r = build(vec![alpha], "all", ts("2026-10-03"), &p, &|_| Some("Conta de teste".into()));
    close(r.totals.cost, 6.71);
    close(r.totals.cost_cache_write, 0.65);
    close(r.totals.custo_regravado, 0.351);
    close(r.custo_sem_cache, 7.0);
    assert_eq!(r.equivalente_cobrado, 3_355_000);
    close(r.combos[0].equivalente_cobrado, 3_355_000.0);
    close(r.combos[0].custo_sem_cache, 7.0);
    assert_eq!(r.by_provider[0].label.as_deref(), Some("Conta de teste"));
    assert_eq!(r.by_model[0].label, None);
    assert_eq!(r.totals.regravado, 120_000);
    assert_eq!(r.totals.cache_write_1h, 100_000);
    assert_eq!(r.rates[0].input, 2.0);
}

#[test]
fn fast_long_context_and_overrides_keep_base_rate_metadata() {
    let (dir, mut p) = pricing();
    let mut fast = row("2026-10-03", "claude-opus-5", "fast");
    fast.fast = true;
    fast.output = 1_000_000;
    fast.cache_write = 1_000_000;
    fast.cache_write_1h = 1_000_000;
    assert_eq!(row_cost(&fast, &p).unwrap(), [10.0, 50.0, 20.0, 0.0]);
    let mut long = row("2026-10-03", "gpt-6-sol", "long");
    long.source = "codex".into();
    long.codex_long_context = true;
    long.output = 1_000_000;
    long.cache_read = 1_000_000;
    assert_eq!(row_cost(&long, &p).unwrap(), [4.0, 12.0, 0.0, 0.4]);
    let r = build(vec![fast.clone(), long], "all", ts("2026-10-03"), &p, &|_| None);
    close(r.custo_sem_cache, 90.0);
    assert_eq!(r.rates.iter().map(|r| r.input).collect::<Vec<_>>(), [5.0, 2.0]);
    std::fs::write(dir.path().join("overrides.json"), json!({"claude-opus-5": {
        "input": 1, "output": 2, "cache_write": 3, "cache_read": 4, "provider": "anthropic"
    }}).to_string()).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(row_cost(&fast, &p).unwrap(), [1.0, 2.0, 3.0, 0.0]);
}

#[test]
fn identities_combos_and_dimension_order_preserve_python_contract() {
    let (_dir, p) = pricing();
    let mut a = row("2026-10-03", "beta", "sessão");
    a.provider = "z".into();
    a.project = "z".into();
    a.account_id = Some("conta-á".into());
    let mut duplicate = a.clone();
    duplicate.model = "alpha".into();
    let mut b = row("2026-10-02", "beta", "sessão");
    b.provider = "a".into();
    b.project = "a".into();
    b.input = 2_000_000;
    let r = build(vec![a.clone(), duplicate, b], "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.totals.sessions, 2);
    assert_eq!(r.by_provider.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["a", "z"]);
    assert_eq!(r.by_project.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["a", "z"]);
    assert_eq!(r.by_model.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["beta", "alpha"]);
    assert_eq!(r.combos.iter().map(|b| b.dia.as_str()).collect::<Vec<_>>(), ["2026-10-02", "2026-10-03", "2026-10-03"]);
    assert_eq!(r.combos[1].session_ids, [r#"["claude", "conta-\u00e1", "sess\u00e3o", false]"#]);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["combos"][0].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["dia", "provider", "source", "project", "model", "subagente", "sessions", "session_ids", "custo_sem_cache", "equivalente_cobrado", "input", "output", "cache_write", "cache_read", "cost", "cost_input", "cost_output", "cost_cache_write", "cost_cache_read", "cache_write_1h", "regravado", "custo_regravado"]);
    assert_eq!(v["rates"][0].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["model", "provider", "input", "output", "cache_read", "cache_write", "origin", "cache_estimado"]);
}

#[test]
fn subagents_join_parent_top_sessions_and_model_ties_choose_first() {
    let (_dir, p) = pricing();
    let mut child = row("2026-10-02", "beta", "parent/subagents/agent-1");
    child.subagente = true;
    child.project = "filho".into();
    let mut parent = row("2026-10-03", "alpha", "parent");
    parent.project = "pai".into();
    let r = build(vec![child.clone(), parent], "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.totals.sessions, 2);
    assert_eq!(r.sessoes.len(), 1);
    let s = &r.sessoes[0];
    assert_eq!((s.session_id.as_str(), s.model.as_str(), s.project.as_str()), ("parent", "beta", "pai"));
    assert_eq!((s.inicio.as_str(), s.fim.as_str(), s.subagentes), ("2026-10-02", "2026-10-03", 1));
    let rows = (0..105).map(|i| row("2026-10-03", "alpha", &format!("s{i}"))).collect();
    let r = build(rows, "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.sessoes.len(), 100);
    assert_eq!(r.sessoes[0].session_id, "s0");
    assert_eq!(r.sessoes[99].session_id, "s99");
    let v = serde_json::to_value(&r.sessoes[0]).unwrap();
    assert_eq!(v.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["session_id", "source", "provider", "project", "model", "inicio", "fim", "subagentes", "input", "output", "cache_write", "cache_read", "cost", "custo_regravado"]);
}

#[test]
fn missing_rates_keep_tokens_ignore_synthetic_names_and_zero_input_has_no_equivalent() {
    let (_dir, p) = pricing();
    let mut free = row("2026-10-03", "free-input", "free");
    free.output = 1;
    let rows = vec![row("2026-10-03", "openai/no-rate", "x"), row("2026-10-03", "no-rate", "y"),
        row("2026-10-03", "unknown", "z"), free];
    let r = build(rows, "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.sem_tarifa, ["no-rate"]);
    assert_eq!(r.totals.input, 4_000_000);
    assert_eq!(r.totals.sessions, 4);
    assert_eq!(r.equivalente_cobrado, 0);
    close(r.totals.cost, 0.000003);
    assert_eq!(r.rates.len(), 1);
    let mut fractional = row("2026-10-03", "beta", "fractional");
    fractional.input = 0;
    fractional.cache_read = 1;
    let r = build(vec![fractional], "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.equivalente_cobrado, 0);
    close(r.combos[0].equivalente_cobrado, 0.5);
}

#[test]
fn each_row_uses_python_compensated_sum_but_rows_keep_insertion_order() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"), json!({"modelos": {
        "wide": {"input": 1e16, "output": 1, "cache_write": 1, "cache_read": 1, "provider": "openai"},
        "tiny": {"input": 1, "output": 1, "cache_write": 1, "cache_read": 1, "provider": "openai"}
    }}).to_string()).unwrap();
    let p = Pricing::load(dir.path());
    let mut wide = row("2026-10-03", "wide", "wide");
    wide.output = 1_000_000;
    wide.cache_write = 1_000_000;
    wide.cache_read = 1_000_000;
    let r = build(vec![wide], "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.totals.cost, 10_000_000_000_000_004.0);
    assert_eq!(r.combos[0].cost, 10_000_000_000_000_004.0);
    assert_eq!(r.sessoes[0].cost, 10_000_000_000_000_004.0);
    let rows = vec![row("2026-10-03", "wide", "a"), row("2026-10-03", "tiny", "b"),
        row("2026-10-03", "tiny", "c"), row("2026-10-03", "tiny", "d")];
    let r = build(rows, "all", ts("2026-10-03"), &p, &|_| None);
    assert_eq!(r.totals.cost, 10_000_000_000_000_000.0);
}

#[test]
fn summary_is_the_same_fields_of_the_full_report() {
    let (_dir, p) = pricing();
    let now = ts("2026-10-03");
    let rows = vec![row("2026-10-03", "alpha", "a"), row("2026-10-02", "beta", "b"), row("2026-09-01", "alpha", "c"),
        row("2026-10-01", "sem-preco", "d"), row("2026-10-03", "beta", "a")];
    for period in ["all", "7d", "30d"] {
        let full = serde_json::to_value(build(rows.clone(), period, now, &p, &|_| None)).unwrap();
        let summary = serde_json::to_value(report_costs::build_summary(rows.clone(), period, now, &p)).unwrap();
        let keys = ["totals", "by_day", "by_model", "sem_tarifa", "applied", "usd_brl"];
        assert_eq!(summary.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), keys);
        for key in keys { assert_eq!(summary[key], full[key], "{period} {key}"); }
        assert_eq!(summary["sem_tarifa"], json!(["sem-preco"]), "{period}");
    }
}
