//! Instaladores de hooks e skills dos harnesses.
use hangar_server::{accounts::environment::AccountEnvironment, harness_setup};
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;

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
fn hooks_keep_partial_success_when_one_provider_fails() {
    use hangar_server::accounts::AccountService;
    use std::fs;
    let root = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                root.path().to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    );
    fs::create_dir_all(root.path().join(".kimi-code")).unwrap();
    fs::write(root.path().join(".kimi-code/config.toml"), "[").unwrap();
    fs::create_dir_all(&env.codex_default).unwrap();
    let service = AccountService::new(env);
    let result = harness_setup::sync_hooks(&service, &root.path().join("repo"));
    assert_eq!(result["kimi"], json!([]));
    assert!(result.get("errors").is_none());
    assert!(!result["codex"].as_array().unwrap().is_empty());
    assert!(service.env.codex_default.join("hooks.json").is_file());
}
