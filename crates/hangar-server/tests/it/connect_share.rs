//! Convite e par externo pelo Connect, sem Tailscale. Programas, DNS e "internet" são falsos.
use crate::connect_supervisor::{alone, gate, linked, pid_of, read_pid, until};
use crate::fake::{OWNER, client, config, spawn_fake};

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::header;
use axum::routing::get;
use futures_util::future::BoxFuture;
use hangar_server::connect::config::{render_frpc, render_guest_site};
use hangar_server::connect::state::{self, Linked};
use hangar_server::connect::supervisor::Supervisor;
use hangar_server::routes::AppState;
use hangar_server::share::ShareState;
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::owner_routes::{LocalNet, OwnerShareState, Reach, owner_router};
use hangar_server::share::pairs::{PairStore, normalize_address, parse_pair_link};
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::{Output, Tailscale};
use hangar_server::share_reach::Probe;
use serde_json::{Value, json};

const GUEST_BASE: &str = "https://pc1--guest.acme.hangar.dev.br";
const GUEST_SITE: &str = "reverse_proxy 127.0.0.1:8766";

struct FakeTs { works: bool }

impl Tailscale for FakeTs {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        let reply = if self.works {
            let stdout = if args.starts_with(&["status", "--json"]) {
                json!({"Self": {"ID": "n1", "DNSName": "nb.ts.net.", "CapMap": {"funnel": null}}}).to_string()
            } else if args.starts_with(&["funnel", "status", "--json"]) {
                json!({"AllowFunnel": {"nb.ts.net:8443": true}}).to_string()
            } else {
                String::new()
            };
            Ok(Output { success: true, stdout, stderr: String::new() })
        } else {
            Err("tailscale nao encontrado".to_owned())
        };
        Box::pin(async move { reply })
    }
    fn needs_operator(&self) -> bool { false }
}

struct FakeLife;

impl LifeProbe for FakeLife {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { Some(1) }) }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
    fn sidecar(&self, _name: &str) -> Option<(String, bool)> { None }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

struct NoNet;

impl LocalNet for NoNet {
    fn lan_ip(&self) -> String { "127.0.0.1".into() }
    fn listening<'a>(&'a self, _ip: &'a str, _port: u16) -> BoxFuture<'a, bool> { Box::pin(async { false }) }
}

struct Env {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    supervisor: Arc<Supervisor>,
    share: Arc<ShareState>,
}

/// Máquina ligada ao Connect, com o `frpc` falso entrando na hora; a 8766 fica por conta do teste.
fn env(tailscale: bool) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let supervisor = Arc::new(alone(&dir));
    state::write(&dir, &linked()).unwrap();
    std::fs::write(gate(&dir), "").unwrap();
    let facts = Arc::new(LifeFacts::default());
    facts.apply_snapshot(HashMap::new(), HashSet::new());
    let share = Arc::new(ShareState {
        store: ShareStore::open(dir.join("shares.json"), Arc::new(|| 1_000.0)),
        pairs: PairStore::open(dir.join("external_pairs.json")),
        lives: Lives::new(Arc::new(FakeLife), facts),
        tailscale: Arc::new(FakeTs { works: tailscale }),
        tunnel: Default::default(),
        connect: Some(supervisor.clone()),
    });
    Env { _tmp: tmp, dir, supervisor, share }
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

/// O DNS público responde 127.0.0.1 e só a porta da entrada esperada entrega a página.
async fn reach_ok(connect: bool) -> Reach {
    let doh = serve(Router::new().route("/resolve", get(|| async {
        ([(header::CONTENT_TYPE, "application/dns-json")],
         r#"{"Status":0,"Answer":[{"name":"x.","type":1,"TTL":60,"data":"127.0.0.1"}]}"#)
    }))).await;
    let page = serve(Router::new().fallback(|| async { "<h1>convite</h1>" })).await;
    let (funnel_port, connect_port) = if connect { (1, page.port()) } else { (page.port(), 1) };
    Reach {
        probe: Probe { doh: vec![format!("http://{doh}/resolve")], funnel_scheme: "http", funnel_port, connect_port, local_base: format!("http://{page}") },
        every: Duration::from_millis(20),
        deadline: Duration::from_secs(5),
    }
}

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

impl Env {
    async fn owner(&self, reach: Reach) -> String {
        let state = Arc::new(OwnerShareState { share: self.share.clone(), owner_port: 8765, bind_ip: "127.0.0.1".into(),
            net: Arc::new(NoNet), reach });
        format!("http://{}", serve(owner_router(state)).await)
    }

    fn published(&self) -> bool { state::read(&self.dir).unwrap().guest_published }

    fn caddyfile(&self) -> String { std::fs::read_to_string(self.dir.join("Caddyfile")).unwrap() }

    async fn caddy_up(&self) { until("Caddy de pé", || read_pid(&self.dir, "caddy").is_some()).await }
}

async fn call(method: reqwest::Method, url: String) -> (u16, Value) {
    let r = client().request(method, url).send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn invite(base: &str) -> (u16, Value) { call(reqwest::Method::POST, format!("{base}/api/sessions/proj/share")).await }

#[tokio::test]
async fn invite_uses_connect_when_tailscale_is_absent() {
    let e = env(false);
    e.supervisor.guest_port_opened();
    let base = e.owner(reach_ok(true).await).await;

    let (status, body) = invite(&base).await;
    assert_eq!(status, 200, "{body}");
    let link = body["link"].as_str().unwrap();
    assert!(link.starts_with(&format!("{GUEST_BASE}/convite/")), "{link}");
    assert!(e.published());
    assert_eq!(e.share.public_guest_url().await.unwrap(), GUEST_BASE);
    let frpc = std::fs::read_to_string(e.dir.join("frpc.toml")).unwrap();
    assert!(frpc.contains(r#"customDomains = ["pc1--guest.acme.hangar.dev.br"]"#), "{frpc}");
    e.caddy_up().await;
    assert!(e.caddyfile().contains(GUEST_SITE));

    // Cada subida pode gastar certificado: o segundo convite não mexe nos filhos.
    let (frpc_pid, caddy_pid) = (pid_of(&e.dir, "frpc"), pid_of(&e.dir, "caddy"));
    let (status, body) = invite(&base).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!((pid_of(&e.dir, "frpc"), pid_of(&e.dir, "caddy")), (frpc_pid, caddy_pid));
    e.supervisor.stop().await;
}

#[tokio::test]
async fn invite_keeps_funnel_when_tailscale_works() {
    let e = env(true);
    e.supervisor.guest_port_opened();
    let base = e.owner(reach_ok(false).await).await;

    let (status, body) = invite(&base).await;
    assert_eq!(status, 200, "{body}");
    let link = body["link"].as_str().unwrap();
    assert!(link.starts_with("https://nb.ts.net:8443/convite/"), "{link}");
    assert!(!e.published(), "com Tailscale nenhum certificado é gasto");
    assert_eq!(e.share.public_guest_url().await.unwrap(), "https://nb.ts.net:8443");
}

#[tokio::test]
async fn invite_is_checked_from_outside_before_showing() {
    let e = env(false);
    e.supervisor.guest_port_opened();
    let base = e.owner(reach_dns_fails().await).await;

    let (status, body) = invite(&base).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "share_reach_failed");
    assert!(!e.share.store.has_active(false), "o link nunca nasce sem abrir de fora");
    assert!(e.share.store.has_any(false));
    assert!(e.published(), "o site fica: refazer gastaria outro certificado");
    e.supervisor.stop().await;
}

#[tokio::test]
async fn guest_entrance_still_refuses_owner_token() {
    let e = env(false);
    let (_python, up) = spawn_fake().await;
    let mut server = AppState::new(config(up, "127.0.0.1"));
    server.share = Some(e.share.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    tokio::spawn(hangar_server::routes::serve_with_guest(listener, server, Some(guest)));
    tokio::time::timeout(Duration::from_secs(20), async {
        while tokio::net::TcpStream::connect(guest).await.is_err() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();

    // O Caddy entrega o site aqui, e nunca na entrada do dono.
    let site = render_guest_site(&linked());
    assert!(site.contains(GUEST_SITE) && !site.contains("8768"), "{site}");
    for path in ["/api/sessions", "/api/connect", "/api/sessions/proj/share"] {
        let r = client().get(format!("http://{guest}{path}")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
    let (status, _) = call(reqwest::Method::GET, format!("http://{guest}/api/sessions?token={OWNER}")).await;
    assert_eq!(status, 401);

    // Convidado de verdade também não alcança rota do dono.
    let life = e.share.lives.life("proj", true).await;
    let (_, token) = e.share.store.create_redeemed("proj", life, Kind::Share).unwrap();
    for path in ["/api/connect", "/api/connect/devices", "/api/sessions/proj/share", "/api/share/prereqs"] {
        let r = client().get(format!("http://{guest}{path}")).bearer_auth(&token).send().await.unwrap();
        assert_eq!(r.status(), 403, "{path}");
    }
}

#[tokio::test]
async fn external_pair_accepts_connect_guest_hosts_only() {
    assert_eq!(normalize_address(GUEST_BASE).as_deref(), Some(GUEST_BASE));
    assert_eq!(normalize_address(" https://PC1--guest.acme.hangar.dev.br/ ").as_deref(), Some(GUEST_BASE));
    assert_eq!(parse_pair_link(&format!("{GUEST_BASE}/par/AB12")), Some((GUEST_BASE.to_owned(), "AB12".to_owned())));
    assert_eq!(normalize_address("https://casa.tail1.ts.net:8443").as_deref(), Some("https://casa.tail1.ts.net:8443"));
    for bad in [
        "https://pc1.acme.hangar.dev.br", "https://pc1--preview.acme.hangar.dev.br", "https://acme.hangar.dev.br",
        "https://--guest.acme.hangar.dev.br", "https://pc1---guest.acme.hangar.dev.br", "https://pc1--guest.hangar.dev.br",
        "https://a.pc1--guest.acme.hangar.dev.br", "https://pc1--guest.acme.hangar.dev.br.", "https://pc1--guest.acme.hangar.dev.br.evil.com",
        "https://pc1--guest.acme.hangar.dev.br:443", "https://pc1--guest.acme.hangar.dev.br:8443", "https://pc1--guest.acme.hangar.dev.br:",
        "http://pc1--guest.acme.hangar.dev.br", "https://u@pc1--guest.acme.hangar.dev.br", "https://u:p@pc1--guest.acme.hangar.dev.br",
        "https://pc1--guest.acme.hangar.dev.br/x", "https://pc1--guest.acme.hangar.dev.br?x=1", "https://pc1--guest.acme.hangar.dev.br#x",
    ] {
        assert_eq!(normalize_address(bad), None, "{bad}");
    }
    assert_eq!(parse_pair_link("https://pc1.acme.hangar.dev.br/par/AB12"), None);
}

#[tokio::test]
async fn prereqs_are_met_by_connect_without_tailscale() {
    let e = env(false);
    let base = e.owner(reach_ok(true).await).await;
    let (status, body) = call(reqwest::Method::GET, format!("{base}/api/share/prereqs")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["missing"], json!([]));
}

#[tokio::test]
async fn reading_the_address_publishes_nothing() {
    let e = env(false);
    e.supervisor.guest_port_opened();
    assert!(e.share.public_guest_url().await.is_err(), "site nunca publicado não é endereço");
    assert!(!e.published(), "a página e o resgate não têm token: ler o endereço não gasta certificado");
}

#[tokio::test]
async fn guest_site_waits_for_this_server_to_open_the_guest_port() {
    let e = env(false);
    let base = e.owner(reach_ok(true).await).await;

    let (status, body) = invite(&base).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "erro_compartilhar_pre_requisito");
    assert!(!e.published());

    // Volta do servidor com o site publicado e a 8766 ainda na mão de outro: a entrada do dono sobe sozinha.
    let saved = Linked { guest_published: true, ..linked() };
    state::write(&e.dir, &saved).unwrap();
    e.supervisor.resume().await;
    e.caddy_up().await;
    assert!(!e.caddyfile().contains(GUEST_SITE));

    let (frpc_pid, caddy_pid) = (pid_of(&e.dir, "frpc"), pid_of(&e.dir, "caddy"));
    e.supervisor.guest_port_opened();
    until("Caddy novo", || read_pid(&e.dir, "caddy").is_some_and(|pid| pid != caddy_pid)).await;
    assert!(e.caddyfile().contains(GUEST_SITE));
    assert_eq!(pid_of(&e.dir, "frpc"), frpc_pid, "o `frpc` já tinha o proxy");
    e.supervisor.stop().await;
}

#[test]
fn guest_proxy_exists_only_once_published() {
    let mut value = linked();
    assert!(!render_frpc(&value).unwrap().contains("--guest"));
    value.guest_published = true;
    let text = render_frpc(&value).unwrap();
    assert_eq!(text.matches("[[proxies]]").count(), 2, "{text}");
    assert!(text.contains(r#"customDomains = ["pc1--guest.acme.hangar.dev.br"]"#), "{text}");

    // `connect.json` de antes do campo vale como nunca publicado.
    let mut old = serde_json::to_value(linked()).unwrap();
    old.as_object_mut().unwrap().remove("guest_published");
    assert!(!serde_json::from_value::<Linked>(old).unwrap().guest_published);
}
