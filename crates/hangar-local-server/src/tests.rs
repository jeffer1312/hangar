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
    /// Outra linha do `ss`, enquanto o processo dela vive: o filho que herdou o socket.
    ss_inherited: Option<(&'static str, u32)>,
    /// Respostas HTTP que só aparecem depois de um `systemctl --user start` (o servidor subiu).
    http_after_start: Vec<(&'static str, Reply)>,
    processes: RefCell<Vec<ProcessInfo>>,
    /// Pids que só saem com `SIGKILL`.
    stubborn: Vec<u32>,
    now: u64,
    ran: RefCell<Vec<String>>,
    signals: RefCell<Vec<(u32, bool)>>,
}

impl Fake {
    fn reply(mut self, prefix: &'static str, code: i32, stdout: &str) -> Fake {
        self.replies.push((prefix, Output { code: Some(code), stdout: stdout.to_owned(), ..Output::default() }));
        self
    }
    fn timeout(mut self, prefix: &'static str) -> Fake {
        self.replies.push((prefix, Output { code: None, timed_out: true, ..Output::default() }));
        self
    }
    fn http(mut self, path: &'static str, status: u16, body: &str) -> Fake {
        self.http.push((path, Reply { status, body: body.to_owned() }));
        self
    }
    fn http_after_start(mut self, path: &'static str, status: u16, body: &str) -> Fake {
        self.http_after_start.push((path, Reply { status, body: body.to_owned() }));
        self
    }
    fn process(self, pid: u32, name: &str, user: &str, start_time: u64) -> Fake { self.child(pid, None, name, user, start_time) }
    fn child(self, pid: u32, parent: Option<u32>, name: &str, user: &str, start_time: u64) -> Fake {
        self.processes.borrow_mut().push(ProcessInfo { pid, parent, name: name.to_owned(), cmd: Vec::new(), user: Some(user.to_owned()), start_time });
        self
    }
    fn ran(&self, prefix: &str) -> bool { self.ran.borrow().iter().any(|line| line.starts_with(prefix)) }
}

impl Host for Fake {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output> {
        let line = std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
        self.ran.borrow_mut().push(line.clone());
        if program == "ss" {
            let alive = |pid: u32| self.processes.borrow().iter().any(|p| p.pid == pid);
            let stdout = [self.ss, self.ss_inherited].into_iter().flatten().filter(|(_, pid)| alive(*pid))
                .map(|(text, _)| text.trim_end()).collect::<Vec<_>>().join("\n");
            return Ok(Output { code: Some(0), stdout, ..Output::default() });
        }
        Ok(self.replies.iter().find(|(prefix, _)| line.starts_with(prefix)).map(|(_, out)| out.clone())
            .unwrap_or(Output { code: Some(0), ..Output::default() }))
    }
    fn get(&self, _port: u16, path: &str) -> Option<Reply> {
        let started = self.ran("systemctl --user start");
        self.http.iter().chain(self.http_after_start.iter().filter(|_| started)).find(|(p, _)| *p == path).map(|(_, reply)| reply.clone())
    }
    fn processes(&self) -> Vec<ProcessInfo> { self.processes.borrow().clone() }
    fn process(&self, pid: u32) -> Option<ProcessInfo> { self.processes.borrow().iter().find(|p| p.pid == pid).cloned() }
    fn user(&self) -> Option<String> { Some(ME.to_owned()) }
    fn signal(&self, pid: u32, force: bool) -> bool {
        self.signals.borrow_mut().push((pid, force));
        if force || !self.stubborn.contains(&pid) { self.processes.borrow_mut().retain(|p| p.pid != pid); }
        true
    }
    fn pause(&self, _duration: Duration) {}
    fn now(&self) -> u64 { self.now }
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

    // Pasta de versão com manifesto sem `channel` não é pacote, no critério do atualizador.
    let half = version(home, "0.1.0.12", true);
    std::fs::write(half.join("manifest.json"), r#"{"version":"0.1.0.12","commit":"c"}"#).unwrap();
    point_current(home, &half);
    assert!(!versions::is_package(&half));
    assert!(server(home, Fake::default()).versions().dev_mode);
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
    // Sem serviço, ou sem conseguir lê-lo, ninguém subiria a versão trocada.
    let missing = server(home, Fake::default().reply("systemctl --user show", 0, "LoadState=not-found\nActiveState=inactive\n"));
    assert!(matches!(missing.use_version("0.1.0.1"), Err(ActionError::NotInstalled)));
    let unreadable = server(home, Fake::default().reply("systemctl --user show", 1, ""));
    assert!(matches!(unreadable.use_version("0.1.0.1"), Err(ActionError::ServiceUnreadable(_))));
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

    assert_eq!(local.use_version("0.1.0.1").unwrap(), StartOutcome::Started);
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

/// Apontar para a própria versão da troca sem prova apaga o registro: senão a próxima atualização
/// o usaria para desfazer a escolha.
#[test]
fn use_version_drops_the_unproven_record() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (old, new) = (version(home, "0.1.0.1", true), version(home, "0.1.0.2", true));
    point_current(home, &old);
    mark_unproven(home, Some(&old), &new);
    let local = server(home, Fake::default().reply("systemctl --user show", 0, FAILED_UNIT));
    local.use_version("0.1.0.2").unwrap();
    assert_eq!(link::real(&link::current(&hangar(home))).unwrap(), link::real(&new).unwrap());
    assert!(!link::unproven_file(&hangar(home)).exists());
}

/// O serviço religando subiu a versão antiga entre a conferência e a troca: quem responde não é a
/// escolhida. E o `systemctl start` que estoura o prazo é subida em curso, não erro.
#[test]
fn use_version_confirms_who_answers_and_a_slow_start_is_still_starting() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (_old, new) = (version(home, "0.1.0.1", true), version(home, "0.1.0.2", true));
    point_current(home, &new);
    let restarting = "LoadState=loaded\nActiveState=activating\nSubState=auto-restart\nExecMainStatus=1\n";
    let raced = server(home, Fake::default().reply("systemctl --user show", 0, restarting)
        .http_after_start("/__hangar_server/health", 200, r#"{"ok":true,"protocol":7,"package_version":"0.1.0.2"}"#)
        .http_after_start("/api/client-api", 401, ""));
    assert!(matches!(raced.use_version("0.1.0.1"), Err(ActionError::VersionMismatch(v)) if v == "0.1.0.2"));
    let last = local_diary(home).pop().unwrap();
    assert_eq!((last["acao"].as_str(), last["codigo"].as_str()), (Some("use_version_confirm"), Some("local_server_version_mismatch")));

    point_current(home, &new);
    let slow = server(home, Fake::default().reply("systemctl --user show", 0, FAILED_UNIT).timeout("systemctl --user start"));
    assert_eq!(slow.start().unwrap(), StartOutcome::StillStarting);
    let last = local_diary(home).pop().unwrap();
    assert_eq!((last["acao"].as_str(), last["nivel"].as_str(), last["resultado"].as_str()), (Some("start"), Some("ok"), Some("ainda_subindo")));
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
    // Parada pedida de um servidor que demorou a sair (SIGKILL no fim do prazo) deixa código, e não é falha.
    let killed = server(home, Fake::default().reply("systemctl --user show", 0, "LoadState=loaded\nActiveState=inactive\nSubState=dead\nExecMainStatus=9\n"));
    assert_eq!(killed.probe(), LocalState::Stopped { failure: None });
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
    assert!(other_user.host.signals.borrow().is_empty());

    // O pid foi reusado por outro processo depois do diagnóstico.
    let reborn = server(home, busy(ME, 1_700_000_500));
    assert!(matches!(reborn.free_port(&seen), Err(ActionError::HolderChanged)));
    assert!(reborn.host.signals.borrow().is_empty());

    let gone = server(home, Fake::default());
    assert!(matches!(gone.free_port(&seen), Err(ActionError::HolderChanged)));

    let same = server(home, busy(ME, 1_700_000_000));
    same.free_port(&seen).unwrap();
    assert_eq!(*same.host.signals.borrow(), vec![(48211, false)]);
    let last = local_diary(home).pop().unwrap();
    assert_eq!((last["acao"].as_str(), last["nivel"].as_str(), last["alvo_pid"].as_str()), (Some("free_port"), Some("ok"), Some("48211")));
}

/// Os filhos do mesmo usuário recebem o pedido junto; quem não sai com ele sai à força, e filho de
/// outro usuário fica.
#[test]
fn free_port_takes_the_tree_and_escalates_to_kill() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake { ss: Some((SS_BUSY, 48211)), stubborn: vec![48211], ..Fake::default() }
        .process(48211, "python3", ME, 1_700_000_000)
        .child(48300, Some(48211), "python3", ME, 1_700_000_010)
        .child(48301, Some(48300), "node", ME, 1_700_000_020)
        .child(48400, Some(48211), "sudo", "0", 1_700_000_030);
    let local = server(dir.path(), fake);
    local.free_port(&PortHolder { pid: 48211, name: "python3".into(), same_user: true, start_time: 1_700_000_000 }).unwrap();
    assert_eq!(*local.host.signals.borrow(), vec![(48211, false), (48300, false), (48301, false), (48211, true)]);

    // No Windows a árvore vai inteira pelo `taskkill`, sem sinal por pid.
    let windows = LocalServer { os: Os::Windows, ..server(dir.path(), Fake::default().reply("powershell", 0, "48211\n").process(48211, "python.exe", ME, 5)) };
    let _ = windows.free_port(&PortHolder { pid: 48211, name: "python.exe".into(), same_user: true, start_time: 5 });
    assert!(windows.host.ran("taskkill /PID 48211 /T /F"));
    assert!(windows.host.signals.borrow().is_empty());

    // Um hangar-server preso sai sem a árvore: os filhos dele são as sessões.
    let ours = LocalServer { os: Os::Windows, ..server(dir.path(), Fake::default().reply("powershell", 0, "48211\n").process(48211, "hangar-server.exe", ME, 5)) };
    let _ = ours.free_port(&PortHolder { pid: 48211, name: "hangar-server.exe".into(), same_user: true, start_time: 5 });
    assert!(ours.host.ran("taskkill /PID 48211 /F") && !ours.host.ran("taskkill /PID 48211 /T"));
    let linux = server(dir.path(), Fake { ss: Some((SS_BUSY, 48211)), ..Fake::default() }
        .process(48211, "hangar-server", ME, 5).child(48300, Some(48211), "hangar-cano", ME, 6));
    linux.free_port(&PortHolder { pid: 48211, name: "hangar-server".into(), same_user: true, start_time: 5 }).unwrap();
    assert_eq!(*linux.host.signals.borrow(), vec![(48211, false)]);
}

/// Quem segurava saiu com o pedido, mas o filho que herdou o socket não: a porta segue presa.
#[test]
fn free_port_is_not_done_while_a_child_still_listens() {
    let dir = tempfile::tempdir().unwrap();
    let child_line = "LISTEN 0      1024         0.0.0.0:8765      0.0.0.0:*    users:((\"python3\",pid=48300,fd=7))";
    let fake = Fake { ss: Some((SS_BUSY, 48211)), ss_inherited: Some((child_line, 48300)), stubborn: vec![48300], ..Fake::default() }
        .process(48211, "python3", ME, 1_700_000_000)
        .child(48300, Some(48211), "python3", ME, 1_700_000_010);
    let local = server(dir.path(), fake);
    let result = local.free_port(&PortHolder { pid: 48211, name: "python3".into(), same_user: true, start_time: 1_700_000_000 });
    assert!(matches!(result, Err(ActionError::PortStillHeld)), "{result:?}");
    assert_eq!(*local.host.signals.borrow(), vec![(48211, false), (48300, false)]);
    let last = local_diary(dir.path()).pop().unwrap();
    assert_eq!((last["acao"].as_str(), last["codigo"].as_str()), (Some("free_port"), Some("local_server_port_still_held")));
}

/// O registro da troca sem prova não sai e o `current` não volta: o erro diz em que versão ele ficou.
#[test]
fn use_version_says_where_the_current_was_left_when_the_undo_fails() {
    let root = Path::new("/h/.hangar");
    let refused = |_: &Path| Err(std::io::Error::other("registro preso"));
    let undo_fails = |_: &Path, _: &Path| Err(std::io::Error::other("sem link"));
    let result = drop_unproven_or_undo(root, "0.1.0.2", Some(PathBuf::from("/v/0.1.0.1")), refused, undo_fails);
    let Err(error) = result else { panic!("a volta falhou") };
    assert_eq!(error.code(), "local_server_current_left_on");
    assert!(error.detail().starts_with("0.1.0.2: registro preso") && error.detail().contains("sem link"), "{}", error.detail());

    // A volta deu certo: o `current` está onde estava, e o erro é só o do registro.
    let undo_ok = |_: &Path, _: &Path| Ok(None);
    assert!(matches!(drop_unproven_or_undo(root, "0.1.0.2", Some(PathBuf::from("/v/0.1.0.1")), refused, undo_ok), Err(ActionError::Io(_))));
    assert!(drop_unproven_or_undo(root, "0.1.0.2", None, |_: &Path| Ok(()), undo_fails).is_ok());

    // Sem `current` antes não há para onde voltar: a troca fica, dita no erro e no diário.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let v1 = version(home, "0.1.0.1", true);
    std::fs::create_dir_all(link::unproven_file(&hangar(home)).join("preso")).unwrap();
    let local = server(home, Fake::default().reply("systemctl --user show", 0, FAILED_UNIT));
    assert!(matches!(local.use_version("0.1.0.1"), Err(ActionError::CurrentLeftOn { version, .. }) if version == "0.1.0.1"));
    assert_eq!(link::real(&link::current(&hangar(home))).unwrap(), link::real(&v1).unwrap());
    let last = local_diary(home).pop().unwrap();
    assert_eq!((last["acao"].as_str(), last["codigo"].as_str()), (Some("use_version"), Some("local_server_current_left_on")));
    assert!(last["detalhe"].as_str().unwrap().starts_with("0.1.0.1: "));
    assert!(!local.host.ran("systemctl --user start"), "sem a troca confirmada, nada sobe");
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

    // O mesmo filho de pé além do limite sem abrir a porta está preso, não subindo.
    let stuck = launcher(Fake { now: 2 + STARTING_LIMIT.as_secs() + 1, ..Fake::default() }.reply("powershell", 0, task));
    stuck.processes.borrow_mut().push(ProcessInfo { pid: 11, parent: Some(10), name: "hangar-server.exe".into(), cmd: vec![], user: Some(ME.into()), start_time: 2 });
    let stuck = LocalServer { os: Os::Windows, ..server(dir.path(), stuck) };
    assert!(matches!(stuck.probe(), LocalState::Stopped { failure: Some(Failure { code, .. }) } if code == "service_failed"));
}

#[test]
fn linux_service_alive_without_port_is_starting_until_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let unit = "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=77\nExecMainStatus=0\n";
    let at = |now: u64| server(dir.path(), Fake { now, ..Fake::default() }.reply("systemctl --user show", 0, unit).process(77, "hangar-server", ME, 1_000));
    assert_eq!(at(1_000 + 60).probe(), LocalState::Starting);
    assert!(matches!(at(1_000 + STARTING_LIMIT.as_secs() + 1).probe(), LocalState::Stopped { failure: Some(Failure { code, .. }) } if code == "service_failed"));
}

#[test]
fn log_base_follows_the_machine_rule() {
    let home = Path::new("/home/fulano");
    if cfg!(windows) {
        assert_eq!(diary::log_base_from(Some(home), Some(std::ffi::OsStr::new(r"C:\L"))), Some(PathBuf::from(r"C:\L").join("hangar").join("logs")));
        assert_eq!(diary::log_base_from(Some(home), Some(std::ffi::OsStr::new(""))), Some(home.join("AppData").join("Local").join("hangar").join("logs")));
    } else {
        assert_eq!(diary::log_base_from(Some(home), Some(std::ffi::OsStr::new("/x"))), Some(home.join(".hangar").join("logs")));
    }
    assert_eq!(diary::log_base_from(None, None), None);
}

#[test]
fn env_port_reads_like_the_server_start() {
    assert_eq!(config::env_value("CP_PORT=1\n  cp_port = 9100  # a porta\nOUTRA=2\n").as_deref(), Some("9100"));
    assert_eq!(config::env_value("CP_PORT=\"9200\"\n").as_deref(), Some("9200"));
    assert_eq!(config::env_value("# CP_PORT=9300\nCP_AUTH_TOKEN=x\n"), None);
    // Presente e vazio não é ausente: o servidor recusa subir com ele.
    assert_eq!(config::env_value("CP_PORT=\n").as_deref(), Some(""));
}

#[test]
fn invalid_port_is_named_instead_of_silently_defaulting() {
    let dir = tempfile::tempdir().unwrap();
    let config = hangar(dir.path()).join("config");
    std::fs::create_dir_all(&config).unwrap();
    let unit = Fake::default().reply("systemctl --user show", 0, "Environment=PATH=/usr/bin\nWorkingDirectory=\n");
    for text in ["CP_PORT=abc\n", "CP_PORT=\n"] {
        std::fs::write(config.join(".env"), text).unwrap();
        let resolved = config::resolve(&unit, Os::Linux, dir.path());
        assert_eq!(resolved.port, DEFAULT_PORT, "{text}");
        assert_eq!(resolved.issue.map(|i| i.code), Some("local_server_port_invalid"), "{text}");
    }
    // A unit que não responde: a porta sai do `.env`, com a ressalva.
    std::fs::write(config.join(".env"), "CP_PORT=8790\n").unwrap();
    let broken = config::resolve(&Fake::default().reply("systemctl --user show", 1, ""), Os::Linux, dir.path());
    assert_eq!((broken.port, broken.issue.map(|i| i.code)), (8790, Some("local_server_unit_unreadable")));
    assert_eq!(config::resolve(&unit, Os::Linux, dir.path()), config::Resolved { port: 8790, issue: None });
}

#[test]
fn service_port_follows_the_unit_then_the_env_file_then_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let unit = |env: &str, work: &str| Fake::default().reply("systemctl --user show", 0, &format!("Environment={env}\nWorkingDirectory={work}\n"));
    let port = |host: &Fake, os: Os, home: &Path| config::resolve(host, os, home).port;
    // Máquina sem `.env`: a padrão.
    assert_eq!(port(&unit("PATH=/usr/bin", ""), Os::Linux, home), DEFAULT_PORT);
    // Clone sem migrar: o `.env` da `backend/` que a unit usa como pasta.
    let backend = home.join("hangar").join("backend");
    std::fs::create_dir_all(&backend).unwrap();
    std::fs::write(backend.join(".env"), "CP_PORT=9000\n").unwrap();
    let work = backend.to_string_lossy().into_owned();
    assert_eq!(port(&unit("PATH=/usr/bin", &work), Os::Linux, home), 9000);
    // Máquina migrada: vale o `.env` dos dados do usuário, nunca o da `backend/`.
    let config = hangar(home).join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".env"), "CP_AUTH_TOKEN=t\nCP_PORT=8790\n").unwrap();
    assert_eq!(port(&unit("PATH=/usr/bin", &work), Os::Linux, home), 8790);
    assert_eq!(port(&Fake::default(), Os::Windows, home), 8790);
    // A porta do ambiente da unit vence o `.env`, como no `Settings::resolve`.
    assert_eq!(port(&unit("PATH=/usr/bin CP_PORT=9555", &work), Os::Linux, home), 9555);
    // Sem `.env` no lugar novo, mas com `current`: o lugar novo ainda vale, e sem arquivo é a padrão.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(hangar(other.path()).join("current")).unwrap();
    assert_eq!(config::env_path(other.path(), Some(&backend)), Some(hangar(other.path()).join("config").join(".env")));
    assert_eq!(port(&unit("", &work), Os::Linux, other.path()), DEFAULT_PORT);
}
