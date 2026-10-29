//! Contratos capturados dos instaladores antes da remoção do Python.
use hangar_server::{accounts::environment::AccountEnvironment, harness_setup};
use serde_json::{Value, json};

#[test]
fn hooks_match_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/hooks.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut data = case["initial"].clone();
        let changed = harness_setup::hooks::sync_hook(
            &mut data,
            case["event"].as_str().unwrap(),
            case["command"].as_str(),
            Some("Bash"),
            "state_hook.py",
        );
        assert_eq!(data, case["expected"]);
        assert_eq!(json!(changed), case["changed"]);
    }
}

#[test]
#[cfg(unix)]
fn skills_match_python() {
    use std::{fs, path::Path};
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/skills.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
            ]
            .into(),
        );
        let scenario = case["scenario"].as_str().unwrap();
        for folder in [".pi/agent", ".kimi-code"] {
            fs::create_dir_all(home.join(folder)).unwrap();
        }
        if scenario != "empty" {
            for (folder, text) in [
                (".claude/skills/alpha", "claude"),
                ("repo/skills/beta", "repo"),
            ] {
                fs::create_dir_all(home.join(folder)).unwrap();
                fs::write(home.join(folder).join("SKILL.md"), text).unwrap();
            }
        }
        if scenario == "precedence" {
            for folder in [
                "repo/skills/alpha",
                ".claude/plugins/cache/a/b/2.2/skills/version",
                ".claude/plugins/cache/a/b/2.10/skills/version",
            ] {
                fs::create_dir_all(home.join(folder)).unwrap();
                fs::write(home.join(folder).join("SKILL.md"), "fixture").unwrap();
            }
        }
        for folder in [".pi/agent/skills-bridge", ".kimi-code/skills-bridge"] {
            let bridge = home.join(folder);
            fs::create_dir_all(&bridge).unwrap();
            let link = |name: &str, target: &Path| {
                std::os::unix::fs::symlink(target, bridge.join(name)).unwrap()
            };
            if scenario == "personal" {
                fs::create_dir_all(bridge.join("alpha")).unwrap();
                link("beta", &home.join("personal"));
            }
            if scenario == "stale" {
                link("gone", &home.join(".claude/skills/gone"));
                link("alpha", &home.join(".claude/skills/missing"));
            }
            if scenario == "relative" {
                link(
                    "alpha",
                    Path::new(if folder.starts_with(".pi") {
                        "../../../.claude/skills/alpha"
                    } else {
                        "../../.claude/skills/alpha"
                    }),
                );
            }
        }
        assert_eq!(
            harness_setup::sync_skills(&env, &repo),
            case["expected"],
            "{scenario}"
        );
        assert_eq!(
            harness_setup::sync_skills(&env, &repo),
            case["again"],
            "repetido {scenario}"
        );
        for (path, target) in case["links"].as_object().unwrap() {
            let actual = fs::read_link(home.join(path))
                .unwrap()
                .to_string_lossy()
                .replace(home.to_str().unwrap(), "$ROOT");
            assert_eq!(actual, target.as_str().unwrap(), "{scenario}:{path}");
        }
    }
}

#[test]
fn kimi_hooks_match_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/kimi-hooks.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let repo = home.join("repo");
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
                ("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned()),
            ]
            .into(),
        );
        if let Some(initial) = case["initial"].as_str() {
            std::fs::write(home.join("config.toml"), initial).unwrap();
        }
        let actual = harness_setup::kimi_hooks::sync(&env, &repo).unwrap_or_default();
        let normalize = |v: Value| -> Value {
            serde_json::from_str(&v.to_string().replace(home.to_str().unwrap(), "$ROOT")).unwrap()
        };
        assert_eq!(normalize(json!(actual)), case["expected"]);
        assert_eq!(
            normalize(json!(
                harness_setup::kimi_hooks::sync(&env, &repo).unwrap_or_default()
            )),
            case["again"]
        );
        for (name, expected) in case["files"].as_object().unwrap() {
            let actual = std::fs::read_to_string(home.join(name))
                .unwrap()
                .replace(home.to_str().unwrap(), "$ROOT");
            let expected = expected.as_str().unwrap();
            match (
                toml::from_str::<Value>(&actual),
                toml::from_str::<Value>(expected),
            ) {
                (Ok(a), Ok(b)) => assert_eq!(a, b),
                _ => assert_eq!(actual, expected),
            }
        }
    }
}

#[test]
fn codex_hooks_match_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/codex-hooks.json"
    ))
    .unwrap();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let repo = home.join("repo");
        if let Some(initial) = case["initial"].as_str() {
            std::fs::write(home.join("hooks.json"), initial).unwrap();
        }
        let windows = case["windows"].as_bool().unwrap();
        let result = harness_setup::codex_hooks::sync(home, &repo, windows).unwrap();
        assert_eq!(json!(result), case["expected"], "caso {index}");
        assert_eq!(
            json!(harness_setup::codex_hooks::sync(home, &repo, windows).unwrap()),
            case["again"],
            "repetição {index}"
        );
        let actual = std::fs::read_to_string(home.join("hooks.json"))
            .ok()
            .map(|s| s.replace(home.to_str().unwrap(), "$ROOT"));
        match (
            actual
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok()),
            case["content"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok()),
        ) {
            (Some(a), Some(b)) => assert_eq!(a, b, "conteúdo {index}"),
            _ => assert_eq!(json!(actual), case["content"], "conteúdo {index}"),
        }
    }
}

#[test]
#[cfg(unix)]
fn persona_matches_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/persona.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        std::fs::create_dir_all(home.join(".claude/plugins")).unwrap();
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            case["installed"]
                .to_string()
                .replace("$ROOT", home.to_str().unwrap()),
        )
        .unwrap();
        let scenario = case["scenario"].as_str().unwrap();
        if scenario != "missing_persona" {
            std::fs::write(home.join(".claude/CLAUDE.md"), "fixture").unwrap();
        }
        if scenario != "empty" {
            for path in [".pi/agent", ".kimi-code"] {
                std::fs::create_dir_all(home.join(path)).unwrap();
            }
            for path in [".pi/agent/CLAUDE.md", ".kimi-code/AGENTS.md"] {
                if scenario == "personal" {
                    std::fs::write(home.join(path), "pessoal").unwrap();
                }
                if scenario == "link" {
                    std::os::unix::fs::symlink(home.join("anterior"), home.join(path)).unwrap();
                }
            }
        }
        let lines = harness_setup::persona::sync(home).unwrap();
        assert_eq!(
            format!("{}\n", lines.join("\n")),
            case["stdout"],
            "{scenario}"
        );
        for (path, target) in case["links"].as_object().unwrap() {
            assert_eq!(
                std::fs::read_link(home.join(path))
                    .unwrap()
                    .to_string_lossy()
                    .replace(home.to_str().unwrap(), "$ROOT"),
                target.as_str().unwrap(),
                "{scenario}:{path}"
            );
        }
        if scenario == "personal" {
            assert_eq!(
                std::fs::read_to_string(home.join(".pi/agent/CLAUDE.md")).unwrap(),
                "pessoal"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn harness_setup_cli_runs_without_python_or_server() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let repo = root.path().join("repo");
    for path in [
        home.join(".claude/skills/fixture"),
        home.join(".pi/agent"),
        repo.join("backend"),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(home.join(".claude/skills/fixture/SKILL.md"), "fixture").unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .args(["harness-setup", "skills", "--repo"])
        .arg(&repo)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["pi"]["criados"],
        1
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".pi/agent/skills-bridge/fixture/SKILL.md")).unwrap(),
        "fixture"
    );
    assert!(!home.join(".hangar/session-specs").exists());
}

#[test]
#[cfg(unix)]
fn skills_unreadable_source_never_prunes_other_sources() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    for path in [
        ".claude/skills/protected",
        "repo/skills/visible",
        ".pi/agent/skills-bridge",
    ] {
        std::fs::create_dir_all(home.join(path)).unwrap();
    }
    for path in [
        ".claude/skills/protected/SKILL.md",
        "repo/skills/visible/SKILL.md",
    ] {
        std::fs::write(home.join(path), "fixture").unwrap();
    }
    let link = home.join(".pi/agent/skills-bridge/protected");
    std::os::unix::fs::symlink(home.join(".claude/skills/protected"), &link).unwrap();
    let source = home.join(".claude/skills");
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0)).unwrap();
    let env =
        AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned())].into());
    let result = harness_setup::sync_skills(&env, &home.join("repo"));
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        link.is_symlink(),
        "varredura incompleta não pode podar o link legítimo"
    );
    assert!(
        result
            .as_object()
            .unwrap()
            .values()
            .any(|v| v.get("erro").is_some()),
        "erro de fonte precisa chegar ao chamador"
    );
}

#[test]
#[cfg(unix)]
fn hook_tokens_keep_shell_escapes_and_unbalanced_quotes() {
    for command in [
        r"python /opt/Hangar\ App/backend/hooks/kimi_state_hook.py",
        "python '/broken quote /opt/kimi_state_hook.py",
    ] {
        assert!(
            harness_setup::hooks::refers(
                &json!(command),
                if command.contains("broken") {
                    "/opt/kimi_state_hook.py"
                } else {
                    "/opt/Hangar App/backend/hooks/kimi_state_hook.py"
                },
                false
            ),
            "{command}"
        );
    }
}

#[test]
fn skills_warn_when_harness_does_not_load_bridge() {
    #[derive(Clone)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    for dir in [".claude/skills/fixture", ".pi/agent", ".kimi-code"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::fs::write(home.join(".claude/skills/fixture/SKILL.md"), "fixture").unwrap();
    std::fs::write(home.join(".pi/agent/settings.json"), "{\"skills\":[]}").unwrap();
    std::fs::write(
        home.join(".kimi-code/config.toml"),
        "extra_skill_dirs = []\n",
    )
    .unwrap();
    let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let env =
        AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned())].into());
    tracing::subscriber::with_default(subscriber, || {
        harness_setup::sync_skills(&env, &home.join("repo"));
    });
    let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(output.contains("não está na config do pi"), "{output}");
    assert!(output.contains("não está na config do kimi"), "{output}");
    assert_eq!(
        std::fs::read_to_string(home.join(".pi/agent/settings.json")).unwrap(),
        "{\"skills\":[]}"
    );
}

#[test]
fn hook_token_platforms_match_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/hook-tokens.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        assert_eq!(
            json!(harness_setup::hooks::refers_platform(
                &case["command"],
                case["script"].as_str().unwrap(),
                case["basename"].as_bool().unwrap(),
                case["windows"].as_bool().unwrap()
            )),
            case["expected"],
            "{case}"
        );
    }
}

#[test]
fn bridge_configuration_expands_home_and_variables_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("SKILL_HOME".into(), home.to_string_lossy().into_owned()),
        ]
        .into(),
    );
    for target in ["pi", "kimi"] {
        let (relative, file, key) = if target == "pi" {
            (".pi/agent", "settings.json", "skills")
        } else {
            (".kimi-code", "config.toml", "extra_skill_dirs")
        };
        std::fs::create_dir_all(home.join(relative)).unwrap();
        let prefixes = if cfg!(windows) {
            vec!["~", "$HOME", "${SKILL_HOME}", "%SKILL_HOME%"]
        } else {
            vec!["~", "$HOME", "${SKILL_HOME}"]
        };
        for prefix in prefixes {
            let path = format!("{prefix}/{relative}/unused/../skills-bridge");
            let raw = if target == "pi" {
                json!({key:[path]}).to_string()
            } else {
                format!("{key} = [{}]\n", json!(path))
            };
            let config = home.join(relative).join(file);
            std::fs::write(&config, &raw).unwrap();
            assert_eq!(
                harness_setup::skills::bridge_configured(&env, target).unwrap(),
                Some(true),
                "{target}:{prefix}"
            );
            assert_eq!(std::fs::read_to_string(config).unwrap(), raw);
        }
    }
}

#[test]
fn skills_configuration_matches_python_review_cases() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/skills-review.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("SKILL_HOME".into(), home.to_string_lossy().into_owned()),
            ]
            .into(),
        );
        let target = case["target"].as_str().unwrap();
        let (relative, file, key) = if target == "pi" {
            (".pi/agent", "settings.json", "skills")
        } else {
            (".kimi-code", "config.toml", "extra_skill_dirs")
        };
        std::fs::create_dir_all(home.join(relative)).unwrap();
        let entries = case["prefix"]
            .as_str()
            .map(|prefix| vec![format!("{prefix}/{relative}/unused/../skills-bridge")])
            .unwrap_or_default();
        let raw = if target == "pi" {
            json!({key:entries}).to_string()
        } else {
            format!("{key} = {}\n", json!(entries))
        };
        let path = home.join(relative).join(file);
        std::fs::write(&path, &raw).unwrap();
        assert_eq!(
            json!(harness_setup::skills::bridge_configured(&env, target).unwrap()),
            case["configured"]
        );
        assert_eq!(
            json!(std::fs::read_to_string(path).unwrap() == raw),
            case["unchanged"]
        );
    }
}
