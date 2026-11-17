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

/// Ctrl+Break no grupo do lançador: sai com 130, derruba o app-server e apaga o arquivo da sessão.
#[test]
fn ctrl_break_exits_130_and_cleans_up() {
    use std::os::windows::process::CommandExt;
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
    std::fs::write(fake.join("config.json"), json!({"app_server": {"threads": threads}}).to_string()).unwrap();
    std::fs::write(fake.join("tui-hang"), "").unwrap();
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut child = Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .arg("codex-tui").arg("--cwd").arg(&work).current_dir(&work).env_clear()
        .envs([("USERPROFILE", home.as_os_str()), ("HOME", home.as_os_str()), ("PATH", stubs.as_os_str()), ("HANGAR_FAKE_CODEX", fake.as_os_str()),
            ("SystemRoot", system_root.as_os_str())])
        .envs([("PATHEXT", ".EXE;.CMD"), ("CP_SESSION_NAME", "codex-proj"), ("CP_SESSION_KEY", "key-0001"), ("CP_PROVIDER", "codex")])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn().unwrap();
    let sidecar: PathBuf = home.join(".hangar/codex-sessions/codex-proj.json");
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
