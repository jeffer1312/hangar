use crate::common;

use common::costs::*;
use hangar_server::costs::collect::*;
use hangar_server::costs::py::LocalTs;
use hangar_server::costs::{CacheKey, ReportCache, report_costs};
use hangar_server::costs::report_uso::{self, UsoFilters};
use hangar_server::transcript::pyjson;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn ready_collector(base: &Path) -> Arc<Collector> {
    struct Source(Scopes);
    impl ScopeSource for Source {
        fn fetch(&self) -> Result<Scopes, CollectError> { Ok(self.0.clone()) }
    }
    let scopes = Scopes {
        claude: vec![ClaudeScope { root: base.join("claude/projects"), account: "anthropic:u-fixture".into(), label: "fixture@exemplo".into() }],
        codex: vec![CodexScope { home: base.join("codex"), account: format!("codex:{}", base.join("codex").display()), label: "Codex · default".into() }],
        pi: vec![PiScope { root: base.join("pi"), source: "pi".into() }],
        kimi: Some(KimiScope { root: base.join("kimi/sessions"), index: base.join("kimi/session_index.jsonl") }),
        repo: base.to_owned(),
    };
    let collector = Arc::new(Collector::new(base.join("../idx"), base.join("pricing"), base.join("../sem-mapa.json"), Arc::new(Source(scopes))));
    collector.prepare(false).unwrap();
    // Só flagra a varredura que não termina: no runner Windows o disco deixa a varredura lenta.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !matches!(collector.prepare(false).unwrap(), Ready::Go) {
        assert!(Instant::now() < deadline, "coletor não concluiu");
        std::thread::sleep(Duration::from_millis(10));
    }
    collector
}

pub fn rebase(value: &Value, marker: &str, base: &Path) -> Value {
    match value {
        Value::Object(values) => Value::Object(values.iter().map(|(key, value)| (key.clone(), rebase(value, marker, base))).collect()),
        Value::Array(values) => Value::Array(values.iter().map(|value| rebase(value, marker, base)).collect()),
        Value::String(text) => {
            if text == &format!("codex:{marker}/codex") {
                return Value::String(format!("codex:{}", base.join("codex").display()));
            }
            // Id de sessão do Pi é caminho relativo: `str(Path)` no Python, `MAIN_SEPARATOR` no Rust.
            if text.starts_with("--repo--/") {
                return Value::String(text.replace('/', std::path::MAIN_SEPARATOR_STR));
            }
            if let Ok(decoded @ (Value::Array(_) | Value::Object(_))) = serde_json::from_str::<Value>(text) {
                let normalized = rebase(&decoded, marker, base);
                if normalized != decoded {
                    return Value::String(pyjson::dumps(&normalized, false));
                }
            }
            value.clone()
        }
        _ => value.clone(),
    }
}

#[test]
fn cost_reports_match_python() {
    let (_dir, base) = fixtures_copy();
    let collector = ready_collector(&base);
    let golden: Value = serde_json::from_slice(&std::fs::read(contract().join("golden/costs_reports.json")).unwrap()).unwrap();
    let now = LocalTs::from_iso(golden["now"].as_str().unwrap()).unwrap();
    for period in ["all", "7d"] {
        let rows = collector.read_costs(report_costs::since(period, now).as_deref()).unwrap();
        let labels = collector.labels_key();
        let got = report_costs::build(rows, period, now, &collector.pricing(), &|key| {
            labels.iter().find(|(name, _)| name == key).map(|(_, label)| label.clone())
        });
        let want = rebase(&golden["costs"][period], "__BASE__", &base);
        assert_close(&serde_json::to_value(&got).unwrap(), &want, period);
        if period == "all" {
            assert_eq!(got.totals.cost_output.to_bits(), want["totals"]["cost_output"].as_f64().unwrap().to_bits());
            assert_eq!(got.by_provider[1].cost_cache_write.to_bits(), want["by_provider"][1]["cost_cache_write"].as_f64().unwrap().to_bits());
        }
    }
}

#[test]
fn usage_reports_match_python() {
    let (_dir, base) = fixtures_copy();
    let collector = ready_collector(&base);
    let golden: Value = serde_json::from_slice(&std::fs::read(contract().join("golden/costs_reports.json")).unwrap()).unwrap();
    let now = LocalTs::from_iso(golden["now"].as_str().unwrap()).unwrap();
    let origins = indexmap::IndexMap::from([("brainstorming".into(), "superpowers".into()), ("minha-skill".into(), "@pessoal".into())]);
    let (usage, tokens) = collector.read_usage(None).unwrap();
    let filters = |accounts: &[&str], projects: &[&str], focus: Option<&str>| UsoFilters {
        conta: accounts.iter().map(|s| s.to_string()).collect(), projeto: projects.iter().map(|s| s.to_string()).collect(),
        foco: focus.map(str::to_owned), ..Default::default()
    };
    for (key, f) in [("all", filters(&[], &[], None)), ("conta", filters(&["anthropic:u-fixture"], &[], None)),
        ("projeto", filters(&[], &["/repo/a"], None)), ("foco_skill", filters(&[], &[], Some("brainstorming"))),
        ("foco_area", filters(&[], &[], Some("back")))] {
        let got = report_uso::build(&usage, &tokens, "all", now, &f, Some(&origins), &collector.pricing(), &|k| collector.label(k));
        assert_close(&serde_json::to_value(got).unwrap(), &rebase(&golden["uso"][key], "__BASE__", &base), key);
    }
}

#[test]
fn rebase_preserves_nested_json_identity_and_windows_backslashes() {
    let base = Path::new(r"C:\temporário\custos");
    let identity = r#"["codex", "codex:__BASE__/codex", "s", false]"#;
    let got = rebase(&json!({"provider":"codex:__BASE__/codex", "ids":[identity], "untouched":"__BASE__/literal"}), "__BASE__", base);
    let expected = format!("codex:{}", base.join("codex").display());
    assert_eq!(got["provider"], expected);
    let nested: Value = serde_json::from_str(got["ids"][0].as_str().unwrap()).unwrap();
    assert_eq!(nested, json!(["codex", expected, "s", false]));
    assert_eq!(got["ids"][0], pyjson::dumps(&nested, false));
    assert_eq!(got["untouched"], "__BASE__/literal");
    let pi = rebase(&json!(["--repo--/2026-09-30_s/t1", r#"["pi", "x", "--repo--/2026-09-30_s", false]"#]), "__BASE__", base);
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(pi[0], format!("--repo--{sep}2026-09-30_s{sep}t1"));
    assert_eq!(serde_json::from_str::<Value>(pi[1].as_str().unwrap()).unwrap()[2], format!("--repo--{sep}2026-09-30_s"));
}

fn cache_key(version: u64, route: &str) -> CacheKey {
    CacheKey { data_version: version, pricing_generation: 1, area_signature: "áreas".into(),
        labels: vec![("a".into(), "Conta".into())], route: vec![route.into(), "all".into(), "2026-10-03".into()] }
}

#[test]
fn report_cache_moves_hits_keeps_eight_and_discards_old_data_versions() {
    let cache = ReportCache::default();
    for i in 0..8 { cache.insert(cache_key(1, &i.to_string()), Arc::new(i)); }
    assert_eq!(*cache.get::<i32>(&cache_key(1, "0")).unwrap(), 0);
    cache.insert(cache_key(1, "8"), Arc::new(8));
    assert!(cache.get::<i32>(&cache_key(1, "1")).is_none());
    assert!(cache.get::<i32>(&cache_key(1, "0")).is_some());
    let mut changed = cache_key(1, "0");
    changed.pricing_generation += 1;
    assert!(cache.get::<i32>(&changed).is_none());
    changed = cache_key(1, "0"); changed.labels[0].1 = "Outro rótulo".into();
    assert!(cache.get::<i32>(&changed).is_none());
    changed = cache_key(1, "0"); changed.area_signature = "outra".into();
    assert!(cache.get::<i32>(&changed).is_none());
    cache.insert(cache_key(2, "0"), Arc::new(100));
    assert!(cache.get::<i32>(&cache_key(1, "0")).is_none());
    assert_eq!(*cache.get::<i32>(&cache_key(2, "0")).unwrap(), 100);
    assert!(cache.get::<String>(&cache_key(2, "0")).is_none());
}
