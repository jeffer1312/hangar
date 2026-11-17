use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::host::{Host, Output, ProcessInfo, Reply};
use crate::*;

const SS_BUSY: &str = include_str!("../tests/fixtures/ss-8765-busy.txt");
const DIARY_FAILURE: &str = include_str!("../tests/fixtures/diario-falha.jsonl");
const ME: &str = "1000";

/// A máquina de mentira: comandos respondidos por prefixo, porta e processos em memória.
#[derive(Default)]
struct Fake {
    /// `(prefixo da linha de comando, resposta)`; sem prefixo que case, código 0 e saída vazia.
    replies: Vec<(&'static str, Output)>,
    http: Vec<(&'static str, Reply)>,
    /// O texto que o `ss` devolve enquanto o processo de `ss_pid` vive.
    ss: Option<(&'static str, u32)>,
    processes: RefCell<Vec<ProcessInfo>>,
    ran: RefCell<Vec<String>>,
    terminated: RefCell<Vec<u32>>,
}

impl Fake {
    fn reply(mut self, prefix: &'static str, code: i32, stdout: &str) -> Fake {
        self.replies.push((prefix, Output { code: Some(code), stdout: stdout.to_owned(), stderr: String::new() }));
        self
    }
    fn http(mut self, path: &'static str, status: u16, body: &str) -> Fake {
        self.http.push((path, Reply { status, body: body.to_owned() }));
        self
    }
    fn process(self, pid: u32, name: &str, user: &str, start_time: u64) -> Fake {
        self.processes.borrow_mut().push(ProcessInfo { pid, parent: None, name: name.to_owned(), cmd: Vec::new(), user: Some(user.to_owned()), start_time });
        self
    }
    fn ran(&self, prefix: &str) -> bool { self.ran.borrow().iter().any(|line| line.starts_with(prefix)) }
}

impl Host for Fake {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output> {
        let line = std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
        self.ran.borrow_mut().push(line.clone());
        if program == "ss" {
            let stdout = match self.ss {
                Some((text, pid)) if self.processes.borrow().iter().any(|p| p.pid == pid) => text.to_owned(),
                _ => String::new(),
            };
            return Ok(Output { code: Some(0), stdout, stderr: String::new() });
        }
        Ok(self.replies.iter().find(|(prefix, _)| line.starts_with(prefix)).map(|(_, out)| out.clone())
            .unwrap_or(Output { code: Some(0), stdout: String::new(), stderr: String::new() }))
    }
    fn get(&self, _port: u16, path: &str) -> Option<Reply> {
        self.http.iter().find(|(p, _)| *p == path).map(|(_, reply)| reply.clone())
    }
    fn processes(&self) -> Vec<ProcessInfo> { self.processes.borrow().clone() }
    fn process(&self, pid: u32) -> Option<ProcessInfo> { self.processes.borrow().iter().find(|p| p.pid == pid).cloned() }
    fn user(&self) -> Option<String> { Some(ME.to_owned()) }
    fn terminate(&self, pid: u32) -> bool {
        self.terminated.borrow_mut().push(pid);
        self.processes.borrow_mut().retain(|p| p.pid != pid);
        true
    }
    fn pause(&self, _duration: Duration) {}
}

const STOPPED_UNIT: &str = "LoadState=loaded\nActiveState=inactive\nSubState=dead\nExecMainStatus=0\n";
const FAILED_UNIT: &str = "LoadState=loaded\nActiveState=failed\nSubState=failed\nExecMainStatus=3\n";
const ACTIVE_UNIT: &str = "LoadState=loaded\nActiveState=active\nSubState=running\nExecMainStatus=0\n";

fn server(home: &Path, host: Fake) -> LocalServer<Fake> {
    LocalServer { home: home.to_path_buf(), port: 8765, log_base: home.join("logs"), os: Os::Linux, host }
}

fn hangar(home: &Path) -> PathBuf { home.join(".hangar") }

/// Uma pasta de versão com manifesto e, se `binary`, o servidor dentro.
fn version(home: &Path, name: &str, binary: bool) -> PathBuf {
    let dir = versions::versions_dir(&hangar(home)).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), format!(r#"{{"version":"{name}","commit":"c","channel":"latest"}}"#)).unwrap();
    if binary {
        let bin = versions::server_binary(&dir);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(bin, "").unwrap();
    }
    dir
}

fn point_current(home: &Path, target: &Path) {
    let current = link::current(&hangar(home));
    link::remove_link(&current).unwrap();
    link::make_link(target, &current).unwrap();
}

fn mark_unproven(home: &Path, previous: Option<&Path>, target: &Path) {
    let record = link::Unproven { previous: previous.map(|p| link::real(p).unwrap()), target: link::real(target).unwrap() };
    std::fs::write(link::unproven_file(&hangar(home)), serde_json::to_vec(&record).unwrap()).unwrap();
}

fn local_diary(home: &Path) -> Vec<serde_json::Value> {
    let dir = diary::local_dir(&home.join("logs"));
    let mut lines = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        lines.extend(std::fs::read_to_string(entry.path()).unwrap().lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()));
    }
    lines
}

fn server_diary(home: &Path, text: &str) {
    let dir = diary::server_dir(&home.join("logs"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("uso-2026-11-17.jsonl"), text).unwrap();
}

// ---------- versões ----------

/// Três versões: a 1 inteira, a 2 sem o servidor dentro e a 3, em uso, a da troca sem prova.
#[test]
fn previous_skips_the_version_without_binary_and_the_one_that_failed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (v1, v2, v3) = (version(home, "0.1.0.1", true), version(home, "0.1.0.2", false), version(home, "0.1.0.3", true));
    point_current(home, &v3);
    mark_unproven(home, None, &v3);
    let read = server(home, Fake::default()).versions();
    assert_eq!(read, Versions { current: Some("0.1.0.3".into()), previous: Some("0.1.0.1".into()),
        installed: vec!["0.1.0.3".into(), "0.1.0.2".into(), "0.1.0.1".into()], dev_mode: false });

    // O registro da troca diz quem estava antes, e ele vence a mais nova abaixo da atual.
    version(home, "0.1.0.2", true);
    mark_unproven(home, Some(&v1), &v3);
    assert_eq!(server(home, Fake::default()).versions().previous.as_deref(), Some("0.1.0.1"));

    // Sem registro, a mais nova abaixo da atual com o servidor dentro.
    std::fs::remove_file(link::unproven_file(&hangar(home))).unwrap();
    assert_eq!(server(home, Fake::default()).versions().previous.as_deref(), Some("0.1.0.2"));

    // A que falhou nunca é oferecida, mesmo abaixo da atual.
    mark_unproven(home, None, &v2);
    assert_eq!(server(home, Fake::default()).versions().previous.as_deref(), Some("0.1.0.1"));
}

#[test]
fn broken_current_offers_the_newest_runnable_and_a_checkout_is_dev_mode() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    version(home, "0.1.0.9", true);
    version(home, "0.1.0.10", false);
    let gone = version(home, "0.1.0.11", true);
    point_current(home, &gone);
    std::fs::remove_dir_all(&gone).unwrap();
    let read = server(home, Fake::default()).versions();
    assert_eq!((read.current, read.previous.as_deref(), read.dev_mode), (None, Some("0.1.0.9"), false));

    let checkout = home.join("hangar-checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    point_current(home, &checkout);
    let read = server(home, Fake::default()).versions();
    assert_eq!((read.current, read.previous, read.dev_mode), (None, None, true));
    let local = server(home, Fake::default().reply("systemctl --user show", 0, STOPPED_UNIT));
    assert!(matches!(local.use_version("0.1.0.9"), Err(ActionError::DevMode)));
}

// ---------- apontar para outra versão ----------

#[test]
fn use_version_refuses_without_the_lock_and_with_the_server_up() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (old, new) = (version(home, "0.1.0.1", true), version(home, "0.1.0.2", true));
    point_current(home, &new);
    let stopped = || Fake::default().reply("systemctl --user show", 0, STOPPED_UNIT);

    {
        let _held = UpdateLock::try_acquire(&hangar(home)).unwrap();
        let local = server(home, stopped());
        assert!(matches!(local.use_version("0.1.0.1"), Err(ActionError::Locked)));
        assert!(!local.host.ran("systemctl --user start"));
    }
    let up = server(home, stopped().http("/__hangar_server/health", 200, r#"{"ok":true,"protocol":7}"#).http("/api/client-api", 200, r#"{"api":19}"#));
    assert!(matches!(up.use_version("0.1.0.1"), Err(ActionError::ServerRunning)));
    let active = server(home, Fake::default().reply("systemctl --user show", 0, ACTIVE_UNIT));
    assert!(matches!(active.use_version("0.1.0.1"), Err(ActionError::ServerRunning)));
    assert_eq!(link::real(&link::current(&hangar(home))).unwrap(), link::real(&new).unwrap(), "recusa não toca no current");

    let local = server(home, stopped());
    assert!(matches!(local.use_version("0.1.0.7"), Err(ActionError::UnknownVersion(v)) if v == "0.1.0.7"));
    assert!(matches!(local.use_version("../x"), Err(ActionError::UnknownVersion(_))));
    let mac = LocalServer { os: Os::Macos, ..server(home, stopped()) };
    assert!(matches!(mac.use_version("0.1.0.1"), Err(ActionError::Unsupported)));
    let _ = old;
}

#[test]
fn use_version_swaps_the_current_and_starts_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (old, new) = (version(home, "0.1.0.1", true), version(home, "0.1.0.2", true));
    version(home, "0.1.0.3", false);
    point_current(home, &new);
    let local = server(home, Fake::default().reply("systemctl --user show", 0, FAILED_UNIT));
    assert!(matches!(local.use_version("0.1.0.3"), Err(ActionError::NoBinary(v)) if v == "0.1.0.3"));

    local.use_version("0.1.0.1").unwrap();
    assert_eq!(link::real(&link::current(&hangar(home))).unwrap(), link::real(&old).unwrap());
    assert!(local.host.ran("systemctl --user start hangar-backend.service"));
    // A troca não deixa sobra ao lado nem link novo pela metade.
    assert!(std::fs::symlink_metadata(link::aside(&hangar(home))).is_err());
    assert!(std::fs::symlink_metadata(hangar(home).join(link::FRESH)).is_err());
    let diary = local_diary(home);
    let actions: Vec<_> = diary.iter().map(|l| (l["acao"].as_str().unwrap().to_owned(), l["nivel"].as_str().unwrap().to_owned())).collect();
    assert_eq!(actions, [("use_version", "erro"), ("use_version", "ok"), ("start", "ok")].map(|(a, n)| (a.to_owned(), n.to_owned())));
    let swap = &diary[1];
    assert_eq!((swap["evento"].as_str(), swap["de"].as_str(), swap["para"].as_str()), (Some("local_server.acao"), Some("0.1.0.2"), Some("0.1.0.1")));
}

// ---------- estado e motivo ----------

#[test]
fn probe_reads_running_starting_and_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let health = r#"{"ok":true,"protocol":7,"pid":42}"#;
    let running = server(dir.path(), Fake::default().http("/__hangar_server/health", 200, health).http("/api/client-api", 401, ""));
    assert_eq!(running.probe(), LocalState::Running);
    let starting = server(dir.path(), Fake::default().http("/__hangar_server/health", 200, health)
        .http("/api/client-api", 503, r#"{"detail":"server_starting"}"#));
    assert_eq!(starting.probe(), LocalState::Starting);
    let activating = server(dir.path(), Fake::default().reply("systemctl --user show", 0, "LoadState=loaded\nActiveState=activating\nSubState=start\n"));
    assert_eq!(activating.probe(), LocalState::Starting);
    let missing = server(dir.path(), Fake::default().reply("systemctl --user show", 0, "LoadState=not-found\nActiveState=inactive\nSubState=dead\n"));
    assert_eq!(missing.probe(), LocalState::NotInstalled);
}

#[test]
fn failure_comes_from_the_last_life_in_the_diary_with_the_journal() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    server_diary(home, DIARY_FAILURE);
    let local = server(home, Fake::default().reply("systemctl --user show", 0, FAILED_UNIT)
        .reply("journalctl --user -u hangar-backend.service", 0, "launch_start: versão 0.1.0.10824\nlaunch_no_server: /x\n"));
    assert_eq!(local.probe(), LocalState::Stopped { failure: Some(Failure {
        code: "launch_no_server".into(), detail: "/home/fulano/.hangar/versions/0.1.0.10824/bin/hangar-server".into(), port_holder: None,
        output: vec!["launch_start: versão 0.1.0.10824".into(), "launch_no_server: /x".into()] }) });

    // Subida e parada limpa depois da falha: ela é velha, e o serviço parado não tem o que explicar.
    server_diary(home, &format!("{DIARY_FAILURE}{}\n{}\n",
        r#"{"ts":"2026-11-17T11:00:00.000-03:00","evento":"backend.inicio","nivel":"ok"}"#,
        r#"{"ts":"2026-11-17T12:00:00.000-03:00","evento":"backend.encerrando","nivel":"ok","causa":"sinal"}"#));
    let stopped = server(home, Fake::default().reply("systemctl --user show", 0, STOPPED_UNIT));
    assert_eq!(stopped.probe(), LocalState::Stopped { failure: None });
    // Sem nada no diário, o código de saída da unit.
    let failed = server(home, Fake::default().reply("systemctl --user show", 0, FAILED_UNIT));
    assert!(matches!(failed.probe(), LocalState::Stopped { failure: Some(Failure { code, detail, .. }) } if code == "service_exit" && detail == "3"));
}

#[test]
fn busy_port_names_the_holder_before_the_diary() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    server_diary(home, DIARY_FAILURE);
    let fake = Fake { ss: Some((SS_BUSY, 48211)), ..Fake::default() }.reply("systemctl --user show", 0, FAILED_UNIT).process(48211, "python3", ME, 1_700_000_000);
    let LocalState::Stopped { failure: Some(failure) } = server(home, fake).probe() else { panic!("parado com motivo") };
    assert_eq!((failure.code.as_str(), failure.detail.as_str()), ("port_unavailable", "python3"));
    assert_eq!(failure.port_holder, Some(PortHolder { pid: 48211, name: "python3".into(), same_user: true, start_time: 1_700_000_000 }));
}

// ---------- liberar a porta ----------

#[test]
fn free_port_kills_only_the_same_process_of_the_same_user() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let busy = |user: &str, start: u64| Fake { ss: Some((SS_BUSY, 48211)), ..Fake::default() }.process(48211, "python3", user, start);
    let seen = PortHolder { pid: 48211, name: "python3".into(), same_user: true, start_time: 1_700_000_000 };

    let other_user = server(home, busy("0", 1_700_000_000));
    assert!(matches!(other_user.free_port(&seen), Err(ActionError::OtherUser)));
    assert!(other_user.host.terminated.borrow().is_empty());

    // O pid foi reusado por outro processo depois do diagnóstico.
    let reborn = server(home, busy(ME, 1_700_000_500));
    assert!(matches!(reborn.free_port(&seen), Err(ActionError::HolderChanged)));
    assert!(reborn.host.terminated.borrow().is_empty());

    let gone = server(home, Fake::default());
    assert!(matches!(gone.free_port(&seen), Err(ActionError::HolderChanged)));

    let same = server(home, busy(ME, 1_700_000_000));
    same.free_port(&seen).unwrap();
    assert_eq!(*same.host.terminated.borrow(), vec![48211]);
    let last = local_diary(home).pop().unwrap();
    assert_eq!((last["acao"].as_str(), last["nivel"].as_str(), last["alvo_pid"].as_str()), (Some("free_port"), Some("ok"), Some("48211")));
}

#[test]
fn windows_launcher_waiting_between_falls_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let task = "tarefa: estado=Running ultima_execucao=2026-11-17T10:00:00 ultimo_resultado=0x00041301 acao=x\n";
    let launcher = |fake: Fake| {
        fake.processes.borrow_mut().push(ProcessInfo { pid: 10, parent: None, name: "hangar-launcher.exe".into(),
            cmd: vec!["hangar-launcher.exe".into(), "launch".into(), "backend".into()], user: Some(ME.into()), start_time: 1 });
        fake
    };
    let waiting = LocalServer { os: Os::Windows, ..server(dir.path(), launcher(Fake::default().reply("powershell", 0, task))) };
    assert!(matches!(waiting.probe(), LocalState::Stopped { failure: Some(Failure { code, .. }) } if code == "service_failed"));
    let child = launcher(Fake::default().reply("powershell", 0, task));
    child.processes.borrow_mut().push(ProcessInfo { pid: 11, parent: Some(10), name: "hangar-server.exe".into(), cmd: vec![], user: Some(ME.into()), start_time: 2 });
    let booting = LocalServer { os: Os::Windows, ..server(dir.path(), child) };
    assert_eq!(booting.probe(), LocalState::Starting);
}
