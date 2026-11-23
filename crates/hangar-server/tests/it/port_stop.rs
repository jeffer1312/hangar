//! Parar o servidor de uma porta (`POST /api/sessions/{name}/ports/{port}/stop`) pelo listener de
//! verdade: porta que não é da sessão, processo protegido, credencial e a porta do convidado sem a rota.
use crate::fake::{OWNER, client, config, spawn_fake};
use crate::list_support;

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use hangar_server::routes::{AppState, guest_router};
use hangar_server::share::ShareState;
use hangar_server::share::gate::GuestConn;
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::Real;
use serde_json::Value;
use tokio::net::TcpListener;

async fn stop(addr: SocketAddr, path: &str, token: &str) -> (u16, Value) {
    let response = client().post(format!("http://{addr}{path}")).bearer_auth(token).send().await.unwrap();
    let status = response.status().as_u16();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
}

fn code(body: &Value) -> &str { body["detail"]["code"].as_str().unwrap_or_default() }

#[tokio::test(flavor = "multi_thread")]
async fn a_port_that_is_not_of_the_session_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    // Ninguém escuta nela pela sessão: o pane da sessão não tem processo.
    let other = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = other.local_addr().unwrap().port();
    for path in [format!("/api/sessions/s0/ports/{port}/stop"), format!("/api/sessions/nada/ports/{port}/stop"),
        "/api/sessions/s0/ports/0/stop".into(), "/api/sessions/s0/ports/abc/stop".into()] {
        let (status, body) = stop(srv.addr, &path, OWNER).await;
        assert_eq!((status, code(&body)), (404, "port_stop_not_found"), "{path}");
        assert!(body["detail"]["params"].is_object() && body["detail"]["msg"].is_string(), "{body}");
    }
}

/// O pane da sessão é este processo, que também é o servidor: a porta é dela, e o próprio processo que
/// escuta é protegido, então nem o grupo nem a árvore dele recebem sinal.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn the_server_group_is_refused_with_409() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let script = dir.path().join("tmux");
    let text = std::fs::read_to_string(&script).unwrap();
    let with_pid = text.replace("s0\\t1\\t\\t", &format!("s0\\t1\\t{}\\t", std::process::id()));
    assert_ne!(text, with_pid, "o pid do pane não entrou no roteiro");
    std::fs::write(&script, with_pid).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (status, body) = stop(srv.addr, &format!("/api/sessions/s0/ports/{port}/stop"), OWNER).await;
    assert_eq!((status, code(&body)), (409, "port_stop_shared_group"), "{body}");
    // Nenhum sinal saiu: o listener segue aceitando.
    assert!(tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_is_401() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let (status, _) = stop(srv.addr, "/api/sessions/s0/ports/3000/stop", "errado").await;
    assert_eq!(status, 401);
}

/// Toda sessão é viva, com a chave `L-<nome>`.
struct Alive;

impl LifeProbe for Alive {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { None }) }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
    fn sidecar(&self, name: &str) -> Option<(String, bool)> { Some((format!("L-{name}"), false)) }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

/// A 8766 não tem a rota: o convite da própria sessão não chega ao handler.
#[tokio::test(flavor = "multi_thread")]
async fn guest_port_has_no_stop() {
    let dir = tempfile::tempdir().unwrap();
    let facts = Arc::new(LifeFacts::default());
    facts.apply_transfer_snapshot(HashMap::new(), HashSet::new());
    let share = Arc::new(ShareState {
        store: ShareStore::open(dir.path().join("shares.json"), Arc::new(|| 100.0)),
        pairs: PairStore::open(dir.path().join("external_pairs.json")),
        lives: Lives::new(Arc::new(Alive), facts),
        tailscale: Arc::new(Real),
        tunnel: Default::default(),
        connect: None,
    });
    let invite = share.store.create_redeemed("s0", Some("k:L-s0".into()), Kind::Share).unwrap().1;
    let (python, _) = spawn_fake().await;
    let mut state = AppState::new(config("127.0.0.1"));
    state.share = Some(share);
    state.side.locate = python.locate();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let guest = listener.local_addr().unwrap();
    let app = guest_router(Arc::new(state)).into_make_service_with_connect_info::<GuestConn>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (status, body) = stop(guest, "/api/sessions/s0/ports/3000/stop", &invite).await;
    assert!(matches!(status, 403 | 404), "{status} {body}");
    assert!(!code(&body).starts_with("port_stop"), "{body}");
}
