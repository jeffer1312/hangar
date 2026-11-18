//! Handlers do Rust atendendo o convidado da 8766 com os limites do Python: lista filtrada e sem
//! nomes de outras sessões, stream da sessão sem avisos de mod nem navegador e sem contar como app do
//! dono, escrita só na própria sessão, terminal que fecha com 4410 e a recusa do clique de mod. Inclui
//! os de lista e stream de `backend/tests/test_share_guest_api.py`.
#![cfg(unix)]
use crate::fake;
use crate::list_support;
use crate::mods_support;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake::*;
use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, guest_router, router};
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeTarget};
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::share::ShareState;
use hangar_server::share::gate::GuestConn;
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::Real;
use hangar_server::terminal_input::TerminalBinding;
use list_support::{calls, names, write_marker};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite;

/// Toda sessão com chave de sidecar é viva para o porteiro; sem chave, a vida é desconhecida.
#[derive(Default)]
struct Probe { keys: Mutex<HashMap<String, String>> }

impl LifeProbe for Probe {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { None }) }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
    fn sidecar(&self, name: &str) -> Option<(String, bool)> { self.keys.lock().unwrap().get(name).cloned().map(|k| (k, false)) }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

struct Guests { share: Arc<ShareState>, probe: Arc<Probe> }

impl Guests {
    fn new(dir: &Path) -> Self {
        let probe = Arc::new(Probe::default());
        let facts = Arc::new(LifeFacts::default());
        facts.apply_transfer_snapshot(HashMap::new(), HashSet::new());
        let share = Arc::new(ShareState {
            store: ShareStore::open(dir.join("shares.json"), Arc::new(|| 100.0)),
            pairs: PairStore::open(dir.join("external_pairs.json")),
            lives: Lives::new(probe.clone(), facts),
            tailscale: Arc::new(Real),
            tunnel: Default::default(),
            connect: None,
        });
        Self { share, probe }
    }
    fn alive(&self, name: &str) {
        self.probe.keys.lock().unwrap().insert(name.into(), format!("L-{name}"));
    }
    fn token(&self, session: &str, kind: Kind) -> String {
        self.alive(session);
        self.share.store.create_redeemed(session, Some(format!("k:L-{session}")), kind).unwrap().1
    }
    fn add(&self, holder: &str, session: &str, kind: Kind) {
        let other = self.token(session, kind);
        assert_eq!(self.share.store.attach(holder, &other).unwrap(), 1);
    }
    fn revoke_all(&self) {
        for s in self.share.store.all() {
            self.share.store.revoke(&s.id).unwrap();
        }
    }
}

struct Srv { owner: SocketAddr, guest: SocketAddr, python: Arc<Fake>, g: Guests, st: Arc<AppState> }

/// Porta do dono e porta do convidado sobre o mesmo estado.
async fn serve(mut state: AppState, python: Arc<Fake>, dir: &Path) -> Srv {
    let g = Guests::new(dir);
    state.share = Some(g.share.clone());
    let state = Arc::new(state);
    let owner = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    // Todas as interfaces: o convidado também chega pelo IP da rede da casa.
    let guest = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let o = owner.local_addr().unwrap();
    let gaddr = SocketAddr::from(([127, 0, 0, 1], guest.local_addr().unwrap().port()));
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(owner, app).await.unwrap() });
    let app = guest_router(state.clone()).into_make_service_with_connect_info::<GuestConn>();
    tokio::spawn(async move { axum::serve(guest, app).await.unwrap() });
    Srv { owner: o, guest: gaddr, python, g, st: state }
}

/// Lista com `n` sessões Claude de mentira (`s0`..), como em `list_support::server`.
async fn list_srv(root: &Path, n: usize) -> Srv {
    let script = list_support::sessions(root, n);
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (python, _) = spawn_fake().await;
    let mut state = AppState::new(config(""));
    state.devices = Arc::new(hangar_server::devices::Registry::new(home.join(".hangar/devices.json")));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new()));
    // O leitor de estado nasce com a lista de `AppState::new`: sem refazê-lo, o pane do agente seria
    // procurado no tmux da máquina.
    state.state = Arc::new(hangar_server::state::live::StateEnv::new(state.terminal.clone(), state.list.clone(), state.diag.clone()));
    let srv = serve(state, python, root).await;
    for i in 0..n {
        srv.g.alive(&format!("s{i}"));
    }
    srv
}

async fn plain_srv(dir: &Path, tune: impl FnOnce(&mut AppState)) -> Srv {
    let (python, _) = spawn_fake().await;
    let mut state = AppState::new(config("127.0.0.1"));
    state.side.locate = python.locate();
    tune(&mut state);
    serve(state, python, dir).await
}

async fn get_json(url: String, token: &str) -> (u16, Value) {
    let r = client().get(url).bearer_auth(token).send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn post_json(url: String, token: &str, body: Value) -> (u16, Value) {
    let r = client().post(url).bearer_auth(token).header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn open_sse(url: String) -> Events {
    let r = client().get(url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    sse(r)
}

fn row<'a>(rows: &'a Value, name: &str) -> &'a Value {
    rows.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()
}

/// Encadeamento `s0 → s2`: é o que a linha do dono cita de outra sessão.
fn chain(root: &Path) {
    let dir = root.join("home/.claude/.hangar-chain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s0.json"), json!({"target": "s2"}).to_string()).unwrap();
}

const OTHERS: [&str; 5] = ["pair_peers", "pair_external", "pair_task", "pair_gid", "then_target"];

// ── lista ──────────────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn lista_do_convidado_so_tem_a_sessao_dele() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 2).await;
    let t = srv.g.token("s0", Kind::Share);
    let (status, body) = get_json(format!("http://{}/api/sessions", srv.guest), &t).await;
    assert_eq!((status, names(&body)), (200, vec!["s0"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn lista_do_convidado_nao_cita_outras_sessoes() {
    let dir = tempfile::tempdir().unwrap();
    chain(dir.path());
    let srv = list_srv(dir.path(), 3).await;
    let (_, owner) = get_json(format!("http://{}/api/sessions", srv.owner), OWNER).await;
    assert_eq!(row(&owner, "s0")["then_target"], "s2", "a linha do dono cita a outra sessão");
    let t = srv.g.token("s0", Kind::Share);
    let r = client().get(format!("http://{}/api/sessions", srv.guest)).bearer_auth(&t).send().await.unwrap();
    let text = r.text().await.unwrap();
    // O nome aleatório da pasta temporária pode conter "s2" (`/tmp/.tmp6s2Xes`).
    let temp_name = dir.path().file_name().unwrap().to_str().unwrap();
    // A pasta do projeto do Claude troca o `.` por `-`: o nome aparece também sem o ponto.
    assert!(!text.replace(temp_name, "").replace(temp_name.trim_start_matches('.'), "").contains("s2"), "{text}");
    let rows: Value = serde_json::from_str(&text).unwrap();
    for field in OTHERS {
        assert_eq!(row(&rows, "s0")[field], Value::Null, "{field}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lista_do_convidado_marca_o_tipo_de_cada_sessao() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 3).await;
    let t = srv.g.token("s0", Kind::Share);
    srv.g.add(&t, "s1", Kind::Pair);
    let (_, rows) = get_json(format!("http://{}/api/sessions", srv.guest), &t).await;
    let kinds: HashMap<String, Value> = rows.as_array().unwrap().iter()
        .map(|r| (r["name"].as_str().unwrap().to_owned(), r["guest_kind"].clone())).collect();
    assert_eq!(kinds, HashMap::from([("s0".into(), json!("share")), ("s1".into(), json!("pair"))]));
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_da_lista_filtrado_e_sem_contar_app() {
    let dir = tempfile::tempdir().unwrap();
    // O dono tem pedido de navegador e terminal de atalho: nenhum dos dois vai ao convidado.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let pending = dir.path().join("home/.hangar/nav/_pendentes.json");
    std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
    std::fs::write(&pending, json!({"s0": {"url": "http://127.0.0.1:5173/", "ts": now}}).to_string()).unwrap();
    let srv = list_srv(dir.path(), 2).await;
    let t = srv.g.token("s0", Kind::Share);
    let mut es = open_sse(format!("http://{}/api/sessions/events?token={t}", srv.guest)).await;
    let ev = next_named(&mut es, "sessions").await;
    assert_eq!(names(&serde_json::from_str(&ev.data).unwrap()), ["s0"]);
    // O produtor roda só com o convidado, que não conta como lista do dono aberta.
    let root = dir.path().to_owned();
    wait_until(move || calls(&root, "list-panes") >= 2).await;
    assert_eq!(srv.st.list.owner_clients(), 0);
    write_marker(dir.path(), 0, "working");
    loop {
        let ev = next_non_ping(&mut es).await;
        assert!(ev.event == "sessions", "o convidado não recebe {}", ev.event);
        if ev.data.contains("working") { break; }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_list_with_a_device_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 2).await;
    let t = srv.g.token("s0", Kind::Share);
    let mut es = open_sse(format!("http://{}/api/sessions/events?token={t}&device=ios%3Bg-1", srv.guest)).await;
    // O dono grava antes de assinar: com a primeira lista na mão, um registro já estaria no disco.
    assert_eq!(names(&serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap()), ["s0"]);
    assert!(!dir.path().join("home/.hangar/devices.json").exists(), "convidado com concessão não registra aparelho");
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_da_lista_nao_cita_outras_sessoes() {
    let dir = tempfile::tempdir().unwrap();
    chain(dir.path());
    let srv = list_srv(dir.path(), 3).await;
    let t = srv.g.token("s0", Kind::Share);
    let mut es = open_sse(format!("http://{}/api/sessions/events?token={t}", srv.guest)).await;
    let ev = next_named(&mut es, "sessions").await;
    let temp_name = dir.path().file_name().unwrap().to_str().unwrap();
    assert!(!ev.data.replace(temp_name, "").contains("s2"), "{}", ev.data);
    let rows: Value = serde_json::from_str(&ev.data).unwrap();
    assert_eq!(row(&rows, "s0")["then_target"], Value::Null);
    assert_eq!(row(&rows, "s0")["guest_kind"], "share");
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_da_lista_enxerga_sessao_ligada_ao_token_depois_de_aberto() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 3).await;
    let t = srv.g.token("s0", Kind::Share);
    let mut es = open_sse(format!("http://{}/api/sessions/events?token={t}", srv.guest)).await;
    assert_eq!(names(&serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap()), ["s0"]);
    // Resgate com token (ou attach) liga outra sessão ao mesmo token com o stream já aberto.
    srv.g.add(&t, "s1", Kind::Share);
    write_marker(dir.path(), 2, "working");
    let ev = next_named(&mut es, "sessions").await;
    assert_eq!(names(&serde_json::from_str(&ev.data).unwrap()), ["s0", "s1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_da_lista_acompanha_a_sessao_renomeada() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 2).await;
    let t = srv.g.token("s0", Kind::Share);
    let mut es = open_sse(format!("http://{}/api/sessions/events?token={t}", srv.guest)).await;
    assert_eq!(names(&serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap()), ["s0"]);
    // A vida acompanha o nome, como no renomear de verdade.
    srv.g.probe.keys.lock().unwrap().insert("s1".into(), "L-s0".into());
    srv.g.share.store.rename("s0", "s1").unwrap();
    write_marker(dir.path(), 1, "working");
    let ev = next_named(&mut es, "sessions").await;
    assert_eq!(names(&serde_json::from_str(&ev.data).unwrap()), ["s1"]);
}

// ── rotas que o convidado não alcança ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn guest_does_not_reach_the_hangar_terminal_routes() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("cc", Kind::Share);
    assert_eq!(get_json(format!("http://{}/api/hangar-terminals", srv.guest), &t).await.0, 403);
    for op in ["close", "restart"] {
        assert_eq!(post_json(format!("http://{}/api/hangar-terminals/abcdef/{op}", srv.guest), &t, json!({})).await.0, 403);
    }
    let url = format!("ws://{}/api/hangar-terminals/abcdef/term?token={t}", srv.guest);
    match tokio_tungstenite::connect_async(url).await {
        Err(tungstenite::Error::Http(r)) => assert_eq!(r.status(), 403),
        other => panic!("esperava recusa no aperto de mão: {:?}", other.map(|_| ())),
    }
}

// ── caminho que o Python não atende ──────────────────────────────────────────────────────────

/// Convidado em caminho sem handler: o 404 que o Python dava sai do Rust, e nada é repassado (I-19).
#[tokio::test(flavor = "multi_thread")]
async fn guest_path_without_handler_is_404_from_the_rust() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("cc", Kind::Share);
    let r = client().get(format!("http://{}/api/sessions/cc/plan-preview?token={t}", srv.guest)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 404);
    let mut s = tokio::net::TcpStream::connect(srv.guest).await.unwrap();
    s.write_all(format!("GET /api/sessions/cc/proxy-ws-test?token={t} HTTP/1.1\r\nHost: h\r\n\
        Connection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut byte)).await.unwrap().unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 404"), "{}", String::from_utf8_lossy(&head));
}

/// Decisão 17 na 8766: corpo declarado acima de 100 MiB é 413 antes do porteiro, sem token nenhum.
#[tokio::test(flavor = "multi_thread")]
async fn guest_port_refuses_a_declared_body_over_100_mib_before_the_gate() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let mut s = tokio::net::TcpStream::connect(srv.guest).await.unwrap();
    s.write_all(b"POST /api/sessions/cc/input HTTP/1.1\r\nHost: h\r\nContent-Length: 104857601\r\n\r\nx").await.unwrap();
    let mut seen = Vec::new();
    let mut chunk = [0u8; 1024];
    tokio::time::timeout(Duration::from_secs(10), async {
        while !String::from_utf8_lossy(&seen).contains("request body too large") {
            let n = s.read(&mut chunk).await.unwrap();
            assert!(n > 0, "a conexão fechou antes da resposta: {}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&chunk[..n]);
        }
    }).await.expect("a recusa não chegou");
    assert!(seen.starts_with(b"HTTP/1.1 413"), "{}", String::from_utf8_lossy(&seen));
}

/// Decisão 18 na 8766: a falha do servidor para o convidado ganha a linha `api.servidor` mascarada.
#[tokio::test(flavor = "multi_thread")]
async fn guest_server_failure_writes_the_masked_access_line() {
    let dir = tempfile::tempdir().unwrap();
    let diary = tempfile::tempdir().unwrap();
    let path = diary.path().to_owned();
    let srv = plain_srv(dir.path(), move |state| state.diag = state.diag.clone().with_diary(Some(path))).await;
    let t = srv.g.token("cc", Kind::Share);
    let (status, body) = post_json(format!("http://{}/api/sessions/cc/input", srv.guest), &t, json!({"text": "oi"})).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("runtime_unavailable")), "{body}");
    let is_line = |row: &Value| row["evento"] == "api.servidor" && row["codigo"] == "503";
    let rows = wait_diary(diary.path(), is_line).await;
    let line = rows.iter().find(|row| is_line(row)).unwrap();
    assert_eq!(line["detalhe"], "POST /api/sessions/*/input");
    assert_eq!(line["causa"], "runtime_unavailable");
}

/// IP desta máquina na rede da casa; `None` sem rede (o teste que precisa dele não roda).
fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip()).filter(|ip| !ip.is_loopback())
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_forward_never_touches_the_owner_backoff() {
    let Some(lan) = lan_ip() else { eprintln!("sem rede local: teste pulado"); return };
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("cc", Kind::Share);
    let url = format!("http://{lan}:{}/api/sessions/cc/input", srv.guest.port());
    // Decisões 1 e 8: a escrita do convidado é do Rust; sem o registro de sessões, 503 com código e
    // nada vai ao Python. Recusas seguidas do convidado nunca entram no bloqueio do dono.
    for _ in 0..9 {
        let (status, body) = post_json(url.clone(), &t, json!({"text": "oi"})).await;
        assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("runtime_unavailable")), "{body}");
    }
    assert!(srv.st.auth.is_owner(&lan.to_string(), Some(OWNER.as_bytes())), "o dono nesse IP segue livre");
}

/// R5-12 (`routes.rs` `pass`): na 8766 um caminho aberto do porteiro, sem convidado decidido, nunca
/// vai ao Python como dono, nem com a senha do dono: 404 do Rust.
#[tokio::test(flavor = "multi_thread")]
async fn r5_12_guest_port_without_a_decided_guest_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let response = client().get(format!("http://{}/par/x/nada", srv.guest)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 404);
    assert_eq!(response.text().await.unwrap(), r#"{"detail":"Not Found"}"#);
}

/// Achado 1 da revisão: na 8766 a senha do dono não fecha sessão; o porteiro recusa e nada morre.
#[tokio::test(flavor = "multi_thread")]
async fn guest_port_delete_with_the_owner_token_closes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 1).await;
    let response = client().delete(format!("http://{}/api/sessions/s0", srv.guest)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(list_support::calls(dir.path(), "kill-session"), 0, "nada foi encerrado");
}

/// R1-01: na 8766 o convidado do convite e o par de leitura (`Kind::Pair`) não fecham a sessão: o
/// porteiro responde 403 `erro_fora_do_convite`, nada é encerrado e o Python não recebe nada.
#[tokio::test(flavor = "multi_thread")]
async fn r1_01_the_invite_and_the_reading_pair_never_close_on_the_guest_port() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 1).await;
    for kind in [Kind::Share, Kind::Pair] {
        let token = srv.g.token("s0", kind);
        let response = client().delete(format!("http://{}/api/sessions/s0", srv.guest)).bearer_auth(&token).send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null);
        assert_eq!((status, body["detail"]["code"].as_str()), (403, Some("erro_fora_do_convite")), "{body}");
    }
    assert_eq!(list_support::calls(dir.path(), "kill-session"), 0, "nada foi encerrado");
}

// ── stream da sessão ─────────────────────────────────────────────────────────────────────────

/// Codex com terminal sem ator no Rust: sem `Monitor` nem feed, o hub só reparte o que as fontes do Rust entregam.
fn codex_session(srv: &Srv, dir: &Path) {
    let jsonl = dir.join("rollout.jsonl");
    std::fs::write(&jsonl, "").unwrap();
    srv.python.set_info(info_json("codex", &jsonl));
}

async fn until_state(es: &mut Events, data: &str) -> Vec<String> {
    let mut seen = Vec::new();
    loop {
        let ev = next_non_ping(es).await;
        let done = ev.event == "state" && ev.data == data;
        seen.push(ev.event);
        if done { return seen; }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_events_drop_plugin_toast_and_nav() {
    let dir = tempfile::tempdir().unwrap();
    let mut mods = None;
    let mut hubs = None;
    let srv = plain_srv(dir.path(), |st| { mods = Some(st.mods.clone()); hubs = Some(st.side.hubs.clone()); }).await;
    let hubs = hubs.unwrap();
    codex_session(&srv, dir.path());
    let t = srv.g.token("s", Kind::Share);
    let mut owner = sse(open_events(srv.owner, "s", "", &[]).await);
    let mut guest = open_sse(format!("http://{}/api/sessions/s/events?token={t}", srv.guest)).await;
    hubs.deliver("s", "state", "\"m0\"");
    until_state(&mut owner, "\"m0\"").await;
    until_state(&mut guest, "\"m0\"").await;
    mods.unwrap().loose_toast("s", "m", "segredo", 60000);
    hubs.deliver("s", "nav", &json!({"url": "http://127.0.0.1:5173/?token=dono"}).to_string());
    hubs.deliver("s", "state", "\"m1\"");
    let seen = until_state(&mut owner, "\"m1\"").await;
    assert!(seen.contains(&"plugin_toast".into()) && seen.contains(&"nav".into()), "{seen:?}");
    let seen = until_state(&mut guest, "\"m1\"").await;
    assert!(!seen.iter().any(|e| e == "plugin_toast" || e == "nav"), "{seen:?}");
    // Quem entra depois não recebe o aviso guardado no retrato.
    let mut late = open_sse(format!("http://{}/api/sessions/s/events?token={t}", srv.guest)).await;
    let mut seen = until_state(&mut late, "\"m1\"").await;
    hubs.deliver("s", "state", "\"m2\"");
    seen.extend(until_state(&mut late, "\"m2\"").await);
    assert!(!seen.iter().any(|e| e == "plugin_toast" || e == "nav"), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_da_sessao_do_convidado_nao_conta_como_app_do_dono() {
    let dir = tempfile::tempdir().unwrap();
    let mut hubs = None;
    let srv = plain_srv(dir.path(), |st| hubs = Some(st.side.hubs.clone())).await;
    let hubs = hubs.unwrap();
    codex_session(&srv, dir.path());
    let t = srv.g.token("s", Kind::Share);
    let mut guest = open_sse(format!("http://{}/api/sessions/s/events?token={t}", srv.guest)).await;
    next_any(&mut guest).await;
    hubs.deliver("s", "state", "\"g\"");
    until_state(&mut guest, "\"g\"").await;
    assert!(!hubs.app_present(), "o convidado assina o hub sem ser o app do dono");
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_keeps_events_when_the_owner_count_crosses_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut hubs = None;
    let srv = plain_srv(dir.path(), |st| hubs = Some(st.side.hubs.clone())).await;
    let hubs = hubs.unwrap();
    codex_session(&srv, dir.path());
    let t = srv.g.token("s", Kind::Share);
    let mut guest = open_sse(format!("http://{}/api/sessions/s/events?token={t}", srv.guest)).await;
    next_any(&mut guest).await;
    let mut owner = sse(open_events(srv.owner, "s", "", &[]).await);
    next_any(&mut owner).await;
    wait_until(|| hubs.app_present()).await;
    hubs.deliver("s", "state", "\"a\"");
    until_state(&mut guest, "\"a\"").await;
    drop(owner);
    wait_until(|| !hubs.app_present()).await;
    hubs.deliver("s", "state", "\"b\"");
    until_state(&mut guest, "\"b\"").await;
}

// ── histórico e escrita ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn pair_guest_only_reads() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let jsonl = dir.path().join("t.jsonl");
    append_lines(&jsonl, 0..2);
    srv.python.set_info(info_json("claude", &jsonl));
    let p = srv.g.token("s", Kind::Pair);
    let (status, body) = get_json(format!("http://{}/api/sessions/s/history", srv.guest), &p).await;
    assert_eq!(status, 200);
    assert_eq!(body, get_json(format!("http://{}/api/sessions/s/history", srv.owner), OWNER).await.1);
    assert_eq!(post_json(format!("http://{}/api/sessions/s/interrupt", srv.guest), &p, json!({})).await.0, 403);
}

/// Cano falso que confirma cada envelope e responde sucesso aos pedidos de controle.
async fn cano(sink: Arc<Mutex<Vec<Value>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let sink = sink.clone();
            tokio::spawn(async move {
                let (read, mut write) = tokio::io::split(stream);
                let mut reader = BufReader::new(read);
                let mut header = String::new();
                if reader.read_line(&mut header).await.is_err() || header != "secret-test\n" { return; }
                write.write_all(format!("{}\n", mods_support::cano_snapshot_json()).as_bytes()).await.unwrap();
                loop {
                    let mut raw = String::new();
                    if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
                    let envelope: Value = serde_json::from_str(&raw).unwrap();
                    sink.lock().unwrap().push(envelope.clone());
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                    let frame: Value = serde_json::from_str(envelope["frame"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
                    if frame["type"] == "control_request" {
                        let response = json!({"type":"control_response","response":{"subtype":"success",
                            "request_id":frame["request_id"],"response":{}}});
                        let _ = write.write_all(format!("{}\n", json!({"type":"cano_output","frame":response.to_string()})).as_bytes()).await;
                    }
                }
            });
        }
    });
    format!("tcp:{address}")
}

#[tokio::test(flavor = "multi_thread")]
async fn share_guest_writes_its_session_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))));
    let sink = Arc::new(Mutex::new(Vec::new()));
    let d = dir.path();
    registry.open(RuntimeTarget { key: "k-s".into(), generation: 1, name: "s".into(), provider: "claude".into(),
        metadata: json!({"name": "s", "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(sink.clone()).await, token: "secret-test".into(), versao: 2 },
        lease_path: d.join("s.lock"), state_path: d.join("s.queue-state.json"), projection_dir: d.join("s-projection"),
        transcript: d.join("s.jsonl"), created: 0.0 }).await.unwrap();
    let srv = plain_srv(dir.path(), |st| {
        st.write_gate_wait = Duration::from_secs(5);
        assert!(st.state.runtime.set(registry).is_ok());
    }).await;
    let t = srv.g.token("s", Kind::Share);
    srv.g.alive("o");
    let (status, body) = post_json(format!("http://{}/api/sessions/s/input", srv.guest), &t, json!({"text": "recado-do-convidado"})).await;
    assert_eq!(status, 200, "{body}");
    assert!(sink.lock().unwrap().iter().any(|e| e.to_string().contains("recado-do-convidado")), "o cano recebeu o texto");
    assert_eq!(post_json(format!("http://{}/api/sessions/o/input", srv.guest), &t, json!({"text": "x"})).await.0, 403);
}

/// HOME da prova com o sidecar de `s` (chave `k-s`, conversa `sid-1`, cano 42), onde o ator grava e
/// confere a troca de modo: nunca o HOME da máquina.
fn proof_home(dir: &Path) -> hangar_server::accounts::environment::AccountEnvironment {
    let home = dir.join("home");
    crate::session_write_permissions::headless_sidecar(&home, "s");
    hangar_server::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(), home.to_string_lossy().into_owned())].into())
}

/// R4-19, decisões 1 e 2 (D2): o convidado completo lê e troca o modo de permissão e o modelo da sessão
/// dele pela rota do dono (antes, o 404 do `pass` na 8766). O par de leitura, outra sessão e as rotas de
/// grupo (R4-17) seguem recusados no porteiro. Nada vai ao Python.
#[tokio::test(flavor = "multi_thread")]
async fn r4_19_share_guest_changes_mode_and_model_through_the_owner_route() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_codex_environment(proof_home(dir.path())));
    let sink = Arc::new(Mutex::new(Vec::new()));
    let d = dir.path();
    registry.open(RuntimeTarget { key: "k-s".into(), generation: 1, name: "s".into(), provider: "claude".into(),
        metadata: json!({"name": "s", "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(sink.clone()).await, token: "secret-test".into(), versao: 2 },
        lease_path: d.join("s.lock"), state_path: d.join("s.queue-state.json"), projection_dir: d.join("s-projection"),
        transcript: d.join("s.jsonl"), created: 0.0 }).await.unwrap();
    let srv = plain_srv(dir.path(), |st| {
        st.write_gate_wait = Duration::from_secs(5);
        assert!(st.state.runtime.set(registry).is_ok());
    }).await;
    let t = srv.g.token("s", Kind::Share);
    let url = |route: &str| format!("http://{}/api/sessions/{route}", srv.guest);
    let (status, body) = get_json(url("s/permission-modes"), &t).await;
    assert_eq!((status, body["sondavel"].clone()), (200, json!(false)), "{body}");
    assert_eq!(body["modes"].as_array().map(Vec::len), Some(6), "{body}");
    let (status, body) = post_json(url("s/permission-mode"), &t, json!({"mode": "plan"})).await;
    assert_eq!((status, body["current"].clone()), (200, json!("plan")), "{body}");
    let (status, body) = post_json(url("s/model-effort"), &t, json!({"model": "sonnet"})).await;
    assert_eq!((status, body), (200, json!({"ok": true, "scope": "session", "result": null})));
    let frames: Vec<Value> = sink.lock().unwrap().iter().filter_map(|e| serde_json::from_str(e["frame"].as_str()?).ok()).collect();
    for subtype in ["set_permission_mode", "set_model"] {
        assert!(frames.iter().any(|f| f["request"]["subtype"] == subtype), "{subtype}: {frames:?}");
    }
    let p = srv.g.token("s", Kind::Pair);
    assert_eq!(get_json(url("s/permission-modes"), &p).await.0, 403, "o par de leitura não alcança os modos");
    assert_eq!(post_json(url("s/permission-mode"), &p, json!({"mode": "plan"})).await.0, 403);
    assert_eq!(post_json(url("s/model-effort"), &p, json!({"model": "sonnet"})).await.0, 403);
    srv.g.alive("o");
    assert_eq!(post_json(url("o/permission-mode"), &t, json!({"mode": "plan"})).await.0, 403, "sessão fora do convite");
    for (route, body) in [("s/pair", json!({"peer": "o"})), ("s/group-message", json!({"text": "oi"}))] {
        let (status, refused) = post_json(url(route), &t, body).await;
        assert_eq!((status, refused["detail"]["code"].clone()), (403, json!("erro_fora_do_convite")), "{route}");
    }
    assert_eq!(get_json(url("s/pair/contract"), &t).await.0, 403);
    for route in ["permission-modes", "permission-mode", "model-effort", "pair", "group-message", "pair/contract"] {
    }
}

/// R4-11 (`_invalidate_lists` depois da troca): a linha da lista do Rust não leva o modo de permissão; quem o
/// mostra ao vivo é o `state` do chat (`claude_permission_mode`), publicado pelo ator assim que a troca é
/// confirmada, sem releitura pedida pela rota.
#[tokio::test(flavor = "multi_thread")]
async fn r4_11_the_changed_mode_reaches_the_live_state_without_a_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_codex_environment(proof_home(d)));
    let srv = plain_srv(d, |st| {
        st.write_gate_wait = Duration::from_secs(5);
        registry.bind_lifecycle();
        registry.bind_hubs(st.side.hubs.downgrade());
        let _ = st.state.lifecycle.set(registry.clone());
        let _ = st.side.runtime.set(registry.clone());
        assert!(st.state.runtime.set(registry.clone()).is_ok());
    }).await;
    let transcript = d.join("s.jsonl");
    append_lines(&transcript, 0..1);
    srv.python.set_info(info_json("claude-headless", &transcript));
    let sink = Arc::new(Mutex::new(Vec::new()));
    registry.open(RuntimeTarget { key: "k-s".into(), generation: 1, name: "s".into(), provider: "claude".into(),
        metadata: json!({"name": "s", "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(sink.clone()).await, token: "secret-test".into(), versao: 2 },
        lease_path: d.join("s.lock"), state_path: d.join("s.queue-state.json"), projection_dir: d.join("s-projection"),
        transcript: transcript.clone(), created: 0.0 }).await.unwrap();
    let mut events = sse(open_events(srv.owner, "s", "", &[]).await);
    next_named(&mut events, "state").await;
    let (status, body) = post_json(format!("http://{}/api/sessions/s/permission-mode", srv.owner), OWNER, json!({"mode": "plan"})).await;
    assert_eq!((status, body["current"].clone()), (200, json!("plan")), "{body}");
    let mut seen = Vec::new();
    for _ in 0..10 {
        let state: Value = serde_json::from_str(&next_named(&mut events, "state").await.data).unwrap();
        if state["claude_permission_mode"] == "plan" { return; }
        seen.push(state["claude_permission_mode"].clone());
    }
    panic!("o modo novo não chegou ao estado ao vivo: {seen:?}");
}

// ── mods ─────────────────────────────────────────────────────────────────────────────────────

fn terminal_target(dir: &Path, name: &str) -> TerminalTarget {
    let binding = TerminalBinding { name: name.into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec!["/does-not-exist/hangar-test-tmux".into()], windows: false, clipboard_lock_path: None };
    let transcript = dir.join(format!("{name}.jsonl"));
    std::fs::write(&transcript, "").unwrap();
    TerminalTarget { key: format!("key-{name}"), generation: 1, name: name.into(), binding, lease_path: dir.join("lease"),
        state_path: dir.join("state"), projection_dir: dir.join("projection"), transcript, created: 0.0, plugin_key: None,
        metadata: hangar_server::terminal_facts::TerminalMetadata { config_dir:dir.into(),agent_pid:std::process::id(),agent_birth:hangar_server::terminal_facts::agent_birth(std::process::id()).unwrap(),legacy_import_after:None,fingerprint:"fixture".into(),origin:"uds:fixture".into() } }
}

/// Terminal no Rust sem a interface dos mods dele: o clique do convidado não tem quem atenda (503), e
/// nunca dirige o pane calado pelo Python.
#[tokio::test(flavor = "multi_thread")]
async fn guest_mod_click_on_a_rust_terminal_without_surface_is_503() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into()));
    registry.open_terminal(terminal_target(dir.path(), "t")).await.unwrap();
    let srv = plain_srv(dir.path(), |st| assert!(st.state.runtime.set(registry).is_ok())).await;
    let t = srv.g.token("t", Kind::Share);
    let (status, body) = post_json(format!("http://{}/api/sessions/t/plugin/press", srv.guest), &t,
        json!({"site": "x", "plugin": "p", "key": "k"})).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("erro_mod_clique_sem_resposta")));
    // A digitação também: o Python respondia o mesmo 503 a convidado em terminal do Rust.
    let (status, body) = post_json(format!("http://{}/api/sessions/t/plugin/input", srv.guest), &t,
        json!({"site": "x", "plugin": "p", "key": "k", "kind": "change", "value": "o"})).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("erro_mod_clique_sem_resposta")));
}

/// R1.5-6: o convidado numa sessão sem mods no Rust recebe a mesma recusa fixa do dono, sem o Python.
#[tokio::test(flavor = "multi_thread")]
async fn guest_mod_on_a_session_outside_the_rust_gets_the_fixed_answer() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |st| {
        let _ = st.state.runtime.set(Arc::new(RuntimeRegistry::new("instance-test".into())));
    }).await;
    let t = srv.g.token("o", Kind::Share);
    let url = |op: &str| format!("http://{}/api/sessions/o/plugin/{op}", srv.guest);
    assert_eq!(post_json(url("press"), &t, json!({"site": "x", "plugin": "p", "key": "k"})).await.1["detail"]["code"], "mods_need_rust_server");
    assert_eq!(post_json(url("close"), &t, json!({"site": "x"})).await.1["detail"]["code"], "mods_need_rust_server");
    assert_eq!(post_json(url("show"), &t, json!({"site": "x"})).await.0, 404);
    assert_eq!(post_json(url("input"), &t, json!({"site": "x", "plugin": "p", "key": "k", "kind": "change", "value": "o"})).await.0, 409);
    for op in ["press", "close", "show", "input"] {
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_reaches_the_mods_the_rust_serves() {
    use hangar_server::mods::click::Limits;
    use hangar_server::mods::terminal::TerminalLink;
    use mods_support::pane::{Effect::*, FakePane, view};
    let dir = tempfile::tempdir().unwrap();
    let mut mods = None;
    let srv = plain_srv(dir.path(), |st| {
        let registry = RuntimeRegistry::new("instance-test".into())
            .with_mods(st.mods.clone());
        let _ = st.state.runtime.set(Arc::new(registry));
        mods = Some(st.mods.clone());
    }).await;
    let mods = mods.unwrap();
    let pane = Arc::new(FakePane::new(&mods, "t", "tmux-01-tres-paineis-150"));
    let life = mods.new_life();
    mods.attach_terminal("t", "proc-t", life, TerminalLink::new("t".into(), life, pane.clone(), mods.clone(), Limits::quick()));
    mods.terminal_ui("t", view(&[("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"), ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx"),
        ("pm-mock-jenkins", "Jenkins", "jenkins-a", "xxxxx")]));
    pane.on_click((0, 104), vec![Show("tmux-02-apos-clicar-mr-150")]);
    let t = srv.g.token("t", Kind::Share);
    assert_eq!(post_json(format!("http://{}/api/sessions/t/plugin/show", srv.guest), &t, json!({"site": "pm-mock-mr"})).await,
        (200, json!({"ok": true, "shown_id": "pm-mock-mr"})));
}

// ── terminal ─────────────────────────────────────────────────────────────────────────────────

struct Tmux { _dir: tempfile::TempDir, socket: std::path::PathBuf }

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux").arg("-S").arg(&self.socket).arg("kill-server").env_remove("TMUX").output();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_terminal_closes_with_4410_when_revoked() {
    let tmux = { let d = tempfile::tempdir().unwrap(); let socket = d.path().join("t.sock"); Tmux { _dir: d, socket } };
    let out = std::process::Command::new("tmux").arg("-S").arg(&tmux.socket).args(["-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "80", "-y", "24", "cat"])
        .env_remove("TMUX").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let dir = tempfile::tempdir().unwrap();
    let socket = tmux.socket.clone();
    let srv = plain_srv(dir.path(), move |st| {
        st.term = Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig { socket: Some(socket), ..Default::default() }));
    }).await;
    let t = srv.g.token("t", Kind::Share);
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/sessions/t/term?token={t}", srv.guest)).await.unwrap();
    // A tela chega: o terminal está anexado.
    let first = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(first, tungstenite::Message::Binary(_)), "{first:?}");
    srv.g.revoke_all();
    let code = tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(Ok(m)) = ws.next().await {
            if let tungstenite::Message::Close(frame) = m { return frame.map(|f| u16::from(f.code)); }
        }
        None
    }).await.expect("o vigia fecha em até 5 s");
    assert_eq!(code, Some(4410));
    let _ = ws.close(None).await;
}

/// T-28: um painel por sessão entre a porta do dono e a do convidado; quem chega derruba o outro com 1000.
#[tokio::test(flavor = "multi_thread")]
async fn owner_and_invite_guest_share_one_panel() {
    let tmux = { let d = tempfile::tempdir().unwrap(); let socket = d.path().join("t.sock"); Tmux { _dir: d, socket } };
    let out = std::process::Command::new("tmux").arg("-S").arg(&tmux.socket).args(["-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "80", "-y", "24", "cat"])
        .env_remove("TMUX").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let dir = tempfile::tempdir().unwrap();
    let socket = tmux.socket.clone();
    let srv = plain_srv(dir.path(), move |st| {
        st.term = Arc::new(hangar_server::term::Terms::new(hangar_server::term::TermConfig { socket: Some(socket), ..Default::default() }));
    }).await;
    let t = srv.g.token("t", Kind::Share);
    let close_of = |mut ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>| async move {
        tokio::time::timeout(Duration::from_secs(12), async {
            while let Some(Ok(m)) = ws.next().await {
                if let tungstenite::Message::Close(frame) = m { return frame.map(|f| (u16::from(f.code), f.reason.to_string())); }
            }
            None
        }).await.expect("o painel anterior fecha")
    };
    let (owner, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/sessions/t/term?token={OWNER}", srv.owner)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async { while !srv.st.term.is_active("t") { tokio::time::sleep(Duration::from_millis(20)).await; } })
        .await.expect("dono anexado");
    let (guest, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/sessions/t/term?token={t}", srv.guest)).await.unwrap();
    assert_eq!(close_of(owner).await, Some((1000, "outra conexao assumiu".to_string())));
    let (_owner, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/sessions/t/term?token={OWNER}", srv.owner)).await.unwrap();
    assert_eq!(close_of(guest).await, Some((1000, "outra conexao assumiu".to_string())));
    assert_eq!(srv.st.term.active(), vec!["t".to_string()]);
}

/// O preflight é o do `CORSMiddleware` do Starlette com `*` em tudo e sem credenciais: sem ele o
/// convidado do PWA (outra origem) não resgata, não manda recado nem escreve.
#[tokio::test(flavor = "multi_thread")]
async fn guest_port_answers_the_cors_preflight_like_starlette() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    for (path, method) in [("/api/guest/redeem", "POST"), ("/api/pair/message", "POST"), ("/api/sessions/x/input", "POST")] {
        let r = client().request(reqwest::Method::OPTIONS, format!("http://{}{path}", srv.guest))
            .header("origin", "https://app-do-convidado.example")
            .header("access-control-request-method", method)
            .header("access-control-request-headers", "authorization, content-type").send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        let h = r.headers();
        assert_eq!(h["access-control-allow-origin"], "*", "{path}");
        assert_eq!(h["access-control-allow-methods"], "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT", "{path}");
        assert_eq!(h["access-control-allow-headers"], "authorization, content-type", "{path}");
        assert_eq!(h["access-control-max-age"], "600", "{path}");
        assert_eq!(h["content-type"], "text/plain; charset=utf-8", "{path}");
        assert!(!h.contains_key("vary"), "{path}");
        assert_eq!(r.text().await.unwrap(), "OK", "{path}");
    }
    // A única recusa possível com essa configuração.
    let r = client().request(reqwest::Method::OPTIONS, format!("http://{}/api/guest/redeem", srv.guest))
        .header("origin", "https://app-do-convidado.example").header("access-control-request-method", "POST")
        .header("access-control-request-private-network", "true").send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert_eq!(r.text().await.unwrap(), "Disallowed CORS private-network");
}

/// O Starlette nunca casava `/` dentro de `{name}`; aqui o `%2F` faria o porteiro conferir `proj` e o
/// handler servir `proj/x`.
#[tokio::test(flavor = "multi_thread")]
async fn encoded_slash_in_the_session_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("proj", Kind::Share);
    srv.g.alive("proj/x");
    for path in ["/api/sessions/proj%2Fx/files/list?path=", "/api/sessions/proj%2Fx/uploads/a.png",
        "/api/sessions/proj%2Fx/history", "/api/sessions/term-proj%2Fx/term"] {
        assert_eq!(get_json(format!("http://{}{path}", srv.guest), &t).await.0, 403, "{path}");
    }
    let url = format!("ws://{}/api/sessions/proj%2Fx/term?token={t}", srv.guest);
    match tokio_tungstenite::connect_async(url).await {
        Err(tungstenite::Error::Http(r)) => {
            assert_eq!(r.status(), 403);
            // Corpo vazio: quem recusou foi o porteiro, não o terminal procurando `proj/x`.
            assert_eq!(r.body().clone().unwrap_or_default(), Vec::<u8>::new());
        }
        other => panic!("esperava recusa no aperto de mão, veio {:?}", other.map(|_| ())),
    }
    // Espaço e acento seguem sendo a mesma sessão: passam o porteiro e chegam ao handler de anexos,
    // que procura a sessão na descoberta (vazia aqui).
    for name in ["a b", "ação"] {
        let a = srv.g.token(name, Kind::Share);
        let seg = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
        let (status, body) = get_json(format!("http://{}/api/sessions/{seg}/uploads/a.png", srv.guest), &a).await;
        assert_eq!((status, body["detail"]["code"].as_str()), (404, Some("erro_sessao_inexistente")), "{name}");
    }
}

/// Grupo `s0`+`s1`+`s2` com tarefa: o convidado vê o grupo, mas só os membros a que tem acesso.
fn group_of_three(root: &Path) {
    let dir = root.join("home/.claude/.hangar-pair");
    std::fs::create_dir_all(&dir).unwrap();
    for me in ["s0", "s1", "s2"] {
        let peers: Vec<&str> = ["s0", "s1", "s2"].into_iter().filter(|p| *p != me).collect();
        std::fs::write(dir.join(format!("{me}.json")), json!({"peers": peers, "gid": "g1", "task": "T"}).to_string()).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn convidado_de_par_ve_o_grupo_dentro_do_acesso() {
    // Um caminho pode conter o nome de uma sessão sem conceder acesso a ela.
    let dir = tempfile::Builder::new().prefix("s2-").tempdir().unwrap();
    group_of_three(dir.path());
    let srv = list_srv(dir.path(), 3).await;
    let (pair, t) = srv.g.share.store.create_redeemed("s0", Some("k:L-s0".into()), Kind::Pair).unwrap();
    srv.g.share.store.grant(&pair.id, "s1", Some("k:L-s1".into()), Kind::Share).unwrap();
    let (_, rows) = get_json(format!("http://{}/api/sessions", srv.guest), &t).await;
    assert_eq!(names(&rows), ["s0", "s1"]);
    let s0 = row(&rows, "s0");
    assert_eq!((&s0["pair_gid"], &s0["pair_peers"], &s0["pair_task"]), (&json!("g1"), &json!(["s1"]), &json!("T")));
    assert_eq!(row(&rows, "s1")["pair_peers"], json!(["s0"]));
    let temp_name = dir.path().file_name().unwrap().to_str().unwrap();
    assert!(!serde_json::to_string(&rows).unwrap().replace(temp_name, "").contains("s2"), "{rows}");
    for field in ["pair_external", "pair_externals", "then_target"] {
        assert_eq!(s0[field], Value::Null, "{field}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn convidado_de_convite_ve_o_grupo_sem_a_tarefa() {
    let dir = tempfile::tempdir().unwrap();
    group_of_three(dir.path());
    let srv = list_srv(dir.path(), 3).await;
    let t = srv.g.token("s0", Kind::Share);
    let (_, rows) = get_json(format!("http://{}/api/sessions", srv.guest), &t).await;
    let s0 = row(&rows, "s0");
    assert_eq!((&s0["pair_gid"], &s0["pair_peers"], &s0["pair_task"]), (&json!("g1"), &json!([]), &Value::Null));
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_outside_its_session_is_refused_without_reaching_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    codex_session(&srv, dir.path());
    let t = srv.g.token("s", Kind::Share);
    // Mais recusas que o teto de falhas (8): se contassem como token errado, a origem travaria.
    for _ in 0..6 {
        for path in ["outra/events", "outra/cost"] {
            let (status, _) = get_json(format!("http://{}/api/sessions/{path}", srv.guest), &t).await;
            assert_eq!(status, 403, "{path}");
        }
    }
    let (status, _) = get_json(format!("http://{}/api/sessions/s/cost", srv.guest), &t).await;
    assert_ne!(status, 429, "o token do convidado segue aberto na própria sessão");
}

/// Na 8766 o método que uma rota do Rust não tem é recusa do Rust, nunca repasse assinado ao Python; troca
/// de modo e de conta são do dono, e o convidado completo leva a recusa do handler.
#[tokio::test(flavor = "multi_thread")]
async fn guest_port_refuses_unserved_methods_and_owner_only_relaunches_in_rust() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("s", Kind::Share);
    let url = |route: &str| format!("http://{}/api/sessions/s/{route}", srv.guest);
    let send = |method: reqwest::Method, route: &'static str, body: Value| {
        let request = client().request(method, url(route)).bearer_auth(&t).header("content-type", "application/json").body(body.to_string());
        async move {
            let r = request.send().await.unwrap();
            let status = r.status().as_u16();
            (status, serde_json::from_str::<Value>(&r.text().await.unwrap()).unwrap_or(Value::Null))
        }
    };
    for (method, route) in [(reqwest::Method::PUT, "permission-mode"), (reqwest::Method::GET, "model-effort"), (reqwest::Method::PUT, "pane")] {
        let (status, body) = send(method, route, json!({})).await;
        assert_eq!((status, body["detail"]["code"].clone()), (403, json!("erro_fora_do_convite")), "{route}: {body}");
    }
    // Antes iam assinados ao Python pelo `pass_any` da 8766 (o "from-python" do dublê); agora é a recusa do
    // handler, com o código do convite: o de login (`erro_fora_do_convidado`) é só da 8765 (`owner_refusal`).
    let refused = json!({"detail": {"code": "erro_fora_do_convite", "params": {}, "msg": "fora da sessão compartilhada"}});
    for (method, route, body) in [(reqwest::Method::POST, "modo-execucao", json!({"terminal": true})), (reqwest::Method::POST, "resume", json!({})),
        (reqwest::Method::GET, "conta", json!({})), (reqwest::Method::POST, "conta", json!({"config_dir": "/x"}))] {
        assert_eq!(send(method, route, body).await, (403, refused.clone()), "{route}");
    }
    for route in ["permission-mode", "model-effort", "pane", "modo-execucao", "resume", "conta"] {
    }
}

// R3-11 (`share/caller.rs:23,99`): pela 8766, o porteiro decide quem é o convidado. O convite completo lê o
// espelho; o par de leitura leva `erro_fora_do_convite`; o terminal escondido `term-X` não é a sessão X.
#[tokio::test(flavor = "multi_thread")]
async fn r3_11_guest_port_pane_is_for_the_full_invite_only() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_srv(dir.path(), 1).await;
    let url = |path: &str| format!("http://{}/api/sessions/{path}", srv.guest);
    let share = srv.g.token("s0", Kind::Share);
    let (status, body) = get_json(url("s0/pane"), &share).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["text"].as_str().unwrap().contains("pronto"), "{body}");
    let pair = srv.g.token("s0", Kind::Pair);
    let (status, body) = get_json(url("s0/pane"), &pair).await;
    assert_eq!((status, body["detail"]["code"].clone()), (403, json!("erro_fora_do_convite")), "{body}");
    assert_eq!(get_json(url("term-s0/pane"), &share).await.0, 403);
}

// R3-31 (`routes.rs:1158-1170`): com a porta da sessão presa pela troca de conversa, o `recarregar` do dono e
// o do convidado completo são o 409 de sessão ocupada pela rota, e o arquivo da sessão não muda.
#[tokio::test(flavor = "multi_thread")]
async fn r3_31_reload_while_a_transfer_holds_the_gate_is_409_busy_for_owner_and_guest() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_codex_environment(proof_home(dir.path())));
    let sidecar = dir.path().join("home/.hangar/claude-headless/s.json");
    let before = std::fs::read(&sidecar).unwrap();
    registry.ingress().hold("s", Duration::from_secs(1)).await.unwrap();
    let srv = plain_srv(dir.path(), |st| {
        // Sem pastas da lista: nada lê as transferências do HOME da máquina.
        let mut env = ListEnv::from_env();
        env.dirs = None;
        st.list = Arc::new(ListBridge::new(env, FactsClient::new()));
        assert!(st.state.runtime.set(registry).is_ok());
    }).await;
    let t = srv.g.token("s", Kind::Share);
    let busy = json!({"detail": {"code": "session_transfer_busy", "params": {}, "msg": "A sessão está trocando de agente; tente novamente quando terminar."}});
    for (base, token) in [(srv.owner, OWNER.to_owned()), (srv.guest, t)] {
        assert_eq!(post_json(format!("http://{base}/api/sessions/s/recarregar"), &token, json!({})).await, (409, busy.clone()), "{base}");
    }
    assert_eq!(std::fs::read(&sidecar).unwrap(), before, "nada mudou");
}

// R4-17 (`caller.rs:20`): o convidado do convite não alcança as rotas de grupo que faltavam provar; o
// porteiro recusa antes do handler, e nada vai ao Python.
#[tokio::test(flavor = "multi_thread")]
async fn r4_17_invite_guest_never_reaches_the_remaining_group_routes() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("s", Kind::Share);
    for (method, route, body) in [(reqwest::Method::DELETE, "pair", json!({})), (reqwest::Method::PUT, "pair/contract", json!({"text": "x"})),
        (reqwest::Method::POST, "pair-remote", json!({"peer": "outra::x"})), (reqwest::Method::POST, "unpair-remote", json!({}))] {
        let response = client().request(method.clone(), format!("http://{}/api/sessions/s/{route}", srv.guest)).bearer_auth(&t)
            .header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
        let status = response.status().as_u16();
        let reply: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null);
        assert_eq!((status, reply["detail"]["code"].clone()), (403, json!("erro_fora_do_convite")), "{method} {route}: {reply}");
    }
}
