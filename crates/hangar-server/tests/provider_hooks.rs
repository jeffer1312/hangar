//! Os hooks rodam como processos, sem Python, servidor ou programas no PATH.
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

fn run(root: &std::path::Path, mode: &str, payload: &[u8], vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command
        .arg(mode)
        .env_clear()
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("LOCALAPPDATA", root.join("local"))
        .env("CLAUDE_CONFIG_DIR", root.join("config"))
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in vars {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn provider_hooks_kimi_turn_permission_and_question_events() {
    let root = tempfile::tempdir().unwrap();
    for (event, tool, expected) in [
        ("UserPromptSubmit", "", "working"),
        ("TurnStarted", "", "working"),
        ("Stop", "", "idle"),
        ("Interrupt", "", "idle"),
        ("PermissionRequest", "", "awaiting_input"),
        ("PreToolUse", "AskUserQuestion", "awaiting_input"),
        ("PostToolUse", "AskUserQuestion", "working"),
    ] {
        let result = run(
            root.path(),
            "kimi-state-hook",
            &serde_json::to_vec(&json!({
                "session_id":"sessão", "hook_event_name":event,"tool_name":tool,"cwd":"/código/ação"
            }))
            .unwrap(),
            &[("TMUX_PANE", "%12")],
        );
        assert!(
            result.status.success(),
            "{event}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
        assert!(
            root.path()
                .join("config/.hangar-state/sessão.json")
                .is_file(),
            "evento {event} precisa publicar o marcador"
        );
        let state: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/.hangar-state/sessão.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["state"], expected);
        assert!(state["ts"].as_f64().unwrap() > 0.0);
        let ticket: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/.hangar-kimi/12.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ticket["session_id"], "sessão");
        assert_eq!(ticket["cwd"], "/código/ação");
    }
}

#[test]
fn provider_hooks_kimi_session_start_only_links_and_psmux_wins() {
    let root = tempfile::tempdir().unwrap();
    for name in ["kimi teste/1", "kimi teste/2"] {
        let result = run(
            root.path(),
            "kimi-state-hook",
            &serde_json::to_vec(&json!({
                "session_id": name,"hook_event_name":"SessionStart"
            }))
            .unwrap(),
            &[("TMUX_PANE", "%1"), ("PSMUX_SESSION", name)],
        );
        assert!(result.status.success());
    }
    assert!(!root.path().join("config/.hangar-state").exists());
    for name in ["kimi-teste-1", "kimi-teste-2"] {
        assert!(
            root.path()
                .join(format!("config/.hangar-kimi/{name}.json"))
                .exists()
        );
    }
    assert!(!root.path().join("config/.hangar-kimi/1.json").exists());
}

#[test]
fn provider_hooks_guard_blocks_both_channels_and_allows_isolated_sockets() {
    let root = tempfile::tempdir().unwrap();
    for script in [
        "tmux kill-server",
        "psmux kill-session -a",
        "bash -lc 'tmux kill-server'",
        "pkill -f 'tm.x'",
        "pkill -f '(?=t)t.*x'",
        "pkill -f '(?P<a>t)mux(?P=a)?'",
        "printf x | xargs tmux",
    ] {
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(2), "{script}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            output["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("probe-x")
        );
        let text = String::from_utf8_lossy(&result.stderr);
        assert!(text.contains("BLOQUEADO"));
        assert!(text.contains("socket próprio"));
        assert!(text.contains("sessões de trabalho"));
        assert!(text.contains("TMUX_TMPDIR não isola"));
    }
    for script in [
        "tmux -L probe-x kill-server",
        "tmux kill-session -t '=alvo:'",
        "git commit -m 'tmux kill-server'",
        "CMD=tmux; ${CMD} kill-server",
        "cat <<'EOF'\ntmux kill-server\nEOF",
    ] {
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert!(result.status.success(), "{script}");
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn provider_hooks_installer_migrates_python_preserves_toml_and_backup() {
    use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::kimi_hooks};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".kimi-code")).unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    let raw = "# Minha configuração\ndefault_model = 'api/ação'\n[providers.x]\nbase_url = 'y'\n[[hooks]]\nevent = 'Stop'\ncommand = 'notifier --message ação'\n[[hooks]]\nevent = 'SessionStart'\ncommand = 'python /antigo/backend/hooks/kimi_state_hook.py' # comentário preservado\ntimeout = 5\n[[hooks]]\nevent = 'PreToolUse'\nmatcher = 'Bash'\ncommand = '''python /antigo/backend/hooks/guard_tmux.py'''\n";
    std::fs::write(&path, raw).unwrap();
    let env = AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    );
    assert_eq!(kimi_hooks::sync(&env, root.path()).unwrap().len(), 1);
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.starts_with(
        "# Minha configuração\ndefault_model = 'api/ação'\n[providers.x]\nbase_url = 'y'\n"
    ));
    assert!(first.contains("command = 'notifier --message ação'"));
    assert!(first.contains("# comentário preservado"));
    assert!(!first.contains("python"));
    assert!(!first.contains(".py"));
    let data: toml::Value = toml::from_str(&first).unwrap();
    assert_eq!(data["hooks"].as_array().unwrap().len(), 10);
    assert_eq!(
        std::fs::read_to_string(root.path().join(".kimi-code/config.toml.bak-hangar")).unwrap(),
        raw
    );
    assert!(kimi_hooks::sync(&env, root.path()).unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
}

#[test]
fn provider_hooks_fail_open_with_private_diagnostics_without_payload() {
    let root = tempfile::tempdir().unwrap();
    for mode in ["kimi-state-hook", "guard-tmux"] {
        for payload in [b"invalid secret-content".as_slice(), b"[]", b"null", b"42"] {
            let result = run(root.path(), mode, payload, &[]);
            assert!(result.status.success());
            assert!(result.stdout.is_empty());
            assert!(result.stderr.is_empty());
        }
        let log_root = if cfg!(windows) {
            root.path().join("local/hangar")
        } else {
            root.path().join(".hangar")
        };
        let log = log_root
            .join("logs/privado")
            .join(if mode == "kimi-state-hook" {
                "kimi_hook_error.log"
            } else {
                "guard_tmux-falhas.log"
            });
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("invalid_json"));
        assert!(text.contains("payload_not_object"));
        assert!(!text.contains("secret-content"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(log).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_hangar-server"))
            .arg(mode)
            .env_clear()
            .env("HOME", root.path())
            .env("USERPROFILE", root.path())
            .env("CLAUDE_CONFIG_DIR", root.path().join("config"))
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"invalid secret-content")
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success() && result.stdout.is_empty() && result.stderr.is_empty());
        let fallback_root = if cfg!(windows) {
            root.path().join("AppData/Local/hangar")
        } else {
            root.path().join(".hangar")
        };
        let filename = if mode == "kimi-state-hook" {
            "kimi_hook_error.log"
        } else {
            "guard_tmux-falhas.log"
        };
        let text =
            std::fs::read_to_string(fallback_root.join("logs/privado").join(filename)).unwrap();
        assert!(text.contains("invalid_json"));
        assert!(!text.contains("secret-content"));
        assert!(!root.path().join("config").join(filename).exists());
    }
}

#[test]
fn provider_hooks_kimi_no_pane_no_ticket_and_ordinary_tool_no_state() {
    let root = tempfile::tempdir().unwrap();
    for event in ["SessionStart", "PreToolUse", "PostToolUse", "Unknown"] {
        assert!(
            run(
                root.path(),
                "kimi-state-hook",
                &serde_json::to_vec(
                    &json!({"session_id":"sid","hook_event_name":event,"tool_name":"Bash"})
                )
                .unwrap(),
                &[]
            )
            .status
            .success()
        );
    }
    assert!(!root.path().join("config/.hangar-state").exists());
    assert!(!root.path().join("config/.hangar-kimi").exists());
}

#[test]
fn provider_hooks_installer_preserves_invalid_inline_and_absent_home() {
    use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::kimi_hooks};
    let root = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    );
    assert!(kimi_hooks::sync(&env, root.path()).unwrap().is_empty());
    let path = root.path().join(".kimi-code/config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for raw in ["[", "hooks = [{event = 'Stop', command = 'foreign'}]\n"] {
        std::fs::write(&path, raw).unwrap();
        assert!(kimi_hooks::sync(&env, root.path()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        assert!(!path.with_file_name("config.toml.bak-hangar").exists());
    }
}

#[test]
fn provider_hooks_kimi_concurrent_processes_publish_complete_json() {
    let root = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..12).map(|i| {
            let root = root.path();
            scope.spawn(move || {
                run(root, "kimi-state-hook", &serde_json::to_vec(&json!({"session_id":"same","hook_event_name":"TurnStarted","cwd":format!("/ação/{i}")})).unwrap(), &[("TMUX_PANE", "%2")])
            })
        }).collect();
        for handle in handles {
            assert!(handle.join().unwrap().status.success());
        }
    });
    for (directory, file) in [(".hangar-state", "same.json"), (".hangar-kimi", "2.json")] {
        let directory = root.path().join("config").join(directory);
        let value: Value =
            serde_json::from_slice(&std::fs::read(directory.join(file)).unwrap()).unwrap();
        assert!(value.is_object());
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(24))]
    #[test]
    fn provider_hooks_external_commands_never_panic(command in ".{0,256}") {
        let root = tempfile::tempdir().unwrap();
        let result = run(root.path(), "guard-tmux", &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":command}})).unwrap(), &[]);
        proptest::prop_assert!(matches!(result.status.code(), Some(0 | 2)));
    }
    #[test]
    fn provider_hooks_external_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)) {
        let root = tempfile::tempdir().unwrap();
        let result = run(root.path(), "kimi-state-hook", &bytes, &[]);
        proptest::prop_assert!(result.status.success());
        proptest::prop_assert!(result.stdout.is_empty());
    }
}

#[test]
fn provider_hooks_pi_guard_installation_replaces_python_and_is_idempotent() {
    use hangar_server::{
        accounts::{AccountService, environment::AccountEnvironment},
        harness_setup::hooks,
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"python /old/.claude/hooks/guard_tmux.py"},{"type":"command","command":"personal --check"}]}]}})).unwrap()).unwrap();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    ));
    hooks::sync_installer(&service, root.path(), "ensure_guard_hooks_installed").unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(!first.contains("python"));
    assert!(!first.contains("guard_tmux.py"));
    assert!(first.contains("guard-tmux"));
    assert!(first.contains("personal --check"));
    hooks::sync_installer(&service, root.path(), "ensure_guard_hooks_installed").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
}

#[test]
fn provider_hooks_guard_preserves_existing_command_positions_and_regex_cases() {
    let root = tempfile::tempdir().unwrap();
    for (script, expected) in [
        ("tmux kill-server", 2),
        ("tmux kill-server 2>/dev/null", 2),
        (
            "which tmux && tmux -V\ntmux kill-server 2>/dev/null\ntmux new-session -d -s 'orig-test'",
            2,
        ),
        ("/usr/bin/tmux kill-server", 2),
        ("tmux kill-session -a", 2),
        ("cd /tmp && tmux kill-server", 2),
        ("tmux kill-session", 2),
        ("pkill tmux", 2),
        ("pkill -f 'tmux server'", 2),
        ("killall tmux", 2),
        ("sudo tmux kill-server", 2),
        ("CP_X=1 tmux kill-server", 2),
        ("echo oi; tmux kill-server", 2),
        ("echo $(tmux kill-server)", 2),
        ("(tmux kill-server)", 2),
        ("eval \"tmux kill-server\"", 2),
        ("bash -c \"tmux kill-server\"", 2),
        ("sh -c \"tmux kill-server\"", 2),
        ("bash -lc \"tmux kill-server\"", 2),
        ("printf \"kill-server\\n\" | xargs tmux", 2),
        ("timeout 5 tmux kill-server", 2),
        ("pkill -f 't*mux'", 2),
        ("pkill -f 'tm.x'", 2),
        ("tmux -L teste kill-server", 0),
        ("tmux -Lteste kill-server", 0),
        ("tmux -S /tmp/sock kill-server", 0),
        ("tmux kill-session -t '=orig-test:'", 0),
        ("tmux -L probe-x kill-session", 0),
        ("pkill -f app.main", 0),
        ("pkill -f 'node .*vite'", 0),
        ("tmux new-session -d -s x", 0),
        ("tmux ls", 0),
        ("echo 'tmux kill-server e proibido'", 0),
        (
            "git commit -m \"explica o incidente: tmux kill-server derrubou tudo\"",
            0,
        ),
        ("grep -rn kill-server backend/", 0),
        (
            "rtk proxy git add x && git commit -m 'bloqueia tmux kill-server'",
            0,
        ),
        (
            "cat > /tmp/doc.md <<'EOF'\nNao rode tmux kill-server nunca.\nEOF",
            0,
        ),
        (
            "echo 'avisa: tmux kill-server e proibido' | hangar-send hangar",
            0,
        ),
        (
            "sed -i 's/tmux kill-server/tmux -L probe kill-server/' script.sh",
            0,
        ),
        ("bash -c \"npm run build\"", 0),
        ("eval \"$(fnm env)\"", 0),
        ("xargs rm < lista.txt", 0),
        ("pkill -f app.main", 0),
        ("pkill -f '(?=t)t.*x'", 2),
        ("pkill -f '(?P<a>t)mux(?P=a)?'", 2),
    ] {
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(expected), "{script}");
        if expected == 2 {
            let output: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
            assert!(String::from_utf8_lossy(&result.stderr).contains("BLOQUEADO"));
        } else {
            assert!(result.stdout.is_empty());
        }
    }
}

#[test]
fn provider_hooks_guard_wrong_command_logs_code_and_ignores_other_tools() {
    let root = tempfile::tempdir().unwrap();
    let payload = serde_json::to_vec(
        &json!({"tool_name":"Bash","tool_input":{"command":{"secret":"never_log_this"}}}),
    )
    .unwrap();
    assert!(
        run(root.path(), "guard-tmux", &payload, &[])
            .status
            .success()
    );
    let log_root = if cfg!(windows) {
        root.path().join("local/hangar")
    } else {
        root.path().join(".hangar")
    };
    let text =
        std::fs::read_to_string(log_root.join("logs/privado/guard_tmux-falhas.log")).unwrap();
    assert!(text.contains("command_not_string"));
    assert!(!text.contains("never_log_this"));
    let payload = serde_json::to_vec(
        &json!({"tool_name":"Read","tool_input":{"command":"tmux kill-server"}}),
    )
    .unwrap();
    let output = run(root.path(), "guard-tmux", &payload, &[]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn provider_hooks_installer_creates_missing_config_with_nine_native_entries() {
    use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::kimi_hooks};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".kimi-code")).unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    let env = AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    );
    for partial in [
        "",
        "[[hooks]]\nevent = 'Stop'\ncommand = '\"/old/bin/hangar-server\" kimi-state-hook'\n",
    ] {
        if !partial.is_empty() {
            std::fs::write(&path, partial).unwrap();
        }
        assert_eq!(kimi_hooks::sync(&env, root.path()).unwrap().len(), 1);
        let raw = std::fs::read_to_string(&path).unwrap();
        let data: toml::Value = toml::from_str(&raw).unwrap();
        let hooks = data["hooks"].as_array().unwrap();
        let mut got: Vec<_> = hooks
            .iter()
            .map(|hook| {
                (
                    hook["event"].as_str().unwrap(),
                    hook.get("matcher").and_then(toml::Value::as_str),
                    hook["command"]
                        .as_str()
                        .unwrap()
                        .split_whitespace()
                        .last()
                        .unwrap(),
                )
            })
            .collect();
        let mut expected = vec![
            ("SessionStart", None, "kimi-state-hook"),
            ("UserPromptSubmit", None, "kimi-state-hook"),
            ("TurnStarted", None, "kimi-state-hook"),
            ("Stop", None, "kimi-state-hook"),
            ("Interrupt", None, "kimi-state-hook"),
            ("PermissionRequest", None, "kimi-state-hook"),
            ("PreToolUse", Some("AskUserQuestion"), "kimi-state-hook"),
            ("PostToolUse", Some("AskUserQuestion"), "kimi-state-hook"),
            ("PreToolUse", Some("Bash"), "guard-tmux"),
        ];
        got.sort();
        expected.sort();
        assert_eq!(got, expected);
        assert_eq!(
            got.iter().filter(|(event, _, _)| *event == "Stop").count(),
            1
        );
        for hook in hooks {
            assert_eq!(hook["timeout"].as_integer(), Some(5));
            assert!(!hook["command"].as_str().unwrap().contains(".py"));
        }
        assert!(kimi_hooks::sync(&env, root.path()).unwrap().is_empty());
    }
}

#[test]
fn provider_hooks_invalid_field_types_preserve_diagnostic_and_ticket_boundaries() {
    for (mode, payload, logged, ticket) in [
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":null}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":null}}),
            false,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":null,"session_id":"sid"}),
            false,
            true,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":false}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":false}}),
            false,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":false,"session_id":"sid"}),
            false,
            true,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":0}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":0}}),
            false,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":0,"session_id":"sid"}),
            false,
            true,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":""}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":""}}),
            false,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":"","session_id":"sid"}),
            false,
            true,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":[]}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":[]}}),
            false,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":[],"session_id":"sid"}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{}}),
            false,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":{}}}),
            false,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":{},"session_id":"sid"}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":["x"]}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":["x"]}}),
            true,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":["x"],"session_id":"sid"}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"x":1}}),
            false,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":{"x":1}}}),
            true,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":{"x":1},"session_id":"sid"}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":42}),
            true,
            false,
        ),
        (
            "guard-tmux",
            json!({"tool_name":"Bash","tool_input":{"command":42}}),
            true,
            false,
        ),
        (
            "kimi-state-hook",
            json!({"hook_event_name":42,"session_id":"sid"}),
            false,
            true,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = run(
            root.path(),
            mode,
            &serde_json::to_vec(&payload).unwrap(),
            &[("TMUX_PANE", "%12")],
        );
        assert!(output.status.success(), "{mode}: {payload}");
        assert!(output.stdout.is_empty());
        let log_root = if cfg!(windows) {
            root.path().join("local/hangar")
        } else {
            root.path().join(".hangar")
        };
        assert_eq!(
            log_root.join("logs/privado").exists(),
            logged,
            "{mode}: {payload}"
        );
        assert_eq!(
            root.path().join("config/.hangar-kimi/12.json").exists(),
            ticket,
            "{mode}: {payload}"
        );
    }
}

#[test]
fn provider_hooks_guard_rejects_non_python_regex_extensions_like_the_previous_hook() {
    let root = tempfile::tempdir().unwrap();
    for (pattern, expected) in [
        (r"t\Kmux", 0),
        (r"(?<first>t)mux", 0),
        (r"t\gmux", 0),
        (r"\Ntmux", 2),
        (r"(?P<first>t)mux", 2),
        (r"(?=t)t.*x", 2),
        (r"[(?<]q|tm.x", 2),
        (r"(?#(?<name>)tm.x", 2),
        ("(?x)tm.x # (?<ignored>", 2),
    ] {
        let script = format!("pkill -f '{pattern}'");
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(expected), "{pattern}");
    }
}

#[test]
fn provider_hooks_installer_repoints_native_hooks_and_keeps_the_first_backup() {
    use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::kimi_hooks};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let raw = "[[hooks]]\nevent = 'Stop'\ncommand = '\"/old/bin/hangar-server\" kimi-state-hook'\n";
    std::fs::write(&path, raw).unwrap();
    let env = AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    );
    assert_eq!(kimi_hooks::sync(&env, root.path()).unwrap().len(), 1);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("/old/"));
    std::fs::write(&path, "default_model = 'changed'\n").unwrap();
    assert_eq!(kimi_hooks::sync(&env, root.path()).unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("config.toml.bak-hangar")).unwrap(),
        raw
    );
}

#[test]
fn provider_hooks_review_empty_quoted_regex_keeps_its_argument() {
    let root = tempfile::tempdir().unwrap();
    for script in [
        "pkill -f ''",
        "pkill -f \"\"",
        "pkill -f ''   ",
        "pkill -f '' && echo done",
    ] {
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(2), "{script}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    }
}

#[test]
fn provider_hooks_review_python_character_classes_keep_literals_and_ranges() {
    let root = tempfile::tempdir().unwrap();
    for (pattern, expected) in [
        ("[t&&x]mux", 2),
        ("[t--z]mux", 0),
        ("[t~~x]mux", 2),
        ("[t||x]mux", 2),
        ("[[t]mux", 2),
        ("[]t]mux", 2),
        ("[t-z]mux", 2),
        ("[z-t]mux", 0),
        (r"[t\-z]mux", 2),
        ("[-t]mux", 2),
        ("[--z]mux", 2),
        ("[t-]mux", 2),
        (r"[\x74&&x]mux", 2),
        (r"[\u0074&&x]mux", 2),
        (r"[\164&&x]mux", 2),
        (r"[\w--z]mux", 0),
        (r"[\d-t]mux", 0),
        (r"[\]t]mux", 2),
        ("(?x)[t&&x]mux # [z--a]", 2),
        ("(?x: [t&&x]mux )", 2),
        ("(?# [z--a])[t&&x]mux", 2),
    ] {
        let script = format!("pkill -f '{pattern}'");
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(expected), "{pattern}");
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(24))]
    #[test]
    fn provider_hooks_review_external_classes_never_panic(chars in proptest::collection::vec(proptest::prelude::any::<char>(), 0..64)) {
        let root = tempfile::tempdir().unwrap();
        let content: String = chars.into_iter().collect();
        let pattern = format!("[{content}]mux");
        let script = format!("pkill -f '{}'", pattern.replace('\'', "'\\''"));
        let result = run(root.path(), "guard-tmux", &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}})).unwrap(), &[]);
        proptest::prop_assert!(matches!(result.status.code(), Some(0 | 2)));
    }
}

#[test]
fn provider_hooks_review_unicode_names_keep_python_spellings_and_aliases() {
    let root = tempfile::tempdir().unwrap();
    for (pattern, expected) in [
        (r"\N{LATIN SMALL LETTER T}mux", 2),
        (r"\N{LATIN SMALL LETTER X}mux", 0),
        (r"\N{THIS NAME DOES NOT EXIST}mux", 0),
        (r"\\N{LATIN SMALL LETTER T}mux", 0),
        (r"[\N{LATIN SMALL LETTER T}]mux", 2),
        (r"\N{latin small letter t}mux", 2),
        (r"\N{latinsmalllettert}mux", 0),
        (r"\N{LATIN_SMALL_LETTER_T}mux", 0),
        (r"(?x)tm.x # \N{THIS NAME DOES NOT EXIST}", 2),
        (r"(?# \N{THIS NAME DOES NOT EXIST})tm.x", 2),
        (r"(?x)tmux\N{SPACE}server", 2),
        (r"[\N{BACKSPACE}t]mux", 2),
        (r"[\N{BS}t]mux", 2),
        (r"[^\N{B S}]mux", 0),
        (r"\N{LEFT SQUARE BRACKET}tmux", 0),
        (r"[^\N{CJK UNIFIED IDEOGRAPH-4E00}]mux", 2),
        (r"[^\N{CJK UNIFIED IDEOGRAPH-04E00}]mux", 0),
        (r"[^\N{CJK UNIFIED IDEOGRAPH-004E00}]mux", 0),
        (r"[^\N{HANGUL SYLLABLE GA}]mux", 2),
    ] {
        let script = format!("pkill -f '{pattern}'");
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}}))
                .unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(expected), "{pattern}");
    }
}
