use super::*;

fn config(text: &str) -> Value {
    json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":text}]}]}})
}
fn text(config: &Value) -> &str {
    config["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap()
}

#[test]
fn native_allow_replaces_legacy_wrapper_without_nesting_or_changing_hook_arguments() {
    for windows in [false, true] {
        for old in [
            "rtk hook claude --extra",
            "rtk hook claude --extra | python3 /old/codex-hook-allow.py",
            "\"/old/python\" \"/old/codex-hook-allow.py\" -- rtk hook claude --extra",
            "\"/old/hangar-server\" codex-hook-allow -- rtk hook claude --extra",
        ] {
            let value = hooks(&config(old), "/fixture/hangar-server", "codex-hook-allow", windows);
            assert_eq!(split(text(&value), windows).unwrap(), [
                "/fixture/hangar-server", "codex-hook-allow", "--", "rtk", "hook", "claude", "--extra",
            ], "{old}, windows={windows}");
            assert_eq!(hooks(&value, "/fixture/hangar-server", "codex-hook-allow", windows), value);
        }
    }
}

#[test]
fn native_json_replaces_the_python_wrapper_and_preserves_the_original_shell_expression() {
    for windows in [false, true] {
        for old in [
            "echo fixture && echo done",
            "\"/old/python\" \"/old/codex-hook-json.py\" -- \"echo fixture && echo done\"",
            "\"/old/hangar-server\" codex-hook-json -- \"echo fixture && echo done\"",
        ] {
            let value = security(&config(old), "/fixture/hangar-server", "codex-hook-json", windows).unwrap();
            assert_eq!(split(text(&value), windows).unwrap(), [
                "/fixture/hangar-server", "codex-hook-json", "--", "echo fixture && echo done",
            ], "{old}, windows={windows}");
            assert_eq!(security(&value, "/fixture/hangar-server", "codex-hook-json", windows).unwrap(), value);
        }
    }
}

#[test]
fn installed_windows_hooks_select_package_or_developer_binary_without_nesting() {
    let executable = "C:/Users/D'Ávila/.hangar/current/bin/hangar-server.exe";
    let allow = hooks(&config("rtk hook claude --extra"), executable, "codex-hook-allow", true);
    assert!(text(&allow).starts_with("& $(if (Test-Path"));
    assert_eq!(hooks(&allow, executable, "codex-hook-allow", true), allow);
    let json = security(&config("echo fixture && echo done"), executable, "codex-hook-json", true).unwrap();
    assert_eq!(security(&json, executable, "codex-hook-json", true).unwrap(), json);
    assert_eq!(split(text(&json), true).unwrap()[3], "echo fixture && echo done");
}
