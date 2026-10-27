//! Máquinas do mesmo dono num processo só, para os testes de grupo entre máquinas: cada uma com o
//! próprio `.hangar-pair`, Python falso, lista de sessões e `peers.json` apontando para as outras.
use crate::fake;
use crate::list_support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake::{OWNER, SECRET, client};
use hangar_server::groups::federation::{self, Timing};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::peers::{PeerBook, PeerClient};
use hangar_server::groups::service::GroupService;
use hangar_server::groups::store::PairDir;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router, terminal_router};
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::share::store::Kind;
use serde_json::{Value, json};

pub const NAMES: [&str; 4] = ["s0", "s1", "s2", "s3"];

/// `addr` = porta do dono; `private` = a porta privada (ponte do Python); `changes` conta os avisos
/// de mudança à lista.
pub struct Machine { pub addr: SocketAddr, pub private: SocketAddr, pub python: Arc<fake::Fake>, pub pair: PathBuf, pub root: PathBuf,
    pub state: Arc<AppState>, pub share: Arc<hangar_server::share::ShareState>, pub changes: Arc<AtomicUsize>, pub runtime: Arc<RuntimeRegistry> }

/// Prazos de teste: o que deve acontecer tem teto folgado (15 s, abaixo dos 20 s de quem pede à
/// dona) e só estoura se o código travar; a fila tenta de novo a cada 150 ms.
pub fn fast() -> Timing {
    let ceiling = Duration::from_secs(15);
    Timing { join_wait: ceiling, ladder: vec![Duration::from_millis(150)], deliver_wait: ceiling, ..Timing::default() }
}

/// Para o teste que afirma o caminho do prazo (máquina fora do ar não confirma): a dona desiste
/// dela em 4 s.
pub fn join_gives_up(m: &Machine) { timing(m, Timing { join_wait: Duration::from_secs(4), ..fast() }); }

/// Troca os prazos da máquina; cada um vale a partir do próximo uso.
pub fn timing(m: &Machine, timing: Timing) { federation::configure(&m.pair, timing); }

async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    addr
}

pub async fn machine(root: &Path, id: &str) -> Machine {
    let script = list_support::sessions(root, NAMES.len());
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, ""));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    let pair = home.join(".claude/.hangar-pair");
    federation::configure(&pair, fast());
    let changes = Arc::new(AtomicUsize::new(0));
    let counter = changes.clone();
    state.groups = Some(Arc::new(GroupService::new(PairDir::new(pair.clone(), root.join("arquivo")), Arc::new(PythonOrq::from_state(&state)), id.into())
        .with_change_hook(Arc::new(move || { counter.fetch_add(1, Ordering::SeqCst); }))));
    state.peers = Arc::new(PeerClient::new(PeerBook::new(Some(root.join("peers.json")))));
    let share = crate::share_pair_routes::share_state(&root.join("shares.json"), &pair, Arc::new(Mutex::new(1_000.0)));
    state.share = Some(share.clone());
    let runtime = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "s".into(), "i".into()));
    assert!(state.state.runtime.set(runtime.clone()).is_ok());
    let state = Arc::new(state);
    let addr = serve(router(state.clone())).await;
    let private = serve(terminal_router(state.clone())).await;
    Machine { addr, private, python, pair, root: root.to_path_buf(), state, share, changes, runtime }
}

/// Endereço que recusa conexão: a máquina fora do ar. A porta fica presa por um socket sem
/// `listen` até o fim do processo: solta, outro teste em paralelo a reutiliza e recebe os pedidos.
pub async fn closed_port() -> SocketAddr {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    std::mem::forget(socket);
    addr
}

pub fn peers(m: &Machine, others: &[(&str, SocketAddr)]) {
    let map: serde_json::Map<String, Value> = others.iter()
        .map(|(id, addr)| ((*id).to_owned(), json!({"base_url": format!("http://{addr}"), "token": OWNER}))).collect();
    std::fs::write(m.root.join("peers.json"), Value::Object(map).to_string()).unwrap();
}

pub struct Net { pub casa: Machine, pub lab: Machine, pub vm: Machine, _dirs: Vec<tempfile::TempDir> }

/// `casa`, `lab` e `vm`, cada uma conhecendo as outras duas.
pub async fn net() -> Net {
    let dirs: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let casa = machine(dirs[0].path(), "casa").await;
    let lab = machine(dirs[1].path(), "lab").await;
    let vm = machine(dirs[2].path(), "vm").await;
    peers(&casa, &[("lab", lab.addr), ("vm", vm.addr)]);
    peers(&lab, &[("casa", casa.addr), ("vm", vm.addr)]);
    peers(&vm, &[("casa", casa.addr), ("lab", lab.addr)]);
    Net { casa, lab, vm, _dirs: dirs }
}

pub async fn call(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let mut req = client().request(method.parse().unwrap(), format!("http://{addr}{path}")).bearer_auth(OWNER);
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

pub async fn push(m: &Machine, gid: &str, body: Value) -> (u16, Value) { call(m.addr, "PUT", &format!("/api/groups/{gid}"), Some(&body)).await }

pub fn read_json(path: PathBuf) -> Option<Value> { std::fs::read(path).ok().and_then(|raw| serde_json::from_slice(&raw).ok()) }

pub fn sidecar(m: &Machine, name: &str) -> Option<Value> { read_json(m.pair.join(format!("{name}.json"))) }

pub fn record(m: &Machine, gid: &str) -> Option<Value> { read_json(m.pair.join("groups").join(format!("{gid}.json"))) }

/// Recados que a sessão `name` de `m` recebeu.
pub fn inputs(m: &Machine, name: &str) -> usize { m.python.hits_to(&format!("/api/sessions/{name}/input")) }

pub fn sorted(v: &Value, f: impl Fn(&Value) -> String) -> Vec<String> {
    let mut out: Vec<String> = v.as_array().unwrap().iter().map(f).collect();
    out.sort();
    out
}

/// `srv::nome` de cada membro da lista `v`, em ordem.
pub fn members(v: &Value) -> Vec<String> { sorted(v, |m| format!("{}::{}", m["server"].as_str().unwrap(), m["name"].as_str().unwrap())) }

pub fn peers_of(s: &Value) -> Vec<String> { sorted(&s["peers"], |p| p.as_str().unwrap().to_owned()) }

pub fn member(server: &str, name: &str) -> Value { json!({"server": server, "name": name, "provider": ""}) }

/// Par externo de `local` antes de tudo; devolve o id do compartilhamento.
pub fn outside_pair(m: &Machine, local: &str, alias: &str, peer: &str) -> String {
    let (s, _) = m.share.store.create_redeemed(local, Some("t:1".into()), Kind::Pair).unwrap();
    m.share.pairs.add(hangar_server::share::pairs::ExternalPair { share_id: s.id.clone(), local_session: local.into(),
        alias: alias.into(), peer_owner: "Ana".into(), peer_session: peer.into(), peer_address: "http://127.0.0.1:1".into(),
        peer_token: "tok".into(), created_at: 1.0 }).unwrap();
    std::fs::create_dir_all(&m.pair).unwrap();
    std::fs::write(m.pair.join(format!("{local}.json")), json!({"peers": [format!("{alias}::{peer}")], "task": "", "gid": "abababab", "harness": {}}).to_string()).unwrap();
    s.id
}
