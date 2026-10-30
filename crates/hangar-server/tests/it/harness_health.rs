//! Resultados capturados das funções Python sobre arquivos isolados.
use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::health};
use serde_json::{Value, json};
#[test]
fn harness_health_configuration_matches_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/harness-health/checks.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let kind = case["kind"].as_str().unwrap();
        if !matches!(
            kind,
            "_mcp" | "_modelo_padrao" | "_hooks_codex" | "_fullscreen" | "_fullscreen_claude"
        ) {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let env = AccountEnvironment::from_pairs([
            ("HOME", root.as_str()),
            ("USERPROFILE", root.as_str()),
        ]);
        for (relative, text) in case["files"].as_object().unwrap() {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text.as_str().unwrap().replace("$ROOT", &root.replace('\\', "/"))).unwrap();
        }
        let cli = case["cli"].as_str().unwrap();
        let result = match kind {
            "_mcp" => health::mcp(&env, cli),
            "_modelo_padrao" => health::default_model(&env, cli),
            "_hooks_codex" => Ok(health::codex_hooks(&env)),
            "_fullscreen_claude" => Ok(health::fullscreen(&env, "claude")),
            _ => Ok(health::fullscreen(&env, cli)),
        };
        let actual = match result {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error":error}),
        };
        if actual != case["expected"] {
            failures.push(format!(
                "{}: {} != {}",
                case["name"], actual, case["expected"]
            ));
        }
        count += 1;
    }
    assert_eq!(count, 40);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn harness_health_external_values_match_python() {
    use hangar_server::orchestration::codec;
    let fixture = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/harness-health/values.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let env = AccountEnvironment::from_pairs([
            ("HOME", root.as_str()),
            ("USERPROFILE", root.as_str()),
        ]);
        for (relative, text) in case["files"].as_object().unwrap() {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text.as_str().unwrap()).unwrap();
        }
        let result = if case["kind"] == "_mcp" {
            health::mcp(&env, case["cli"].as_str().unwrap())
        } else {
            health::default_model(&env, case["cli"].as_str().unwrap())
        };
        match result {
            Ok(value)
                if case["expected"].get("error").is_none() && value == case["expected"]["ok"] =>
            {
                ()
            }
            Err(error) if case["expected"].get("error").is_some() => {
                let expected = match (case["expected"]["error"].as_str(), case["kind"].as_str()) {
                    (Some("UnicodeDecodeError"), _) => "configuração TOML não está em UTF-8",
                    (Some("AttributeError"), Some("_mcp")) => "configuração MCP não é um objeto",
                    (Some("TypeError"), Some("_modelo_padrao")) => "modelo padrão não é texto",
                    other => panic!("classe original sem contrato: {other:?}"),
                };
                if error != expected {
                    failures.push(format!("{}: {error:?} != {expected:?}", case["name"]));
                }
            }
            other => failures.push(format!(
                "{}: {:?} != {}",
                case["name"],
                other,
                codec::dumps(&case["expected"], false)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn harness_health_files_match_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/harness-health/checks.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let kind = case["kind"].as_str().unwrap();
        if !matches!(
            kind,
            "_wrapper" | "_conf_psmux" | "_extensoes" | "_hooks_claude"
        ) {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let mut base: std::collections::BTreeMap<String, String> = [
            ("HOME".into(), root.clone()),
            ("USERPROFILE".into(), root.clone()),
            (
                "PATH".into(),
                home.path().join("bin").to_string_lossy().into_owned(),
            ),
        ]
        .into();
        for (k, v) in case["env"].as_object().unwrap() {
            base.insert(k.clone(), v.as_str().unwrap().replace("$ROOT", &root));
        }
        let env = AccountEnvironment::from_map(base);
        for directory in case["directories"].as_array().unwrap() {
            std::fs::create_dir_all(home.path().join(directory.as_str().unwrap())).unwrap();
        }
        for (relative, text) in case["files"].as_object().unwrap() {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text.as_str().unwrap().replace("$ROOT", &root.replace('\\', "/"))).unwrap();
        }
        for (relative, target) in case["links"].as_object().unwrap() {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let target = target.as_str().unwrap().replace("$ROOT", &root);
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, path).unwrap();
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(target, path).unwrap();
        }
        for program in case["programs"].as_array().unwrap() {
            let path = home.path().join("bin").join(program.as_str().unwrap());
            #[cfg(windows)]
            let path = path.with_extension("cmd");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let cli = case["cli"].as_str().unwrap();
        let actual = match kind {
            "_wrapper" => health::wrapper(&env, cli, case["windows"].as_bool().unwrap()),
            "_conf_psmux" => json!(
                health::psmux_config(&env)
                    .to_string_lossy()
                    .replace(&root, "$ROOT").replace('\\', "/")
            ),
            "_extensoes" => health::extensions(&env, &home.path().join("repo"), cli).unwrap(),
            _ => health::claude_hooks(&home.path().join(".claude")),
        };
        let actual = hangar_server::config_sync::paths::map_strings(&actual, &|text| if text.contains("~/") { text.replace('\\', "/") } else { text.to_owned() });
        if actual != case["expected"]["ok"] {
            failures.push(format!(
                "{}: {} != {}",
                case["name"], actual, case["expected"]["ok"]
            ));
        }
        count += 1;
    }
    assert_eq!(count, 25);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn harness_health_never_rewrites_external_configuration(raw in proptest::collection::vec(proptest::prelude::any::<u8>(),0..4096)) {
        let home=tempfile::tempdir().unwrap();let root=home.path().to_string_lossy().into_owned();
        let env=AccountEnvironment::from_pairs([("HOME",root.as_str()),("USERPROFILE",root.as_str())]);
        let paths=[".claude.json",".claude/settings.json",".codex/config.toml",".codex/hooks.json",".pi/agent/fullscreen-tui.json",".bashrc"];
        for relative in paths {let path=home.path().join(relative);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,&raw).unwrap();}
        for cli in ["claude","codex","pi","omp","kimi"] {
            let _=health::mcp(&env,cli);let _=health::default_model(&env,cli);
            health::fullscreen(&env,cli);health::wrapper(&env,cli,false);
        }
        health::codex_hooks(&env);health::claude_hooks(&home.path().join(".claude"));
        for relative in paths {proptest::prop_assert!(std::fs::read(home.path().join(relative)).unwrap()==raw);}
    }
}

#[test]
fn harness_health_read_types_match_python() {
    use hangar_server::orchestration::codec;
    let fixture = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/harness-health/read-types.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let env = AccountEnvironment::from_pairs([
            ("HOME", root.as_str()),
            ("USERPROFILE", root.as_str()),
        ]);
        for (relative, text) in case["files"].as_object().unwrap() {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = case["files_hex"]
                .get(relative)
                .and_then(Value::as_str)
                .map(|hex| {
                    hex.as_bytes()
                        .chunks_exact(2)
                        .map(|pair| {
                            u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| text.as_str().unwrap().as_bytes().to_vec());
            std::fs::write(path, bytes).unwrap();
        }
        let result = if case["kind"] == "_mcp" {
            health::mcp(&env, case["cli"].as_str().unwrap())
        } else {
            health::default_model(&env, case["cli"].as_str().unwrap())
        };
        match result {
            Ok(value)
                if case["expected"].get("error").is_none() && value == case["expected"]["ok"] =>
            {
                ()
            }
            Err(error) if case["expected"].get("error").is_some() => {
                let expected = match (case["expected"]["error"].as_str(), case["kind"].as_str()) {
                    (Some("UnicodeDecodeError"), _) => "configuração TOML não está em UTF-8",
                    (Some("AttributeError"), Some("_mcp")) => "configuração MCP não é um objeto",
                    (Some("TypeError"), Some("_modelo_padrao")) => "modelo padrão não é texto",
                    other => panic!("classe original sem contrato: {other:?}"),
                };
                if error != expected {
                    failures.push(format!("{}: {error:?} != {expected:?}", case["name"]));
                }
            }
            other => failures.push(format!(
                "{}: {:?} != {}",
                case["name"],
                other,
                codec::dumps(&case["expected"], false)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
