//! De qual sessão é cada porta. Os processos da sessão vêm do escopo do systemd dela (pega o
//! `nohup … &` que o init adotou), da descendência da raiz (pane ou cano) e dos terminais de atalho
//! dela. Agente, cano e app-server do Codex ficam fora: as portas de loopback deles não são
//! servidores da pessoa. Quem escuta fora disso (`setsid`, outro pai) é da sessão cuja
//! `CP_SESSION_KEY` herdou; navegador com `--remote-debugging-port` nunca conta.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hangar_api::session::{PortEntry, SessionRow};

use crate::{ProcessInfo, Scan};

/// Pid → filhos diretos.
pub type ChildrenMap = HashMap<i64, Vec<i64>>;

/// Um pane do multiplexador: o da sessão ou o de um terminal de atalho.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneProc { pub session: String, pub pid: Option<u32>, pub dead: bool }

/// O sidecar de uma sessão sem terminal (Claude) ou do Codex: o cano e o app-server dela.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Runner { pub name: String, pub app_pid: Option<u32>, pub cano_pid: Option<u32> }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope { Pane, Cano }

impl Scope {
    /// O tmux abre um escopo por pane e o cano nasce no próprio `systemd-run --scope`. Qualquer outro
    /// grupo (o do servidor tmux antigo, o do login) é de várias sessões.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn own(self, unit: &str) -> bool {
        let prefix = match self { Scope::Pane => "tmux-spawn-", Scope::Cano => "run-" };
        unit.starts_with(prefix) && unit.ends_with(".scope")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Root { pub pid: u32, pub scope: Scope }

/// (caminho do cgroup, pids dele) quando a raiz está num escopo só da sessão. Duas leituras pequenas.
pub fn system_scope(root: Root) -> Option<(String, Vec<u32>)> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_to_string(format!("/proc/{}/cgroup", root.pid)).ok()?;
        let path = raw.lines().find_map(|line| line.strip_prefix("0::"))?.trim().to_owned();
        if !root.scope.own(path.rsplit('/').next()?) { return None; }
        let procs = std::fs::read_to_string(format!("/sys/fs/cgroup{path}/cgroup.procs")).ok()?;
        Some((path, procs.lines().filter_map(|line| line.trim().parse().ok()).collect()))
    }
    #[cfg(not(target_os = "linux"))]
    { let _ = root; None }
}

// ponytail: cópia do `descendants` da lista do servidor; este crate não depende dele.
fn descendants(root: i64, children: &ChildrenMap) -> Vec<i64> {
    let (mut out, mut seen, mut stack) = (Vec::new(), HashSet::new(), vec![root]);
    while let Some(p) = stack.pop() {
        // ppid reciclado no Windows fecha anel no mapa.
        if !seen.insert(p) { continue; }
        out.push(p);
        stack.extend(children.get(&p).into_iter().flatten());
    }
    out
}

/// As raízes de cada linha e os pids que nunca contam. `shortcuts`: (dona, pid do pane do atalho).
pub fn roots(rows: &[SessionRow], panes: &[PaneProc], shortcuts: &[(String, u32)], agent_pids: &HashMap<String, u32>,
             children: &ChildrenMap, runners: &[Runner]) -> (BTreeMap<String, Vec<Root>>, HashSet<u32>) {
    let by_name: HashMap<&str, &SessionRow> = rows.iter().map(|row| (row.name.as_str(), row)).collect();
    let mut roots: BTreeMap<String, Vec<Root>> = BTreeMap::new();
    let mut excluded: HashSet<u32> = agent_pids.values().copied().collect();
    let mut add = |name: &str, pid: u32, scope: Scope| roots.entry(name.to_owned()).or_default().push(Root { pid, scope });
    for pane in panes.iter().filter(|pane| !pane.dead && by_name.contains_key(pane.session.as_str())) {
        if let Some(pid) = pane.pid.filter(|pid| *pid > 0) { add(&pane.session, pid, Scope::Pane); }
    }
    for (owner, pid) in shortcuts.iter().filter(|(owner, _)| by_name.contains_key(owner.as_str())) { add(owner, *pid, Scope::Pane); }
    for runner in runners {
        let Some(row) = by_name.get(runner.name.as_str()) else { continue };
        if let Some(app) = runner.app_pid { excluded.insert(app); }
        // ponytail: pid de cano morto pode ter sido reciclado; a marca de adormecida tira o caso comum.
        let Some(cano) = runner.cano_pid else { continue };
        if !row.headless || row.asleep { continue; }
        // O cano e o agente que ele lança.
        excluded.insert(cano);
        excluded.extend(children.get(&i64::from(cano)).into_iter().flatten().filter_map(|pid| u32::try_from(*pid).ok()));
        add(&row.name, cano, Scope::Cano);
    }
    (roots, excluded)
}

/// Os pids de cada sessão. Um escopo visto em duas sessões não é de nenhuma.
pub fn owned(roots: &BTreeMap<String, Vec<Root>>, excluded: &HashSet<u32>, children: &ChildrenMap,
             scope: &dyn Fn(Root) -> Option<(String, Vec<u32>)>) -> BTreeMap<String, BTreeSet<u32>> {
    let read: Vec<(&str, Root, Option<(String, Vec<u32>)>)> =
        roots.iter().flat_map(|(name, list)| list.iter().map(move |root| (name.as_str(), *root))).map(|(name, root)| (name, root, scope(root))).collect();
    let mut sessions_of: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (name, _, found) in &read {
        if let Some((path, _)) = found { sessions_of.entry(path.as_str()).or_default().insert(name); }
    }
    let mut out: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for (name, root, found) in &read {
        let pids = out.entry((*name).to_owned()).or_default();
        if let Some((path, members)) = found && sessions_of.get(path.as_str()).is_some_and(|s| s.len() == 1) {
            pids.extend(members.iter().copied());
        }
        pids.extend(descendants(i64::from(root.pid), children).into_iter().filter_map(|pid| u32::try_from(pid).ok()));
    }
    for pids in out.values_mut() { pids.retain(|pid| !excluded.contains(pid)); }
    out
}

/// Um socket em LISTEN desta rodada; `pid` vazio = dono desconhecido (outro usuário, fora do teto).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socket { pub port: u16, pub pid: Option<u32>, pub process: String }

/// Teto de fds lidos por varredura atrás do dono de um socket de fora.
pub const FD_BUDGET: usize = 50_000;
/// Varreduras sem achar o dono antes de desistir dele enquanto o socket existir.
const ATTEMPTS: u8 = 3;

enum Holder { Pid(u32), Missing(u8) }

/// Dono de cada socket de fora e o que cada pid que escuta herdou. Cada entrada sai quando o socket
/// ou o pid some, então a varredura de `/proc` só roda para socket novo.
#[derive(Default)]
pub struct Outsiders { inodes: HashMap<u64, Holder>, procs: HashMap<u32, ProcessInfo> }

impl Outsiders {
    pub fn procs(&self) -> &HashMap<u32, ProcessInfo> { &self.procs }
}

/// Os sockets da rodada, em ordem de porta, com o dono dos de fora quando se acha.
pub fn resolve(cache: &mut Outsiders, scan: &Scan, alive: &dyn Fn(u32) -> bool,
               find: &mut dyn FnMut(&HashSet<u64>) -> HashMap<u64, u32>, info: &mut dyn FnMut(u32) -> ProcessInfo) -> Vec<Socket> {
    let live: HashSet<u64> = scan.outside.iter().filter_map(|o| o.inode).collect();
    // Dono morto com o socket vivo: o fd ficou com outro processo; procura de novo.
    cache.inodes.retain(|inode, holder| live.contains(inode) && !matches!(holder, Holder::Pid(pid) if !alive(*pid)));
    let wanted: HashSet<u64> = live.iter().copied()
        .filter(|inode| match cache.inodes.get(inode) { None => true, Some(Holder::Missing(n)) => *n < ATTEMPTS, Some(Holder::Pid(_)) => false })
        .collect();
    if !wanted.is_empty() {
        let found = find(&wanted);
        for inode in wanted {
            let next = match (found.get(&inode), cache.inodes.get(&inode)) {
                (Some(pid), _) => Holder::Pid(*pid),
                (None, Some(Holder::Missing(n))) => {
                    if n + 1 == ATTEMPTS { tracing::debug!(inode, "session ports: socket owner not found, giving up"); }
                    Holder::Missing(n + 1)
                }
                (None, _) => Holder::Missing(1),
            };
            cache.inodes.insert(inode, next);
        }
    }
    let holder = |inode: Option<u64>| match cache.inodes.get(&inode?) { Some(Holder::Pid(pid)) => Some(*pid), _ => None };
    let mut sockets: Vec<Socket> = scan.listeners.iter().map(|l| Socket { port: l.port, pid: Some(l.pid), process: l.process.clone() })
        .chain(scan.outside.iter().map(|o| Socket { port: o.port, pid: holder(o.inode), process: String::new() }))
        .collect();
    let seen: HashSet<u32> = sockets.iter().filter_map(|s| s.pid).collect();
    cache.procs.retain(|pid, _| seen.contains(pid));
    for pid in &seen { cache.procs.entry(*pid).or_insert_with(|| info(*pid)); }
    for socket in sockets.iter_mut().filter(|s| s.process.is_empty()) {
        if let Some(found) = socket.pid.and_then(|pid| cache.procs.get(&pid)) { socket.process = found.process.clone(); }
    }
    sockets.sort_by_key(|s| (s.port, s.pid));
    sockets
}

/// As portas de cada sessão, em ordem de porta. Pid fora de toda árvore e escopo, e fora dos
/// excluídos, entra na sessão da chave que herdou. `shared`: outro socket na mesma porta não é dela.
pub fn assign(owned: &BTreeMap<String, BTreeSet<u32>>, keys: &HashMap<String, String>, excluded: &HashSet<u32>,
              sockets: &[Socket], procs: &HashMap<u32, ProcessInfo>) -> BTreeMap<String, Vec<PortEntry>> {
    let in_tree: HashSet<u32> = owned.values().flatten().copied().collect();
    let mut pids = owned.clone();
    for pid in sockets.iter().filter_map(|s| s.pid).filter(|pid| !in_tree.contains(pid) && !excluded.contains(pid)) {
        if let Some(name) = procs.get(&pid).and_then(|p| p.session_key.as_ref()).and_then(|key| keys.get(key)) {
            pids.entry(name.clone()).or_default().insert(pid);
        }
    }
    let devtools = |pid: &u32| procs.get(pid).is_some_and(|p| p.devtools);
    pids.into_iter().filter_map(|(name, mut mine)| {
        mine.retain(|pid| !devtools(pid));
        let ours = |s: &Socket| s.pid.is_some_and(|pid| mine.contains(&pid));
        let mut ports: BTreeMap<u16, PortEntry> = BTreeMap::new();
        for s in sockets.iter().filter(|s| ours(s)) {
            ports.entry(s.port).or_insert_with(|| PortEntry { port: s.port, process: s.process.clone(), pid: s.pid.unwrap_or_default(), shared: false });
        }
        for entry in ports.values_mut() { entry.shared = sockets.iter().any(|s| s.port == entry.port && !ours(s)); }
        (!ports.is_empty()).then(|| (name, ports.into_values().collect()))
    }).collect()
}

/// A chave de runtime de cada linha, para o pid que a herdou (`assign`).
pub fn session_keys(rows: &[SessionRow]) -> HashMap<String, String> {
    rows.iter().filter_map(|row| Some((hangar_api::session::runtime_key(row.lifecycle_id.as_deref())?.to_owned(), row.name.clone()))).collect()
}

/// A última leitura, pelos mesmos candidatos: o tique e um `GET` no mesmo instante leem uma vez.
pub struct LastScan { ttl: Duration, last: Mutex<Option<(Instant, Vec<u32>, Arc<Scan>)>> }

impl LastScan {
    pub const fn new(ttl: Duration) -> Self { Self { ttl, last: Mutex::new(None) } }

    pub fn read(&self, candidates: Vec<u32>, scan: impl FnOnce(&[u32]) -> std::io::Result<Scan>) -> std::io::Result<Arc<Scan>> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, seen, found)) = last.as_ref() && *seen == candidates && at.elapsed() < self.ttl {
            return Ok(found.clone());
        }
        let found = Arc::new(scan(&candidates)?);
        *last = Some((Instant::now(), candidates, found.clone()));
        Ok(found)
    }

    pub fn forget(&self) { *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None; }
}

/// Para a parada de servidor: os pids de cada sessão, o que a lista nunca conta e o que a parada
/// nunca sinaliza (raízes de todas as sessões, agentes, canos e app-servers do Codex).
pub struct StopFacts { owned: BTreeMap<String, BTreeSet<u32>>, excluded: HashSet<u32>, guarded: HashSet<u32>,
    keys: HashMap<String, String>, name: String }

impl StopFacts {
    /// `keys` vem das linhas publicadas (`session_keys`): as da descoberta ainda não têm a vida de todas.
    /// Bloqueante (lê o escopo de cada raiz).
    pub fn new(roots: &BTreeMap<String, Vec<Root>>, excluded: HashSet<u32>, children: &ChildrenMap, keys: HashMap<String, String>,
               name: &str) -> Self {
        let owned = owned(roots, &excluded, children, &system_scope);
        let mut guarded = excluded.clone();
        guarded.extend(roots.values().flatten().map(|root| root.pid));
        Self { owned, excluded, guarded, keys, name: name.to_owned() }
    }

    pub fn guarded(&self) -> &HashSet<u32> { &self.guarded }

    /// Quem escuta em `port` pela sessão, relido agora pela regra da lista (`resolve` + `assign`): a
    /// árvore e o escopo dela e, fora deles, quem herdou a chave dela. Bloqueante.
    pub fn listener(&self, port: u16) -> std::io::Result<Option<u32>> {
        let candidates: Vec<u32> = self.owned.values().flatten().copied().collect::<BTreeSet<_>>().into_iter().collect();
        let scan = crate::scan(&candidates)?;
        Ok(self.listener_in(scan, port, &crate::alive, &mut |wanted| crate::holders(wanted, FD_BUDGET), &mut crate::process_info))
    }

    fn listener_in(&self, mut scan: Scan, port: u16, alive: &dyn Fn(u32) -> bool,
                   find: &mut dyn FnMut(&HashSet<u64>) -> HashMap<u64, u32>, info: &mut dyn FnMut(u32) -> ProcessInfo) -> Option<u32> {
        // Só a porta pedida: o dono de socket de fora é procurado só para ela.
        scan.listeners.retain(|l| l.port == port);
        scan.outside.retain(|o| o.port == port);
        let mut cache = Outsiders::default();
        let sockets = resolve(&mut cache, &scan, alive, find, info);
        let ports = assign(&self.owned, &self.keys, &self.excluded, &sockets, &cache.procs);
        ports.get(&self.name).and_then(|mine| mine.iter().find(|p| p.port == port)).map(|p| p.pid)
    }
}

#[cfg(test)]
mod tests;
