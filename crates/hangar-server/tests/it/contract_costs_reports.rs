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

#[test]
fn claude_turn_areas_keep_streaming_weights_costs_totals_and_focus() {
    use hangar_server::costs::{areas::AreaMap, claude, index::Index, pricing::Pricing};
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("projects");
    let transcript = root.join("p/s1.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let usage = |input: i64, output: i64, cache_write: i64| json!({
        "input_tokens":input, "output_tokens":output,
        "cache_creation_input_tokens":cache_write, "cache_read_input_tokens":0
    });
    let assistant = |blocks: Value, id: &str, usage: Value| json!({
        "type":"assistant", "timestamp":"2026-09-10T12:00:00Z", "cwd":"/repo",
        "requestId":format!("r{id}"),
        "message":{"id":id, "model":"claude-opus-5", "usage":usage, "content":blocks}
    });
    let user = |text: &str, prompt: &str| json!({
        "type":"user", "timestamp":"2026-09-10T12:00:00Z", "cwd":"/repo",
        "promptId":prompt, "message":{"role":"user", "content":text}
    });
    let tool = |name: &str, input: Value, id: &str| json!({
        "type":"tool_use", "id":id, "name":name, "input":input
    });
    let records = [
        user("mexe no front e no back", "p1"),
        assistant(json!([tool("Read", json!({"file_path":"/repo/frontend/src/A.svelte"}), "t1")]), "m1", usage(100, 10, 0)),
        assistant(json!([tool("Edit", json!({"file_path":"/repo/backend/app/x.py"}), "t2")]), "m2", usage(200, 20, 0)),
        assistant(json!([tool("Bash", json!({"command":"uv run pytest backend/tests/test_x.py 2>/dev/null"}), "t3")]), "m3", usage(300, 30, 10)),
        assistant(json!([{"type":"text", "text":"..."}]), "m3", usage(300, 50, 10)),
        assistant(json!([tool("Read", json!({"file_path":"/tmp/print.png"}), "t4")]), "m4", usage(0, 0, 0)),
        user("e aí?", "p2"),
        assistant(json!([{"type":"text", "text":"só conversa"}]), "m5", usage(50, 5, 0)),
        user("olha o banco", "p3"),
        assistant(json!([tool("Skill", json!({"skill":"acme:database"}), "t6")]), "m6", usage(70, 7, 0)),
    ];
    let text = records.iter().map(|r| serde_json::to_string(r).unwrap()).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&transcript, text).unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let map = AreaMap::load(&d.path().join("areas.json"));
    ix.sync("synthetic", &[transcript], &claude::new_fold(&root), claude::VERSION,
        map.signature(), &|e| map.area_lines(e), &progress()).unwrap();
    let rows = ix.read_usage("synthetic", None).unwrap();
    let mut areas = indexmap::IndexMap::new();
    for row in rows.iter().filter(|r| r.tipo == "area") {
        let total = areas.entry(row.nome.as_str()).or_insert((0, 0, 0, 0));
        total.0 += row.chamadas;
        total.1 += row.input;
        total.2 += row.output;
        total.3 += row.cache_write;
    }
    let front = areas["front"];
    let back = areas["back"];
    let other = areas["outros"];
    assert_eq!((front.0, front.1, front.2), (1, 150, 20));
    assert_eq!((back.0, back.1, back.2), (2, 300, 40));
    assert_eq!((other.0, other.1, other.2), (1, 150, 20));
    assert_eq!(front.3 + back.3 + other.3, 10);
    assert_eq!(back.3, 5);
    assert_eq!(areas["conversa"], (0, 50, 5, 0));
    assert_eq!(areas["banco"], (1, 70, 7, 0));
    assert_eq!(areas.values().map(|r| r.1).sum::<i64>(), 720);
    let rows = rows.into_iter().map(|row| (row, "anthropic:synthetic".to_owned())).collect::<Vec<_>>();

    let pricing_dir = d.path().join("pricing");
    std::fs::create_dir_all(&pricing_dir).unwrap();
    std::fs::write(pricing_dir.join("overrides.json"), json!({"claude-opus-5":{
        "provider":"anthropic", "input":1.0, "output":10.0, "cache_read":0.1,
        "cache_write":1.25
    }}).to_string()).unwrap();
    let pricing = Pricing::load(&pricing_dir);
    assert!(!pricing.rate_for("claude-opus-5").unwrap().cache_estimado);
    let now = LocalTs::from_iso("2026-09-10T12:00:00Z").unwrap();
    let report = report_uso::build(&rows, &[], "all", now, &UsoFilters::default(), None, &pricing, &|_| None);
    let back_cost = 300.0 / 1e6 + 40.0 / 1e6 * 10.0 + 5.0 / 1e6 * 1.25;
    assert_close(&json!(report.by_area.iter().find(|b| b.key == "back").unwrap().cost), &json!(back_cost), "back cost");
    assert_close(&json!(report.totals.cost), &json!(report.by_skill.iter().map(|b| b.cost).sum::<f64>()), "total sem duplicar área");
    assert!(report.by_area_dia.iter().any(|b| b.key == "2026-09-10|front" && b.label.as_deref() == Some("front")));
    let focus = UsoFilters { foco: Some("back".into()), ..Default::default() };
    let focused = report_uso::build(&rows, &[], "all", now, &focus, None, &pricing, &|_| None);
    assert_eq!(focused.by_day.iter().map(|b| (b.key.as_str(), b.chamadas)).collect::<Vec<_>>(), [("2026-09-10", 2)]);
    assert_close(&json!(focused.by_day[0].cost), &json!(back_cost), "foco back");
    let tokens = |b: &report_uso::UsoBucket| b.input + b.output + b.cache_write + b.cache_read;
    assert_eq!(tokens(&report.totals), 822);
    assert_eq!(report.by_day.iter().map(tokens).collect::<Vec<_>>(), [822]);
    assert_eq!(report.by_projeto.iter().map(|b| (b.key.as_str(), tokens(b))).collect::<Vec<_>>(), [("/repo", 822)]);
    let focus = UsoFilters { foco: Some("acme:database".into()), ..Default::default() };
    let focused = report_uso::build(&rows, &[], "all", now, &focus, None, &pricing, &|_| None);
    assert_eq!(focused.by_day.iter().map(tokens).collect::<Vec<_>>(), [0]);
}

#[test]
fn text_only_claude_parent_and_child_keep_session_counts_and_input() {
    use hangar_server::costs::{areas::AreaMap, claude, index::Index, pricing::Pricing};
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("projects");
    let parent = root.join("p/s1.jsonl");
    let child = root.join("p/s1/subagents/agent-a1.jsonl");
    std::fs::create_dir_all(child.parent().unwrap()).unwrap();
    for (path, id, input) in [(&parent, "m-s1", 100), (&child, "m-agent-a1", 40)] {
        let records = [
            json!({"type":"user", "timestamp":"2026-09-10T12:00:00Z", "cwd":"/repo",
                "promptId":"p1", "message":{"role":"user", "content":"x"}}),
            json!({"type":"assistant", "timestamp":"2026-09-10T12:00:01Z", "cwd":"/repo",
                "requestId":format!("r{id}"), "message":{"id":id, "model":"claude-opus-5",
                    "usage":{"input_tokens":input, "output_tokens":5,
                        "cache_creation_input_tokens":0, "cache_read_input_tokens":0},
                    "content":[{"type":"text", "text":"."}]}}),
        ];
        let text = records.iter().map(|r| serde_json::to_string(r).unwrap()).collect::<Vec<_>>().join("\n") + "\n";
        std::fs::write(path, text).unwrap();
    }
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let map = AreaMap::load(&d.path().join("areas.json"));
    ix.sync("synthetic", &[parent, child], &claude::new_fold(&root), claude::VERSION,
        map.signature(), &|e| map.area_lines(e), &progress()).unwrap();
    let rows = ix.read_usage("synthetic", None).unwrap();
    let rows = rows.into_iter().map(|row| (row, "anthropic:synthetic".to_owned())).collect::<Vec<_>>();
    let pricing = Pricing::load(&d.path().join("pricing"));
    let now = LocalTs::from_iso("2026-09-10T15:00:00Z").unwrap();
    let report = report_uso::build(&rows, &[], "all", now, &UsoFilters::default(), None, &pricing, &|_| None);
    assert_eq!((report.totals.sessions, report.totals.subagentes), (1, 1));
    assert_eq!(report.totals.input, 140);
}
