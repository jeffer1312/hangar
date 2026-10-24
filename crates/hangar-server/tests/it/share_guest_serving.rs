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

use axum::http::StatusCode;
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
        facts.apply_snapshot(HashMap::new(), HashSet::new());
        let share = Arc::new(ShareState {
            store: ShareStore::open(dir.join("shares.json"), Arc::new(|| 100.0)),
            pairs: PairStore::open(dir.join("external_pairs.json")),
            lives: Lives::new(probe.clone(), facts),
            tailscale: Arc::new(Real),
            tunnel: Default::default(),
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
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, ""));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    let srv = serve(state, python, root).await;
    for i in 0..n {
        srv.g.alive(&format!("s{i}"));
    }
    srv
}

async fn plain_srv(dir: &Path, tune: impl FnOnce(&mut AppState)) -> Srv {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.side.locate = fake::fake_locate(upstream);
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
    assert!(!text.contains("s2"), "{text}");
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
async fn stream_da_lista_nao_cita_outras_sessoes() {
    let dir = tempfile::tempdir().unwrap();
    chain(dir.path());
    let srv = list_srv(dir.path(), 3).await;
    let t = srv.g.token("s0", Kind::Share);
    let mut es = open_sse(format!("http://{}/api/sessions/events?token={t}", srv.guest)).await;
    let ev = next_named(&mut es, "sessions").await;
    assert!(!ev.data.contains("s2"), "{}", ev.data);
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
    assert_eq!(srv.python.hits_to("/api/hangar-terminals"), 0);
}

/// No Python o atalho "no Hangar" do convidado é 403 próprio: o pedido chega lá como convidado
/// assinado, nunca com a identidade do dono.
#[tokio::test(flavor = "multi_thread")]
async fn guest_cannot_start_a_hangar_copy() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("cc", Kind::Share);
    post_json(format!("http://{}/api/sessions/cc/shortcut-shell", srv.guest), &t,
        json!({"command": "notepad", "runs_in": "hangar", "key": "global:k"})).await;
    assert_eq!(srv.python.hits_to("/api/sessions/cc/shortcut-shell"), 1);
    let (_, h) = srv.python.last_hit();
    assert!(!h.contains_key("authorization"));
    assert!(h.contains_key("x-hangar-guest"));
}

// ── repasse assinado ao Python ───────────────────────────────────────────────────────────────

/// Confere o `x-hangar-guest` como o `guest_header.verify` do Python: HMAC sobre método, caminho
/// cru e payload; devolve o payload.
fn verified(h: &axum::http::HeaderMap, method: &str, raw_path: &str) -> Value {
    use base64::Engine;
    let value = h["x-hangar-guest"].to_str().unwrap();
    let (payload, mac) = value.split_once('.').unwrap();
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, SECRET.as_bytes());
    let expected: String = ring::hmac::sign(&key, format!("{method}\n{raw_path}\n{payload}").as_bytes())
        .as_ref().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(mac, expected, "HMAC do cabeçalho");
    let body: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let exp = body["exp"].as_u64().unwrap();
    assert!(exp > now && exp <= now + 31, "validade curta: {exp} vs {now}");
    body
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_forward_is_signed_and_carries_no_invite_token() {
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("cc", Kind::Share);
    let r = client().get(format!("http://{}/api/sessions/cc/plan-preview?token={t}&x=1%202&y=a+b", srv.guest))
        .header("x-hangar-guest", "forjado").send().await.unwrap();
    assert_eq!((r.status().as_u16(), r.text().await.unwrap()), (200, "from-python".into()));
    let (path, h) = srv.python.last_hit();
    assert_eq!(path, "/api/sessions/cc/plan-preview?x=1%202&y=a+b");
    assert!(!h.contains_key("authorization"));
    assert_eq!(h["x-forwarded-for"], "127.0.0.1");
    assert_eq!(verified(&h, "GET", "/api/sessions/cc/plan-preview")["s"], json!({"cc": "share"}));

    // Rota global (fora de sessão) também vai assinada, e o Bearer não segue.
    let r = client().get(format!("http://{}/api/model-options", srv.guest)).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let (path, h) = srv.python.last_hit();
    assert_eq!(path, "/api/model-options");
    assert!(!h.contains_key("authorization"));
    verified(&h, "GET", "/api/model-options");
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
    for status in [StatusCode::UNAUTHORIZED, StatusCode::TOO_MANY_REQUESTS] {
        srv.python.set_input_reply(Some((status, json!({}))));
        assert_eq!(post_json(url.clone(), &t, json!({"text": "oi"})).await.0, status.as_u16());
    }
    assert_eq!(srv.python.input_calls(), 2);
    assert!(srv.st.auth.is_owner(&lan.to_string(), Some(OWNER.as_bytes())), "o dono nesse IP segue livre");
}

/// WebSocket repassado ao Python (o navegador remoto): revogar fecha com 4410, que o Python não sabe.
#[tokio::test(flavor = "multi_thread")]
async fn forwarded_guest_websocket_closes_4410_when_revoked() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let srv = plain_srv(dir.path(), |_| {}).await;
    let t = srv.g.token("cc", Kind::Share);
    let mut s = tokio::net::TcpStream::connect(srv.guest).await.unwrap();
    s.write_all(format!("GET /api/sessions/cc/nav-remoto?token={t} HTTP/1.1\r\nHost: h\r\n\
        Connection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut byte)).await.unwrap().unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    s.write_all(b"ola").await.unwrap();
    let mut got = [0u8; 3];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut got)).await.unwrap().unwrap();
    assert_eq!(&got, b"ola");
    let (_, h) = srv.python.last_hit();
    verified(&h, "GET", "/api/sessions/cc/nav-remoto");
    srv.g.revoke_all();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(12), s.read_to_end(&mut rest)).await.expect("o vigia fecha em até 5 s").unwrap();
    assert_eq!(rest, [0x88, 0x02, 0x11, 0x3A]);
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
    assert_eq!(srv.python.hits_to("/api/sessions/s/history"), 0);
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
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "secret-test".into(), "instance-test".into())
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
    assert_eq!(srv.python.hits_to("/api/sessions/s/input"), 0);
    assert_eq!(srv.python.hits_to("/api/sessions/s/interrupt"), 0);
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
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "secret-test".into(), "instance-test".into()));
    registry.open_terminal(terminal_target(dir.path(), "t")).await.unwrap();
    let srv = plain_srv(dir.path(), |st| assert!(st.state.runtime.set(registry).is_ok())).await;
    let t = srv.g.token("t", Kind::Share);
    let (status, body) = post_json(format!("http://{}/api/sessions/t/plugin/press", srv.guest), &t,
        json!({"site": "x", "plugin": "p", "key": "k"})).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("erro_mod_clique_sem_resposta")));
    assert_eq!(srv.python.hits_to("/api/sessions/t/plugin/press"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_reaches_the_mods_the_rust_serves() {
    use hangar_server::mods::click::Limits;
    use hangar_server::mods::terminal::TerminalLink;
    use mods_support::pane::{Effect::*, FakePane, view};
    let dir = tempfile::tempdir().unwrap();
    let mut mods = None;
    let srv = plain_srv(dir.path(), |st| {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "secret-test".into(), "instance-test".into())
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
    assert_eq!(srv.python.hits_to("/api/sessions/t/plugin/show"), 0);
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
    assert_eq!(srv.python.hits_to("/api/sessions/t/term"), 0);
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
    assert_eq!(srv.python.hits_to("/api/sessions/x/input"), 0);
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
    assert_eq!(srv.python.input_calls(), 0);
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
    let dir = tempfile::tempdir().unwrap();
    group_of_three(dir.path());
    let srv = list_srv(dir.path(), 3).await;
    let (pair, t) = srv.g.share.store.create_redeemed("s0", Some("k:L-s0".into()), Kind::Pair).unwrap();
    srv.g.share.store.grant(&pair.id, "s1", Some("k:L-s1".into()), Kind::Share).unwrap();
    let (_, rows) = get_json(format!("http://{}/api/sessions", srv.guest), &t).await;
    assert_eq!(names(&rows), ["s0", "s1"]);
    let s0 = row(&rows, "s0");
    assert_eq!((&s0["pair_gid"], &s0["pair_peers"], &s0["pair_task"]), (&json!("g1"), &json!(["s1"]), &json!("T")));
    assert_eq!(row(&rows, "s1")["pair_peers"], json!(["s0"]));
    assert!(!serde_json::to_string(&rows).unwrap().contains("s2"), "{rows}");
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
