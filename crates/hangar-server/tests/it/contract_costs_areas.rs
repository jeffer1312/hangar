use hangar_server::costs::areas::{AreaEntries, AreaHeader, AreaMap, ToolReg, Unit, candidates, repartir};
use hangar_server::costs::uso_rules::{comando_bash, pede_agente, plugin_de, plugin_de_hook, skill_do_caminho, texto_len, tokens_de_imagem};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::Path;

fn golden() -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/golden/costs_areas.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn default_map() -> AreaMap { AreaMap::load(Path::new("/nao/existe.json")) }

#[test]
fn area_rules_match_python() {
    let g = golden();
    for r in g["comando_bash"].as_array().unwrap() {
        assert_eq!(comando_bash(r[0].as_str().unwrap()), r[1].as_str().unwrap(), "{r}");
    }
    for r in g["candidatos"].as_array().unwrap() {
        let want: Vec<String> = serde_json::from_value(r[1].clone()).unwrap();
        assert_eq!(candidates(r[0].as_str().unwrap()), want, "{r}");
    }
    for r in g["skill_do_caminho"].as_array().unwrap() {
        let got = skill_do_caminho(r[0].as_str().unwrap()).map(|(n, md)| json!([n, md]));
        assert_eq!(got.unwrap_or(Value::Null), r[1], "{r}");
    }
    for r in g["repartir"].as_array().unwrap() {
        let weights: IndexMap<String, i64> = serde_json::from_value(r[1].clone()).unwrap();
        let want: IndexMap<String, i64> = serde_json::from_value(r[2].clone()).unwrap();
        assert_eq!(repartir(r[0].as_i64().unwrap(), &weights), want, "{r}");
    }
    let map = default_map();
    let rules = map.rules_for("/qualquer");
    for r in g["area_do_alvo"].as_array().unwrap() {
        assert_eq!(map.area_of_target(r[0].as_str().unwrap(), &rules).map(Value::from).unwrap_or(Value::Null), r[1], "{r}");
    }
}

#[test]
fn repo_root_comes_from_the_file_not_the_cwd() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("vizinho");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("backend")).unwrap();
    let map = default_map();
    let rules = map.rules_for("/outro");
    assert_eq!(map.area_of_path(repo.join("backend/x.py").to_str().unwrap(), "/outro", &rules), "back");
    assert_eq!(map.area_of_path("/tmp/solto.py", "/outro", &rules), "outros");
}

#[test]
fn map_overrides_are_ordered_and_invalid_entries_are_ignored() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("areas.json");
    std::fs::write(&file, r#"{"padrao":[["generic",["*.py",3]],[2,[]],["ignored","x"]],"projetos":{"acme":[["first",["backend/*"]]],"/tmp/acme":[["second",["*.py"]]]}}"#).unwrap();
    let map = AreaMap::load(&file);
    assert_eq!(map.rules_for("/tmp/acme/sub").iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["first", "second", "generic"]);
    assert_eq!(map.rules_for("/tmp/acme-other").len(), 1);
    let rules = map.rules_for("/tmp/acme");
    assert_eq!(map.area_of_target("backend/x.py", &rules).as_deref(), Some("first"));
    assert_eq!(map.area_of_target("other.py", &rules).as_deref(), Some("second"));
    assert_eq!(map.signature(), AreaMap::load(&file).signature());
    std::fs::write(&file, "{invalid").unwrap();
    assert_eq!(AreaMap::load(&file).rules_for("/tmp/acme"), default_map().rules_for("/tmp/acme"));
    assert_ne!(map.signature(), AreaMap::load(&file).signature());
    std::fs::write(&file, r#"{"padrao":null}"#).unwrap();
    assert!(AreaMap::load(&file).rules_for("x").is_empty());
}

#[test]
fn project_overrides_accept_both_path_separators_without_changing_case_or_boundaries() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("areas.json");
    for key in ["C:/acme", r"C:\acme"] {
        std::fs::write(&file, json!({"padrao":[["generic",["*.py"]]], "projetos":{
            "acme":[["first",["backend/*"]]], key:[["second",["*.py"]]]
        }}).to_string()).unwrap();
        let map = AreaMap::load(&file);
        for cwd in ["C:/acme", r"C:\acme", "C:/acme/sub", r"C:\acme\sub", r"C:/acme\sub"] {
            assert_eq!(map.rules_for(cwd).iter().map(|rule| rule.0.as_str()).collect::<Vec<_>>(),
                ["first", "second", "generic"], "{key}: {cwd}");
        }
        for cwd in ["C:/acme-other", r"C:\acme-other", "C:/ACME/sub", r"C:\ACME\sub"] {
            assert_eq!(map.rules_for(cwd).iter().map(|rule| rule.0.as_str()).collect::<Vec<_>>(),
                ["generic"], "{key}: {cwd}");
        }
    }
}

#[test]
fn fnmatch_handles_suffixes_newlines_case_and_character_sets() {
    let map = default_map();
    for (pattern, target, want) in [
        ("a/**/x?.[ch]", "a/b/c/xx.c", true),
        ("*.py", "a\nb.py", true), ("*.py", "A.PY", false),
        ("[!a-c]", "z", true), ("[!a-c]", "b", false),
        ("[]]", "]", true), ("[[]", "[", true), ("[abc", "[abc", true),
        ("[z-a]", "z", false), ("[!z-a]", "z", true),
        ("[a&&b]", "&", true), ("[a--b]", "a", false),
        ("[a--b]", "b", true), ("[!a--b]", "a", true),
        ("[a-b-c]", "-", true), ("[\\]", "\\", true), ("[--z]", "a", true),
        ("a*", "x/a\ny", true), ("a?", "x/ab", true),
    ] {
        let rules = vec![("hit".into(), vec![pattern.into()])];
        assert_eq!(map.area_of_target(target, &rules).is_some(), want, "{pattern}: {target}");
    }
}

#[test]
fn paths_normalize_without_resolving_symlinks_and_git_files_count() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir_all(repo.join("backend")).unwrap();
    std::fs::write(repo.join(".git"), "gitdir: elsewhere").unwrap();
    let map = default_map();
    let cwd = repo.to_str().unwrap();
    let rules = map.rules_for(cwd);
    assert_eq!(map.area_of_path("./backend/../backend/x.py", cwd, &rules), "back");
    assert_eq!(map.area_of_path("../loose.py", cwd, &rules), "outros");
    let plain = d.path().join("plain");
    assert_eq!(map.area_of_path("x.py", plain.to_str().unwrap(), &rules), "back");
    assert_eq!(map.area_of_path("x.py", "", &rules), "outros");
}

#[test]
fn usage_helpers_keep_python_tokenization_and_skill_semantics() {
    assert_eq!(comando_bash("sudo\u{a0}env\u{2003}X=1 /usr/bin/pytest"), "pytest");
    assert_eq!(comando_bash("cd here\nFOO=$(git status)"), "git");
    assert_eq!(comando_bash("123foo -q env"), "?");
    assert_eq!(comando_bash("²x\u{1c}pytest"), "pytest");
    assert_eq!(candidates("X='a.py' ./foo/bar -x.py https://x/y $PWD/z .hidden ../x"), ["a.py", "./foo/bar", "../x"]);
    assert_eq!(skill_do_caminho(r"C:\a\skills\my\references\x.md"), Some(("my".into(), false)));
    assert_eq!(skill_do_caminho("/skills/foo/notes.md"), Some(("foo".into(), false)));
    assert_eq!(skill_do_caminho("/skills/notes.md"), None);
    assert_eq!(skill_do_caminho("/skills/SKILL.md"), Some(("skills".into(), true)));
    assert_eq!(plugin_de("superpowers:x:y"), "superpowers");
    assert_eq!(plugin_de("x"), "");
    assert_eq!(plugin_de_hook("  [ skill-suggester ] hello"), "skill-suggester");
    assert_eq!(plugin_de_hook("PONYTAIL MODE ACTIVE"), "ponytail");
    assert_eq!(plugin_de_hook("mentions superpowers later"), "");
    for prompt in ["Use sub-agentes", "AGENTS", "fanout", "subagent-driven", "explore isso"] { assert!(pede_agente(prompt)); }
    for prompt in ["superagente", "explorer", "éagente", "parallelism"] { assert!(!pede_agente(prompt)); }
    assert_eq!(texto_len(Some(&json!(["á", {"text":"你好", "content":"ignored"}, {"content":["x",{"text":"🙂"}]}, 5]))), 5);
    assert_eq!(texto_len(Some(&json!({"text":"ignored"}))), 0);
}

#[test]
fn png_header_estimates_pixels_and_invalid_or_other_formats_only_count_chars() {
    let data = "iVBORw0KGgoAAAANSUhEUgAAAyAAAAJY";
    assert_eq!(tokens_de_imagem(json!({"source":{"media_type":"image/png","data":data}}).as_object().unwrap()), (32, 640));
    for (media, value, chars) in [("image/jpeg", data, 32), ("image/png", "invalid!", 8), ("image/png", "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!", 32)] {
        assert_eq!(tokens_de_imagem(json!({"source":{"media_type":media,"data":value}}).as_object().unwrap()), (chars, 0));
    }
    assert_eq!(tokens_de_imagem(json!({"source":[]}).as_object().unwrap()), (0, 0));
}

fn unit(day: &str, fast: bool, values: [i64; 5]) -> Unit {
    Unit { dia: day.into(), cwd: "/project".into(), model: "m".into(), fast, values }
}

#[test]
fn areas_deduplicate_each_tool_and_preserve_header_turns_and_totals() {
    let map = default_map();
    let regs = vec![
        ToolReg::P { rules_cwd: "/project".into(), cwd: "/project".into(), paths: vec!["a.py".into(), "b.py".into()] },
        ToolReg::C { rules_cwd: "/project".into(), cwd: "/project".into(), candidates: vec!["a.py".into(), "/else/a.py".into()] },
        ToolReg::S { rules_cwd: "/project".into(), target: "skill:database-x".into() },
    ];
    assert_eq!(map.count_areas(&regs), IndexMap::from([("back".into(), 2), ("banco".into(), 1)]));
    let entries = AreaEntries {
        header: AreaHeader { fonte: Some("codex".into()), session_id: Some("session".into()), subagente: Some(true) },
        turns: vec![(regs, vec![unit("2026-09-30", false, [10, 7, 3, 2, 9]), unit("2026-10-01", true, [1, 2, 0, 0, 0])]), (vec![], vec![unit("2026-09-30", false, [4, 3, 2, 1, -1])])],
    };
    let rows = map.area_lines(&entries);
    assert_eq!(rows.iter().map(|r| (r.dia.as_str(), r.nome.as_str(), r.chamadas)).collect::<Vec<_>>(), [("2026-09-30", "back", 2), ("2026-09-30", "banco", 1), ("2026-10-01", "back", 0), ("2026-10-01", "banco", 0), ("2026-09-30", "conversa", 0)]);
    assert_eq!(rows.iter().map(|r| r.input).sum::<i64>(), 15);
    assert_eq!(rows.iter().map(|r| r.output).sum::<i64>(), 12);
    assert_eq!(rows.iter().map(|r| r.cache_write_1h).sum::<i64>(), 3);
    assert!(rows[2].fast && rows[3].fast);
    assert!(rows.iter().all(|r| r.fonte == "codex" && r.session_id == "session" && r.subagente));
    assert_eq!(serde_json::from_value::<AreaEntries>(serde_json::to_value(&entries).unwrap()).unwrap(), entries);
    let default_entries = AreaEntries { header: AreaHeader { fonte: None, session_id: None, subagente: None }, turns: vec![(vec![], vec![unit("d", false, [1,0,0,0,0])])] };
    assert_eq!(map.area_lines(&default_entries)[0].fonte, "claude");
}

#[test]
fn negative_allocation_retains_python_slice_semantics_and_key_order() {
    let weights = IndexMap::from([("z".into(), 2), ("a".into(), 1), ("b".into(), 1)]);
    for (value, expected) in [(-7, [-2,-1,-1]), (-5, [-2,0,0]), (-1, [0,1,1]), (1, [1,0,0]), (5, [3,1,1])] {
        let got = repartir(value, &weights);
        assert_eq!(got.keys().map(String::as_str).collect::<Vec<_>>(), ["z", "a", "b"]);
        assert_eq!(got.into_values().collect::<Vec<_>>(), expected);
    }
}

#[test]
fn neighbor_repository_gets_its_own_rules_and_new_map_resets_root_cache() {
    let d = tempfile::tempdir().unwrap();
    let neighbor = d.path().join("neighbor");
    std::fs::create_dir_all(neighbor.join("backend")).unwrap();
    let file = d.path().join("areas.json");
    std::fs::write(&file, json!({"projetos":{neighbor.to_str().unwrap():[["custom",["*.py"]]]}}).to_string()).unwrap();
    let map = AreaMap::load(&file);
    let rules = map.rules_for("/outside");
    let target = neighbor.join("backend/x.py");
    assert_eq!(map.area_of_path(target.to_str().unwrap(), "/outside", &rules), "outros");
    std::fs::create_dir_all(neighbor.join(".git")).unwrap();
    let map = AreaMap::load(&file);
    assert_eq!(map.area_of_path(target.to_str().unwrap(), "/outside", &rules), "custom");
}

#[test]
fn merging_turns_counts_each_first_group_and_ors_fast() {
    let map = default_map();
    let reg = ToolReg::S { rules_cwd: "/project".into(), target: "skill:database".into() };
    let entries = AreaEntries {
        header: AreaHeader { fonte: None, session_id: None, subagente: None },
        turns: vec![(vec![reg.clone()], vec![unit("d", false, [1,2,3,4,5])]), (vec![reg], vec![unit("d", true, [6,7,8,9,10])])],
    };
    let rows = map.area_lines(&entries);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].chamadas, rows[0].input, rows[0].output, rows[0].cache_write, rows[0].cache_read, rows[0].cache_write_1h, rows[0].fast), (2,7,9,11,13,11,true));
    assert_eq!(rows[0].session_id, "");
    assert!(!rows[0].subagente);
}

#[test]
fn command_outside_paths_and_unmapped_skills_do_not_claim_an_area() {
    let map = default_map();
    let regs = vec![
        ToolReg::C { rules_cwd: "/project".into(), cwd: "/project".into(), candidates: vec!["/outside/file.py".into()] },
        ToolReg::S { rules_cwd: "/project".into(), target: "skill:unmapped".into() },
    ];
    assert!(map.count_areas(&regs).is_empty());
    let path = ToolReg::P { rules_cwd: "/project".into(), cwd: "/project".into(), paths: vec!["/outside/file.py".into()] };
    assert_eq!(map.count_areas(&[path]), IndexMap::from([("outros".into(), 1)]));
}

#[test]
fn agent_request_boundaries_and_turkish_i_follow_python_re() {
    for prompt in ["agente\u{301}", "\u{200c}agente", "agente\u{203f}", "dıspara", "DİSPARA"] {
        assert!(pede_agente(prompt), "{prompt}");
    }
    for prompt in ["²agente", "agenté", "agente_", "agenteⅧ"] { assert!(!pede_agente(prompt), "{prompt}"); }
}

#[test]
fn rejected_agent_matches_do_not_consume_valid_overlapping_matches() {
    for prompt in ["xsub-agente", "xsub-agentes", "éxsub-agente", "éxsub-agentes", "xsub-agente\u{301}"] {
        assert!(pede_agente(prompt), "{prompt}");
    }
    for prompt in ["xsub-agente_", "xsub-agentesé", "xsubagente", "xsubagentes", "éxsub-agenteⅧ"] {
        assert!(!pede_agente(prompt), "{prompt}");
    }
}

#[test]
fn allocation_outside_i64_panics_with_static_code_before_casting() {
    for (value, pairs) in [
        (i64::MAX, vec![("a", 1)]),
        (i64::MAX, vec![("a", 2), ("b", -1)]),
        (i64::MIN, vec![("a", 2), ("b", -1)]),
        (i64::MIN, vec![("a", -1), ("b", 2)]),
    ] {
        let weights: IndexMap<String, i64> = pairs.into_iter().map(|(k,v)| (k.into(),v)).collect();
        let error = std::panic::catch_unwind(|| repartir(value, &weights)).expect_err("o rateio fora de faixa deve falhar");
        assert_eq!(error.downcast_ref::<&'static str>(), Some(&"area_allocation_out_of_range"));
    }
    let weights = IndexMap::from([("a".into(), 1)]);
    for value in [i64::MIN, i64::MIN + 1024, i64::MAX - 1023] {
        assert_eq!(repartir(value, &weights)["a"], value);
    }
}

#[test]
fn empty_area_name_is_kept_by_target_but_not_by_paths_or_skills() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("areas.json");
    std::fs::write(&file, r#"{"padrao":[["",["*.py","skill:*"]]]}"#).unwrap();
    let map = AreaMap::load(&file);
    let cwd = d.path().to_str().unwrap();
    let rules = map.rules_for(cwd);
    assert_eq!(map.area_of_target("a.py", &rules), Some(String::new()));
    assert_eq!(map.area_of_path("a.py", cwd, &rules), "outros");
    let skill = ToolReg::S { rules_cwd: cwd.into(), target: "skill:database".into() };
    assert!(map.count_areas(&[skill.clone()]).is_empty());
    let path = ToolReg::P { rules_cwd: cwd.into(), cwd: cwd.into(), paths: vec!["a.py".into()] };
    assert_eq!(map.count_areas(&[path]), IndexMap::from([("outros".into(), 1)]));
    let command = ToolReg::C { rules_cwd: cwd.into(), cwd: cwd.into(), candidates: vec!["a.py".into()] };
    assert!(map.count_areas(&[command]).is_empty());
    let entries = AreaEntries {
        header: AreaHeader { fonte: None, session_id: None, subagente: None },
        turns: vec![(vec![skill], vec![unit("d", false, [3,2,1,0,0])])],
    };
    let rows = map.area_lines(&entries);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].nome.as_str(), rows[0].input, rows[0].chamadas), ("conversa", 3, 0));
}

#[test]
fn project_folder_matching_discards_the_dot_component_like_pathlib() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("areas.json");
    std::fs::write(&file, r#"{"padrao":[],"projetos":{".":[["dot",["*"]]],"repo":[["repo",["*"]]]}}"#).unwrap();
    let map = AreaMap::load(&file);
    assert!(map.rules_for(".").is_empty());
    assert_eq!(map.rules_for("./repo/sub").iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["repo"]);
}

#[test]
fn allocation_rounds_the_integer_ratio_once_like_python() {
    let weights = IndexMap::from([("a".into(), 98), ("b".into(), 58), ("c".into(), 61)]);
    assert_eq!(repartir(571197764309871498, &weights).into_values().collect::<Vec<_>>(), [257960280656071009, 152670370184205281, 160567113469595201]);
    let weights = IndexMap::from([("a".into(), i64::MAX), ("b".into(), i64::MAX)]);
    assert_eq!(repartir(5, &weights).into_values().collect::<Vec<_>>(), [3, 2]);
    let weights = IndexMap::from([("a".into(), 1), ("b".into(), i64::MAX)]);
    assert_eq!(repartir(1, &weights).into_values().collect::<Vec<_>>(), [0, 1]);
    let weights = IndexMap::from([("a".into(), 1), ("b".into(), 1)]);
    assert_eq!(repartir(18014398509481986, &weights).into_values().collect::<Vec<_>>(), [9007199254740993, 9007199254740993]);
    assert_eq!(repartir(18014398509481990, &weights).into_values().collect::<Vec<_>>(), [9007199254740996, 9007199254740996]);
    assert_eq!(repartir(i64::MAX, &weights).into_values().collect::<Vec<_>>(), [4611686018427387905, 4611686018427387904]);
    let weights = IndexMap::from([("a".into(), -3), ("b".into(), -1)]);
    assert_eq!(repartir(10, &weights).into_values().collect::<Vec<_>>(), [8, 2]);
    let weights = IndexMap::from([("a".into(), -1), ("b".into(), 2)]);
    assert_eq!(repartir(10, &weights).into_values().collect::<Vec<_>>(), [-10, 20]);
}

#[test]
fn empty_weights_return_empty_and_zero_total_weights_raise() {
    assert!(repartir(5, &IndexMap::new()).is_empty());
    let zero = IndexMap::from([("a".into(), 0)]);
    assert!(std::panic::catch_unwind(|| repartir(0, &zero)).is_err());
    let canceled = IndexMap::from([("a".into(), 1), ("b".into(), -1)]);
    assert!(std::panic::catch_unwind(|| repartir(10, &canceled)).is_err());
}

#[cfg(windows)]
#[test]
fn windows_paths_join_roots_and_drives_and_compare_case_insensitively() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let map = default_map();
    let cwd = root.to_str().unwrap().replace('\\', "/");
    let rules = map.rules_for(&cwd);
    assert_eq!(map.area_of_path("backend/../backend/x.py", &cwd, &rules), "back");
    assert_eq!(map.area_of_path(&format!("{}/backend/x.py", cwd.to_uppercase()), &cwd, &rules), "back");
    assert_eq!(map.area_of_path("../outside.py", &cwd, &rules), "outros");
    assert_eq!(map.area_of_path("x.py", "C:/Root", &rules), "back");
    assert_eq!(map.area_of_path("C:x.py", "c:/Root", &rules), "back");
    assert_eq!(map.area_of_path("D:x.py", "C:/Root", &rules), "outros");
    assert_eq!(map.area_of_path("/Root/x.py", "C:/Root", &rules), "back");
    assert_eq!(map.area_of_path("//server/share/root/x.py", "\\\\SERVER\\share\\root", &rules), "back");
    assert_eq!(map.area_of_path("é:/else/x.py", "é:/Root", &rules), "outros");
    let file = d.path().join("drive-rules.json");
    std::fs::write(&file, r#"{"padrao":[],"projetos":{"C:":[["drive",["*"]]]}}"#).unwrap();
    let map = AreaMap::load(&file);
    assert!(map.rules_for("C:/Root").is_empty());
    assert_eq!(map.rules_for("C:Root")[0].0, "drive");
}

#[test]
fn areas_per_tool_have_common_name_order_without_reordering_tools() {
    let d = tempfile::tempdir().unwrap();
    let cwd = d.path().to_str().unwrap();
    let map = default_map();
    let regs = vec![
        ToolReg::S { rules_cwd: cwd.into(), target: "skill:database".into() },
        ToolReg::P { rules_cwd: cwd.into(), cwd: cwd.into(), paths: vec!["a.css".into(), "a.py".into(), "b.css".into()] },
        ToolReg::C { rules_cwd: cwd.into(), cwd: cwd.into(), candidates: vec!["a.md".into(), "a.py".into(), "/outside/file.py".into()] },
    ];
    let counts = map.count_areas(&regs);
    assert_eq!(counts.into_iter().collect::<Vec<_>>(), vec![
        ("banco".into(), 1), ("back".into(), 2), ("front".into(), 1), ("docs".into(), 1),
    ]);
    let entries = AreaEntries {
        header: AreaHeader { fonte: None, session_id: None, subagente: None },
        turns: vec![(regs, vec![unit("d", false, [11,7,5,3,2])])],
    };
    let rows = map.area_lines(&entries);
    assert_eq!(rows.iter().map(|r| (r.nome.as_str(), r.chamadas, r.input, r.output, r.cache_write, r.cache_read, r.cache_write_1h)).collect::<Vec<_>>(), vec![
        ("banco", 1, 2, 2, 1, 1, 1), ("back", 2, 5, 3, 2, 1, 1),
        ("front", 1, 2, 1, 1, 0, 0), ("docs", 1, 2, 1, 1, 1, 0),
    ]);
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedAreasFold { entries: AreaEntries }

impl hangar_server::costs::index::Fold for SavedAreasFold {
    fn line(&mut self, _: &[u8]) {}

    fn close(&mut self) -> hangar_server::costs::rows::FoldOutput {
        use hangar_server::costs::rows::{FoldOutput, UsageRow, UsoLinha};
        let cost = UsageRow {
            ts: hangar_server::costs::py::LocalTs(0), source: "synthetic".into(),
            provider: "".into(), model: "m".into(), project: "p".into(), session_id: "s".into(),
            input: 11, output: 7, cache_write: 5, cache_read: 3, cache_write_1h: 2,
            subagente: false, account_id: None, codex_long_context: false, fast: false,
            regravado: 0, regravado_1h: 0,
        };
        let tool = UsoLinha { dia: "d".into(), cwd: "p".into(), model: "m".into(),
            tipo: "tool".into(), nome: "Read".into(), chamadas: 1, ..UsoLinha::default() };
        FoldOutput { costs: vec![cost], usage: vec![tool], areas: Some(self.entries.clone()) }
    }
}

#[test]
fn matching_signature_refolds_only_saved_areas_without_reading_the_transcript() {
    use hangar_server::costs::index::{Index, Progress};
    use md5::{Digest, Md5};
    let d = tempfile::tempdir().unwrap();
    let cwd = d.path().to_str().unwrap();
    let transcript = d.path().join("synthetic.jsonl");
    std::fs::write(&transcript, "synthetic\n").unwrap();
    let map = default_map();
    let old_signature = format!("{:x}", Md5::digest(format!("divisao:3{}", serde_json::to_string(&map.rules_for("/synthetic")).unwrap()).as_bytes()));
    let entries = AreaEntries {
        header: AreaHeader { fonte: None, session_id: None, subagente: None },
        turns: vec![(vec![ToolReg::P { rules_cwd: cwd.into(), cwd: cwd.into(), paths: vec!["a.css".into(), "a.py".into()] }], vec![unit("d", false, [11,7,5,3,2])])],
    };
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let seed = |_: &Path| SavedAreasFold { entries: entries.clone() };
    let legacy = |e: &AreaEntries| {
        let mut rows = map.area_lines(e);
        rows.sort_by(|a,b| b.nome.cmp(&a.nome));
        rows
    };
    ix.sync("synthetic", &[transcript.clone()], &seed, "v1", &old_signature, &legacy, &Progress::default()).unwrap();
    let costs_before = ix.read_costs(Some("synthetic"), None, None).unwrap();
    let usage_before = ix.read_usage("synthetic", None).unwrap();
    assert_eq!(usage_before.iter().filter(|r| r.tipo == "area").map(|r| r.nome.as_str()).collect::<Vec<_>>(), ["front", "back"]);
    assert_ne!(old_signature, map.signature());
    let forbid_read = |_: &Path| -> SavedAreasFold { panic!("a troca de assinatura não pode reler o transcript") };
    let redo = |e: &AreaEntries| map.area_lines(e);
    assert!(ix.sync("synthetic", &[transcript.clone()], &forbid_read, "v1", map.signature(), &redo, &Progress::default()).unwrap());
    assert_eq!(ix.read_costs(Some("synthetic"), None, None).unwrap(), costs_before);
    let usage = ix.read_usage("synthetic", None).unwrap();
    assert_eq!(usage.iter().filter(|r| r.tipo != "area").cloned().collect::<Vec<_>>(), usage_before.into_iter().filter(|r| r.tipo != "area").collect::<Vec<_>>());
    assert_eq!(usage.iter().filter(|r| r.tipo == "area").map(|r| (r.nome.as_str(), r.input, r.output, r.cache_write, r.cache_read, r.cache_write_1h)).collect::<Vec<_>>(), [("back",6,4,3,2,1), ("front",5,3,2,1,1)]);
    assert!(!ix.sync("synthetic", &[transcript], &forbid_read, "v1", map.signature(), &redo, &Progress::default()).unwrap());
}
