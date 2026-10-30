use serde_json::Value;
use std::{ffi::OsString, io::Write, process::{Command, Output, Stdio}};

fn bytes(value: &Value) -> Vec<u8> {
    value.as_str().unwrap().as_bytes().chunks_exact(2).map(|pair| {
        u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
    }).collect()
}

fn invoke(mode: &str, arguments: &[OsString], input: &[u8]) -> Output {
    let binary = std::env::var_os("HANGAR_TEST_SERVER_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hangar-server").into());
    let mut child = Command::new(binary).arg(mode).args(arguments)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn native_allow_command_preserves_the_original_filter_contract() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/hook-allow.json")).unwrap();
    let mut count = 0;
    for row in fixture["cases"].as_array().unwrap().iter().filter(|row| row["mode"] == "filter") {
        let output = invoke("codex-hook-allow", &[], &bytes(&row["input_hex"]));
        assert_eq!(output.status.code().unwrap(), row["code"].as_i64().unwrap() as i32);
        assert_eq!(output.stdout, bytes(&row["stdout_hex"]));
        assert_eq!(output.stderr, bytes(&row["stderr_hex"]));
        count += 1;
    }
    assert_eq!(count, 30);
}

#[cfg(unix)]
#[test]
fn persisted_hook_follows_package_and_developer_switch_with_the_same_command() {
    use std::os::unix::{fs::{PermissionsExt, symlink}};
    let binary = std::env::var_os("HANGAR_TEST_SERVER_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hangar-server").into());
    let home = tempfile::tempdir().unwrap();
    let package = home.path().join(".hangar/versions/package");
    let developer = home.path().join("checkout");
    for root in [&package, &developer] {
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        let script = root.join("scripts/hangar-server-command");
        std::fs::write(&script, include_bytes!("../../../../scripts/hangar-server-command")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for path in [package.join("bin/hangar-server"), home.path().join(".hangar/bin/hangar-server")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(&binary, &path).unwrap();
    }
    let current = home.path().join(".hangar/current");
    let command = current.join("scripts/hangar-server-command");
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/hook-allow.json")).unwrap();
    for target in [&package, &developer] {
        if current.exists() { std::fs::remove_file(&current).unwrap(); }
        symlink(target, &current).unwrap();
        for row in fixture["cases"].as_array().unwrap().iter().filter(|row| row["mode"] == "filter") {
            let mut child = Command::new(&command).arg("codex-hook-allow")
                .env("HOME", home.path()).env_remove("CP_RUST_SERVER_BIN")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(&bytes(&row["input_hex"])).unwrap();
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.code().unwrap(), row["code"].as_i64().unwrap() as i32);
            assert_eq!(output.stdout, bytes(&row["stdout_hex"]));
            assert_eq!(output.stderr, bytes(&row["stderr_hex"]));
        }
    }
}

#[cfg(unix)]
fn quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[cfg(unix)]
#[test]
fn native_json_command_preserves_shell_arguments_streams_and_exit_status() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/hook-json.json")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("entrada");
    let stdout = temp.path().join("saída");
    let script = temp.path().join("hook com espaço.sh");
    for row in fixture["process_cases"].as_array().unwrap() {
        std::fs::write(&input, bytes(&row["input_hex"])).unwrap();
        std::fs::write(&stdout, bytes(&row["output_hex"])).unwrap();
        let outcome = row["outcome"].as_str().unwrap();
        let finish = if outcome == "signal" { "kill -TERM $$".to_owned() } else { format!("exit {outcome}") };
        std::fs::write(&script, format!(
            "[ \"$#\" -eq 3 ] && [ \"$1\" = 'a;b' ] && [ \"$2\" = 'a|b' ] && [ -z \"$3\" ] || exit 99\ncmp - {} || exit 98\ncat {}\nprintf 'aviso\\377\\n' >&2\n{finish}\n",
            quote(&input), quote(&stdout),
        )).unwrap();
        let command = format!("/bin/sh {} 'a;b' 'a|b' ''", quote(&script));
        let output = invoke("codex-hook-json", &["--".into(), command.into()], &bytes(&row["input_hex"]));
        assert_eq!(output.status.code().unwrap(), row["code"].as_i64().unwrap() as i32, "{outcome}");
        assert_eq!(output.stdout, bytes(&row["stdout_hex"]), "{outcome}");
        assert_eq!(output.stderr, bytes(&row["stderr_hex"]), "{outcome}");
    }
}
