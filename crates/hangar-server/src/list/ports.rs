//! Portas TCP em LISTEN de cada sessão da lista do dono. A regra de qual porta é de qual sessão é do
//! `hangar_session_ports::owners`; aqui se lê o que é da lista (panes, sidecars, atalhos), se guarda a
//! leitura entre os tiques e se publica nas linhas.
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use hangar_api::session::SessionRow;
use hangar_session_ports::Scan;
use hangar_session_ports::owners::{self, FD_BUDGET, LastScan, Outsiders, PaneProc, Root, Runner};
use serde_json::Value;

use super::discover::Problems;
use super::discover_other::{Dirs, sidecars};
use super::mux::Pane;
use super::procs::{CHILDREN_TTL, ChildrenMap};

pub(crate) use hangar_session_ports::owners::{StopFacts, session_keys};

/// Leitura dos sockets falhou: só log, a linha sai sem `ports`.
pub(crate) const UNAVAILABLE: &str = "session_ports_unavailable";
/// Porta de sessão sem loopback só dela: só log, o túnel segue a ordem de sempre.
pub(crate) const UNREACHED: &str = "session_port_unreached";

fn pid_of(value: &Value) -> Option<u32> { value.as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0) }

/// O cano e o app-server de cada sidecar sem terminal e do Codex.
fn runners(dirs: &Dirs) -> Vec<Runner> {
    // Os sidecars já foram lidos pela descoberta desta rodada: aqui sai do cache, e o problema já foi ao diário.
    let mut ignored = Problems::default();
    let headless = sidecars(&dirs.home.join(".hangar").join("claude-headless"), "", &mut ignored);
    let codex = sidecars(&dirs.home.join(".hangar").join("codex-sessions"), "", &mut ignored);
    headless.iter().chain(codex.iter()).filter_map(|(_, meta)| Some(Runner {
        name: meta.get("name")?.as_str()?.to_owned(),
        app_pid: meta.get("app_pid").and_then(pid_of),
        cano_pid: meta.get("cano").and_then(|cano| cano.get("pid")).and_then(pid_of),
    })).collect()
}

fn roots(rows: &[SessionRow], panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap, dirs: &Dirs)
    -> (std::collections::BTreeMap<String, Vec<Root>>, std::collections::HashSet<u32>) {
    let panes: Vec<PaneProc> = panes.iter().map(|p| PaneProc { session: p.session.clone(), pid: p.pid, dead: p.dead }).collect();
    owners::roots(rows, &panes, &super::local_shortcuts::owner_pids(), agent_pids, children, &runners(dirs))
}

static LAST: LastScan = LastScan::new(CHILDREN_TTL);
static OUTSIDERS: LazyLock<Mutex<Outsiders>> = LazyLock::new(Mutex::default);
static FAILING: AtomicBool = AtomicBool::new(false);
static DIAL_FIRST: LazyLock<Mutex<HashMap<u16, IpAddr>>> = LazyLock::new(Mutex::default);
static UNREACHED_LAST: LazyLock<Mutex<BTreeSet<u16>>> = LazyLock::new(Mutex::default);

/// O loopback que chega ao processo da sessão em `port` quando não é o `127.0.0.1` de sempre, pela última rodada
/// da lista (`port_tunnel`). Sem lista aberta não há rodada, e o túnel segue a ordem de sempre.
pub(crate) fn dial_first(port: u16) -> Option<IpAddr> {
    DIAL_FIRST.lock().unwrap_or_else(|e| e.into_inner()).get(&port).copied()
}

/// Quantas portas o mapa anterior desviava.
fn set_dial_first(found: HashMap<u16, IpAddr>) -> usize {
    std::mem::replace(&mut *DIAL_FIRST.lock().unwrap_or_else(|e| e.into_inner()), found).len()
}

/// As portas que passaram a ficar sem loopback só da sessão desde a rodada anterior.
fn newly_unreached(last: &mut BTreeSet<u16>, now: BTreeSet<u16>) -> Vec<u16> {
    let new = now.difference(last).copied().collect();
    *last = now;
    new
}

/// Uma linha por porta que fica sem caminho só da sessão, não uma por tique: a lista roda a cada 1,5 s.
fn note_unreached(now: BTreeSet<u16>) {
    let new = newly_unreached(&mut UNREACHED_LAST.lock().unwrap_or_else(|e| e.into_inner()), now);
    if !new.is_empty() {
        tracing::warn!(code = UNREACHED, ports = ?new, "porta da sessão sem loopback só dela; o túnel segue a ordem de sempre e pode chegar a outro processo");
    }
}

fn listening(candidates: Vec<u32>) -> std::io::Result<Arc<Scan>> { LAST.read(candidates, hangar_session_ports::scan) }

/// Depois de parar um servidor (`port_stop`): a próxima rodada relê os sockets em vez de servir a
/// leitura guardada, que ainda tem a porta. O mapa de filhos não precisa: pid morto não tem socket.
pub(crate) fn forget_scan() { LAST.forget(); }

/// Bloqueante (proc e sockets): chamar fora da thread do runtime.
pub(crate) fn fill(rows: &mut [SessionRow], panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap, dirs: &Dirs) {
    if rows.is_empty() { set_dial_first(HashMap::new()); note_unreached(BTreeSet::new()); return; }
    let (roots, excluded) = roots(rows, panes, agent_pids, children, dirs);
    let owned = owners::owned(&roots, &excluded, children, &owners::system_scope);
    let candidates: Vec<u32> = owned.values().flatten().copied().collect::<BTreeSet<_>>().into_iter().collect();
    let scan = match listening(candidates) {
        Ok(found) => { FAILING.store(false, Ordering::Relaxed); found }
        Err(error) => {
            let cleared = set_dial_first(HashMap::new());
            note_unreached(BTreeSet::new());
            // Uma linha por queda: a lista roda a cada 1,5 s.
            if !FAILING.swap(true, Ordering::Relaxed) {
                tracing::warn!(code = UNAVAILABLE, kind = ?error.kind(), dial_first_cleared = cleared,
                    "portas das sessões ilegíveis; linhas sem portas e túnel na ordem de sempre");
            }
            return;
        }
    };
    let keys = session_keys(rows);
    let mut cache = OUTSIDERS.lock().unwrap_or_else(|e| e.into_inner());
    let sockets = owners::resolve(&mut cache, &scan, &hangar_session_ports::alive,
        &mut |wanted| hangar_session_ports::holders(wanted, FD_BUDGET), &mut hangar_session_ports::process_info);
    let mut by_session = owners::assign(&owned, &keys, &excluded, &sockets, cache.procs());
    let dial = owners::dial_first(&owned, &keys, &excluded, &sockets, cache.procs());
    drop(cache);
    set_dial_first(dial.first);
    note_unreached(dial.unreached);
    for row in rows.iter_mut() { row.ports = by_session.remove(&row.name).unwrap_or_default(); }
}

/// Para a parada de servidor (`port_stop`). `keys` vem das linhas publicadas (`session_keys`). Bloqueante.
pub(crate) fn of_session(rows: &[SessionRow], panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap,
                         dirs: &Dirs, keys: HashMap<String, String>, name: &str) -> StopFacts {
    let (roots, excluded) = roots(rows, panes, agent_pids, children, dirs);
    StopFacts::new(&roots, excluded, children, keys, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreached_port_is_logged_once_until_it_comes_back() {
        let mut last = BTreeSet::new();
        assert_eq!(newly_unreached(&mut last, BTreeSet::from([3000])), vec![3000]);
        assert_eq!(newly_unreached(&mut last, BTreeSet::from([3000])), Vec::<u16>::new(), "o tique seguinte não repete");
        assert_eq!(newly_unreached(&mut last, BTreeSet::from([3000, 5432])), vec![5432], "só a nova");
        assert_eq!(newly_unreached(&mut last, BTreeSet::new()), Vec::<u16>::new());
        assert_eq!(newly_unreached(&mut last, BTreeSet::from([3000])), vec![3000], "voltou a ficar sem caminho: nova linha");
    }

    #[test]
    fn the_sidecars_give_the_cano_and_the_codex_app_server() {
        let home = tempfile::tempdir().unwrap();
        let write = |dir: &str, file: &str, body: Value| {
            let dir = home.path().join(".hangar").join(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), body.to_string()).unwrap();
        };
        write("claude-headless", "h.json", serde_json::json!({"name": "h", "session_id": "x", "cwd": "/", "cano": {"pid": 20}}));
        write("codex-sessions", "cx.json", serde_json::json!({"name": "cx", "app_pid": 70}));
        let dirs = Dirs { home: home.path().to_path_buf(), claude: home.path().join(".claude"), codex_home: home.path().join(".codex"),
            pi_sessions: home.path().join(".pi"), omp_config: home.path().join(".omp"), omp_agent: home.path().join(".omp/agent"),
            kimi_home: home.path().join(".kimi-code") };
        let mut found = runners(&dirs);
        found.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(found, vec![Runner { name: "cx".into(), app_pid: Some(70), cano_pid: None },
            Runner { name: "h".into(), app_pid: None, cano_pid: Some(20) }]);
    }
}
