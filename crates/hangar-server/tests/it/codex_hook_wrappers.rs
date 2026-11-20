//! Os filtros dos hooks do Codex (`codex-hook-allow` e `codex-hook-json`): os casos que os testes dos
//! antigos `scripts/codex-hook-*.py` cobriam, agora contra o subcomando e as funções dele.
use hangar_server::codex_setup::hook_wrappers::{complete_allow, normalize_json};
use serde_json::{Value, json};

fn parsed(bytes: &[u8]) -> Value { serde_json::from_slice(bytes).unwrap() }

/// rtk devolve `updatedInput` sem `permissionDecision` quando reescreve só um pedaço de um comando
/// com `;`. O Claude Code aceita; o Codex recusa a reescrita e roda o original.
#[test]
fn allow_completes_the_decision_that_rtk_leaves_out() {
    let out = complete_allow(br#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"pwd; rtk read x"}}}"#);
    let out = parsed(&out);
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
    assert_eq!(out["hookSpecificOutput"]["updatedInput"], json!({"command": "pwd; rtk read x"}));
}

#[test]
fn allow_keeps_a_decision_already_given_and_empty_or_foreign_output() {
    let deny = br#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","updatedInput":{"command":"x"}}}"#;
    assert_eq!(parsed(&complete_allow(deny))["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(complete_allow(b""), b"");
    assert_eq!(complete_allow(b"texto que nao e json\n"), b"texto que nao e json\n");
}

#[test]
fn the_allow_subcommand_filters_stdin_without_a_command() {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server")).arg("codex-hook-allow")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"a; b"}}}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(parsed(&output.stdout)["hookSpecificOutput"]["permissionDecision"], "allow");
}

#[test]
fn async_bootstrap_and_metrics_become_a_valid_answer() {
    let out = normalize_json(b"{\"async\":true,\"asyncTimeout\":180000}\n{\"metrics\":{\"sdk_bootstrap\":0}}\n");
    assert_eq!(parsed(&out), json!({}));
}

#[test]
fn block_and_message_are_kept_and_metrics_dropped() {
    let raw = json!({"metrics": {"vulns_found": 1}, "rewakeSummary": "Revise a alteração",
        "decision": "block", "reason": "Problema encontrado", "systemMessage": "Confira"});
    let out = normalize_json(raw.to_string().as_bytes());
    assert_eq!(parsed(&out), json!({"decision": "block", "reason": "Problema encontrado", "systemMessage": "Confira"}));
}

#[test]
fn unknown_output_or_output_without_telemetry_passes_untouched() {
    for raw in [&b""[..], b"texto", b"\xff", b"null", br#"{"continue":false,"stopReason":"pare"}"#,
        b"{\"decision\":\"block\",\"reason\":\"a\"}\n{\"decision\":\"block\",\"reason\":\"b\"}"] {
        assert_eq!(normalize_json(raw), raw, "{}", String::from_utf8_lossy(raw));
    }
}

#[test]
fn context_and_summary_are_kept() {
    let context = json!({"hookEventName": "SessionStart", "additionalContext": "Instruções"});
    let raw = json!({"metrics": {}, "hookSpecificOutput": context, "rewakeSummary": "Aviso"});
    assert_eq!(parsed(&normalize_json(raw.to_string().as_bytes())), json!({"hookSpecificOutput": context, "systemMessage": "Aviso"}));
}

/// O hook bloqueou: o código de saída e o stderr dele chegam ao Codex como vieram.
#[cfg(unix)]
#[test]
fn the_json_subcommand_keeps_the_exit_code_and_stderr() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("hook com espaço.sh");
    std::fs::write(&script, "#!/bin/sh\n[ \"$(cat)\" = entrada ] || exit 9\n\
        printf '%s' '{\"metrics\":{\"vulns_found\":1},\"decision\":\"block\",\"reason\":\"Problema encontrado\"}'\n\
        printf 'diagnóstico' >&2\nexit 2\n").unwrap();
    let command = format!("/bin/sh '{}'", script.display());
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server")).args(["codex-hook-json", "--", &command])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"entrada").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, "diagnóstico".as_bytes());
    assert_eq!(parsed(&output.stdout), json!({"decision": "block", "reason": "Problema encontrado"}));
}
