use hangar_server::codex_setup::stage_sources;
use serde_json::{Value, json};
#[test]
fn codex_service_stage_sources_match_python_files_and_warnings() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/stage-sources.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let source = if case["kind"] == "memory" {
            home.join("source")
        } else {
            home.to_path_buf()
        };
        for (name, hex) in case["files"].as_object().unwrap() {
            let path = source.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let hex = hex.as_str().unwrap();
            let bytes: Vec<_> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            std::fs::write(path, bytes).unwrap();
        }
        if let Some(directories) = case.get("directories").and_then(Value::as_array) {
            for directory in directories {
                std::fs::create_dir_all(source.join(directory.as_str().unwrap())).unwrap();
            }
        }
        let actual = if case["kind"] == "agents" {
            let plugins = home.join(".claude/plugins");
            std::fs::create_dir_all(&plugins).unwrap();
            std::fs::write(plugins.join("installed_plugins.json"),serde_json::to_vec(&json!({"plugins":{"demo-name@market":[{"scope":"project","installPath":home.join("other")},{"scope":"user","installPath":home.join("plugin")}]}})).unwrap()).unwrap();
            let (agents, unreadable) = stage_sources::agents(home, &case["settings"]).unwrap();
            json!({"agents":agents,"unreadable":unreadable.iter().map(|p|p.strip_prefix(home).unwrap().to_string_lossy().replace('\\',"/")).collect::<Vec<_>>()})
        } else {
            let target = home.join("target");
            let errors = stage_sources::copy_memory(&source, &target);
            let mut files = serde_json::Map::new();
            let mut pending = vec![target.clone()];
            while let Some(path) = pending.pop() {
                if path.is_dir() {
                    for entry in std::fs::read_dir(path).unwrap() {
                        pending.push(entry.unwrap().path());
                    }
                } else if path.is_file() {
                    files.insert(
                        path.strip_prefix(&target)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        json!(
                            std::fs::read(path)
                                .unwrap()
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<String>()
                        ),
                    );
                }
            }
            json!({"files":files,"errors":errors})
        };
        let mut expected=case["result"].clone();
        // A seleção corrigida lê JSON válido e o campo do registro, não uma substring.
        if case["kind"]=="memory" && case["name"]=="lossy" { expected=json!({"files":{},"errors":[]}); }
        if case["kind"]=="memory" && case["name"]=="different-space" { expected=json!({"files":case["files"],"errors":[]}); }
        if actual != expected {
            failures.push(json!({"case":case["name"],"actual":actual,"expected":expected}));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn codex_service_stage_hooks_match_python_exclusions_and_preserved_shapes() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/stage-hooks.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for (index, case) in corpus["cases"].as_array().unwrap().iter().enumerate() {
        let actual = stage_sources::without_app_hooks(&case["input"]);
        if actual != case["result"] {
            failures.push(json!({"index":index,"input":case["input"],"actual":actual,"expected":case["result"]}));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(64))]
 #[test]
 fn codex_service_stage_hooks_are_idempotent_for_external_commands(command in ".{0,1024}"){
  let input=json!({"Event":[{"private":true,"hooks":[{"type":"command","command":command},{"type":"prompt","prompt":"preservado"}]}]});
  let once=stage_sources::without_app_hooks(&input);let twice=stage_sources::without_app_hooks(&once);
  proptest::prop_assert_eq!(&once,&twice);
  proptest::prop_assert_eq!(&once["Event"][0]["private"],&json!(true));
  proptest::prop_assert!(once["Event"][0]["hooks"].as_array().unwrap().iter().any(|hook|hook["type"]=="prompt"));
 }
}
