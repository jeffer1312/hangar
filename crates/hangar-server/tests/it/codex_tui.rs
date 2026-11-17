//! `hangar-server codex-tui` de ponta a ponta com o `codex` falso (`hangar-codex-fixture`): o mesmo
//! arranjo da captura do lançador Python (casa e pasta temporárias, PATH só com o falso, `.env` do
//! servidor com a sincronização desligada e o controle de atualização em dia).
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Case {
    _root: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
    fake: PathBuf,
    stubs: PathBuf,
}

impl Case {
    fn new(config: Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let (home, work, fake, stubs) = (root.path().join("home"), root.path().join("work"), root.path().join("fake"), root.path().join("stubs"));
        for dir in [&home, &work, &fake, &stubs, &home.join(".hangar/config"), &home.join(".codex")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        crate::copy_executable(Path::new(env!("CARGO_BIN_EXE_hangar-codex-fixture")), &stubs.join("codex"));
        std::fs::write(home.join(".hangar/config/.env"), "# servidor\nCP_AUTH_TOKEN=fake-owner-token\nCP_PORT=1\nCP_CODEX_SYNC_ENABLED=0\n").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
        std::fs::write(home.join(".hangar/codex-atualizacao.json"), json!({"proxima_em": now + 3600.0}).to_string()).unwrap();
        let mut config = config;
        if config["app_server"]["threads"].is_null() {
            config["app_server"]["threads"] = json!([{"id": "thread-1", "path": home.join(".codex/sessions/2026/11/17/rollout-thread-1.jsonl"),
                "cwd": work, "source": "cli", "threadSource": "user"}]);
        }
        std::fs::write(fake.join("config.json"), config.to_string()).unwrap();
        Self { _root: root, home, work, fake, stubs }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
        command.arg("codex-tui").args(args).current_dir(&self.work).env_clear()
            .envs([("HOME", self.home.as_os_str()), ("USERPROFILE", self.home.as_os_str()), ("PATH", self.stubs.as_os_str()),
                ("HANGAR_FAKE_CODEX", self.fake.as_os_str())])
            .envs([("CP_SESSION_NAME", "codex-proj"), ("CP_SESSION_KEY", "key-0001"), ("CP_PROVIDER", "codex"),
                ("TMUX", "/tmp/tmux-1000/default,1234,5"), ("TMUX_PANE", "%7")])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output { self.command(args).output().unwrap() }

    fn spawns(&self) -> Vec<Value> {
        std::fs::read_to_string(self.fake.join("spawn.jsonl")).unwrap_or_default().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    fn role(&self, role: &str) -> Vec<Value> { self.spawns().into_iter().filter(|spawn| spawn["role"] == role).collect() }

    fn sidecar(&self) -> PathBuf { self.home.join(".hangar/codex-sessions/codex-proj.json") }

    fn wait_for(&self, role: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.role(role).is_empty() {
            assert!(Instant::now() < deadline, "{role} não apareceu: {:?}", self.spawns());
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn stderr(output: &Output) -> String { String::from_utf8_lossy(&output.stderr).into_owned() }

fn alive(pid: u64) -> bool { unsafe { libc::kill(pid as libc::pid_t, 0) == 0 } }

fn endpoint(spawn: &Value) -> String {
    let argv = spawn["argv"].as_array().unwrap();
    argv[argv.iter().position(|a| a == "--listen").unwrap() + 1].as_str().unwrap().to_owned()
}

/// Conversa nova: a ordem das linhas, os argv, o arquivo da sessão que a TUI vê (agora com `tui_pid` e
/// `launcher_pid` também na conversa nova) e a limpeza na saída (`launcher__new-session-sync-off-full-args`).
#[test]
fn a_new_session_runs_the_tui_over_its_app_server_and_cleans_up() {
    let case = Case::new(json!({}));
    let output = case.run(&["--cwd", case.work.to_str().unwrap(), "--codex-home", case.home.join(".codex").to_str().unwrap(), "--codex-account",
        "default", "--model", "gpt-6", "--effort", "high", "--approval-policy", "on-request", "--sandbox", "workspace-write",
        "--tool-output-token-limit", "144000", "--service-tier", "priority", "--prompt", "olá, começa", "--config", "model_verbosity=\"low\""]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "hangar-codex-tui: conferindo os plugins…\nhangar-codex-tui: subindo o app-server do Codex…\n");
    let [server] = case.role("codex app-server").try_into().unwrap();
    let endpoint = endpoint(&server);
    assert_eq!(server["argv"], json!(["app-server", "--listen", endpoint, "-c", "sandbox_mode=\"workspace-write\"", "-c", "approval_policy=\"on-request\"",
        "-c", "tool_output_token_limit=144000", "-c", "service_tier=\"priority\"", "-c", "shell_environment_policy.set.CP_SESSION_NAME=\"codex-proj\"",
        "-c", "shell_environment_policy.set.CP_SESSION_KEY=\"key-0001\"", "-c", "shell_environment_policy.set.TMUX=\"/tmp/tmux-1000/default,1234,5\"",
        "-c", "shell_environment_policy.set.TMUX_PANE=\"%7\"", "-c", "model_verbosity=\"low\""]));
    assert_eq!(server["cwd"], json!(case.work));
    assert!(server["env"].get("CP_AUTH_TOKEN").is_none(), "o token do servidor não chega ao Codex");
    let [tui] = case.role("codex tui").try_into().unwrap();
    assert_eq!(tui["argv"], json!(["--remote", endpoint, "--no-alt-screen", "-C", case.work, "--sandbox", "workspace-write", "--ask-for-approval",
        "on-request", "-c", "service_tier=\"priority\"", "-c", "check_for_update_on_startup=false", "-m", "gpt-6", "-c",
        "model_reasoning_effort=\"high\"", "olá, começa"]));
    let [snapshot] = case.role("tui-sidecar-snapshot").try_into().unwrap();
    let meta = &snapshot["sidecar"];
    for (field, value) in [("name", json!("codex-proj")), ("thread_id", json!("thread-1")), ("cwd", json!(case.work)), ("model", json!("gpt-6")),
        ("effort", json!("high")), ("endpoint", json!(endpoint)), ("app_pid", server["pid"].clone()), ("service_tier", json!("priority")),
        ("codex_account", json!("default")), ("key", json!("key-0001")), ("permission_mode", json!("Approve for me")),
        ("tool_output_token_limit", json!(144000)), ("tui_pid", tui["pid"].clone())] {
        assert_eq!(meta[field], value, "{field}: {meta}");
    }
    assert!(meta["launcher_pid"].as_u64().is_some(), "{meta}");
    assert!(!case.sidecar().exists(), "a saída apaga o arquivo desta vida");
    assert!(!alive(server["pid"].as_u64().unwrap()), "o app-server saiu junto com a TUI");
    // Os pedidos ao app-server: a escuta e a confiança nos hooks.
    let rpc: Vec<String> = case.role("app-server-rpc").iter().map(|r| r["msg"]["method"].as_str().unwrap_or("").to_owned()).collect();
    assert!(rpc.iter().filter(|m| *m == "hooks/list").count() == 1 && rpc.iter().any(|m| m == "initialize"), "{rpc:?}");
}

/// O código da TUI é o do lançador (`launcher__tui-exit-code-propagates`).
#[test]
fn the_tui_exit_code_is_the_launcher_exit_code() {
    let case = Case::new(json!({"tui": {"rc": 7}}));
    let output = case.run(&["--cwd", case.work.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
}

/// Sem `codex` no PATH: a frase, a linha no diário e o pane aberto com o Enter (`launcher__codex-absent`;
/// o Python fechava o pane e a sessão sumia da lista).
#[test]
fn without_codex_the_pane_stays_open_with_the_error() {
    let case = Case::new(json!({}));
    std::fs::remove_file(case.stubs.join("codex")).unwrap();
    let output = case.run(&["--cwd", case.work.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "hangar-codex-tui: nao achei o executavel `codex` no PATH deste processo. Instale o Codex CLI (npm install -g \
        @openai/codex) e reabra a sessao.\nPressione Enter para fechar esta sessão.\n");
    let diary = case.home.join(".hangar/logs/diario/codex-tui");
    let lines: Vec<Value> = std::fs::read_dir(&diary).unwrap().flatten()
        .flat_map(|entry| std::fs::read_to_string(entry.path()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect::<Vec<Value>>())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0]["evento"].as_str(), lines[0]["codigo"].as_str(), lines[0]["sessao"].as_str()),
        (Some("codex.app_server.falhou"), Some("criar_processo"), Some("codex-proj")));
}

/// App-server que morre na largada: a cauda do stderr na tela, sem TUI, pane aberto
/// (`launcher__app-server-dies-at-start`).
#[test]
fn an_app_server_that_dies_at_start_shows_why_and_opens_no_tui() {
    let case = Case::new(json!({"app_server": {"launches": [{"die_at_start": 3, "stderr": "linha privada\nerror: unexpected argument '--listen' found\n"}]}}));
    let output = case.run(&["--cwd", case.work.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let [server] = case.role("codex app-server").try_into().unwrap();
    assert_eq!(stderr(&output), format!("hangar-codex-tui: conferindo os plugins…\nhangar-codex-tui: subindo o app-server do Codex…\n\
        hangar-codex-tui: o app-server saiu com codigo 3 antes de abrir {}:\nlinha privada\nerror: unexpected argument '--listen' found\n\
        Pressione Enter para fechar esta sessão.\n", endpoint(&server)));
    assert!(case.role("codex tui").is_empty());
    assert!(std::fs::read_dir(case.home.join(".hangar/logs/privado")).unwrap().count() == 1);
}

/// Queda com a TUI viva: outro app-server na mesma porta, aviso no pane (`launcher__app-server-crash-resubido`).
#[test]
fn an_app_server_that_falls_with_the_tui_alive_comes_back_on_the_same_port() {
    let case = Case::new(json!({"app_server": {"launches": [{"crash_after_tui_ms": 300}, {}]}, "tui": {"hold_ms": 5000}}));
    let output = case.run(&["--cwd", case.work.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let servers = case.role("codex app-server");
    assert_eq!(servers.len(), 2, "{servers:?}");
    let endpoint = endpoint(&servers[0]);
    assert_eq!(endpoint, self::endpoint(&servers[1]), "a mesma porta: a TUI reconecta sozinha");
    let text = stderr(&output);
    assert!(text.contains(&format!("hangar-codex-tui: o app-server saiu com codigo 9 antes de abrir {endpoint}:\nfake app-server crashed\n")), "{text}");
    assert!(text.ends_with(&format!("hangar-codex-tui: app-server ressubido em {endpoint}\n")), "{text}");
    assert_eq!(case.role("codex tui").len(), 1, "a TUI não é relançada");
    assert!(!case.sidecar().exists());
}

/// `SIGTERM` vira 143, derruba o app-server e apaga o arquivo (`launcher__sigterm`).
#[test]
fn sigterm_exits_143_and_takes_the_app_server_down() {
    let case = Case::new(json!({}));
    std::fs::write(case.fake.join("tui-hang"), "").unwrap();
    let mut child = case.command(&["--cwd", case.work.to_str().unwrap()]).spawn().unwrap();
    case.wait_for("tui-sidecar-snapshot");
    assert!(case.sidecar().exists());
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM); }
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(143));
    let [server] = case.role("codex app-server").try_into().unwrap();
    assert!(!alive(server["pid"].as_u64().unwrap()));
    assert!(!case.sidecar().exists());
    let [tui] = case.role("codex tui").try_into().unwrap();
    unsafe { libc::kill(tui["pid"].as_u64().unwrap() as libc::pid_t, libc::SIGKILL); }
}

/// Conta sem a marca do Hangar não abre (o servidor recusaria igual); antes o lançador abria.
#[test]
fn an_account_without_the_hangar_marker_is_refused_before_anything_runs() {
    let case = Case::new(json!({}));
    std::fs::create_dir_all(case.home.join(".codex-x")).unwrap();
    let output = case.run(&["--cwd", case.work.to_str().unwrap(), "--codex-home", case.home.join(".codex-x").to_str().unwrap(), "--codex-account", "x"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "hangar-codex-tui: a conta Codex x não é uma conta do Hangar (codex_account_invalid_marker); nenhuma sessão foi \
        aberta\nPressione Enter para fechar esta sessão.\n");
    assert!(case.spawns().is_empty());
    // Com a marca, a adicional prepara pela API: servidor fora e sem a marca do preparo, não abre.
    std::fs::write(case.home.join(".codex-x/.hangar-codex-conta"), json!({"version": 1, "id": "x"}).to_string()).unwrap();
    let output = case.run(&["--cwd", case.work.to_str().unwrap(), "--codex-home", case.home.join(".codex-x").to_str().unwrap(), "--codex-account", "x"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "hangar-codex-tui: a conta não abriu porque o preparo não foi confirmado (sem conexão com 127.0.0.1:1)\n\
        Pressione Enter para fechar esta sessão.\n");
    assert!(case.spawns().is_empty(), "nada sobe antes do preparo confirmado");
}

/// Recusa de argumento: uso e frase do lançador Python, código 2, pane aberto.
#[test]
fn a_refused_argument_keeps_the_usage_text() {
    let case = Case::new(json!({}));
    let output = case.run(&["--approval-policy", "always"]);
    assert_eq!(output.status.code(), Some(2));
    let wanted = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex_tui/launcher__refuse-bad-approval.json")).unwrap();
    let wanted: Value = serde_json::from_str(&wanted).unwrap();
    assert_eq!(stderr(&output), format!("{}Pressione Enter para fechar esta sessão.\n", wanted["output"]["stderr"]["text"].as_str().unwrap()));
}
