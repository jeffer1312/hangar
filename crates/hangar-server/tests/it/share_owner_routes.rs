//! Rotas do dono para convites (`test_share_api.py`) e a varredura que mantém o Funnel coerente.
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{Router, http::header, routing::get};
use futures_util::future::BoxFuture;
use hangar_server::share::ShareState;
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::owner_routes::{owner_router, sweep_once, sync_tunnel, LocalNet, OwnerShareState, Reach, ENDED_GRACE};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::{Output, Tailscale};
use hangar_server::share_reach::Probe;
use serde_json::{Value, json};

#[derive(Default)]
struct TsState {
    on: bool,
    no_operator: bool,
    funnel_refused: Option<String>,
    missing_binary: bool,
    calls: Vec<Vec<String>>,
}

#[derive(Default)]
struct FakeTs(Mutex<TsState>);

impl FakeTs {
    fn set(&self, f: impl FnOnce(&mut TsState)) { f(&mut self.0.lock().unwrap()) }
    fn on(&self) -> bool { self.0.lock().unwrap().on }
    fn calls(&self) -> Vec<Vec<String>> { self.0.lock().unwrap().calls.clone() }
    fn turned_on(&self) -> bool {
        self.calls().iter().any(|c| c[0] == "funnel" && c.iter().any(|a| a == "--bg"))
    }
}

impl Tailscale for FakeTs {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move { self.reply(args) })
    }
    fn needs_operator(&self) -> bool { true }
    fn user(&self) -> String { "jefferson".into() }
}

impl FakeTs {
    fn reply(&self, args: &[&str]) -> Result<Output, String> {
        let mut s = self.0.lock().unwrap();
        s.calls.push(args.iter().map(|a| a.to_string()).collect());
        if s.missing_binary { return Err("tailscale nao encontrado".into()); }
        let (mut out, mut success) = (String::new(), true);
        if args.starts_with(&["status", "--json"]) {
            out = json!({"Self": {"ID": "n1", "DNSName": "nb.ts.net.", "CapMap": {"funnel": null}}}).to_string();
        } else if args.starts_with(&["debug", "prefs"]) {
            out = if s.no_operator { json!({}) } else { json!({"OperatorUser": "jefferson"}) }.to_string();
        } else if args.starts_with(&["funnel", "status", "--json"]) {
            out = if s.on { json!({"AllowFunnel": {"nb.ts.net:8443": true}}) } else { json!({}) }.to_string();
        } else if args[0] == "funnel" && args.last() == Some(&"off") {
            s.on = false;
        } else if let Some(text) = s.funnel_refused.clone() {
            (out, success) = (text, false);
        } else {
            s.on = true;
        }
        Ok(Output { success, stdout: out, stderr: String::new() })
    }
}

/// Só "proj" existe; o resto o multiplexador confirma ausente.
#[derive(Default)]
struct FakeLife { births: Mutex<HashMap<String, u64>> }

impl LifeProbe for FakeLife {
    fn birth<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<u64>> {
        let b = self.births.lock().unwrap().get(name).copied();
        Box::pin(async move { b })
    }
    fn has_session<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<bool>> {
        let has = self.births.lock().unwrap().contains_key(name);
        Box::pin(async move { Some(has) })
    }
    fn sidecar(&self, _name: &str) -> Option<(String, bool)> { None }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

#[derive(Default)]
struct FakeNet { listening: Mutex<bool>, asked: Mutex<Vec<(String, u16)>> }

impl LocalNet for FakeNet {
    fn lan_ip(&self) -> String { "192.168.77.142".into() }
    fn listening<'a>(&'a self, ip: &'a str, port: u16) -> BoxFuture<'a, bool> {
        self.asked.lock().unwrap().push((ip.into(), port));
        let on = *self.listening.lock().unwrap();
        Box::pin(async move { on })
    }
}

struct Env {
    _dir: tempfile::TempDir,
    ts: Arc<FakeTs>,
    life: Arc<FakeLife>,
    net: Arc<FakeNet>,
    clock: Arc<Mutex<f64>>,
    share: Arc<ShareState>,
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

/// O DNS público responde 127.0.0.1 e o "Funnel" entrega a mesma página que a porta local.
async fn reach_ok() -> Reach {
    let doh = serve(Router::new().route("/resolve", get(|| async {
        ([(header::CONTENT_TYPE, "application/dns-json")],
         r#"{"Status":0,"Answer":[{"name":"nb.ts.net.","type":1,"TTL":60,"data":"127.0.0.1"}]}"#)
    }))).await;
    let page = serve(Router::new().fallback(|| async { "<h1>convite</h1>" })).await;
    Reach {
        probe: Probe { doh: vec![format!("http://{doh}/resolve")], funnel_scheme: "http", funnel_port: page.port(), connect_port: 1, local_base: format!("http://{page}") },
        every: Duration::from_millis(20),
        deadline: Duration::from_secs(5),
    }
}

/// O DNS público nunca conhece o nome: a conferência falha no estágio `dns`.
async fn reach_dns_fails() -> Reach {
    let doh = serve(Router::new().route("/resolve", get(|| async {
        ([(header::CONTENT_TYPE, "application/dns-json")], r#"{"Status":3}"#)
    }))).await;
    Reach {
        probe: Probe { doh: vec![format!("http://{doh}/resolve")], funnel_scheme: "http", funnel_port: 1, connect_port: 1, local_base: "http://127.0.0.1:1".into() },
        every: Duration::from_millis(20),
        deadline: Duration::from_millis(200),
    }
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(1_000.0));
    let c = clock.clone();
    let store = ShareStore::open(dir.path().join("shares.json"), Arc::new(move || *c.lock().unwrap()));
    let life = Arc::new(FakeLife::default());
    life.births.lock().unwrap().insert("proj".into(), 1);
    let facts = Arc::new(LifeFacts::default());
    facts.apply_transfer_snapshot(HashMap::new(), HashSet::new());
    let ts = Arc::new(FakeTs::default());
    let share = Arc::new(ShareState {
        store,
        pairs: PairStore::open(dir.path().join("external_pairs.json")),
        lives: Lives::new(life.clone(), facts),
        tailscale: ts.clone(),
        tunnel: Default::default(),
        connect: None,
    });
    Env { _dir: dir, ts, life, net: Arc::new(FakeNet::default()), clock, share }
}

impl Env {
    async fn app(&self, owner_port: u16, bind_ip: &str, reach: Reach) -> Client {
        let state = Arc::new(OwnerShareState {
            share: self.share.clone(), owner_port, bind_ip: bind_ip.into(), net: self.net.clone(), reach,
        });
        Client { base: format!("http://{}", serve(owner_router(state)).await), http: reqwest::Client::new() }
    }
    async fn default_app(&self) -> Client { self.app(8765, "127.0.0.1", reach_ok().await).await }
}

struct Client { base: String, http: reqwest::Client }

impl Client {
    async fn call(&self, method: reqwest::Method, path: &str) -> (u16, Value) {
        let r = self.http.request(method, format!("{}{path}", self.base)).send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    async fn post(&self, path: &str) -> (u16, Value) { self.call(reqwest::Method::POST, path).await }
    async fn get(&self, path: &str) -> (u16, Value) { self.call(reqwest::Method::GET, path).await }
    async fn delete(&self, path: &str) -> (u16, Value) { self.call(reqwest::Method::DELETE, path).await }
}

fn code_of(link: &str) -> String { link.rsplit('/').next().unwrap().to_owned() }

#[tokio::test]
async fn creates_lists_and_revokes() {
    let e = env();
    let cli = e.default_app().await;
    let (status, body) = cli.post("/api/sessions/proj/share").await;
    assert_eq!(status, 200, "{body}");
    let link = body["link"].as_str().unwrap();
    assert!(link.starts_with("https://nb.ts.net:8443/convite/"), "{link}");
    assert_eq!(e.share.store.peek(&code_of(link), Kind::Share).unwrap().session, "proj");
    assert!(body["expires_at"].is_number());

    let (_, list) = cli.get("/api/sessions/proj/share").await;
    let shares = list["shares"].as_array().unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["id"], body["id"]);
    assert_eq!(shares[0]["pending"], true);
    for key in ["device", "created_at", "redeemed_at", "expires_at"] {
        assert!(shares[0].get(key).is_some(), "{key}");
    }

    let id = body["id"].as_str().unwrap();
    assert_eq!(cli.delete(&format!("/api/sessions/proj/share/{id}")).await, (200, json!({"ok": true})));
    assert!(e.ts.on(), "carência: o convidado ainda recebe o \"encerrado\"");
    let (status, body) = cli.delete(&format!("/api/sessions/proj/share/{id}")).await;
    assert_eq!(status, 404);
    assert_eq!(body["detail"]["code"], "erro_compartilhamento_inexistente");
}

#[tokio::test]
async fn local_link_skips_funnel_and_reach_check() {
    let e = env();
    *e.net.listening.lock().unwrap() = true;
    // Conferência que falharia: o link local não passa por ela.
    let cli = e.app(8765, "0.0.0.0", reach_dns_fails().await).await;
    let (status, body) = cli.post("/api/sessions/proj/share?local=true").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["link"].as_str().unwrap().starts_with("http://192.168.77.142:8766/convite/"));
    assert_eq!(*e.net.asked.lock().unwrap(), vec![("192.168.77.142".to_owned(), 8766)]);
    assert!(!e.share.store.has_active(true));
    cli.delete("/api/sessions/proj/share").await;
    assert!(e.ts.calls().is_empty(), "nunca toca o tailscale: {:?}", e.ts.calls());
}

#[tokio::test]
async fn app_on_guest_port_refuses_invite() {
    for local in ["false", "true"] {
        let e = env();
        *e.net.listening.lock().unwrap() = true;
        let cli = e.app(8766, "0.0.0.0", reach_ok().await).await;
        let (status, body) = cli.post(&format!("/api/sessions/proj/share?local={local}")).await;
        assert_eq!(status, 409);
        assert_eq!(body["detail"]["code"], "erro_compartilhar_porta_do_convite");
        assert_eq!(body["detail"]["params"], json!({"port": 8766}));
        assert!(!e.ts.turned_on());
    }
}

#[tokio::test]
async fn local_link_with_loopback_bind_explains() {
    let e = env();
    *e.net.listening.lock().unwrap() = true;
    let cli = e.default_app().await;
    let (status, body) = cli.post("/api/sessions/proj/share?local=true").await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"]["code"], "erro_compartilhar_sem_rede_local");
}

#[tokio::test]
async fn local_link_with_nobody_listening_is_refused() {
    let e = env();
    let cli = e.app(8765, "::", reach_ok().await).await;
    let (status, _) = cli.post("/api/sessions/proj/share?local=1").await;
    assert_eq!(status, 409);
    assert!(!e.share.store.has_any(false));
}

#[tokio::test]
async fn bad_local_flag_is_422() {
    let e = env();
    let (status, _) = e.default_app().await.post("/api/sessions/proj/share?local=talvez").await;
    assert_eq!(status, 422);
}

#[tokio::test]
async fn revoke_all() {
    let e = env();
    let cli = e.default_app().await;
    cli.post("/api/sessions/proj/share").await;
    cli.post("/api/sessions/proj/share").await;
    assert_eq!(cli.delete("/api/sessions/proj/share").await, (200, json!({"ok": true, "revoked": 2})));
    assert!(!e.share.store.has_active(false));
}

#[tokio::test]
async fn missing_session_is_404() {
    let e = env();
    let (status, body) = e.default_app().await.post("/api/sessions/nada/share").await;
    assert_eq!(status, 404);
    assert_eq!(body["detail"]["code"], "erro_sessao_inexistente");
}

#[tokio::test]
async fn missing_prereq_saves_nothing() {
    let e = env();
    e.ts.set(|s| s.no_operator = true);
    let (status, body) = e.default_app().await.post("/api/sessions/proj/share").await;
    assert_eq!(status, 409);
    let d = &body["detail"];
    assert_eq!(d["code"], "erro_compartilhar_pre_requisito");
    assert_eq!(d["params"]["missing"], json!(["operator"]));
    assert!(d["params"]["fix"].as_str().unwrap().contains("--operator"));
    assert!(d["params"].get("enable_url").is_none());
    assert!(d["msg"].as_str().unwrap().starts_with("falta preparar o compartilhamento: "));
    assert!(!e.share.store.has_any(false));
}

#[tokio::test]
async fn refused_funnel_carries_enable_link() {
    let e = env();
    e.ts.set(|s| s.funnel_refused = Some("Funnel off: https://login.tailscale.com/f/funnel?node=n1.".into()));
    let (_, body) = e.default_app().await.post("/api/sessions/proj/share").await;
    assert_eq!(body["detail"]["params"]["enable_url"], "https://login.tailscale.com/f/funnel?node=n1");
}

#[tokio::test]
async fn owner_prereqs() {
    let e = env();
    e.ts.set(|s| s.no_operator = true);
    let (status, body) = e.default_app().await.get("/api/share/prereqs").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"missing": ["operator"], "fix": "sudo tailscale set --operator=$USER", "enable_url": null}));
}

#[tokio::test]
async fn prereqs_without_tailscale_is_prereq_409() {
    let e = env();
    e.ts.set(|s| s.missing_binary = true);
    let (status, body) = e.default_app().await.get("/api/share/prereqs").await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"]["code"], "erro_compartilhar_pre_requisito");
    assert_eq!(body["detail"]["params"], json!({"missing": [], "fix": "tailscale nao encontrado"}));
}

#[tokio::test]
async fn funnel_link_waits_for_reach_check() {
    let e = env();
    let (status, body) = e.default_app().await.post("/api/sessions/proj/share").await;
    assert_eq!(status, 200, "{body}");
    assert!(e.share.store.has_active(true));
}

#[tokio::test]
async fn failed_reach_check_revokes_invite_and_is_409() {
    let e = env();
    let cli = e.app(8765, "127.0.0.1", reach_dns_fails().await).await;
    let (status, body) = cli.post("/api/sessions/proj/share").await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "share_reach_failed");
    assert_eq!(body["detail"]["params"]["stage"], "dns");
    assert!(body["detail"]["params"]["detail"].is_string());
    assert!(!e.share.store.has_active(false), "o link nunca nasce sem abrir de fora");
    assert!(e.share.store.has_any(false));
}

#[tokio::test]
async fn sync_never_touches_tailscale_of_who_never_shared() {
    let e = env();
    sync_tunnel(&e.share).await;
    assert!(e.ts.calls().is_empty());
    e.share.store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    sync_tunnel(&e.share).await;
    assert!(e.ts.on());
}

#[tokio::test]
async fn funnel_stays_on_during_grace_then_drops() {
    let e = env();
    let (s, _) = e.share.store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    e.share.store.revoke(&s.id).unwrap();
    sync_tunnel(&e.share).await;
    assert!(e.ts.on());
    *e.clock.lock().unwrap() += ENDED_GRACE.as_secs_f64() + 1.0;
    sync_tunnel(&e.share).await;
    assert!(!e.ts.on());
}

#[tokio::test]
async fn sync_skips_tailscale_once_funnel_is_known_off() {
    let e = env();
    let (s, _) = e.share.store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    e.share.store.revoke(&s.id).unwrap();
    *e.clock.lock().unwrap() += ENDED_GRACE.as_secs_f64() + 1.0;
    sync_tunnel(&e.share).await;
    let n = e.ts.calls().len();
    assert!(n > 0, "a primeira rodada confere o Funnel");
    sync_tunnel(&e.share).await;
    sync_tunnel(&e.share).await;
    assert_eq!(e.ts.calls().len(), n, "já desligado: nenhum `tailscale` a mais");
    // Um convite novo volta a conferir.
    e.share.store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    sync_tunnel(&e.share).await;
    assert!(e.ts.on());
}

#[tokio::test]
async fn active_pair_alone_keeps_funnel() {
    let e = env();
    e.share.store.create_redeemed("proj", Some("t:1".into()), Kind::Pair).unwrap();
    sync_tunnel(&e.share).await;
    assert!(e.ts.on());
}

#[tokio::test]
async fn revoke_all_keeps_the_pair() {
    let e = env();
    let (_, token) = e.share.store.create_redeemed("proj", Some("t:1".into()), Kind::Pair).unwrap();
    let cli = e.default_app().await;
    cli.post("/api/sessions/proj/share").await;
    assert_eq!(cli.delete("/api/sessions/proj/share").await, (200, json!({"ok": true, "revoked": 1})));
    assert!(e.share.store.lookup_token(&token).unwrap().share_for("proj").unwrap().revoked_at.is_none());
}

#[tokio::test]
async fn sweep_once_revokes_reborn_session_and_keeps_funnel_in_grace() {
    let e = env();
    e.share.store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    e.life.births.lock().unwrap().insert("proj".into(), 2);
    sweep_once(&e.share).await;
    assert!(!e.share.store.has_active(false));
    assert!(e.ts.on(), "carência do funnel");
}
