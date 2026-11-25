use super::*;
use crate::{Listener, Outside};

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

fn row(v: serde_json::Value) -> SessionRow { serde_json::from_value(v).unwrap() }

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
    let pane = |session: &str, pid: u32| PaneProc { session: session.into(), pid: Some(pid), dead: false };
    // Pane 10 é o próprio agente; o atalho `shortcut-x` (pane 40 → npm 41) é da sessão `s`.
    let children = tree(&[(40, 41)]);
    let (roots, excluded) = roots(&[row(serde_json::json!({"name": "s"}))], &[pane("s", 10), pane("shortcut-x", 40)],
        &[("s".into(), 40), ("outra".into(), 60)], &HashMap::from([("s".to_owned(), 10)]), &children, &[]);
    assert_eq!(roots["s"], vec![Root { pid: 10, scope: Scope::Pane }, Root { pid: 40, scope: Scope::Pane }]);
    assert!(!roots.contains_key("shortcut-x") && !roots.contains_key("outra"));
    let owned = owned(&roots, &excluded, &children, &|_| None);
    assert_eq!(owned["s"], set(&[40, 41]));
    let sockets = [socket(3000, Some(41), "node"), socket(8123, Some(10), "claude"), socket(9000, Some(99), "x")];
    let ports = assign(&owned, &HashMap::new(), &excluded, &sockets, &HashMap::new());
    assert_eq!(ports["s"], vec![entry(3000, 41, "node", false)]);
}

#[test]
fn the_headless_cano_and_the_codex_app_server_stay_out() {
    let runners = [Runner { name: "h".into(), app_pid: None, cano_pid: Some(20) }, Runner { name: "cx".into(), app_pid: Some(70), cano_pid: None },
        Runner { name: "gone".into(), app_pid: Some(80), cano_pid: Some(90) }];
    let children = tree(&[(20, 21), (21, 22)]);
    let rows = [row(serde_json::json!({"name": "h", "headless": true})), row(serde_json::json!({"name": "cx", "provider": "codex"}))];
    let (roots, excluded) = roots(&rows, &[], &[], &HashMap::new(), &children, &runners);
    assert_eq!(roots["h"], vec![Root { pid: 20, scope: Scope::Cano }]);
    assert_eq!(excluded, HashSet::from([20, 21, 70]));
    assert_eq!(owned(&roots, &excluded, &children, &|_| None)["h"], set(&[22]));
}

#[test]
fn an_asleep_headless_session_has_no_cano_root() {
    let runners = [Runner { name: "h".into(), app_pid: None, cano_pid: Some(20) }];
    let rows = [row(serde_json::json!({"name": "h", "headless": true, "asleep": true}))];
    let (roots, excluded) = roots(&rows, &[], &[], &HashMap::new(), &ChildrenMap::new(), &runners);
    assert!(roots.is_empty() && excluded.is_empty());
}

fn socket(port: u16, pid: Option<u32>, process: &str) -> Socket { socket_at(port, pid, process, "127.0.0.1") }
fn socket_at(port: u16, pid: Option<u32>, process: &str, addr: &str) -> Socket { Socket { port, pid, process: process.into(), addr: addr.parse().unwrap() } }
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

#[test]
fn session_keys_map_the_runtime_key_of_each_row_to_its_name() {
    let rows = [row(serde_json::json!({"name": "s", "lifecycle_id": "k:abc"})), row(serde_json::json!({"name": "t"}))];
    assert_eq!(session_keys(&rows), HashMap::from([("abc".to_owned(), "s".to_owned())]));
}

fn scan_of(listeners: &[(u16, u32)], outside: &[(u16, u64)]) -> Scan {
    Scan {
        listeners: listeners.iter().map(|&(port, pid)| Listener { port, pid, process: "node".into(), addr: Ipv4Addr::LOCALHOST.into() }).collect(),
        outside: outside.iter().map(|&(port, inode)| Outside { port, inode: Some(inode), addr: Ipv4Addr::LOCALHOST.into() }).collect(),
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
fn the_stop_guards_every_root_and_every_excluded_pid() {
    let roots = roots_of(&[("s", 10, Scope::Pane), ("t", 30, Scope::Pane)]);
    let facts = StopFacts::new(&roots, HashSet::from([11]), &ChildrenMap::new(), HashMap::new(), "s");
    assert_eq!(facts.guarded(), &HashSet::from([10, 11, 30]));
}

#[test]
fn after_a_stop_the_next_round_reads_the_sockets_again() {
    let last = LastScan::new(Duration::from_secs(3));
    let read = |listening: &[(u16, u32)]| { let scan = scan_of(listening, &[]); last.read(vec![11], |_| Ok(scan)).unwrap() };
    assert_eq!(read(&[(3000, 11)]).listeners.len(), 1);
    // Dentro do prazo a leitura guardada serve, e a porta parada ainda apareceria.
    assert_eq!(read(&[]).listeners.len(), 1);
    last.forget();
    assert!(read(&[]).listeners.is_empty());
}

fn ip(text: &str) -> IpAddr { text.parse().unwrap() }

#[test]
fn the_address_survives_the_resolve_of_session_and_outside_sockets() {
    let mut scan = scan_of(&[(3000, 11)], &[(3000, 900)]);
    scan.listeners[0].addr = ip("::");
    let sockets = resolve(&mut Outsiders::default(), &scan, &|_| true, &mut |_| HashMap::from([(900, 50)]), &mut |_| ProcessInfo::default());
    assert_eq!(sockets.iter().map(|s| (s.pid, s.addr)).collect::<Vec<_>>(), vec![(Some(11), ip("::")), (Some(50), ip("127.0.0.1"))]);
}

#[test]
fn the_tunnel_dials_the_loopback_that_reaches_the_session_process() {
    let ours = |s: &Socket| s.pid == Some(11);
    let pick = |list: &[Socket]| reach(&list.iter().collect::<Vec<_>>(), &ours);
    let (s, out) = (Some(11), Some(50));
    // Sem disputa, o de sempre.
    assert_eq!(pick(&[socket_at(3000, s, "node", "0.0.0.0")]), Some(ip("127.0.0.1")));
    assert_eq!(pick(&[socket_at(3000, s, "node", "127.0.0.1"), socket_at(3000, out, "x", "::1")]), Some(ip("127.0.0.1")));
    // O `portproxy` em 127.0.0.1 e a sessão no `::` de duas pilhas: o [::1] chega a ela.
    assert_eq!(pick(&[socket_at(3000, out, "svchost", "127.0.0.1"), socket_at(3000, s, "node", "::")]), Some(ip("::1")));
    assert_eq!(pick(&[socket_at(5173, out, "x", "127.0.0.1"), socket_at(5173, s, "vite", "::1")]), Some(ip("::1")));
    // A sessão só em 0.0.0.0, tapada no 127.0.0.1: outro endereço do loopback IPv4 ainda chega ao 0.0.0.0.
    assert_eq!(pick(&[socket_at(3000, out, "svchost", "127.0.0.1"), socket_at(3000, s, "node", "0.0.0.0")]), Some(ip("127.0.0.2")));
    // O 0.0.0.0 vence o `::` no IPv4; mapeado conta como IPv4.
    assert_eq!(pick(&[socket_at(3000, s, "node", "0.0.0.0"), socket_at(3000, out, "x", "::")]), Some(ip("127.0.0.1")));
    assert_eq!(pick(&[socket_at(3000, out, "x", "::ffff:127.0.0.1"), socket_at(3000, s, "node", "::1")]), Some(ip("::1")));
    // Dono desconhecido no mesmo endereço, ou nenhum endereço só da sessão: ordem de sempre.
    assert_eq!(pick(&[socket_at(3000, s, "node", "0.0.0.0"), socket_at(3000, None, "", "0.0.0.0")]), None);
    assert_eq!(pick(&[socket_at(3000, out, "x", "127.0.0.1"), socket_at(3000, out, "x", "::1"), socket_at(3000, s, "node", "127.0.0.5")]), None);
}

#[test]
fn dial_first_keeps_only_the_ports_where_the_usual_loopback_misses_the_session() {
    // 3000: `portproxy` 50 em 127.0.0.1, Next 11 no `::`. 5432: só a sessão. 8000: só de fora.
    let owned = BTreeMap::from([("s".to_owned(), set(&[11]))]);
    let sockets = [socket_at(3000, Some(11), "node", "::"), socket_at(3000, Some(50), "svchost", "127.0.0.1"),
        socket_at(5432, Some(11), "pg", "127.0.0.1"), socket_at(8000, Some(50), "x", "::1")];
    let dial = dial_first(&owned, &HashMap::new(), &HashSet::new(), &sockets, &HashMap::new());
    assert_eq!(dial.first, HashMap::from([(3000, ip("::1"))]));
    assert!(dial.unreached.is_empty(), "toda porta da sessão tem um loopback só dela");
    // A linha continua uma por porta, com o aviso de porta dividida.
    assert_eq!(assign(&owned, &HashMap::new(), &HashSet::new(), &sockets, &HashMap::new())["s"],
        vec![entry(3000, 11, "node", true), entry(5432, 11, "pg", false)]);
}

#[test]
fn dial_first_names_the_session_ports_that_no_loopback_reaches_alone() {
    // 3000: a sessão e um dono desconhecido no mesmo 0.0.0.0. 5432: só a sessão. 8000: só de fora.
    let owned = BTreeMap::from([("s".to_owned(), set(&[11]))]);
    let sockets = [socket_at(3000, Some(11), "node", "0.0.0.0"), socket_at(3000, None, "", "0.0.0.0"),
        socket_at(5432, Some(11), "pg", "127.0.0.1"), socket_at(8000, Some(50), "x", "127.0.0.1")];
    let dial = dial_first(&owned, &HashMap::new(), &HashSet::new(), &sockets, &HashMap::new());
    assert_eq!(dial, Dial { first: HashMap::new(), unreached: BTreeSet::from([3000]) });
    // A porta que outra sessão alcança sozinha não entra: ela tem para onde discar.
    let owned = BTreeMap::from([("a".to_owned(), set(&[11])), ("b".to_owned(), set(&[12]))]);
    let sockets = [socket_at(3000, Some(11), "node", "0.0.0.0"), socket_at(3000, Some(12), "node", "127.0.0.1"), socket_at(3000, None, "", "127.0.0.2")];
    assert_eq!(dial_first(&BTreeMap::from([("a".to_owned(), set(&[11]))]), &HashMap::new(), &HashSet::new(), &sockets, &HashMap::new()).unreached,
        BTreeSet::from([3000]), "sozinha, a sessão a não tem loopback só dela");
    assert_eq!(dial_first(&owned, &HashMap::new(), &HashSet::new(), &sockets, &HashMap::new()).unreached, BTreeSet::new());
}
