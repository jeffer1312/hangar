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
fn provider_hooks_guard_blocks_reading_vault_files_but_not_passing_the_path() {
    let root = tempfile::tempdir().unwrap();
    let linux = "/run/user/1000/hangar/vault-files/abc";
    let windows = r"C:\Users\eu\AppData\Local\hangar\vault-files\abc";
    let guard = |payload: Value| {
        run(root.path(), "guard-tmux", &serde_json::to_vec(&payload).unwrap(), &[("XDG_RUNTIME_DIR", "/run/user/1000")])
    };
    let bash = |command: String| json!({"tool_name":"Bash","tool_input":{"command":command}});
    for payload in [
        bash(format!("cat {linux}")),
        bash(format!("wc -c < {linux}")),
        bash(format!("sha256sum {linux} | cut -c1-8")),
        bash(format!("bash -c 'head -c 3 {linux}'")),
        bash(format!("Get-Content {windows}")),
        bash(format!("powershell -c \"[IO.File]::ReadAllText('{windows}').Length\"")),
        bash(r#"powershell -c "gc C:\\Users\\eu\\AppData\\Local\\hangar\\vault-files\\abc""#.into()),
        bash(format!("< {linux} cat")),
        bash(format!("echo $(<{linux})")),
        bash(format!("mapfile -t x < {linux}")),
        bash(format!(": <<< x; cat {linux}")),
        bash(format!("git commit -m \"$(cat <<'EOF'\nmsg\nEOF\n)\" && cat {linux}")),
        bash(format!("Select-String -Path {linux} -Pattern .")),
        bash(format!("grep -o -f {linux} alvo.txt")),
        bash(r"powershell -c 'sls . C:\Users\eu\AppData\Local\hangar\vault-files\abc'".into()),
        bash(format!("cmd /c type {windows}")),
        bash(format!("powershell -NoProfile -ExecutionPolicy Bypass -Command \"gc {windows}\"")),
        bash(format!("bash -o pipefail -c 'cat {linux}'")),
        bash(format!("bash <<'EOF'\ncat {linux}\nEOF")),
        bash(format!("powershell -c \"[IO.File]::ReadLines('{windows}') | Measure-Object -Character\"")),
        json!({"tool_name":"PowerShell","tool_input":{"command":format!("Get-Content {windows}")}}),
        bash("cd /run/user/1000/hangar/vault-files && cat *".into()),
        bash("cd /run/user/1000/hangar && wc -c vault-files/abc".into()),
        json!({"tool_name":"Read","tool_input":{"file_path":linux}}),
        json!({"tool_name":"Read","tool_input":{"file_path":"../vault-files/abc"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":r"C:\Users\eu\AppData\Local\hangar\vault-files"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/run/user/1000/hangar"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/run/user/1000"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","glob":"**/vault-files/*"}}),
    ] {
        let result = guard(payload.clone());
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("vault_file") && reason.contains("bytes"), "{reason}");
        assert!(!reason.contains("probe-x"), "{reason}");
    }
    for payload in [
        bash(format!("mysql --defaults-extra-file={linux} -e 'select 1'")),
        bash(format!("curl --netrc-file {windows} https://exemplo.com")),
        bash(format!("ls -l {linux}")),
        bash("cat README.md".into()),
        bash("cat docs/vault-files-notes.md".into()),
        bash(r#"rg -n "hangar/vault-files" crates"#.into()),
        bash("grep -rn 'vault-files' docs/".into()),
        bash("git commit -m \"$(cat <<'EOF'\nfix: guard hangar/vault-files\nEOF\n)\"".into()),
        bash("cat > notas.md <<'EOF'\n- Read the vault-files folder\nEOF".into()),
        bash("git commit -m \"guard: [IO.File]::ReadAllText em hangar/vault-files\"".into()),
        json!({"tool_name":"Read","tool_input":{"file_path":"/home/eu/notas.txt"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/home/eu"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/run"}}),
    ] {
        let result = guard(payload.clone());
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// Pasta temporária com espaço no nome: o caminho só chega inteiro ao guarda entre aspas.
fn spaced_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("meu projeto ").tempdir().unwrap()
}

/// O caminho como uma palavra só do shell, entre aspas simples.
fn sq(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// Uma cópia do Hangar de mentira (o guarda a reconhece por `crates/hangar-server`) e outro projeto
/// com a mesma `backend/`.
fn checkouts(root: &std::path::Path) -> (String, String) {
    let hangar = root.join("hangar");
    std::fs::create_dir_all(hangar.join("crates/hangar-server")).unwrap();
    std::fs::create_dir_all(hangar.join("backend")).unwrap();
    let other = root.join("outro");
    std::fs::create_dir_all(other.join("backend")).unwrap();
    (hangar.to_string_lossy().into_owned(), other.to_string_lossy().into_owned())
}

/// O payload com a pasta do agente, como o Claude Code manda.
fn in_dir(payload: &Value, cwd: &str) -> Value {
    let mut payload = payload.clone();
    payload["cwd"] = json!(cwd);
    payload
}

/// R6 do envio pelo WhatsApp: a sessão não lê as chaves do dono nem responde ao rascunho no lugar dele.
#[test]
fn provider_hooks_guard_blocks_the_owner_keys_and_the_draft_answer_routes() {
    let root = spaced_tempdir();
    let (hangar, _) = checkouts(root.path());
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(&in_dir(payload, &hangar)).unwrap(), &[]);
    let bash = |command: &str| json!({"tool_name":"Bash","tool_input":{"command":command}});
    let denied = |payload: &Value| -> String {
        let result = guard(payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny", "{payload}");
        output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_owned()
    };
    for payload in [
        bash("cat ~/.hangar/whatsapp/control.key"),
        bash("xxd /home/eu/.hangar/whatsapp/control.key"),
        bash("cat /home/eu/.hangar/config/.env"),
        bash("grep CP_AUTH_TOKEN ~/.hangar/config/.env"),
        bash("source backend/.env && echo $CP_AUTH_TOKEN"),
        bash(&format!(". {}", sq(&format!("{hangar}/backend/.env")))),
        bash("TOKEN=$(sed -n 's/^CP_AUTH_TOKEN=//p' backend/.env)"),
        bash("cat < ~/.hangar/config/.env"),
        bash("cd ~/.hangar/whatsapp && cat control.key"),
        bash("cd backend && cat .env"),
        bash("bash -c 'cat ~/.hangar/config/.env'"),
        json!({"tool_name":"PowerShell","tool_input":{"command":r"Get-Content C:\Users\eu\.hangar\config\.env"}}),
        json!({"tool_name":"Read","tool_input":{"file_path":"/home/eu/.hangar/whatsapp/control.key"}}),
        json!({"tool_name":"Read","tool_input":{"file_path":"/home/eu/.hangar/config/.env"}}),
        json!({"tool_name":"Read","tool_input":{"file_path":format!("{hangar}/backend/.env")}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"TOKEN","path":"/home/eu/.hangar/config"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/home/eu/.hangar/whatsapp"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/home/eu/.hangar"}}),
    ] {
        let reason = denied(&payload);
        assert!(reason.contains("control.key") && reason.contains("whatsapp_draft"), "{payload}: {reason}");
    }
    for payload in [
        bash("curl -X POST -H 'Authorization: Bearer x' http://127.0.0.1:8765/api/whatsapp/outbox/d1/send -d '{\"via\":\"desktop\"}'"),
        bash("curl -s -X POST \"$BASE/api/whatsapp/outbox/$ID/decline\""),
        bash("wget --post-data='' http://127.0.0.1:8765/api/whatsapp/outbox/d1/send"),
        bash("python3 -c \"import urllib.request as u; u.urlopen('http://127.0.0.1:8765/api/whatsapp/outbox/d1/send', b'{}')\""),
        bash("bash -c 'curl -X POST localhost:8765/api/whatsapp/outbox/d1/send'"),
        bash("git log -1 && curl -X POST localhost:8765/api/whatsapp/outbox/d1/send"),
        bash("git commit -m \"$(curl -X POST localhost:8765/api/whatsapp/outbox/d1/send)\""),
        json!({"tool_name":"PowerShell","tool_input":{"command":"Invoke-RestMethod -Method Post http://127.0.0.1:8765/api/whatsapp/outbox/d1/decline"}}),
    ] {
        let reason = denied(&payload);
        assert!(reason.contains("/api/whatsapp/outbox/") && reason.contains("dono"), "{payload}: {reason}");
    }
    for payload in [
        bash("cat README.md"),
        bash("cat backend/.env.example"),
        bash("ls -la ~/.hangar/whatsapp"),
        bash("cd backend && uv run pytest tests/test_x.py"),
        bash("cd backend && cat pyproject.toml"),
        bash(r#"rg -n "backend/.env" crates"#),
        bash("grep -rn kill-server backend/"),
        bash("curl -s http://127.0.0.1:8765/api/whatsapp/outbox"),
        bash("curl -s http://127.0.0.1:8765/api/whatsapp/outbox/d1"),
        bash(r#"rg -n "/api/whatsapp/outbox/{id}/send" crates"#),
        bash("git commit -m \"$(cat <<'EOF'\nfeat: guard /api/whatsapp/outbox/x/send\nEOF\n)\""),
        json!({"tool_name":"Read","tool_input":{"file_path":"/home/eu/.hangar/whatsapp/media/foto.jpg"}}),
        json!({"tool_name":"Read","tool_input":{"file_path":"/home/eu/Projetos/hangar/backend/.env.example"}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"x","path":"/home/eu/Projetos/hangar"}}),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// `.` e `..` no caminho não escondem as chaves do dono: o guarda resolve o caminho antes de comparar.
#[test]
fn provider_hooks_guard_resolves_dot_segments_before_matching_the_owner_keys() {
    let root = spaced_tempdir();
    let (hangar, _) = checkouts(root.path());
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(&in_dir(payload, &hangar)).unwrap(), &[]);
    let bash = |command: &str| json!({"tool_name":"Bash","tool_input":{"command":command}});
    let read = |path: &str| json!({"tool_name":"Read","tool_input":{"file_path":path}});
    for payload in [
        read("/home/eu/.hangar/config/./.env"),
        read("/home/eu/.hangar/config/../config/.env"),
        read(&format!("{hangar}/backend/../backend/.env")),
        read("backend/../backend/.env"),
        read("/home/eu/.hangar/whatsapp/./control.key"),
        json!({"tool_name":"Grep","tool_input":{"pattern":"TOKEN","path":"/home/eu/.hangar/config/."}}),
        json!({"tool_name":"Grep","tool_input":{"pattern":"TOKEN","path":"/home/eu/.hangar/x/../config"}}),
        bash("cat /home/eu/.hangar/config/./.env"),
        bash("cat /home/eu/.hangar/config/../config/.env"),
        bash("grep TOKEN backend/./.env"),
        bash("cd /home/eu/.hangar/config/. && cat .env"),
        bash("cd /home/eu/.hangar/x/../config && cat .env"),
        bash("cd backend/. && cat .env"),
    ] {
        let result = guard(&payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny", "{payload}");
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("control.key"), "{payload}: {reason}");
    }
    for payload in [
        read("backend/.env.example"),
        read("backend/./.env.example"),
        read("/home/eu/.hangar/config/../whatsapp/media/foto.jpg"),
        bash("cat backend/../backend/.env.example"),
        bash("cd backend/. && cat pyproject.toml"),
        bash(". ./scripts/env.sh"),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// O ´backend/.env´ só é chave do dono numa cópia do Hangar (checkout ou a anotada pelo servidor): noutro
/// projeto o arquivo, a pasta ´backend´ e o ´*´ dentro dela são livres. Sem ´cwd´ a pasta é desconhecida e o
/// relativo conta como cópia do Hangar. As chaves de ´~/.hangar´ valem em toda pasta.
#[test]
fn provider_hooks_guard_anchors_backend_env_to_the_hangar_copy() {
    let root = spaced_tempdir();
    let (hangar, other) = checkouts(root.path());
    let installed = root.path().join("instalado");
    std::fs::create_dir_all(installed.join("backend")).unwrap();
    std::fs::write(installed.join("manifest.json"), "{}").unwrap();
    std::fs::create_dir_all(root.path().join(".hangar")).unwrap();
    std::fs::write(root.path().join(".hangar/install-root-dir"), installed.to_string_lossy().as_bytes()).unwrap();
    let installed = installed.to_string_lossy().into_owned();
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(payload).unwrap(), &[]);
    let bash = |command: &str, cwd: &str| json!({"tool_name":"Bash","cwd":cwd,"tool_input":{"command":command}});
    let read = |path: &str, cwd: &str| json!({"tool_name":"Read","cwd":cwd,"tool_input":{"file_path":path}});
    for payload in [
        bash("cp backend/.env.example backend/.env", &other),
        bash("cd backend && grep -rn TODO *", &other),
        bash("cat backend/.env", &other),
        bash("source backend/.env", &other),
        bash(&format!("cat {}", sq(&format!("{other}/backend/.env"))), &hangar),
        read("backend/.env", &other),
        read(&format!("{other}/backend/.env"), &hangar),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
    for payload in [
        bash("cat backend/.env", &hangar),
        bash("cd backend && grep -rn TODO *", &hangar),
        bash(&format!("cat {}", sq(&format!("{hangar}/backend/.env"))), &other),
        bash(&format!("cat {}", sq(&format!("{installed}/backend/.env"))), &other),
        read("backend/.env", &hangar),
        bash("cat ~/.hangar/config/.env", &other),
        bash("cat ~/.hangar/whatsapp/control.key", &other),
        bash("cd ~/.hangar/config && grep -rn TODO *", &other),
        json!({"tool_name":"Bash","tool_input":{"command":"cat ~/.hangar/config/.env"}}),
        json!({"tool_name":"Bash","tool_input":{"command":"cat backend/.env"}}),
        json!({"tool_name":"Bash","tool_input":{"command":"cd backend && cat .env"}}),
        json!({"tool_name":"Read","tool_input":{"file_path":"backend/.env"}}),
    ] {
        let result = guard(&payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("control.key"), "{payload}: {reason}");
    }
}

/// O `cd` do próprio comando muda a pasta em que o `backend/.env` é procurado; `cd` que não dá para seguir
/// (variável, subshell, pipeline) deixa a pasta desconhecida, e desconhecida conta como cópia do Hangar.
#[test]
fn provider_hooks_guard_follows_the_command_cd_and_fails_closed_on_an_unknown_folder() {
    let root = spaced_tempdir();
    let (hangar, other) = checkouts(root.path());
    let (h, o) = (sq(&hangar), sq(&other));
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(payload).unwrap(), &[]);
    let bash = |command: &str, cwd: &str| json!({"tool_name":"Bash","cwd":cwd,"tool_input":{"command":command}});
    let pwsh = |command: &str, cwd: &str| json!({"tool_name":"PowerShell","cwd":cwd,"tool_input":{"command":command}});
    for payload in [
        bash(&format!("cd {h} && cat backend/.env"), &other),
        bash("cd hangar && cat backend/.env", root.path().to_str().unwrap()),
        bash("cd ~/hangar && cat backend/.env", &other),
        bash("cd crates && cat ../backend/.env", &hangar),
        bash("cd crates && cd ../backend && cat .env", &hangar),
        bash("cd crates/hangar-server && cd ../.. && cat backend/.env", &hangar),
        bash("cd backend/sub && cd .. && cat .env", &hangar),
        // Dentro do `bash -c "..."` o caminho vai entre aspas simples.
        bash(&format!("bash -c \"cd {h} && cat backend/.env\""), &other),
        bash(&format!("cd {o}; cd {h}; cat backend/.env"), &other),
        bash("cd $DIR && cat backend/.env", &other),
        bash("cd \"$(git rev-parse --show-toplevel)\" && cat backend/.env", &other),
        bash("cd - && cat backend/.env", &other),
        bash(&format!("(cd {o}); cat backend/.env"), &hangar),
        bash(&format!("cd {o} | true; cat backend/.env"), &hangar),
        bash("cat .env", &format!("{hangar}/backend")),
        pwsh(&format!("Set-Location {h}; Get-Content backend\\.env"), &other),
    ] {
        let result = guard(&payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("control.key"), "{payload}: {reason}");
    }
    for payload in [
        bash("cp backend/.env.example backend/.env", &other),
        bash("cd backend && grep -rn TODO *", &other),
        bash("cd backend && grep -rn TODO * | head", &other),
        bash("cd backend && cat .env.example", &other),
        bash("cd crates && cat ../backend/.env", &other),
        bash(&format!("cd {o} && cat backend/.env"), &hangar),
        bash("cd backend && cat pyproject.toml", &hangar),
        bash("cat .env", &format!("{other}/backend")),
        bash("cat *", &format!("{other}/backend")),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// Aspas, barra de escape e curinga de shell não escondem as chaves do dono nem a rota do rascunho: o
/// guarda lê o comando como o shell o entrega e casa o curinga com o nome protegido.
#[test]
fn provider_hooks_guard_sees_owner_keys_through_quotes_escapes_and_globs() {
    let root = tempfile::tempdir().unwrap();
    let (_, other) = checkouts(root.path());
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(payload).unwrap(), &[]);
    let bash = |command: &str| json!({"tool_name":"Bash","cwd":other,"tool_input":{"command":command}});
    let reason_of = |payload: &Value| -> String {
        let result = guard(payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_owned()
    };
    for payload in [
        bash("cat ~/.hangar/whatsapp/control.ke?"),
        bash("cat ~/.hangar/config/.en?"),
        bash("cat ~/.hangar/config/.e[n]v"),
        bash("cat ~/.hangar/config/{peers.json,.env}"),
        bash("cat ~/.hangar/conf*/.env"),
        bash("cat ~/.hangar/config/*"),
        bash("cat ~/.hangar/config/.e\"n\"v"),
        bash(r"cat ~/.hangar/config/.e\nv"),
        bash("cat ~/.hangar/whatsapp/'control'.key"),
        bash("cd ~/.hangar/whatsapp && cat control.ke?"),
        json!({"tool_name":"PowerShell","tool_input":{"command":r"Get-Content C:\Users\eu\.hangar\config\.en?"}}),
    ] {
        let reason = reason_of(&payload);
        assert!(reason.contains("control.key"), "{payload}: {reason}");
    }
    for payload in [
        bash("curl -X POST http://127.0.0.1:8765/api/whatsapp/outbox/$ID/\"send\""),
        bash(r"curl -X POST http://127.0.0.1:8765/api/whatsapp/outbox/d1/se\nd"),
        bash("curl -X POST 'http://127.0.0.1:8765/api/whatsapp/outbox/d1/{send,x}'"),
        bash("curl -X POST http://127.0.0.1:8765/api/whatsapp/outbox/d1/dec'line'"),
    ] {
        let reason = reason_of(&payload);
        assert!(reason.contains("/api/whatsapp/outbox/"), "{payload}: {reason}");
    }
    for payload in [
        bash("cat ~/.hangar/config/peers.json"),
        bash("cat ~/.hangar/config/.env.example"),
        bash("cat ~/.hangar/whatsapp/media/*.jpg"),
        bash("cat */*/*"),
        bash("wc -l */*"),
        bash("ls ~/.hangar/config/*"),
        bash("curl -s http://127.0.0.1:8765/api/whatsapp/outbox/d1"),
        bash("curl -s 'http://127.0.0.1:8765/api/whatsapp/outbox/d1/media'"),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// `file_path` e `path` do `Read`/`Grep` são um caminho inteiro: o espaço do nome da pasta não o parte e
/// `?`/`*` são letras do nome; curinga só no `glob`. No Bash, o caminho entre aspas também chega inteiro.
#[test]
fn provider_hooks_guard_takes_read_and_grep_paths_whole_and_literal() {
    let root = spaced_tempdir();
    let (hangar, other) = checkouts(root.path());
    assert!(hangar.contains(' '), "{hangar}");
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(payload).unwrap(), &[]);
    let read = |path: &str, cwd: &str| json!({"tool_name":"Read","cwd":cwd,"tool_input":{"file_path":path}});
    let grep = |field: &str, value: &str, cwd: &str| {
        let mut payload = json!({"tool_name":"Grep","cwd":cwd,"tool_input":{"pattern":"TOKEN"}});
        payload["tool_input"][field] = json!(value);
        payload
    };
    let bash = |command: &str, cwd: &str| json!({"tool_name":"Bash","cwd":cwd,"tool_input":{"command":command}});
    let hangar_env = format!("{hangar}/backend/.env");
    let other_env = format!("{other}/backend/.env");
    for payload in [
        read(&hangar_env, &other),
        grep("path", &hangar_env, &other),
        grep("glob", "**/.hangar/config/.en?", &other),
        bash(&format!("cat {}", sq(&hangar_env)), &other),
        bash(&format!("cat {}/backend/.env", sq(&hangar)), &other),
        bash(&format!("wc -c < {}", sq(&hangar_env)), &other),
        bash(&format!("grep -f {} alvo.txt", sq(&hangar_env)), &other),
        bash(&format!("cd {} && cat .env", sq(&format!("{hangar}/backend"))), &other),
    ] {
        let result = guard(&payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("control.key"), "{payload}: {reason}");
    }
    for payload in [
        read(&other_env, &hangar),
        read("~/.hangar/config/.e?v", &other),
        read("/home/eu/.hangar/whatsapp/control.ke*", &other),
        grep("path", "/home/eu/.hangar/config/.e?v", &other),
        bash(&format!("cat {}", sq(&other_env)), &hangar),
        bash(&format!("rg -n TOKEN {}", sq(&format!("{other}/backend"))), &hangar),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// O `cd` só leva a pasta quando o resto depende dele (`&&`): depois de `;`, nova linha, `||` ou `&` ele pode ter
/// falhado e o shell seguir na de antes. Pipeline, fundo ou subshell só tiram a pasta do `cd` que está neles, e
/// um segundo `cd` confirmado refaz a conta de estar ou não na pasta da chave.
#[test]
fn provider_hooks_guard_cd_moves_the_folder_only_when_the_rest_depends_on_it() {
    let root = spaced_tempdir();
    let (hangar, other) = checkouts(root.path());
    let (h, o) = (sq(&hangar), sq(&other));
    let guard = |payload: &Value| run(root.path(), "guard-tmux", &serde_json::to_vec(payload).unwrap(), &[]);
    let bash = |command: &str, cwd: &str| json!({"tool_name":"Bash","cwd":cwd,"tool_input":{"command":command}});
    for payload in [
        bash(&format!("cd {o}; cat backend/.env"), &hangar),
        bash(&format!("cd {o}\ncat backend/.env"), &hangar),
        bash(&format!("cd {o} || cat backend/.env"), &hangar),
        bash(&format!("cd {o} && true; cat backend/.env"), &hangar),
        bash(&format!("cd {o} && true || cat backend/.env"), &hangar),
        bash(&format!("cd {o} && true & cat backend/.env"), &hangar),
        bash(&format!("cd {o} & cat backend/.env"), &hangar),
        bash(&format!("true | cd {o} && cat backend/.env"), &hangar),
        bash(&format!("true || cd {o} && cat backend/.env"), &hangar),
        bash(&format!("cd {o} && cd {h}/backend && cat .env"), &other),
    ] {
        let result = guard(&payload);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("control.key"), "{payload}: {reason}");
    }
    for payload in [
        bash(&format!("cd {o} && cat backend/.env | head"), &hangar),
        bash("cd ../outro && cat backend/.env | head", &hangar),
        bash(&format!("cd {o} && cat backend/.env; ls"), &hangar),
        bash(&format!("cd backend && cd {o}/backend && cat .env"), &hangar),
        bash(&format!("cd backend && cd {o}/backend && cat *"), &hangar),
    ] {
        let result = guard(&payload);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
    }
}

/// Nome cheio de `{` não prende o guarda: a expansão das chaves tem teto, e passar dele conta como casamento
/// possível, que barra.
#[test]
fn provider_hooks_guard_brace_expansion_has_a_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let braces = "{".repeat(200_000);
    let payload = json!({"tool_name":"Bash","tool_input":{"command":format!("cat ~/.hangar/config/{braces}")}});
    let result = run(root.path(), "guard-tmux", &serde_json::to_vec(&payload).unwrap(), &[]);
    assert_eq!(result.status.code(), Some(2), "{}", String::from_utf8_lossy(&result.stderr));
    let output: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("control.key"));
    let fine = json!({"tool_name":"Bash","tool_input":{"command":"cat ~/.hangar/config/{a,b}{c,d}{e,f}"}});
    let result = run(root.path(), "guard-tmux", &serde_json::to_vec(&fine).unwrap(), &[]);
    assert!(result.status.success() && result.stdout.is_empty(), "{fine}");
}

/// A ação do rascunho é o trecho inteiro da URL: `send.html` é outro caminho, e `send?`, `send#` e `send/`
/// continuam a rota.
#[test]
fn provider_hooks_guard_draft_route_needs_the_whole_action_segment() {
    let root = tempfile::tempdir().unwrap();
    let bash = |command: &str| json!({"tool_name":"Bash","tool_input":{"command":command}});
    for payload in [
        bash("curl -X POST 'http://127.0.0.1:8765/api/whatsapp/outbox/d1/send?via=desktop'"),
        bash("curl -X POST http://127.0.0.1:8765/api/whatsapp/outbox/d1/decline#x"),
        bash("curl -X POST http://127.0.0.1:8765/api/whatsapp/outbox/d1/send/"),
        bash("curl -X POST 'http://127.0.0.1:8765/api/whatsapp/outbox/d1/{send,x}'"),
    ] {
        let result = run(root.path(), "guard-tmux", &serde_json::to_vec(&payload).unwrap(), &[]);
        assert_eq!(result.status.code(), Some(2), "{payload}");
        let output: Value = serde_json::from_slice(&result.stdout).unwrap();
        let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("/api/whatsapp/outbox/"), "{payload}: {reason}");
    }
    for payload in [
        bash("curl -s http://127.0.0.1:8765/api/whatsapp/outbox/exemplo/send.html"),
        bash("curl -s 'http://127.0.0.1:8765/api/whatsapp/outbox/exemplo/decline.json'"),
        bash("curl -s 'http://127.0.0.1:8765/api/whatsapp/outbox/exemplo/{send,x}.html'"),
    ] {
        let result = run(root.path(), "guard-tmux", &serde_json::to_vec(&payload).unwrap(), &[]);
        assert!(result.status.success(), "{payload}");
        assert!(result.stdout.is_empty(), "{payload}");
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
    assert_eq!(data["hooks"].as_array().unwrap().len(), 9);
    assert_eq!(
        std::fs::read_to_string(root.path().join(".kimi-code/config.toml.bak-hangar")).unwrap(),
        raw
    );
    assert!(kimi_hooks::sync(&env, root.path()).unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
}

/// O Kimi Code recusa gravar a configuração com evento que ele não conhece; o `TurnStarted` que o
/// Hangar instalava sai na subida, e o resto do arquivo fica.
#[test]
fn provider_hooks_kimi_repair_drops_the_retired_turn_started_hook() {
    use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::kimi_hooks};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".kimi-code")).unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    let raw = "default_model = 'k3'\n\n# hangar: integração da sessão.\n[[hooks]]\nevent = 'TurnStarted'\ncommand = 'python /antigo/backend/hooks/kimi_state_hook.py'\ntimeout = 5\n\n[[hooks]]\nevent = 'TurnStarted'\ncommand = 'notifier --pessoal'\n";
    std::fs::write(&path, raw).unwrap();
    let env = AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    );
    assert_eq!(kimi_hooks::repair(&env).unwrap().len(), 1);
    let fixed = std::fs::read_to_string(&path).unwrap();
    assert_eq!(fixed, "default_model = 'k3'\n\n[[hooks]]\nevent = 'TurnStarted'\ncommand = 'notifier --pessoal'\n");
    assert!(kimi_hooks::repair(&env).unwrap().is_empty());
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
                run(root, "kimi-state-hook", &serde_json::to_vec(&json!({"session_id":"same","hook_event_name":"UserPromptSubmit","cwd":format!("/ação/{i}")})).unwrap(), &[("TMUX_PANE", "%2")])
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
fn provider_hooks_installer_creates_missing_config_with_eight_native_entries() {
    use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::kimi_hooks};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".kimi-code")).unwrap();
    let path = root.path().join(".kimi-code").join("config.toml");
    let env = AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    );
    for partial in [
        "",
        "[[hooks]]\nevent = 'Stop'\ncommand = 'python /antigo/backend/hooks/kimi_state_hook.py'\ntimeout = 5\n",
    ] {
        if !partial.is_empty() {
            std::fs::write(&path, partial).unwrap();
        }
        assert_eq!(
            kimi_hooks::sync(&env, root.path()).unwrap(),
            vec![path.to_string_lossy().into_owned()]
        );
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

/// O caminho do Windows é conferido com `/` também entre aspas e depois do `&` do
/// PowerShell; escape de padrão do `pkill -f` não vira caminho.
#[test]
fn guard_reads_quoted_windows_paths_but_leaves_pattern_escapes() {
    let root = tempfile::tempdir().unwrap();
    for (script, expected) in [
        (r#""C:\Program Files\psmux\tmux.exe" kill-server"#, 2),
        (r#"& 'C:\tools\tmux.exe' kill-server"#, 2),
        (r"C:\tools\tmux.exe kill-server", 2),
        (r"pkill -f '[\d-t]mux'", 0),
    ] {
        let result = run(
            root.path(),
            "guard-tmux",
            &serde_json::to_vec(&json!({"tool_name":"Bash","tool_input":{"command":script}})).unwrap(),
            &[],
        );
        assert_eq!(result.status.code(), Some(expected), "{script}");
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

/// As capturas do `guard_tmux.py`, pelo binário (que o Codex também usa). A decisão (passa/barra) é a mesma; barrar = `deny` no stdout, motivo com `BLOQUEADO`
/// e `probe-x` no stderr, código 2; passar = 0 sem saída. O texto do motivo e o log são os do Rust.
/// `tmux.exe` é o tmux (`extra-windows-path`): sem isso o guarda deixava passar o comando.
#[test]
fn h48_h53_guard_matches_the_python_captures_and_bars_tmux_exe() {
    use base64::Engine;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/guard_contract");
    let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
    names.sort();
    assert_eq!(names.len(), 86);
    for path in names {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let fixture: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mut locked = Vec::new();
        for (relative, entry) in fixture["input"]["files_before"].as_object().into_iter().flatten() {
            if entry["type"] == "dir" {
                std::fs::create_dir_all(root.path().join(relative)).unwrap();
                locked.push(root.path().join(relative));
            }
        }
        if name == "guard__log-dir-not-writable" {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(home.join(".hangar"), std::fs::Permissions::from_mode(0o500)).unwrap();
            }
        }
        let steps: Vec<Value> = fixture.get("steps").and_then(Value::as_array).cloned().unwrap_or_else(|| vec![fixture["output"].clone()]);
        for (index, step) in steps.iter().enumerate() {
            let stdin = if step.get("stdin").is_some() { &step["stdin"] } else { &fixture["input"]["stdin"] };
            let payload = match stdin.get("text").and_then(Value::as_str) {
                Some(text) => text.as_bytes().to_vec(),
                None => base64::engine::general_purpose::STANDARD.decode(stdin["b64"].as_str().unwrap_or_default()).unwrap(),
            };
            let mut command = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
            command.arg("guard-tmux").env_clear().env("HOME", &home).env("USERPROFILE", &home)
                .env("LOCALAPPDATA", root.path().join("local")).env("PATH", "")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            child.stdin.take().unwrap().write_all(&payload).unwrap();
            let result = child.wait_with_output().unwrap();
            let expected = if name == "guard__extra-windows-path" { 2 } else { step["code"].as_i64().unwrap() as i32 };
            assert_eq!(result.status.code(), Some(expected), "{name}[{index}]");
            if expected == 2 {
                let out: Value = serde_json::from_slice(&result.stdout).unwrap();
                assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{name}");
                let reason = String::from_utf8_lossy(&result.stderr);
                assert!(reason.contains("BLOQUEADO") && reason.contains("probe-x"), "{name}: {reason}");
            } else {
                assert!(result.stdout.is_empty() && result.stderr.is_empty(), "{name}[{index}]");
            }
        }
        // Falha aberta deixa a linha no log quando o Python também deixava (o texto é o código).
        let python_logged = fixture.to_string().contains("guard_tmux-falhas.log");
        if python_logged && name != "guard__log-dir-not-writable" {
            // Logs da máquina: `%LOCALAPPDATA%\hangar\logs` no Windows, `~/.hangar/logs` no resto.
            let logs = if cfg!(windows) { root.path().join("local").join("hangar") } else { home.join(".hangar") };
            assert!(logs.join("logs").join("privado").join("guard_tmux-falhas.log").is_file(), "{name}");
        }
        #[cfg(unix)]
        for dir in locked {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        }
    }
}

/// As capturas `preview__concurrent-8-*` (8 processos da mesma mensagem
/// ao mesmo tempo, 20 vezes, na ordem de largada de cada repetição) repetidas com processos de
/// verdade: cada processo sai 0 calado, o sidecar final é o da captura (`ts` por presença) e ao lado
/// dele só fica a trava (os temporários com pid não sobram).
#[test]
fn h17_h21_concurrent_preview_processes_match_the_captures() {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/../hangar-agent-hooks/tests/fixtures/contract");
    for name in ["preview__concurrent-8-in-order", "preview__concurrent-8-shuffled"] {
        let fixture: Value = serde_json::from_slice(&std::fs::read(format!("{fixtures}/{name}.json")).unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        let sidecar = config.join(".hangar-preview/abc.json");
        for run in fixture["runs"].as_array().unwrap() {
            let _ = std::fs::remove_file(&sidecar);
            let mut children = Vec::new();
            for index in run["start_order"].as_array().unwrap() {
                let index = index.as_u64().unwrap();
                let child = Command::new(env!("CARGO_BIN_EXE_hangar-server"))
                    .arg("preview-hook").env_clear().env("HOME", root.path()).env("USERPROFILE", root.path())
                    .env("CLAUDE_CONFIG_DIR", &config).env("PATH", "")
                    .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                children.push((index, child));
            }
            // A entrada só vai depois de todos nascidos: a corrida pela trava é de verdade.
            for (index, child) in &mut children {
                let event = json!({"hook_event_name": "MessageDisplay", "session_id": "sess-cmdline",
                    "transcript_path": "/home/u/.claude/projects/-home-u-proj/abc.jsonl", "cwd": "/home/u/proj",
                    "message_id": "msg_01", "index": *index, "final": false, "delta": format!("[{index}]")});
                let mut stdin = child.stdin.take().unwrap();
                stdin.write_all(&serde_json::to_vec(&event).unwrap()).unwrap();
            }
            for (index, child) in children {
                let out = child.wait_with_output().unwrap();
                assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty(), "{name} {index}: {out:?}");
            }
            let mut got: Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            let mut want = run["sidecar"].clone();
            assert!(got["ts"].is_number());
            got["ts"] = Value::Null;
            want["ts"] = Value::Null;
            assert_eq!(got, want, "{name} rep {}", run["rep"]);
            let mut others: Vec<String> = std::fs::read_dir(sidecar.parent().unwrap()).unwrap().flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != "abc.json").collect();
            others.sort();
            assert_eq!(serde_json::json!(others), run["other_files"], "{name} rep {}", run["rep"]);
        }
    }
}

/// O `nav-hook` lê os BYTES do stdin e decodifica UTF-8 com troca, sem depender do encoding do
/// sistema (cp1252 no Windows tirava a dica); bytes inválidos noutro campo não impedem "página" de casar.
#[test]
fn d05_nav_hook_reads_stdin_bytes_as_utf8() {
    let root = tempfile::tempdir().unwrap();
    let nav = root.path().join(".hangar/nav");
    std::fs::create_dir_all(&nav).unwrap();
    std::fs::write(nav.join("_srv.json"), format!("{{\"pid\": {}}}", std::process::id())).unwrap();
    let mut payload = b"{\"hook_event_name\": \"UserPromptSubmit\", \"cwd\": \"/c\xe1\xff\", \"prompt\": \"verifica a ".to_vec();
    payload.extend_from_slice("página nova do cadastro agora\"}".as_bytes());
    let result = run(root.path(), "nav-hook", &payload, &[("LANG", "C"), ("PYTHONIOENCODING", "cp1252")]);
    assert!(result.status.success());
    let out: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("navegador embutido"), "{out}");
}


const AGY_CONV: &str = "76f42045-fea3-4d76-880b-9bafad9a794f";

fn agy_payload(event: &str, extra: Value) -> Vec<u8> {
    let mut value = json!({
        "conversationId": AGY_CONV, "hookEventName": event, "cwd": "/tmp/agy-prova",
        "transcriptPath": format!("/h/.gemini/antigravity-cli/brain/{AGY_CONV}/.system_generated/logs/transcript_full.jsonl"),
    });
    for (k, v) in extra.as_object().unwrap() { value[k] = v.clone(); }
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn provider_hooks_agy_state_and_ticket_answer_an_empty_object() {
    let root = tempfile::tempdir().unwrap();
    let started = run(root.path(), "agy-state-hook", &agy_payload("SessionStart", json!({})), &[("TMUX_PANE", "%7")]);
    assert!(started.status.success());
    assert!(!root.path().join(format!("config/.hangar-state/{AGY_CONV}.json")).exists(), "SessionStart não marca trabalhando");
    assert!(root.path().join("config/.hangar-agy/7.json").is_file(), "SessionStart grava o bilhete");
    for (event, extra, expected) in [
        ("PreInvocation", json!({}), Some("working")),
        ("Stop", json!({"fullyIdle": false}), Some("working")),
        ("Stop", json!({"fullyIdle": true}), Some("idle")),
    ] {
        let result = run(root.path(), "agy-state-hook", &agy_payload(event, extra), &[("TMUX_PANE", "%7")]);
        assert!(result.status.success(), "{event}: {}", String::from_utf8_lossy(&result.stderr));
        // O agy espera um objeto JSON; `{}` não injeta nada (no PreToolUse ele bloquearia: lá a guarda responde `decision`).
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "{}");
        let state: Value = serde_json::from_slice(&std::fs::read(root.path().join(format!("config/.hangar-state/{AGY_CONV}.json"))).unwrap()).unwrap();
        assert_eq!(state["state"].as_str(), expected, "{event}");
    }
    let ticket: Value = serde_json::from_slice(&std::fs::read(root.path().join("config/.hangar-agy/7.json")).unwrap()).unwrap();
    assert_eq!(ticket["session_id"], AGY_CONV);
    assert_eq!(ticket["cwd"], "/tmp/agy-prova");
    assert!(ticket["transcript_path"].as_str().unwrap().ends_with("transcript_full.jsonl"));
}

#[test]
fn provider_hooks_agy_without_a_pane_writes_nothing_even_with_a_session_key() {
    // O `agy acp` sem terminal também roda os hooks e herda o CP_SESSION_KEY de quem o lançou.
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), "agy-state-hook", &agy_payload("PreInvocation", json!({})), &[("CP_SESSION_KEY", "abc")]);
    assert!(result.status.success());
    assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "{}");
    assert!(!root.path().join("config/.hangar-state").exists());
    assert!(!root.path().join("config/.hangar-agy").exists());
}

#[test]
fn provider_hooks_agy_refuses_a_conversation_id_that_is_not_a_uuid() {
    let root = tempfile::tempdir().unwrap();
    let payload = serde_json::to_vec(&json!({"conversationId": "../../etc", "hookEventName": "Stop", "fullyIdle": true})).unwrap();
    let result = run(root.path(), "agy-state-hook", &payload, &[("TMUX_PANE", "%7")]);
    assert!(result.status.success(), "falha do Hangar nunca prende o agente");
    assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "{}");
    assert!(!root.path().join("config/.hangar-state").exists());
}


#[test]
fn provider_hooks_agy_session_start_injects_the_sandbox_note_and_the_group_protocol() {
    let root = tempfile::tempdir().unwrap();
    // Sem tmux no teste: o nome vem do sidecar sem terminal pela chave, como no `pair-context`.
    std::fs::create_dir_all(root.path().join(".hangar/claude-headless")).unwrap();
    std::fs::write(root.path().join(".hangar/claude-headless/filha.json"), r#"{"key":"abc","name":"filha"}"#).unwrap();
    std::fs::create_dir_all(root.path().join("config/.hangar-pair")).unwrap();
    std::fs::write(root.path().join("config/.hangar-pair/filha.json"), r#"{"peers":["pai"],"gid":"g1","task":"T-1 teste"}"#).unwrap();
    let result = run(root.path(), "agy-state-hook", &agy_payload("SessionStart", json!({})), &[("CP_SESSION_KEY", "abc")]);
    assert!(result.status.success());
    let out: Value = serde_json::from_slice(&result.stdout).unwrap();
    let steps: Vec<&str> = out["injectSteps"].as_array().unwrap().iter()
        .map(|step| step["systemMessage"]["systemMessage"].as_str().unwrap()).collect();
    assert_eq!(steps.len(), 2, "{out}");
    assert!(steps[0].contains("BypassSandbox"), "{}", steps[0]);
    assert!(steps[1].contains("Você, 'filha', está num grupo de trabalho na tarefa: T-1 teste"), "{}", steps[1]);
    assert!(steps[1].contains("pela tool `send` do MCP hangar"), "o agy tem o MCP agora: {}", steps[1]);
    // Fora de sessão do Hangar nada é injetado.
    let alone = run(root.path(), "agy-state-hook", &agy_payload("SessionStart", json!({})), &[]);
    assert_eq!(String::from_utf8_lossy(&alone.stdout).trim(), "{}");
    // As outras chamadas do turno não repetem nada.
    let later = run(root.path(), "agy-state-hook", &agy_payload("PreInvocation", json!({"invocationNum": 1, "lastUserInput": "abre a página"})), &[("CP_SESSION_KEY", "abc")]);
    assert_eq!(String::from_utf8_lossy(&later.stdout).trim(), "{}");
}

fn agy_tool(name: &str, args: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"conversationId": AGY_CONV, "hookEventName": "PreToolUse", "toolCall": {"name": name, "args": args}})).unwrap()
}

#[test]
fn provider_hooks_agy_guard_always_answers_a_decision() {
    let root = tempfile::tempdir().unwrap();
    let decide = |payload: &[u8]| {
        let result = run(root.path(), "agy-guard-tmux", payload, &[]);
        assert!(result.status.success(), "saída diferente de zero barra a ferramenta no agy");
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    };
    let killed = decide(&agy_tool("run_command", json!({"CommandLine": "tmux kill-server"})));
    assert_eq!(killed["decision"], "deny");
    assert!(killed["reason"].as_str().unwrap().starts_with("BLOQUEADO:"));
    assert_eq!(decide(&agy_tool("run_command", json!({"CommandLine": "echo ok"}))), json!({"decision": "allow"}));
    assert_eq!(decide(&agy_tool("view_file", json!({"AbsolutePath": "/run/user/1/hangar/vault-files/x"})))["decision"], "deny");
    assert_eq!(decide(&agy_tool("view_file", json!({"AbsolutePath": "/tmp/x.txt"}))), json!({"decision": "allow"}));
    assert_eq!(decide(&agy_tool("list_dir", json!({"DirectoryPath": "/"}))), json!({"decision": "allow"}));
    // Falha da guarda deixa passar: `{}` bloquearia toda ferramenta.
    assert_eq!(decide(b"{ quebrado"), json!({"decision": "allow"}));
}

/// No `agy` a pasta do `run_command` vem em `toolCall.args.Cwd`: o `backend/.env` é procurado nela.
#[test]
fn provider_hooks_agy_guard_runs_the_command_in_the_run_command_cwd() {
    let root = spaced_tempdir();
    let (hangar, other) = checkouts(root.path());
    let decide = |args: Value| {
        let result = run(root.path(), "agy-guard-tmux", &agy_tool("run_command", args), &[]);
        assert!(result.status.success());
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    };
    let in_hangar = decide(json!({"CommandLine": "cat backend/.env", "Cwd": hangar}));
    assert_eq!(in_hangar["decision"], "deny", "{in_hangar}");
    assert!(in_hangar["reason"].as_str().unwrap().contains("control.key"), "{in_hangar}");
    assert_eq!(decide(json!({"CommandLine": "cat backend/.env", "Cwd": other})), json!({"decision": "allow"}));
    // Sem pasta nenhuma o relativo conta como cópia do Hangar.
    assert_eq!(decide(json!({"CommandLine": "cat backend/.env"}))["decision"], "deny");
}
