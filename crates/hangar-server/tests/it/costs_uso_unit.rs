use hangar_server::costs;
use costs::{pricing::Pricing, py::LocalTs, rows::{UsageRow, UsoLinha}};
use costs::report_uso::{build, UsoFilters};
use indexmap::IndexMap;
use serde_json::json;

fn now() -> LocalTs { LocalTs::from_iso("2026-10-03T12:00:00-03:00").unwrap() }
fn pricing() -> (tempfile::TempDir, Pricing) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"), json!({"modelos": {
        "alpha": {"input": 2, "output": 8, "cache_write": 2.5, "cache_read": 0.2, "provider": "anthropic"}
    }}).to_string()).unwrap();
    let p = Pricing::load(dir.path());
    (dir, p)
}
fn usage(kind: &str, name: &str) -> UsoLinha {
    UsoLinha { dia: "2026-10-03".into(), cwd: "/repo/a".into(), model: "alpha".into(),
        tipo: kind.into(), nome: name.into(), chamadas: 1, session_id: "parent".into(),
        fonte: "claude".into(), ..Default::default() }
}
fn token(account: &str, project: &str, input: i64) -> UsageRow {
    UsageRow { ts: now(), source: "claude".into(), provider: "anthropic".into(), model: "alpha".into(),
        project: project.into(), session_id: "parent/subagents/agent-child".into(), input,
        output: 0, cache_write: 0, cache_read: 0, subagente: true, account_id: Some(account.into()),
        codex_long_context: false, cache_write_1h: 0, fast: false, regravado: 0, regravado_1h: 0 }
}
fn close(actual: f64, expected: f64) { assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}"); }

#[test]
fn totals_count_calls_once_and_dimensions_keep_unfiltered_options() {
    let (_dir, p) = pricing();
    let mut skill = usage("skill", "brainstorming");
    skill.ctx_chars = 11; skill.ocupados = 26; skill.ocupados_eq = 13; skill.respostas = 2;
    skill.input = 1_000_000; skill.origem = "voce".into();
    let mut area = usage("area", "back"); area.input = 3_000_000;
    let mut context = usage("contexto", "hook"); context.ctx_chars = 9;
    let mut other = usage("area", "front"); other.cwd = "/repo/b".into(); other.input = 7_000_000;
    let rows = vec![(skill, "a".into()), (usage("tool", "Skill"), "a".into()),
        (usage("tool", "Read"), "a".into()), (usage("bash", "cat"), "a".into()),
        (context, "a".into()), (area, "a".into()), (other, "b".into())];
    let origins = IndexMap::from([("brainstorming".into(), "superpowers".into())]);
    let f = UsoFilters { conta: vec!["".into(), "a".into()], ..Default::default() };
    let r = build(&rows, &[], "all", now(), &f, Some(&origins), &p, &|k| Some(format!("label-{k}")));
    assert_eq!((r.totals.chamadas, r.totals.ctx_chars, r.totals.ctx_tokens_est, r.totals.input), (2, 20, 5, 3_000_000));
    close(r.totals.cost, 2.0);
    assert_eq!((r.by_skill[0].ctx_tokens_est, r.by_skill[0].ocupados_tokens_est, r.by_skill[0].ocupados_eq_tokens_est), (4, 10, 5));
    assert_eq!(r.by_skill[0].pedidas, 1);
    assert_eq!(r.by_skill[0].plugin, "superpowers");
    assert_eq!(r.by_conta.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    assert_eq!(r.by_conta[0].label.as_deref(), Some("label-b"));
    assert_eq!(r.by_projeto.len(), 2);
    assert_eq!(r.conta, ["a"]);
    assert_eq!(r.by_area_dia[0].label.as_deref(), Some("back"));
    close(r.by_area_dia[0].cost, 6.0);
}

#[test]
fn agent_costs_use_filtered_child_tokens_but_selectors_use_all_children() {
    let (_dir, p) = pricing();
    let mut agent = usage("agente", "plugin:worker"); agent.detalhe = "child".into();
    let rows = vec![(agent, "a".into())];
    let tokens = vec![token("a", "/repo/a", 1_000_000), token("b", "/repo/a", 2_000_000), token("a", "/repo/b", 4_000_000)];
    let f = UsoFilters { conta: vec!["a".into()], projeto: vec!["/repo/a".into()], foco: Some("plugin:worker".into()), ..Default::default() };
    let r = build(&rows, &tokens, "all", now(), &f, None, &p, &|_| None);
    close(r.totals.cost, 2.0); close(r.by_conta[0].cost, 14.0);
    assert_eq!(r.by_agente[0].input, 1_000_000);
    assert_eq!(r.by_plugin[0].key, "plugin");
    assert_eq!(r.by_day[0].input, 1_000_000); close(r.by_day[0].cost, 2.0);
}

#[test]
fn area_focus_ignores_same_named_skill_and_period_uses_local_day() {
    let (_dir, p) = pricing();
    let mut area = usage("area", "back"); area.input = 1_000_000;
    let mut old = area.clone(); old.dia = "2026-10-02".into();
    let mut skill = usage("skill", "back"); skill.input = 2_000_000;
    let f = UsoFilters { foco: Some("back".into()), ..Default::default() };
    let r = build(&[(area, "a".into()), (old, "a".into()), (skill, "a".into())], &[], "1d", now(), &f, None, &p, &|_| None);
    assert_eq!(r.by_day.len(), 1); assert_eq!(r.by_day[0].input, 1_000_000); close(r.by_day[0].cost, 2.0);
}

#[test]
fn empty_report_preserves_pydantic_order_nulls_and_empty_focus() {
    let (_dir, p) = pricing();
    let f = UsoFilters { foco: Some(String::new()), ..Default::default() };
    let r = build(&[], &[], "all", now(), &f, None, &p, &|_| None);
    let v = serde_json::to_value(r).unwrap();
    assert_eq!(v.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["totals", "by_skill", "by_tool", "by_bash", "by_mcp", "by_agente", "by_contexto", "by_plugin", "by_imagem", "by_area", "by_area_dia", "by_conta", "by_projeto", "by_modelo", "by_day", "conta", "projeto", "modelo", "plugin", "foco", "applied", "usd_brl"]);
    assert!(v["foco"].is_null()); assert!(v["usd_brl"].is_null()); assert!(v["totals"]["label"].is_null());
}

#[test]
fn item_focus_tracks_own_tokens_subagents_and_truncates_negative_context() {
    let (_dir, p) = pricing();
    let mut skill = usage("skill", "missing"); skill.ctx_chars = -11; skill.ocupados = -26;
    skill.ocupados_eq = -13; skill.tokens_est = 7; skill.input = 1_000_000; skill.subagente = true;
    let f = UsoFilters { foco: Some("missing".into()), ..Default::default() };
    let r = build(&[(skill, "a".into())], &[], "all", now(), &f, Some(&IndexMap::new()), &p, &|_| None);
    assert_eq!(r.by_skill[0].plugin, "@embutida");
    assert_eq!((r.by_skill[0].ctx_tokens_est, r.by_skill[0].ocupados_tokens_est, r.by_skill[0].ocupados_eq_tokens_est), (3, -10, -5));
    assert_eq!((r.totals.sessions, r.totals.subagentes, r.totals.input), (0, 1, 0));
    assert_eq!((r.by_day[0].input, r.by_day[0].ctx_tokens_est), (1_000_000, 5));
    close(r.by_day[0].cost, 2.0);
}

#[test]
fn model_plugin_filters_keep_selectors_and_rank_by_occupied_before_cost() {
    let (_dir, p) = pricing();
    let mut alpha = usage("skill", "z"); alpha.plugin = "p".into(); alpha.ocupados_eq = 20; alpha.input = 1_000_000;
    let mut beta = usage("skill", "a"); beta.plugin = "p".into(); beta.input = 10_000_000;
    let mut unknown = usage("skill", "unknown"); unknown.model.clear(); unknown.cwd.clear(); unknown.plugin = "q".into();
    let f = UsoFilters { modelo: vec!["alpha".into()], plugin: vec!["p".into()], ..Default::default() };
    let r = build(&[(beta, "a".into()), (unknown, "b".into()), (alpha, "a".into())], &[], "all", now(), &f, None, &p, &|_| None);
    assert_eq!(r.by_skill.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["z", "a"]);
    assert_eq!(r.by_modelo.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["alpha", "?"]);
    assert!(r.by_projeto.iter().any(|b| b.key == "desconhecido"));
    assert_eq!(r.by_plugin[0].key, "p"); assert_eq!(r.totals.chamadas, 2);
}

#[test]
fn token_period_cutoff_uses_utc_minus_three_and_not_timestamp_utc_date() {
    let (_dir, p) = pricing();
    let mut agent = usage("agente", "worker"); agent.detalhe = "child".into();
    let mut before = token("a", "/repo/a", 9_000_000); before.ts = LocalTs::from_iso("2026-10-03T02:59:59Z").unwrap();
    let mut inside = token("a", "/repo/a", 1_000_000); inside.ts = LocalTs::from_iso("2026-10-03T03:00:00Z").unwrap();
    let r = build(&[(agent, "a".into())], &[before, inside], "1d", now(), &UsoFilters::default(), None, &p, &|_| None);
    close(r.totals.cost, 2.0); assert_eq!(r.by_agente[0].input, 1_000_000);
}

#[test]
fn sum_of_price_components_compensates_but_addition_between_rows_stays_sequential() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"), json!({"modelos": {
        "large": {"input": 1e16, "output": 1, "cache_write": 1, "cache_read": 1, "provider": "anthropic"}
    }}).to_string()).unwrap();
    let p = Pricing::load(dir.path());
    let mut skill = usage("skill", "huge"); skill.model = "large".into(); skill.input = 1_000_000;
    skill.output = 1_000_000; skill.cache_write = 1_000_000; skill.cache_read = 1_000_000;
    let mut small = usage("skill", "small"); small.model = "large".into(); small.output = 1_000_000;
    let r = build(&[(skill, "a".into()), (small.clone(), "a".into()), (small.clone(), "a".into()), (small, "a".into())], &[], "all", now(), &UsoFilters::default(), None, &p, &|_| None);
    assert_eq!(r.totals.cost, 10_000_000_000_000_004.0);
}
