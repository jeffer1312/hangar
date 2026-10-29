//! A porta do convidado servida pelo Rust: roteador próprio atrás do porteiro, bind que espera a
//! porta soltar e a saúde que anuncia quando ela está de pé.
use crate::fake;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::routes::AppState;
use hangar_server::share::ShareState;
use hangar_server::share::life::{LifeFacts, Lives, MuxProbe};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::ShareStore;
use hangar_server::share::store::Kind;
use futures_util::future::BoxFuture;
use hangar_server::share::tunnel::{Output, Real, Tailscale};
use serde_json::{Value, json};

fn share(dir: &std::path::Path) -> Arc<ShareState> {
    share_with(dir, Arc::new(Real))
}

fn share_with(dir: &std::path::Path, tailscale: Arc<dyn Tailscale>) -> Arc<ShareState> {
    Arc::new(ShareState {
        store: ShareStore::open(dir.join("shares.json"), Arc::new(|| 100.0)),
        pairs: PairStore::open(dir.join("external_pairs.json")),
        lives: Lives::new(Arc::new(MuxProbe::new("/nonexistent/tmux", dir.to_owned())), Arc::new(LifeFacts::default())),
        tailscale,
        tunnel: Default::default(),
    })
}

/// Túnel fora do ar: o convite que cair no ramo do Funnel responde 503.
struct NoTunnel;

impl Tailscale for NoTunnel {
    fn run<'a>(&'a self, _args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async { Err("sem tailscale".into()) })
    }
}

/// Sobe o servidor com a porta do convidado em `guest`; devolve a porta principal.
async fn spawn(up: SocketAddr, dir: &std::path::Path, guest: Option<SocketAddr>) -> SocketAddr {
    spawn_share(up, share(dir), guest).await
}

async fn spawn_share(up: SocketAddr, share: Arc<ShareState>, guest: Option<SocketAddr>) -> SocketAddr {
    let mut state = AppState::new(config(up, "127.0.0.1"));
    state.share = Some(share);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(hangar_server::routes::serve_with_guest(listener, state, guest));
    addr
}

async fn bound(main: SocketAddr) -> Value {
    let r = client().get(format!("http://{main}/__hangar_server/health")).send().await.unwrap();
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    v["guest_port_bound"].clone()
}

async fn wait_bound(main: SocketAddr) {
    for _ in 0..200 {
        if bound(main).await == json!(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a porta do convidado nunca subiu");
}

/// Porta livre agora; quem bindar depois disputa com outros testes, mas só por instantes.
async fn free_addr() -> SocketAddr {
    tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap()
}

#[tokio::test]
async fn guest_port_answers_through_the_gate() {
    let (_fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let guest = free_addr().await;
    let main = spawn(up, dir.path(), Some(guest)).await;
    wait_bound(main).await;

    let r = client().get(format!("http://{guest}/api/sessions")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["detail"]["code"], "erro_nao_autorizado");

    // O token do dono não vale aqui: a 8766 nunca pergunta `is_owner`.
    let r = client().get(format!("http://{guest}/api/sessions")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 401);

    // Rotas abertas passam o porteiro e são servidas pelo Rust: página do convite e a do par.
    for path in ["/convite/x", "/par/x"] {
        let r = client().get(format!("http://{guest}{path}")).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
        assert!(r.text().await.unwrap().contains("Convite indisponível"), "{path}");
    }

    // A porta do convidado não serve a saúde nem repassa ao Python.
    let r = client().get(format!("http://{guest}/__hangar_server/health")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn occupied_guest_port_is_retried_without_blocking_the_main_port() {
    let (_fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let holder = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let guest = holder.local_addr().unwrap();
    let main = spawn(up, dir.path(), Some(guest)).await;

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(bound(main).await, json!(false));
    let r = client().get(format!("http://{main}/api/qualquer")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python", "a porta principal atende enquanto a do convidado espera");

    drop(holder);
    wait_bound(main).await;
    let r = client().get(format!("http://{guest}/convite/x")).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

/// IP desta máquina na rede da casa; `None` sem rede (o teste que precisa dele não roda).
fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip()).filter(|ip| !ip.is_loopback())
}

/// O endereço local da conexão decide, como no Python: pela rede da casa o link é o IP dela; por
/// loopback (o Funnel) é o túnel, aqui fora do ar.
#[tokio::test]
async fn redeem_picks_lan_or_funnel_by_the_connection_local_address() {
    let Some(lan) = lan_ip() else { eprintln!("sem rede local: teste pulado"); return };
    let (_fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let share = share_with(dir.path(), Arc::new(NoTunnel));
    let port = free_addr().await.port();
    let main = spawn_share(up, share.clone(), Some(SocketAddr::from(([0, 0, 0, 0], port)))).await;
    wait_bound(main).await;

    let code = share.store.create("proj", None, Kind::Share, false).unwrap().1;
    let r = client().post(format!("http://127.0.0.1:{port}/api/guest/redeem"))
        .header("content-type", "application/json").body(json!({"code": code}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), 503, "loopback é o Funnel");
    let r = client().post(format!("http://{lan}:{port}/api/guest/redeem"))
        .header("content-type", "application/json").body(json!({"code": code}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["address"], format!("http://{lan}:8766"));
}

#[tokio::test]
async fn without_guest_address_nothing_listens() {
    let (_fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client().get(format!("http://{srv}/__hangar_server/health")).send().await.unwrap();
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["guest_port_bound"], json!(false));
    assert!(hangar_server::config::GuestEnv::from_lookup(|_| None).unwrap().listen.is_none());
}

#[test]
fn guest_env_reads_address_and_shares_file() {
    let env = |k: &str| match k {
        "HANGAR_SERVER_GUEST_LISTEN" => Some("0.0.0.0:8766".to_owned()),
        "HANGAR_SHARES_FILE" => Some("/tmp/x/shares.json".to_owned()),
        _ => None,
    };
    let g = hangar_server::config::GuestEnv::from_lookup(env).unwrap();
    assert_eq!(g.listen, Some("0.0.0.0:8766".parse().unwrap()));
    assert_eq!(g.shares_file, Some(std::path::PathBuf::from("/tmp/x/shares.json")));
    let bad = |k: &str| (k == "HANGAR_SERVER_GUEST_LISTEN").then(|| "nao".to_owned());
    assert!(hangar_server::config::GuestEnv::from_lookup(bad).unwrap_err().contains("HANGAR_SERVER_GUEST_LISTEN"));
}
