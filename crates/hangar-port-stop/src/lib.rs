//! Parar o servidor que escuta numa porta da sessão: o grupo de processos dele e, quando o grupo tem
//! o Hangar, o agente ou a raiz da sessão (o `npm run dev &` de um shell sem controle de job), só a
//! árvore que nasce no processo que escuta. Processo protegido nunca recebe sinal. A tabela vem de
//! `/proc/*/stat`, sem `ps`; fora do Linux a parada não existe (`SUPPORTED`).
#![deny(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Duration;

pub const SUPPORTED: bool = cfg!(target_os = "linux");

/// Intervalo entre as consultas depois do SIGTERM, e quanto se espera antes do SIGKILL.
pub const POLL: Duration = Duration::from_millis(250);
pub const GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    /// Início em tiques desde o boot: separa o processo original de um pid reusado.
    pub start: u64,
    /// O estado do `stat` (`R`, `S`, `Z`…).
    pub state: u8,
}

impl Proc {
    /// Zumbi (`Z`) ou morto (`X`) ainda na tabela: já saiu, só falta o pai recolher.
    fn exited(&self) -> bool { matches!(self.state, b'Z' | b'X') }

    /// O mesmo processo de `original`, ainda rodando.
    fn still(&self, original: &Proc) -> bool { self.start == original.start && !self.exited() }
}

pub type Table = HashMap<u32, Proc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal { Term, Kill }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Saiu antes do sinal.
    Gone,
    Terminated,
    Killed,
}

/// O que recebeu os sinais.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// O grupo de processos inteiro.
    Group,
    /// O processo que escuta e os descendentes dele, um sinal por processo.
    Tree,
}

#[derive(Debug)]
pub enum Error {
    /// O próprio processo que escuta, ou um descendente dele, é protegido: sinalizar derrubaria o
    /// agente, a sessão ou o Hangar.
    SharedGroup,
    /// O pid não é mais o processo visto escutando: saiu e o número foi reusado.
    Reused,
    Io(io::Error),
}

/// O que a parada lê e faz no sistema; os testes trocam por um roteiro.
pub trait System {
    /// Falha é erro, nunca "todos saíram".
    fn table(&mut self) -> io::Result<Table>;
    /// `false` = o grupo já não existe.
    fn signal_group(&mut self, pgid: u32, signal: Signal) -> io::Result<bool>;
    /// `false` = o processo já não existe.
    fn signal_pid(&mut self, pid: u32, signal: Signal) -> io::Result<bool>;
    fn sleep(&mut self, duration: Duration);
}

/// Os processos que nunca recebem sinal: `protected`, o pai de cada um (o servidor tmux da raiz do
/// pane, o cano do agente), `server` e os ancestrais dele, os filhos diretos de `server` e os pids 0 e 1.
struct Guard { pids: HashSet<u32>, server: u32 }

impl Guard {
    fn new(table: &Table, server: u32, protected: &HashSet<u32>) -> Self {
        let mut pids: HashSet<u32> = protected.clone();
        pids.extend(protected.iter().filter_map(|p| table.get(p)).map(|p| p.ppid).filter(|ppid| *ppid > 1));
        let mut ancestors = HashSet::new();
        let mut at = server;
        while at > 1 && ancestors.insert(at) { at = table.get(&at).map_or(0, |p| p.ppid); }
        pids.extend(ancestors);
        pids.insert(server);
        Self { pids, server }
    }

    fn covers(&self, p: &Proc) -> bool { p.pid <= 1 || p.ppid == self.server || self.pids.contains(&p.pid) }
}

/// O grupo de `pid` quando nenhum membro dele é protegido (`Guard`).
pub fn stoppable_group(table: &Table, pid: u32, server: u32, protected: &HashSet<u32>) -> Option<u32> {
    let group = table.get(&pid)?.pgid;
    if group <= 1 { return None; }
    let guard = Guard::new(table, server, protected);
    (!table.values().any(|p| p.pgid == group && guard.covers(p))).then_some(group)
}

/// `pid` e seus descendentes na tabela, `pid` primeiro.
fn subtree(table: &Table, pid: u32) -> Vec<Proc> {
    let mut seen = HashSet::from([pid]);
    let mut out: Vec<Proc> = table.get(&pid).copied().into_iter().collect();
    let mut i = 0;
    while let Some(at) = out.get(i).map(|p| p.pid) {
        out.extend(table.values().filter(|p| p.ppid == at && seen.insert(p.pid)));
        i += 1;
    }
    out
}

/// O que parar `pid` sinaliza: o grupo dele quando nenhum membro é protegido; senão a árvore dele, se
/// nem ele nem um descendente é protegido; senão nada.
pub fn target(table: &Table, pid: u32, server: u32, protected: &HashSet<u32>) -> Option<(Scope, Vec<Proc>)> {
    if let Some(group) = stoppable_group(table, pid, server, protected) {
        return Some((Scope::Group, table.values().filter(|p| p.pgid == group).copied().collect()));
    }
    let guard = Guard::new(table, server, protected);
    let tree = subtree(table, pid);
    (!tree.is_empty() && !tree.iter().any(|p| guard.covers(p))).then_some((Scope::Tree, tree))
}

/// SIGTERM ao alvo de `pid` (`target`), consulta a cada `POLL` por `GRACE` e SIGKILL só ao que segue
/// vivo dos originais. `start` é o início de `pid` quando foi visto escutando (`start_of`): outro
/// início é outro processo, e nada recebe sinal. Escopo `None`: o pid já tinha saído. Bloqueante.
pub fn stop(sys: &mut dyn System, pid: u32, start: u64, server: u32, protected: &HashSet<u32>) -> Result<(Stopped, Option<Scope>), Error> {
    let table = sys.table().map_err(Error::Io)?;
    let Some(listener) = table.get(&pid).filter(|p| !p.exited()) else { return Ok((Stopped::Gone, None)) };
    if listener.start != start { return Err(Error::Reused); }
    let group = listener.pgid;
    let (scope, members) = target(&table, pid, server, protected).ok_or(Error::SharedGroup)?;
    let stopped = match scope {
        Scope::Group => stop_group(sys, group, &members)?,
        Scope::Tree => stop_tree(sys, &members)?,
    };
    Ok((stopped, Some(scope)))
}

fn stop_group(sys: &mut dyn System, group: u32, members: &[Proc]) -> Result<Stopped, Error> {
    if !sys.signal_group(group, Signal::Term).map_err(Error::Io)? { return Ok(Stopped::Gone); }
    let mut waited = Duration::ZERO;
    while waited < GRACE {
        sys.sleep(POLL);
        waited += POLL;
        let now = sys.table().map_err(Error::Io)?;
        if !members.iter().any(|m| now.get(&m.pid).is_some_and(|p| p.pgid == group && p.still(m))) { return Ok(Stopped::Terminated); }
    }
    // Com um membro original ainda rodando nele, o número do grupo não foi reusado: o KILL vai só ao
    // job que ignorou o TERM.
    Ok(if sys.signal_group(group, Signal::Kill).map_err(Error::Io)? { Stopped::Killed } else { Stopped::Terminated })
}

/// Um sinal por processo: o grupo tem quem não pode recebê-lo.
fn stop_tree(sys: &mut dyn System, members: &[Proc]) -> Result<Stopped, Error> {
    let mut reached = false;
    for p in members { reached |= sys.signal_pid(p.pid, Signal::Term).map_err(Error::Io)?; }
    if !reached { return Ok(Stopped::Gone); }
    // Reparentado quando o pai sai, o original segue o mesmo processo enquanto o início não muda.
    let alive = |now: &Table| -> Vec<u32> {
        members.iter().filter(|m| now.get(&m.pid).is_some_and(|p| p.still(m))).map(|m| m.pid).collect()
    };
    let mut left = Vec::new();
    let mut waited = Duration::ZERO;
    while waited < GRACE {
        sys.sleep(POLL);
        waited += POLL;
        left = alive(&sys.table().map_err(Error::Io)?);
        if left.is_empty() { return Ok(Stopped::Terminated); }
    }
    let mut killed = false;
    for pid in left { killed |= sys.signal_pid(pid, Signal::Kill).map_err(Error::Io)?; }
    Ok(if killed { Stopped::Killed } else { Stopped::Terminated })
}

/// O sistema de verdade.
pub struct Real;

impl System for Real {
    fn table(&mut self) -> io::Result<Table> { read_table() }
    fn signal_group(&mut self, pgid: u32, signal: Signal) -> io::Result<bool> { signal_group(pgid, signal) }
    fn signal_pid(&mut self, pid: u32, signal: Signal) -> io::Result<bool> { signal_pid(pid, signal) }
    fn sleep(&mut self, duration: Duration) { std::thread::sleep(duration) }
}

#[cfg(target_os = "linux")]
fn read_table() -> io::Result<Table> {
    let mut table = Table::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        // Saiu entre a listagem e a leitura: não entra.
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        if let Some(proc) = parse_stat(pid, &stat) { table.insert(pid, proc); }
    }
    Ok(table)
}

#[cfg(not(target_os = "linux"))]
fn read_table() -> io::Result<Table> { Err(io::ErrorKind::Unsupported.into()) }

/// O início de `pid` (o `start` de `stop`); `None` = já saiu.
#[cfg(target_os = "linux")]
pub fn start_of(pid: u32) -> io::Result<Option<u64>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => Ok(parse_stat(pid, &stat).filter(|p| !p.exited()).map(|p| p.start)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn start_of(_pid: u32) -> io::Result<Option<u64>> { Err(io::ErrorKind::Unsupported.into()) }

/// `pid (comm) estado ppid pgrp … starttime` (22º campo): o `comm` pode ter espaço e parêntese, então
/// o corte é no último `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat(pid: u32, stat: &str) -> Option<Proc> {
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
    let state = *fields.next()?.as_bytes().first()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgid = fields.next()?.parse().ok()?;
    let start = fields.nth(16)?.parse().ok()?;
    Some(Proc { pid, ppid, pgid, start, state })
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn signal_group(pgid: u32, signal: Signal) -> io::Result<bool> {
    // `killpg(0)` e `killpg(1)` nunca chegam aqui: `stoppable_group` recusa grupo <= 1.
    let pgid = libc::pid_t::try_from(pgid).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let signal = match signal { Signal::Term => libc::SIGTERM, Signal::Kill => libc::SIGKILL };
    // SAFETY: só números; nenhum ponteiro cruza a chamada.
    if unsafe { libc::killpg(pgid, signal) } == 0 { return Ok(true); }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { Ok(false) } else { Err(error) }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn signal_pid(pid: u32, signal: Signal) -> io::Result<bool> {
    // `kill` com 0, 1 ou negativo atinge grupos ou todos: recusado aqui, além do `Guard`.
    let pid = libc::pid_t::try_from(pid).ok().filter(|pid| *pid > 1).ok_or(io::Error::from(io::ErrorKind::InvalidInput))?;
    let signal = match signal { Signal::Term => libc::SIGTERM, Signal::Kill => libc::SIGKILL };
    // SAFETY: só números; nenhum ponteiro cruza a chamada.
    if unsafe { libc::kill(pid, signal) } == 0 { return Ok(true); }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { Ok(false) } else { Err(error) }
}

#[cfg(not(unix))]
fn signal_group(_pgid: u32, _signal: Signal) -> io::Result<bool> { Err(io::ErrorKind::Unsupported.into()) }

#[cfg(not(unix))]
fn signal_pid(_pid: u32, _signal: Signal) -> io::Result<bool> { Err(io::ErrorKind::Unsupported.into()) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const SERVER: u32 = 500;

    /// (pid, ppid, pgid)
    fn table(rows: &[(u32, u32, u32)]) -> Table {
        rows.iter().map(|&(pid, ppid, pgid)| (pid, proc(pid, ppid, pgid, 0))).collect()
    }

    fn proc(pid: u32, ppid: u32, pgid: u32, start: u64) -> Proc { Proc { pid, ppid, pgid, start, state: b'S' } }

    /// Os `pids` da tabela viram zumbis.
    fn zombies(mut t: Table, pids: &[u32]) -> Table {
        for pid in pids { t.get_mut(pid).unwrap().state = b'Z'; }
        t
    }

    /// systemd 1 → hangar-server 500 → cano 600 → agente 601; tmux 700 → pane 710 (líder do próprio grupo)
    /// → agente 711; `npm run dev` 720 → node 721 num grupo só deles.
    fn machine() -> Vec<(u32, u32, u32)> {
        vec![(1, 0, 1), (400, 1, 400), (SERVER, 400, 400), (600, SERVER, 600), (601, 600, 600),
            (700, 1, 700), (710, 700, 710), (711, 710, 711), (720, 710, 720), (721, 720, 720)]
    }

    /// Mais o `npm run dev &` de um shell sem controle de job, no grupo do pane: npm 730 → node 731
    /// (escuta) → esbuild 732.
    fn shared() -> Vec<(u32, u32, u32)> {
        let mut rows = machine();
        rows.extend([(730, 710, 710), (731, 730, 710), (732, 731, 710)]);
        rows
    }

    fn protected() -> HashSet<u32> { HashSet::from([710, 711, 600, 601]) }

    fn pids(procs: &[Proc]) -> Vec<u32> {
        let mut out: Vec<u32> = procs.iter().map(|p| p.pid).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn a_job_in_its_own_group_is_stoppable() {
        assert_eq!(stoppable_group(&table(&machine()), 721, SERVER, &protected()), Some(720));
        let (scope, members) = target(&table(&machine()), 721, SERVER, &protected()).unwrap();
        assert_eq!((scope, pids(&members)), (Scope::Group, vec![720, 721]));
    }

    #[test]
    fn a_shared_group_falls_back_to_the_tree_of_the_listener_and_a_protected_listener_is_refused() {
        let mut rows = machine();
        // Shell sem controle de job: o servidor 730 fica no grupo do pane.
        rows.push((730, 710, 710));
        // Ferramenta do agente sem grupo próprio: no grupo do agente.
        rows.push((740, 711, 711));
        // Ferramenta do agente sem terminal: no grupo do cano.
        rows.push((750, 601, 600));
        // Servidor de dev aberto pelo terminal do painel, filho direto do hangar-server.
        rows.push((760, SERVER, 760));
        // Grupo que tem o servidor tmux (pai de uma raiz protegida).
        rows.push((770, 700, 700));
        // Grupo do ancestral do hangar-server.
        rows.push((780, 400, 400));
        let t = table(&rows);
        for pid in [730, 740, 750, 760, 770, 780, SERVER] {
            assert_eq!(stoppable_group(&t, pid, SERVER, &protected()), None, "pid {pid}");
        }
        for pid in [730, 740, 750, 770, 780] {
            let (scope, members) = target(&t, pid, SERVER, &protected()).unwrap();
            assert_eq!((scope, pids(&members)), (Scope::Tree, vec![pid]), "pid {pid}");
        }
        // O próprio pid é protegido: o filho direto do servidor, o servidor, os ancestrais, a raiz, o
        // agente, o cano, o pai da raiz.
        for pid in [760, SERVER, 400, 710, 711, 600, 601, 700, 1] {
            assert_eq!(target(&t, pid, SERVER, &protected()), None, "pid {pid}");
        }
    }

    #[test]
    fn a_tree_that_holds_a_protected_process_is_refused() {
        // 800 → 801 → 802 (protegido): 801 é o pai de um protegido, e 800 levaria os dois junto.
        let mut rows = machine();
        rows.extend([(800, 1, 800), (801, 800, 800), (802, 801, 802)]);
        let mut guarded = protected();
        guarded.insert(802);
        assert_eq!(target(&table(&rows), 800, SERVER, &guarded), None);
    }

    #[test]
    fn group_zero_or_one_and_an_unknown_pid_are_refused() {
        let t = table(&[(1, 0, 1), (30, 1, 1), (31, 1, 0)]);
        for pid in [30, 31, 99] { assert_eq!(stoppable_group(&t, pid, SERVER, &HashSet::new()), None, "pid {pid}"); }
        assert_eq!(target(&t, 99, SERVER, &HashSet::new()), None);
    }

    #[test]
    fn the_stat_line_is_cut_at_the_last_parenthesis() {
        let tail = "0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 12345 9000";
        assert_eq!(parse_stat(42, &format!("42 (node) S 7 42 42 {tail}")), Some(Proc { pid: 42, ppid: 7, pgid: 42, start: 12345, state: b'S' }));
        assert_eq!(parse_stat(43, &format!("43 (a) b (c) R 9 11 11 {tail}")), Some(Proc { pid: 43, ppid: 9, pgid: 11, start: 12345, state: b'R' }));
        assert!(parse_stat(46, &format!("46 (node) Z 7 46 46 {tail}")).unwrap().exited());
        assert_eq!(parse_stat(44, "44 (cortado"), None);
        assert_eq!(parse_stat(45, "45 (sem início) S 7 45 45 0"), None);
    }

    /// Roteiro: cada `table` tira a próxima tabela (a última se repete); sinais e esperas ficam anotados.
    struct Script { tables: VecDeque<io::Result<Table>>, last: Option<Table>, signals: Vec<(u32, Signal)>,
        pid_signals: Vec<(u32, Signal)>, alive: bool, slept: Duration }

    impl Script {
        fn new(tables: Vec<Table>) -> Self {
            Self { tables: tables.into_iter().map(Ok).collect(), last: None, signals: Vec::new(), pid_signals: Vec::new(),
                alive: true, slept: Duration::ZERO }
        }
    }

    impl System for Script {
        fn table(&mut self) -> io::Result<Table> {
            match self.tables.pop_front() {
                Some(Ok(t)) => { self.last = Some(t.clone()); Ok(t) }
                Some(Err(e)) => Err(e),
                None => Ok(self.last.clone().expect("roteiro sem tabela")),
            }
        }
        fn signal_group(&mut self, pgid: u32, signal: Signal) -> io::Result<bool> {
            self.signals.push((pgid, signal));
            Ok(self.alive)
        }
        fn signal_pid(&mut self, pid: u32, signal: Signal) -> io::Result<bool> {
            self.pid_signals.push((pid, signal));
            Ok(self.alive)
        }
        fn sleep(&mut self, duration: Duration) { self.slept += duration; }
    }

    fn without(rows: &[(u32, u32, u32)], gone: &[u32]) -> Table {
        table(&rows.iter().copied().filter(|(pid, ..)| !gone.contains(pid)).collect::<Vec<_>>())
    }

    fn sorted(mut signals: Vec<(u32, Signal)>) -> Vec<(u32, Signal)> {
        signals.sort_by_key(|(pid, signal)| (*signal == Signal::Kill, *pid));
        signals
    }

    #[test]
    fn term_is_enough_when_the_group_leaves_within_the_grace() {
        let rows = machine();
        let mut sys = Script::new(vec![table(&rows), table(&rows), without(&rows, &[720, 721])]);
        assert_eq!(stop(&mut sys, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Group)));
        assert_eq!(sys.signals, vec![(720, Signal::Term)]);
        assert!(sys.pid_signals.is_empty());
        assert_eq!(sys.slept, POLL * 2);
    }

    #[test]
    fn kill_follows_after_the_grace_while_an_original_member_stays_in_the_group() {
        let rows = machine();
        let mut sys = Script::new(vec![table(&rows), without(&rows, &[721])]);
        assert_eq!(stop(&mut sys, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Killed, Some(Scope::Group)));
        assert_eq!(sys.signals, vec![(720, Signal::Term), (720, Signal::Kill)]);
        assert_eq!(sys.slept, GRACE);
    }

    #[test]
    fn a_reused_group_number_never_gets_kill() {
        let rows = machine();
        // Os dois saíram; um processo novo, de outra árvore, ganhou o mesmo número de grupo.
        let mut reused = without(&rows, &[720, 721]);
        reused.insert(900, proc(900, 1, 720, 0));
        let mut sys = Script::new(vec![table(&rows), reused]);
        assert_eq!(stop(&mut sys, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Group)));
        assert_eq!(sys.signals, vec![(720, Signal::Term)]);
    }

    #[test]
    fn in_a_shared_group_only_the_tree_of_the_listener_gets_term() {
        let rows = shared();
        let mut sys = Script::new(vec![table(&rows), without(&rows, &[731, 732])]);
        assert_eq!(stop(&mut sys, 731, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Tree)));
        // Nem o grupo do pane nem o npm 730, pai do que escuta.
        assert!(sys.signals.is_empty());
        assert_eq!(sorted(sys.pid_signals), vec![(731, Signal::Term), (732, Signal::Term)]);
        assert_eq!(sys.slept, POLL);
    }

    #[test]
    fn in_the_tree_kill_goes_only_to_an_original_still_alive() {
        let rows = shared();
        // O node 731 saiu e o esbuild 732, reparentado ao init, ignorou o TERM.
        let mut left = without(&rows, &[731]);
        left.insert(732, proc(732, 1, 710, 0));
        let mut sys = Script::new(vec![table(&rows), left]);
        assert_eq!(stop(&mut sys, 731, 0, SERVER, &protected()).unwrap(), (Stopped::Killed, Some(Scope::Tree)));
        assert_eq!(sorted(sys.pid_signals), vec![(731, Signal::Term), (732, Signal::Term), (732, Signal::Kill)]);
        assert_eq!(sys.slept, GRACE);
    }

    #[test]
    fn in_the_tree_a_reused_pid_never_gets_kill() {
        let rows = shared();
        // Os dois saíram; um processo novo ganhou o pid 732.
        let mut reused = without(&rows, &[731, 732]);
        reused.insert(732, proc(732, 710, 710, 99));
        let mut sys = Script::new(vec![table(&rows), reused]);
        assert_eq!(stop(&mut sys, 731, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Tree)));
        assert!(sys.pid_signals.iter().all(|(_, signal)| *signal == Signal::Term));
    }

    #[test]
    fn a_tree_that_left_before_term_is_gone() {
        let mut sys = Script::new(vec![table(&shared())]);
        sys.alive = false;
        assert_eq!(stop(&mut sys, 731, 0, SERVER, &protected()).unwrap(), (Stopped::Gone, Some(Scope::Tree)));
        assert_eq!(sys.slept, Duration::ZERO);
    }

    #[test]
    fn a_pid_already_gone_is_not_signaled_and_a_protected_listener_is_refused_before_any_signal() {
        let rows = shared();
        let mut gone = Script::new(vec![without(&rows, &[721])]);
        assert_eq!(stop(&mut gone, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Gone, None));
        for pid in [711, 710, 601] {
            let mut refused = Script::new(vec![table(&rows)]);
            assert!(matches!(stop(&mut refused, pid, 0, SERVER, &protected()), Err(Error::SharedGroup)), "pid {pid}");
            assert!(refused.signals.is_empty() && refused.pid_signals.is_empty(), "pid {pid}");
        }
        assert!(gone.signals.is_empty() && gone.pid_signals.is_empty());
    }

    #[test]
    fn a_table_that_cannot_be_read_is_an_error_and_never_leads_to_kill() {
        let rows = machine();
        let mut first = Script { tables: VecDeque::from([Err(io::ErrorKind::PermissionDenied.into())]), ..Script::new(vec![]) };
        assert!(matches!(stop(&mut first, 721, 0, SERVER, &protected()), Err(Error::Io(_))));
        let mut poll = Script::new(vec![table(&rows)]);
        poll.tables.push_back(Err(io::ErrorKind::PermissionDenied.into()));
        assert!(matches!(stop(&mut poll, 721, 0, SERVER, &protected()), Err(Error::Io(_))));
        assert!(first.signals.is_empty());
        assert_eq!(poll.signals, vec![(720, Signal::Term)]);
        let mut tree = Script::new(vec![table(&shared())]);
        tree.tables.push_back(Err(io::ErrorKind::PermissionDenied.into()));
        assert!(matches!(stop(&mut tree, 731, 0, SERVER, &protected()), Err(Error::Io(_))));
        assert!(tree.pid_signals.iter().all(|(_, signal)| *signal == Signal::Term));
    }

    #[test]
    fn a_group_that_left_before_term_is_gone() {
        let mut sys = Script::new(vec![table(&machine())]);
        sys.alive = false;
        assert_eq!(stop(&mut sys, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Gone, Some(Scope::Group)));
        assert_eq!(sys.slept, Duration::ZERO);
    }

    #[test]
    fn zombies_left_after_term_count_as_exited_and_never_get_kill() {
        // O shell do pane ainda não recolheu o npm 720 nem o node 721.
        let rows = machine();
        let mut group = Script::new(vec![table(&rows), zombies(table(&rows), &[720, 721])]);
        assert_eq!(stop(&mut group, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Group)));
        assert_eq!(group.signals, vec![(720, Signal::Term)]);
        assert_eq!(group.slept, POLL);
        let rows = shared();
        let mut tree = Script::new(vec![table(&rows), zombies(table(&rows), &[731, 732])]);
        assert_eq!(stop(&mut tree, 731, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Tree)));
        assert!(tree.pid_signals.iter().all(|(_, signal)| *signal == Signal::Term));
        // Já zumbi antes de qualquer sinal: saiu.
        let mut before = Script::new(vec![zombies(table(&machine()), &[721])]);
        assert_eq!(stop(&mut before, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Gone, None));
        assert!(before.signals.is_empty() && before.pid_signals.is_empty());
    }

    #[test]
    fn in_the_group_a_reused_pid_never_gets_kill() {
        let rows = machine();
        // Os dois saíram; um processo novo ganhou o pid 720 e virou líder de um grupo 720.
        let mut reused = without(&rows, &[720, 721]);
        reused.insert(720, proc(720, 1, 720, 99));
        let mut sys = Script::new(vec![table(&rows), reused]);
        assert_eq!(stop(&mut sys, 721, 0, SERVER, &protected()).unwrap(), (Stopped::Terminated, Some(Scope::Group)));
        assert_eq!(sys.signals, vec![(720, Signal::Term)]);
    }

    #[test]
    fn a_listener_whose_start_changed_is_another_process_and_gets_no_signal() {
        // Entre ler quem escuta e parar, o node 721 saiu e o pid voltou noutro processo.
        let mut sys = Script::new(vec![table(&machine())]);
        assert!(matches!(stop(&mut sys, 721, 7, SERVER, &protected()), Err(Error::Reused)));
        assert!(sys.signals.is_empty() && sys.pid_signals.is_empty());
    }
}
