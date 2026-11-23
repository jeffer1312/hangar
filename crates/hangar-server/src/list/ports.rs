//! Portas TCP em LISTEN de cada sessão da lista do dono. Os processos da sessão vêm do escopo do
//! systemd dela (pega o `nohup … &` que o init adotou), da descendência da raiz (pane ou cano) e dos
//! terminais de atalho dela. Agente, cano e app-server do Codex ficam fora: as portas de loopback
//! deles não são servidores da pessoa. Quem escuta fora disso (`setsid`, outro pai) é da sessão cuja
//! `CP_SESSION_KEY` herdou; navegador com `--remote-debugging-port` nunca conta.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use hangar_api::session::{PortEntry, SessionRow};
use hangar_session_ports::{ProcessInfo, Scan};
use serde_json::Value;

use super::discover::{Problems, descendants};
use super::discover_other::{Dirs, sidecars};
use super::mux::Pane;
use super::procs::{CHILDREN_TTL, ChildrenMap};

/// Leitura dos sockets falhou: só log, a linha sai sem `ports`.
pub(crate) const UNAVAILABLE: &str = "session_ports_unavailable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope { Pane, Cano }

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
pub(crate) struct Root { pub pid: u32, pub scope: Scope }

/// (caminho do cgroup, pids dele) quando a raiz está num escopo só da sessão. Duas leituras pequenas.
fn system_scope(root: Root) -> Option<(String, Vec<u32>)> {
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

fn pid_of(value: &Value) -> Option<u32> { value.as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0) }

/// As raízes de cada linha e os pids que nunca contam.
fn roots(rows: &[SessionRow], panes: &[Pane], shortcuts: &[(String, u32)], agent_pids: &HashMap<String, u32>,
         children: &ChildrenMap, dirs: &Dirs) -> (BTreeMap<String, Vec<Root>>, HashSet<u32>) {
    let by_name: HashMap<&str, &SessionRow> = rows.iter().map(|row| (row.name.as_str(), row)).collect();
    let mut roots: BTreeMap<String, Vec<Root>> = BTreeMap::new();
    let mut excluded: HashSet<u32> = agent_pids.values().copied().collect();
    let mut add = |name: &str, pid: u32, scope: Scope| roots.entry(name.to_owned()).or_default().push(Root { pid, scope });
    for pane in panes.iter().filter(|pane| !pane.dead && by_name.contains_key(pane.session.as_str())) {
        if let Some(pid) = pane.pid.filter(|pid| *pid > 0) { add(&pane.session, pid, Scope::Pane); }
    }
    for (owner, pid) in shortcuts.iter().filter(|(owner, _)| by_name.contains_key(owner.as_str())) { add(owner, *pid, Scope::Pane); }
    // Os sidecars já foram lidos pela descoberta desta rodada: aqui sai do cache, e o problema já foi ao diário.
    let mut ignored = Problems::default();
    let headless = sidecars(&dirs.home.join(".hangar").join("claude-headless"), "", &mut ignored);
    let codex = sidecars(&dirs.home.join(".hangar").join("codex-sessions"), "", &mut ignored);
    for (_, meta) in headless.iter().chain(codex.iter()) {
        let Some(row) = meta.get("name").and_then(Value::as_str).and_then(|name| by_name.get(name)) else { continue };
        if let Some(app) = meta.get("app_pid").and_then(pid_of) { excluded.insert(app); }
        // ponytail: pid de cano morto pode ter sido reciclado; a marca de adormecida tira o caso comum.
        let Some(cano) = meta.get("cano").and_then(|cano| cano.get("pid")).and_then(pid_of) else { continue };
        if !row.headless || row.asleep { continue; }
        // O cano e o agente que ele lança.
        excluded.insert(cano);
        excluded.extend(children.get(&i64::from(cano)).into_iter().flatten().filter_map(|pid| u32::try_from(*pid).ok()));
        add(&row.name, cano, Scope::Cano);
    }
    (roots, excluded)
}

/// Os pids de cada sessão. Um escopo visto em duas sessões não é de nenhuma.
pub(crate) fn owned(roots: &BTreeMap<String, Vec<Root>>, excluded: &HashSet<u32>, children: &ChildrenMap,
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
pub(crate) struct Socket { pub port: u16, pub pid: Option<u32>, pub process: String }

/// Teto de fds lidos por varredura atrás do dono de um socket de fora.
const FD_BUDGET: usize = 50_000;
/// Varreduras sem achar o dono antes de desistir dele enquanto o socket existir.
const ATTEMPTS: u8 = 3;

enum Holder { Pid(u32), Missing(u8) }

/// Dono de cada socket de fora e o que cada pid que escuta herdou. Cada entrada sai quando o socket
/// ou o pid some, então a varredura de `/proc` só roda para socket novo.
#[derive(Default)]
pub(crate) struct Outsiders { inodes: HashMap<u64, Holder>, procs: HashMap<u32, ProcessInfo> }

/// Os sockets da rodada, em ordem de porta, com o dono dos de fora quando se acha.
pub(crate) fn resolve(cache: &mut Outsiders, scan: &Scan, alive: &dyn Fn(u32) -> bool,
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
                (None, Some(Holder::Missing(n))) => Holder::Missing(n + 1),
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
pub(crate) fn assign(owned: &BTreeMap<String, BTreeSet<u32>>, keys: &HashMap<String, String>, excluded: &HashSet<u32>,
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

/// A última leitura, pelos mesmos candidatos: o tique e um `GET` no mesmo instante leem uma vez.
struct LastScan(Mutex<Option<(Instant, Vec<u32>, Arc<Scan>)>>);

impl LastScan {
    const fn new() -> Self { Self(Mutex::new(None)) }

    fn read(&self, candidates: Vec<u32>, scan: impl FnOnce(&[u32]) -> std::io::Result<Scan>) -> std::io::Result<Arc<Scan>> {
        let mut last = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, seen, found)) = last.as_ref() && *seen == candidates && at.elapsed() < CHILDREN_TTL {
            return Ok(found.clone());
        }
        let found = Arc::new(scan(&candidates)?);
        *last = Some((Instant::now(), candidates, found.clone()));
        Ok(found)
    }

    fn forget(&self) { *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None; }
}

static LAST: LastScan = LastScan::new();
static OUTSIDERS: LazyLock<Mutex<Outsiders>> = LazyLock::new(Mutex::default);
static FAILING: AtomicBool = AtomicBool::new(false);

fn listening(candidates: Vec<u32>) -> std::io::Result<Arc<Scan>> { LAST.read(candidates, hangar_session_ports::scan) }

/// Depois de parar um servidor (`port_stop`): a próxima rodada relê os sockets em vez de servir a
/// leitura guardada, que ainda tem a porta. O mapa de filhos não precisa: pid morto não tem socket.
pub(crate) fn forget_scan() { LAST.forget(); }

/// Bloqueante (proc e sockets): chamar fora da thread do runtime.
pub(crate) fn fill(rows: &mut [SessionRow], panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap, dirs: &Dirs) {
    if rows.is_empty() { return; }
    let (roots, excluded) = roots(rows, panes, &super::local_shortcuts::owner_pids(), agent_pids, children, dirs);
    let owned = owned(&roots, &excluded, children, &system_scope);
    let candidates: Vec<u32> = owned.values().flatten().copied().collect::<BTreeSet<_>>().into_iter().collect();
    let scan = match listening(candidates) {
        Ok(found) => { FAILING.store(false, Ordering::Relaxed); found }
        Err(error) => {
            // Uma linha por queda: a lista roda a cada 1,5 s.
            if !FAILING.swap(true, Ordering::Relaxed) {
                tracing::warn!(code = UNAVAILABLE, kind = ?error.kind(), "portas das sessões ilegíveis; linhas sem portas");
            }
            return;
        }
    };
    let keys = session_keys(rows);
    let mut cache = OUTSIDERS.lock().unwrap_or_else(|e| e.into_inner());
    let sockets = resolve(&mut cache, &scan, &hangar_session_ports::alive,
        &mut |wanted| hangar_session_ports::holders(wanted, FD_BUDGET), &mut hangar_session_ports::process_info);
    let mut by_session = assign(&owned, &keys, &excluded, &sockets, &cache.procs);
    drop(cache);
    for row in rows.iter_mut() { row.ports = by_session.remove(&row.name).unwrap_or_default(); }
}

/// A chave de runtime de cada linha, para o pid que a herdou (`assign`).
pub(crate) fn session_keys(rows: &[SessionRow]) -> HashMap<String, String> {
    rows.iter().filter_map(|row| Some((super::links::runtime_key(row.lifecycle_id.as_deref())?.to_owned(), row.name.clone()))).collect()
}

/// Para a parada de servidor (`port_stop`): os pids de cada sessão, o que a lista nunca conta e o que
/// a parada nunca sinaliza (raízes de todas as sessões, agentes, canos e app-servers do Codex).
pub(crate) struct StopFacts { owned: BTreeMap<String, BTreeSet<u32>>, excluded: HashSet<u32>, guarded: HashSet<u32>,
    keys: HashMap<String, String>, name: String }

/// `keys` vem das linhas publicadas (`session_keys`): as da descoberta ainda não têm a vida de todas.
/// Bloqueante.
pub(crate) fn of_session(rows: &[SessionRow], panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap,
                         dirs: &Dirs, keys: HashMap<String, String>, name: &str) -> StopFacts {
    let (roots, excluded) = roots(rows, panes, &super::local_shortcuts::owner_pids(), agent_pids, children, dirs);
    let owned = owned(&roots, &excluded, children, &system_scope);
    let mut guarded = excluded.clone();
    guarded.extend(roots.values().flatten().map(|root| root.pid));
    StopFacts { owned, excluded, guarded, keys, name: name.to_owned() }
}

impl StopFacts {
    pub(crate) fn guarded(&self) -> &HashSet<u32> { &self.guarded }

    /// Quem escuta em `port` pela sessão, relido agora pela regra da lista (`resolve` + `assign`): a
    /// árvore e o escopo dela e, fora deles, quem herdou a chave dela. Com o pid vai o início dele
    /// (`hangar_port_stop::start_of`), que a parada confere antes de sinalizar. Bloqueante.
    pub(crate) fn listener(&self, port: u16) -> std::io::Result<Option<(u32, u64)>> {
        let candidates: Vec<u32> = self.owned.values().flatten().copied().collect::<BTreeSet<_>>().into_iter().collect();
        let scan = hangar_session_ports::scan(&candidates)?;
        let Some(pid) = self.listener_in(scan, port, &hangar_session_ports::alive,
            &mut |wanted| hangar_session_ports::holders(wanted, FD_BUDGET), &mut hangar_session_ports::process_info) else { return Ok(None) };
        Ok(hangar_port_stop::start_of(pid)?.map(|start| (pid, start)))
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
mod tests {
    use super::*;

    fn tree(pairs: &[(i64, i64)]) -> ChildrenMap {
        let mut map = ChildrenMap::new();
        for (parent, child) in pairs { map.entry(*parent).or_default().push(*child); }
        map
    }

    fn roots_of(list: &[(&str, u32, Scope)]) -> BTreeMap<String, Vec<Root>> {
        let mut out: BTreeMap<String, Vec<Root>> = BTreeMap::new();
        for (name, pid, scope) in list { out.entry((*name).to_owned()).or_default().push(Root { pid: *pid, scope: *scope }); }
        out
    }

    fn set(pids: &[u32]) -> BTreeSet<u32> { pids.iter().copied().collect() }

    fn dirs_at(home: &std::path::Path) -> Dirs {
        Dirs { home: home.to_path_buf(), claude: home.join(".claude"), codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi"), omp_config: home.join(".omp"), omp_agent: home.join(".omp/agent"),
            kimi_home: home.join(".kimi-code") }
    }

    #[test]
    fn the_session_scope_brings_a_process_that_lost_its_parent() {
        // pane 10 → agente 11; o `nohup` 50 foi adotado pelo init, mas segue no escopo do pane.
        let children = tree(&[(10, 11), (1, 50)]);
        let scope = |root: Root| (root.pid == 10).then(|| ("/app.slice/tmux-spawn-a.scope".to_owned(), vec![10, 11, 50]));
        let owned = owned(&roots_of(&[("s", 10, Scope::Pane)]), &HashSet::from([11]), &children, &scope);
        assert_eq!(owned["s"], set(&[10, 50]));
    }

    #[test]
    fn without_a_scope_the_descendants_of_the_root_count() {
        // Cano 20 → agente 21 → ferramenta 22 → servidor 23; o 99 é de fora.
        let children = tree(&[(20, 21), (21, 22), (22, 23), (1, 99)]);
        let owned = owned(&roots_of(&[("h", 20, Scope::Cano)]), &HashSet::from([20, 21]), &children, &|_| None);
        assert_eq!(owned["h"], set(&[22, 23]));
    }

    #[test]
    fn a_scope_shared_by_two_sessions_belongs_to_neither() {
        let children = tree(&[(10, 12), (30, 31)]);
        let scope = |_: Root| Some(("/tmux-server-antigo.scope".to_owned(), vec![10, 12, 30, 31, 77]));
        let owned = owned(&roots_of(&[("a", 10, Scope::Pane), ("b", 30, Scope::Pane)]), &HashSet::new(), &children, &scope);
        assert_eq!((owned["a"].clone(), owned["b"].clone()), (set(&[10, 12]), set(&[30, 31])));
    }

    #[test]
    fn only_the_per_session_scopes_are_trusted() {
        assert!(Scope::Pane.own("tmux-spawn-7ba9eb0b.scope"));
        assert!(Scope::Cano.own("run-p144970-i151514.scope"));
        for unit in ["run-p1-i2.scope", "app-kitty.scope", "session-2.scope", "hangar-backend.service"] { assert!(!Scope::Pane.own(unit), "{unit}"); }
        for unit in ["tmux-spawn-a.scope", "hangar-backend.service"] { assert!(!Scope::Cano.own(unit), "{unit}"); }
    }

    #[test]
    fn the_shortcut_terminal_counts_for_its_owner_and_the_agent_never_does() {
        let row = |name: &str| -> SessionRow { serde_json::from_value(serde_json::json!({"name": name})).unwrap() };
        let pane = |session: &str, pid: u32| Pane { session: session.into(), active: true, pid: Some(pid), cwd: String::new(),
            pane_id: "%1".into(), hidden: false, provider: None, session_created: None, window_index: None, pane_index: None, dead: false };
        let home = tempfile::tempdir().unwrap();
        let dirs = dirs_at(home.path());
        // Pane 10 é o próprio agente; o atalho `shortcut-x` (pane 40 → npm 41) é da sessão `s`.
        let children = tree(&[(40, 41)]);
        let (roots, excluded) = roots(&[row("s")], &[pane("s", 10), pane("shortcut-x", 40)], &[("s".into(), 40), ("outra".into(), 60)],
            &HashMap::from([("s".to_owned(), 10)]), &children, &dirs);
        assert_eq!(roots["s"], vec![Root { pid: 10, scope: Scope::Pane }, Root { pid: 40, scope: Scope::Pane }]);
        assert!(!roots.contains_key("shortcut-x") && !roots.contains_key("outra"));
        let owned = owned(&roots, &excluded, &children, &|_| None);
        assert_eq!(owned["s"], set(&[40, 41]));
        let sockets = [socket(3000, Some(41), "node"), socket(8123, Some(10), "claude"), socket(9000, Some(99), "x")];
        let ports = assign(&owned, &HashMap::new(), &excluded, &sockets, &HashMap::new());
        assert_eq!(ports["s"], vec![entry(3000, 41, "node", false)]);
    }

    fn socket(port: u16, pid: Option<u32>, process: &str) -> Socket { Socket { port, pid, process: process.into() } }
    fn entry(port: u16, pid: u32, process: &str, shared: bool) -> PortEntry { PortEntry { port, process: process.into(), pid, shared } }
    fn key(key: &str) -> ProcessInfo { ProcessInfo { session_key: Some(key.into()), ..ProcessInfo::default() } }

    #[test]
    fn a_listener_outside_every_tree_belongs_to_the_session_of_its_inherited_key() {
        // `setsid` 50 saiu da árvore de `s`; 60 herdou a chave de uma sessão fechada; 70 não herdou nada.
        let owned = BTreeMap::from([("s".to_owned(), set(&[11])), ("t".to_owned(), set(&[21]))]);
        let keys = HashMap::from([("ks".to_owned(), "s".to_owned()), ("kt".to_owned(), "t".to_owned())]);
        let procs = HashMap::from([(50, key("ks")), (60, key("gone")), (70, ProcessInfo::default())]);
        let sockets = [socket(3000, Some(11), "node"), socket(8125, Some(50), "python3"), socket(8126, Some(60), "x"), socket(8127, Some(70), "y")];
        let ports = assign(&owned, &keys, &HashSet::new(), &sockets, &procs);
        assert_eq!(ports["s"], vec![entry(3000, 11, "node", false), entry(8125, 50, "python3", false)]);
        assert!(!ports.contains_key("t"));
    }

    #[test]
    fn the_tree_wins_over_the_key_and_excluded_pids_never_enter_by_it() {
        // 21 está na árvore de `t` mas herdou a chave de `s`; 10 é o agente de `s`.
        let owned = BTreeMap::from([("s".to_owned(), set(&[11])), ("t".to_owned(), set(&[21]))]);
        let keys = HashMap::from([("ks".to_owned(), "s".to_owned())]);
        let procs = HashMap::from([(21, key("ks")), (10, key("ks"))]);
        let sockets = [socket(4000, Some(21), "vite"), socket(8123, Some(10), "claude")];
        let ports = assign(&owned, &keys, &HashSet::from([10]), &sockets, &procs);
        assert_eq!((ports.get("s"), ports["t"].clone()), (None, vec![entry(4000, 21, "vite", false)]));
    }

    #[test]
    fn a_devtools_browser_is_never_a_port_of_the_session() {
        let owned = BTreeMap::from([("s".to_owned(), set(&[11, 12]))]);
        let keys = HashMap::from([("ks".to_owned(), "s".to_owned())]);
        let devtools = |key: Option<&str>| ProcessInfo { session_key: key.map(str::to_owned), devtools: true, ..ProcessInfo::default() };
        // 12 está na árvore; 50 entraria pela chave.
        let procs = HashMap::from([(12, devtools(None)), (50, devtools(Some("ks")))]);
        let sockets = [socket(3000, Some(11), "node"), socket(9333, Some(12), "chrome"), socket(9334, Some(50), "chrome")];
        assert_eq!(assign(&owned, &keys, &HashSet::new(), &sockets, &procs)["s"], vec![entry(3000, 11, "node", false)]);
    }

    #[test]
    fn a_port_another_process_also_listens_on_is_shared() {
        // 8126: v4 é de `s`, v6 de um dono desconhecido. 5432: `s` e `t` na mesma porta. 3000: dois
        // processos da própria `s`.
        let owned = BTreeMap::from([("s".to_owned(), set(&[11, 12])), ("t".to_owned(), set(&[21]))]);
        let sockets = [socket(3000, Some(11), "node"), socket(3000, Some(12), "node"), socket(5432, Some(11), "pg"),
            socket(5432, Some(21), "pg"), socket(8126, None, ""), socket(8126, Some(12), "python3")];
        let ports = assign(&owned, &HashMap::new(), &HashSet::new(), &sockets, &HashMap::new());
        assert_eq!(ports["s"], vec![entry(3000, 11, "node", false), entry(5432, 11, "pg", true), entry(8126, 12, "python3", true)]);
        assert_eq!(ports["t"], vec![entry(5432, 21, "pg", true)]);
    }

    fn scan_of(listeners: &[(u16, u32)], outside: &[(u16, u64)]) -> Scan {
        Scan {
            listeners: listeners.iter().map(|&(port, pid)| hangar_session_ports::Listener { port, pid, process: "node".into() }).collect(),
            outside: outside.iter().map(|&(port, inode)| hangar_session_ports::Outside { port, inode: Some(inode) }).collect(),
        }
    }

    #[test]
    fn an_outside_owner_is_searched_once_and_read_once_while_it_listens() {
        let mut cache = Outsiders::default();
        let (mut walks, mut reads) = (Vec::new(), Vec::new());
        let scan = scan_of(&[(3000, 11)], &[(8125, 900)]);
        for _ in 0..3 {
            let sockets = resolve(&mut cache, &scan, &|_| true,
                &mut |wanted| { walks.push(wanted.clone()); HashMap::from([(900, 50)]) },
                &mut |pid| { reads.push(pid); ProcessInfo { process: format!("p{pid}"), ..ProcessInfo::default() } });
            assert_eq!(sockets, vec![socket(3000, Some(11), "node"), socket(8125, Some(50), "p50")]);
        }
        assert_eq!(walks, vec![HashSet::from([900])]);
        assert_eq!(reads.iter().copied().collect::<BTreeSet<_>>(), set(&[11, 50]));
        assert_eq!(reads.len(), 2);
        // O socket fechou: o dono sai do cache e o pid deixa de ser lido.
        resolve(&mut cache, &scan_of(&[(3000, 11)], &[]), &|_| true, &mut |_| unreachable!(), &mut |_| unreachable!());
        assert!(cache.inodes.is_empty() && !cache.procs.contains_key(&50));
    }

    #[test]
    fn an_owner_never_found_stops_being_searched_and_a_dead_owner_is_searched_again() {
        let mut cache = Outsiders::default();
        let scan = scan_of(&[], &[(53, 1), (8125, 2)]);
        let mut walks = 0;
        let mut alive_50 = true;
        for _ in 0..5 {
            let alive = |pid: u32| pid != 50 || alive_50;
            resolve(&mut cache, &scan, &alive, &mut |_| { walks += 1; HashMap::from([(2, 50)]) }, &mut |_| ProcessInfo::default());
        }
        // A 1ª acha o 2 e erra o 1; mais duas erram o 1; depois ninguém procura.
        assert_eq!(walks, 3);
        alive_50 = false;
        let alive = |pid: u32| pid != 50 || alive_50;
        let sockets = resolve(&mut cache, &scan, &alive, &mut |wanted| { walks += 1; assert_eq!(*wanted, HashSet::from([2])); HashMap::from([(2, 51)]) },
            &mut |_| ProcessInfo::default());
        assert_eq!((walks, sockets.iter().find(|s| s.port == 8125).and_then(|s| s.pid)), (4, Some(51)));
    }

    #[test]
    fn the_stop_finds_the_listener_that_entered_the_session_by_its_inherited_key() {
        // 11 é da árvore de `s`; o `setsid` 50 escuta na 8125 de fora e herdou a chave de `s`; 60 a de `t`.
        let facts = StopFacts { owned: BTreeMap::from([("s".to_owned(), set(&[11])), ("t".to_owned(), set(&[21]))]),
            excluded: HashSet::from([10]), guarded: HashSet::new(),
            keys: HashMap::from([("ks".to_owned(), "s".to_owned()), ("kt".to_owned(), "t".to_owned())]), name: "s".into() };
        let scan = || scan_of(&[(3000, 11)], &[(8125, 900), (8126, 901)]);
        let holders = |wanted: &HashSet<u64>| wanted.iter().map(|inode| (*inode, if *inode == 900 { 50 } else { 60 })).collect();
        let info = |pid: u32| key(if pid == 50 { "ks" } else { "kt" });
        let pick = |port: u16| facts.listener_in(scan(), port, &|_| true, &mut |w| holders(w), &mut |pid| info(pid));
        assert_eq!((pick(3000), pick(8125), pick(8126), pick(9999)), (Some(11), Some(50), None, None));
    }

    #[test]
    fn the_headless_cano_and_the_codex_app_server_stay_out() {
        let row = |v: Value| -> SessionRow { serde_json::from_value(v).unwrap() };
        let home = tempfile::tempdir().unwrap();
        let write = |dir: &str, file: &str, body: Value| {
            let dir = home.path().join(".hangar").join(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), body.to_string()).unwrap();
        };
        write("claude-headless", "h.json", serde_json::json!({"name": "h", "session_id": "x", "cwd": "/", "cano": {"pid": 20}}));
        write("codex-sessions", "cx.json", serde_json::json!({"name": "cx", "app_pid": 70}));
        let dirs = dirs_at(home.path());
        let children = tree(&[(20, 21), (21, 22)]);
        let rows = [row(serde_json::json!({"name": "h", "headless": true})), row(serde_json::json!({"name": "cx", "provider": "codex"}))];
        let (roots, excluded) = roots(&rows, &[], &[], &HashMap::new(), &children, &dirs);
        assert_eq!(roots["h"], vec![Root { pid: 20, scope: Scope::Cano }]);
        assert_eq!(excluded, HashSet::from([20, 21, 70]));
        assert_eq!(owned(&roots, &excluded, &children, &|_| None)["h"], set(&[22]));
    }

    #[test]
    fn after_a_stop_the_next_round_reads_the_sockets_again() {
        let last = LastScan::new();
        let read = |listening: &[(u16, u32)]| { let scan = scan_of(listening, &[]); last.read(vec![11], |_| Ok(scan)).unwrap() };
        assert_eq!(read(&[(3000, 11)]).listeners.len(), 1);
        // Dentro do prazo a leitura guardada serve, e a porta parada ainda apareceria.
        assert_eq!(read(&[]).listeners.len(), 1);
        last.forget();
        assert!(read(&[]).listeners.is_empty());
    }
}
