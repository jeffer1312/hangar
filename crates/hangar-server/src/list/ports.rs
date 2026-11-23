//! Portas TCP em LISTEN de cada sessão da lista do dono. Os processos da sessão vêm do escopo do
//! systemd dela (pega o `nohup … &` que o init adotou), da descendência da raiz (pane ou cano) e dos
//! terminais de atalho dela. Agente, cano e app-server do Codex ficam fora: as portas de loopback
//! deles não são servidores da pessoa.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hangar_api::session::{PortEntry, SessionRow};
use hangar_session_ports::Listener;
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

/// As portas de cada sessão, em ordem de porta (a do `listening`).
pub(crate) fn assign(owned: &BTreeMap<String, BTreeSet<u32>>, listeners: &[Listener]) -> BTreeMap<String, Vec<PortEntry>> {
    owned.iter().filter_map(|(name, pids)| {
        let ports: Vec<PortEntry> = listeners.iter().filter(|l| pids.contains(&l.pid))
            .map(|l| PortEntry { port: l.port, process: l.process.clone(), pid: l.pid }).collect();
        (!ports.is_empty()).then(|| (name.clone(), ports))
    }).collect()
}

/// A última leitura, pelos mesmos candidatos: o tique e um `GET` no mesmo instante leem uma vez.
static LAST: Mutex<Option<(Instant, Vec<u32>, Arc<Vec<Listener>>)>> = Mutex::new(None);
static FAILING: AtomicBool = AtomicBool::new(false);

fn listening(candidates: Vec<u32>) -> std::io::Result<Arc<Vec<Listener>>> {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, seen, found)) = last.as_ref() && *seen == candidates && at.elapsed() < CHILDREN_TTL {
        return Ok(found.clone());
    }
    let found = Arc::new(hangar_session_ports::listening(&candidates)?);
    *last = Some((Instant::now(), candidates, found.clone()));
    Ok(found)
}

/// Bloqueante (proc e sockets): chamar fora da thread do runtime.
pub(crate) fn fill(rows: &mut [SessionRow], panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap, dirs: &Dirs) {
    let (roots, excluded) = roots(rows, panes, &super::local_shortcuts::owner_pids(), agent_pids, children, dirs);
    let owned = owned(&roots, &excluded, children, &system_scope);
    let candidates: Vec<u32> = owned.values().flatten().copied().collect::<BTreeSet<_>>().into_iter().collect();
    let listeners = match listening(candidates) {
        Ok(found) => { FAILING.store(false, Ordering::Relaxed); found }
        Err(error) => {
            // Uma linha por queda: a lista roda a cada 1,5 s.
            if !FAILING.swap(true, Ordering::Relaxed) {
                tracing::warn!(code = UNAVAILABLE, kind = ?error.kind(), "portas das sessões ilegíveis; linhas sem portas");
            }
            return;
        }
    };
    let mut by_session = assign(&owned, &listeners);
    for row in rows.iter_mut() { row.ports = by_session.remove(&row.name).unwrap_or_default(); }
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
        let l = |port, pid, process: &str| Listener { port, pid, process: process.into() };
        let ports = assign(&owned, &[l(3000, 41, "node"), l(8123, 10, "claude"), l(9000, 99, "x")]);
        assert_eq!(ports["s"], vec![PortEntry { port: 3000, process: "node".into(), pid: 41 }]);
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
}
