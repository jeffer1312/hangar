//! `hangar-server codex-tui` no Windows com o `codex` falso: Ctrl+Break (e Ctrl+C) fazem a limpeza que o
//! `SIGINT` faz no Unix. O Python limpava pelo `KeyboardInterrupt` + `finally`; sem tratador, o processo
//! morria e o arquivo da sessão ficava.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CTRL_BREAK_EVENT: u32 = 1;

unsafe extern "system" {
    fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
}

fn spawns(fake: &Path) -> Vec<Value> {
    std::fs::read_to_string(fake.join("spawn.jsonl")).unwrap_or_default().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

struct Case {
    _root: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
    fake: PathBuf,
    stubs: PathBuf,
}

impl Case {
    fn new(tui: Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let (home, work, fake, stubs) = (root.path().join("home"), root.path().join("work"), root.path().join("fake"), root.path().join("stubs"));
        for dir in [&home, &work, &fake, &stubs, &home.join(".hangar/config"), &home.join(".codex")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        crate::copy_executable(Path::new(env!("CARGO_BIN_EXE_hangar-codex-fixture")), &stubs.join("codex.exe"));
        std::fs::write(home.join(".hangar/config/.env"), "CP_AUTH_TOKEN=fake-owner-token\nCP_PORT=1\nCP_CODEX_SYNC_ENABLED=0\n").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
        std::fs::write(home.join(".hangar/codex-atualizacao.json"), json!({"proxima_em": now + 3600.0}).to_string()).unwrap();
        let threads = json!([{"id": "thread-1", "path": home.join(".codex/sessions/2026/11/17/rollout-thread-1.jsonl"), "cwd": work,
            "source": "cli", "threadSource": "user"}]);
        std::fs::write(fake.join("config.json"), json!({"app_server": {"threads": threads}, "tui": tui}).to_string()).unwrap();
        Self { _root: root, home, work, fake, stubs }
    }

    fn command(&self) -> Command {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let mut command = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
        command.arg("codex-tui").arg("--cwd").arg(&self.work).current_dir(&self.work).env_clear()
            .envs([("USERPROFILE", self.home.as_os_str()), ("HOME", self.home.as_os_str()), ("PATH", self.stubs.as_os_str()),
                ("HANGAR_FAKE_CODEX", self.fake.as_os_str()), ("SystemRoot", system_root.as_os_str())])
            .envs([("PATHEXT", ".EXE;.CMD"), ("CP_SESSION_NAME", "codex-proj"), ("CP_SESSION_KEY", "key-0001"), ("CP_PROVIDER", "codex")])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command
    }

    fn sidecar(&self) -> PathBuf { self.home.join(".hangar/codex-sessions/codex-proj.json") }
}

/// PR-01: aberto com `Path` e `SystemRoot` (a caixa que o Windows dá), o subcomando sobe as chaves como o
/// `os.environ` do Python: o app-server e a TUI recebem `PATH` e `SYSTEMROOT`.
#[test]
fn subcommands_hand_children_uppercase_environment_keys() {
    let case = Case::new(json!({}));
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut command = case.command();
    command.env_clear()
        .envs([("USERPROFILE", case.home.as_os_str()), ("HOME", case.home.as_os_str()), ("Path", case.stubs.as_os_str()),
            ("HANGAR_FAKE_CODEX", case.fake.as_os_str()), ("SystemRoot", system_root.as_os_str())])
        .envs([("PathExt", ".EXE;.CMD"), ("CP_SESSION_NAME", "codex-proj"), ("CP_SESSION_KEY", "key-0001"), ("CP_PROVIDER", "codex")]);
    assert_eq!(command.status().unwrap().code(), Some(0));
    let spawns = spawns(&case.fake);
    let server = spawns.iter().find(|spawn| spawn["role"] == "codex app-server").expect("o app-server subiu");
    let keys: Vec<&str> = server["env"].as_object().unwrap().keys().map(String::as_str).collect();
    for (upper, mixed) in [("PATH", "Path"), ("SYSTEMROOT", "SystemRoot"), ("PATHEXT", "PathExt")] {
        assert!(keys.contains(&upper) && !keys.contains(&mixed), "{upper}: {keys:?}");
    }
}

/// PR-14: o pane recebe o código da TUI inteiro (`0xC000013A`, console fechado), como o `SystemExit` do
/// Python, não só os 8 bits de baixo.
#[test]
fn the_tui_exit_code_reaches_the_pane_with_all_32_bits() {
    let code = 0xC000_013A_u32 as i32;
    let case = Case::new(json!({"rc": code}));
    let status = case.command().status().unwrap();
    assert_eq!(status.code(), Some(code));
    assert!(!case.sidecar().exists());
}

/// Ctrl+Break no grupo do lançador: sai com 130, derruba o app-server e apaga o arquivo da sessão.
#[test]
fn ctrl_break_exits_130_and_cleans_up() {
    use std::os::windows::process::CommandExt;
    let case = Case::new(json!({}));
    let fake = case.fake.clone();
    std::fs::write(fake.join("tui-hang"), "").unwrap();
    let mut child = case.command().creation_flags(CREATE_NEW_PROCESS_GROUP).spawn().unwrap();
    let sidecar = case.sidecar();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !spawns(&fake).iter().any(|spawn| spawn["role"] == "tui-sidecar-snapshot") {
        assert!(Instant::now() < deadline, "a TUI não chegou a ver o arquivo: {:?}", spawns(&fake));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(sidecar.exists());
    // SAFETY: o grupo é o do filho, criado com CREATE_NEW_PROCESS_GROUP; este processo divide o console.
    assert_ne!(unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) }, 0);
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() { break status; }
        assert!(Instant::now() < deadline, "o Ctrl+Break não encerrou o lançador");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(130));
    assert!(!sidecar.exists(), "a limpeza apagou o arquivo da sessão");
    for spawn in spawns(&fake).iter().filter(|spawn| spawn["role"] == "codex tui") {
        let _ = Command::new("taskkill").args(["/F", "/PID", &spawn["pid"].to_string()]).output();
    }
}

/// PR-13 (decisão 26): com o `codex.cmd` do npm, a primeira mensagem de várias linhas não vai no argv
/// (o std recusa quebra de linha em `.cmd`): a TUI abre sem ela, e ela chega inteira, uma vez, como
/// `turn/start` na thread que a TUI anunciou.
#[test]
fn a_multiline_first_prompt_reaches_a_cmd_codex_through_the_app_server() {
    let case = Case::new(json!({}));
    let real = case.fake.join("bin");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::rename(case.stubs.join("codex.exe"), real.join("codex-real.exe")).unwrap();
    std::fs::write(case.stubs.join("codex.cmd"), format!("@\"{}\" %*\r\n", real.join("codex-real.exe").display())).unwrap();
    let text = "linha 1\r\nlinha 2\nlinha 3";
    let output = case.command().arg("--prompt").arg(text).stderr(Stdio::piped()).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let spawns = spawns(&case.fake);
    let tui = spawns.iter().find(|spawn| spawn["role"] == "codex tui").expect("a TUI abriu");
    assert!(!tui["argv"].as_array().unwrap().iter().any(|arg| arg.as_str().is_some_and(|arg| arg.contains("linha"))), "{tui}");
    let turns: Vec<&Value> = spawns.iter().filter(|spawn| spawn["role"] == "app-server-rpc" && spawn["msg"]["method"] == "turn/start").collect();
    assert_eq!(turns.len(), 1, "{spawns:?}");
    assert_eq!(turns[0]["msg"]["params"]["threadId"], "thread-1");
    assert_eq!(turns[0]["msg"]["params"]["input"], json!([{"type": "text", "text": text}]));
    assert!(stderr.contains("primeira mensagem entregue pelo app-server"), "{stderr}");
}
