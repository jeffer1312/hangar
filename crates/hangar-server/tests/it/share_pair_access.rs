//! Acesso de fora segue o grupo: entrar dá "ver e recado", o "Liberar" do dono dá acesso completo,
//! sair revoga na hora e a ponta do par saindo (ou morrendo) desfaz o par para o grupo inteiro.
#![cfg(unix)]
use crate::fake;
use crate::list_support;
use crate::share_pair_routes::{Other, other_machine, share_state};

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::Method;
use fake::{OWNER, SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::GroupService;
use hangar_server::groups::store::PairDir;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, guest_router, router, terminal_router};
use hangar_server::share::ShareState;
use hangar_server::share::caller::{GuestGrant, guest_allowed};
use hangar_server::share::gate::GuestConn;
use hangar_server::share::pair_routes::{self, Direct};
use hangar_server::share::pairs::ExternalPair;
use hangar_server::share::store::Kind;
use serde_json::{Value, json};

struct Srv {
    _dir: tempfile::TempDir,
    public: SocketAddr,
    private: SocketAddr,
    guest: SocketAddr,
    app: Arc<AppState>,
    share: Arc<ShareState>,
    pair: PathBuf,
    other: Arc<Other>,
    other_base: String,
    _sink: crate::sink::Sink,
}

async fn server() -> Srv {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let script = list_support::sessions(&root, 4);
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (_python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, ""));
    // Cada sessão é um Claude sem terminal do Rust: o aviso do grupo chega ao cano falso dela.
    let runtime = Arc::new(crate::sink::registry().await);
    let sink = crate::sink::Sink::default();
    std::fs::create_dir_all(root.join("rt")).unwrap();
    for name in ["s0", "s1", "s2", "s3"] { sink.open(&runtime, &root.join("rt"), name).await; }
    assert!(state.state.runtime.set(runtime).is_ok());
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    let pair = home.join(".claude/.hangar-pair");
    state.groups = Some(Arc::new(GroupService::new(PairDir::new(pair.clone(), root.join("arquivo")),
        Arc::new(PythonOrq::from_state(&state)), "casa".into())));
    let share = share_state(&root.join("shares.json"), &pair, Arc::new(Mutex::new(1_000.0)));
    state.share = Some(share.clone());
    let app = Arc::new(state);
    let bind = || async { tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap() };
    let (l1, l2, l3) = (bind().await, bind().await, bind().await);
    let (public, private, guest) = (l1.local_addr().unwrap(), l2.local_addr().unwrap(), l3.local_addr().unwrap());
    let (r1, r2, r3) = (router(app.clone()), terminal_router(app.clone()), guest_router(app.clone()));
    tokio::spawn(async move { axum::serve(l1, r1.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    tokio::spawn(async move { axum::serve(l2, r2.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    tokio::spawn(async move { axum::serve(l3, r3.into_make_service_with_connect_info::<GuestConn>()).await.unwrap() });
    let (other, other_base) = other_machine().await;
    Srv { _dir: dir, public, private, guest, app, share, pair, other, other_base, _sink: sink }
}

impl Srv {
    async fn call(&self, addr: SocketAddr, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = client().request(method.parse().unwrap(), format!("http://{addr}{path}")).bearer_auth(token);
        if let Some(b) = body { req = req.header("content-type", "application/json").body(b.to_string()); }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    async fn owner(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call(self.public, method, path, OWNER, body).await
    }
    fn sidecar(&self, name: &str) -> Option<Value> {
        std::fs::read(self.pair.join(format!("{name}.json"))).ok().map(|b| serde_json::from_slice(&b).unwrap())
    }
    fn group(&self, name: &str, peers: &[&str]) {
        std::fs::create_dir_all(&self.pair).unwrap();
        std::fs::write(self.pair.join(format!("{name}.json")),
            json!({"peers": peers, "task": "", "gid": "abababab", "harness": {}}).to_string()).unwrap();
    }
    /// Par de `s0` com `ana::Y` em acesso completo, sozinho num grupo; devolve (raiz, token de fora).
    async fn full_pair(&self) -> (String, String) {
        let life = self.share.lives.life("s0", true).await.unwrap();
        let (root, token) = self.share.store.create_redeemed("s0", Some(life.clone()), Kind::Pair).unwrap();
        self.share.pairs.add(ExternalPair { share_id: root.id.clone(), local_session: "s0".into(), alias: "ana".into(),
            peer_owner: "ana".into(), peer_session: "Y".into(), peer_address: self.other_base.clone(),
            peer_token: "r".repeat(32), created_at: 1.0 }).unwrap();
        self.share.store.grant(&root.id, "s0", Some(life), Kind::Share).unwrap();
        self.group("s0", &["ana::Y"]);
        (root.id, token)
    }
    fn kind(&self, token: &str, session: &str) -> Option<Kind> {
        let guest = self.share.store.lookup_token(token)?;
        guest.sees(session).then(|| guest.kind_of(session)).flatten()
    }
    fn revoked(&self, id: &str) -> bool { self.share.store.all().iter().any(|s| s.id == id && s.revoked_at.is_some()) }
    async fn join(&self, name: &str) {
        let (status, body) = self.owner("POST", &format!("/api/sessions/{name}/pair"), Some(json!({"peer": "s0"}))).await;
        assert_eq!(status, 200, "{body}");
    }
    async fn join_s1(&self) { self.join("s1").await }
    async fn join_s2(&self) { self.join("s2").await }
}

#[tokio::test(flavor = "multi_thread")]
async fn joining_the_group_gives_view_even_when_the_pair_is_full() {
    let s = server().await;
    let (_, token) = s.full_pair().await;
    s.join_s1().await;
    assert_eq!(s.kind(&token, "s1"), Some(Kind::Pair), "agente pondo no grupo nunca dá escrita");
    assert_eq!(s.kind(&token, "s0"), Some(Kind::Share), "a ponta continua com o que tinha");
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_releases_full_access_and_the_guest_cannot() {
    let s = server().await;
    let (_, token) = s.full_pair().await;
    s.join_s1().await;
    let (status, body) = s.owner("POST", "/api/sessions/s1/pair-access", Some(json!({"alias": "ana", "level": "full"}))).await;
    assert_eq!((status, &body), (200, &json!({"ok": true})));
    assert_eq!(s.kind(&token, "s1"), Some(Kind::Share));
    let (status, _) = s.call(s.guest, "POST", "/api/sessions/s1/pair-access", &token, Some(json!({"alias": "ana", "level": "full"}))).await;
    assert_eq!(status, 403);
    s.owner("POST", "/api/sessions/s1/pair-access", Some(json!({"alias": "ana", "level": "view"}))).await;
    assert_eq!(s.kind(&token, "s1"), Some(Kind::Pair));
    let (status, body) = s.owner("POST", "/api/sessions/s1/pair-access", Some(json!({"alias": "bia", "level": "full"}))).await;
    assert_eq!((status, &body["detail"]["code"]), (404, &json!("erro_par_inexistente")));
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_reaches_pair_access_and_undo_by_private_routes() {
    let s = server().await;
    let (root, token) = s.full_pair().await;
    s.join_s1().await;
    let private = |path: &str| client().post(format!("http://{}/__hangar_server/share/owner/{path}", s.private))
        .header("x-hangar-internal", SECRET).header("content-type", "application/json");
    let r = private("pair_access/s1").body(json!({"alias": "ana", "level": "full"}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(s.kind(&token, "s1"), Some(Kind::Share));
    let r = client().delete(format!("http://{}/__hangar_server/share/owner/pairs_undo/ana?session=Y", s.private))
        .header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(s.revoked(&root));
}

#[tokio::test(flavor = "multi_thread")]
async fn member_leaving_loses_access_at_once_and_the_pair_stays() {
    let s = server().await;
    let (root, token) = s.full_pair().await;
    s.join_s1().await;
    let (before, _) = s.call(s.guest, "GET", "/api/sessions/s1/history", &token, None).await;
    assert!(![403, 410].contains(&before), "{before}");
    let (status, body) = s.owner("DELETE", "/api/sessions/s1/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(s.kind(&token, "s1"), None);
    let (after, _) = s.call(s.guest, "GET", "/api/sessions/s1/history", &token, None).await;
    assert!([403, 410].contains(&after), "{after}");
    assert!(!s.revoked(&root) && s.share.pairs.all().len() == 1, "o par continua");
    assert!(s.other.keys().is_empty(), "o outro lado não é avisado");
    assert_eq!(s.sidecar("s0").unwrap()["peers"], json!(["ana::Y"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn anchor_leaving_ends_the_pair_for_the_whole_group() {
    let s = server().await;
    let (root, token) = s.full_pair().await;
    s.join_s1().await;
    let (status, body) = s.owner("DELETE", "/api/sessions/s0/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(s.other.keys(), ["DELETE /api/pair"]);
    assert!(s.revoked(&root) && s.share.pairs.all().is_empty());
    assert_eq!(s.kind(&token, "s1"), None);
    assert!(s.sidecar("s1").is_none_or(|v| !v["peers"].as_array().unwrap().contains(&json!("ana::Y"))), "{:?}", s.sidecar("s1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn dead_anchor_ends_the_pair_and_dead_member_only_loses_access() {
    let s = server().await;
    let (root, token) = s.full_pair().await;
    s.join_s1().await;
    s.join_s2().await;
    pair_routes::session_died(s.app.clone(), s.share.clone(), Arc::new(Direct), "s2").await;
    assert_eq!(s.kind(&token, "s2"), None);
    assert!(!s.revoked(&root));
    pair_routes::session_died(s.app.clone(), s.share.clone(), Arc::new(Direct), "s0").await;
    assert!(s.revoked(&root) && s.share.pairs.all().is_empty());
    assert!(s.sidecar("s1").is_none_or(|v| !v["peers"].as_array().unwrap().contains(&json!("ana::Y"))));
    let other = s.other.clone();
    fake::wait_until(move || other.keys() == ["DELETE /api/pair"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_undo_route_ends_the_pair_on_both_sides() {
    let s = server().await;
    let (root, _) = s.full_pair().await;
    s.join_s1().await;
    let (status, body) = s.owner("DELETE", "/api/external-pairs/ana?session=Y", None).await;
    assert_eq!((status, &body["ok"]), (200, &json!(true)), "{body}");
    assert_eq!(s.other.keys(), ["DELETE /api/pair"]);
    assert!(s.revoked(&root) && s.share.pairs.all().is_empty());
    for name in ["s0", "s1"] {
        assert_eq!(s.sidecar(name).unwrap()["peers"], json!([if name == "s0" { "s1" } else { "s0" }]), "os locais seguem juntos");
    }
    let (status, body) = s.owner("DELETE", "/api/external-pairs/ana?session=Y", None).await;
    assert_eq!((status, &body["detail"]["code"]), (404, &json!("erro_par_inexistente")));
}

#[tokio::test(flavor = "multi_thread")]
async fn full_access_from_a_pair_does_not_reach_the_global_chat_routes() {
    let s = server().await;
    let (_, token) = s.full_pair().await;
    let grant = |t: &str| GuestGrant::new(t, &s.share.store.lookup_token(t).unwrap());
    for path in ["/api/tts", "/api/ditado/relimpar", "/api/pensamento/pt"] {
        assert!(!guest_allowed(&Method::POST, path, &grant(&token)), "{path}: o crédito é do dono");
    }
    assert!(guest_allowed(&Method::GET, "/api/sessions/s0/history", &grant(&token)));
    let (_, invite) = s.share.store.create_redeemed("s3", Some("t:1".into()), Kind::Share).unwrap();
    assert!(guest_allowed(&Method::POST, "/api/tts", &grant(&invite)), "convite de verdade alcança");
}
