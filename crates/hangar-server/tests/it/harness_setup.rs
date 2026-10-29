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
