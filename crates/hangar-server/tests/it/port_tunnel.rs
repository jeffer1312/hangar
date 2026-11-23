//! Túnel de porta (`GET /api/port-tunnel`) pelo listener de verdade: eco, recusas antes do aperto de
//! mão, a porta do convidado sem túnel e o aparelho revogado que derruba o túnel aberto.
use crate::connect_port::{free_port, wait_up};
use crate::fake::{Fake, OWNER, client, config, spawn_fake, spawn_state_for};

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use hangar_server::connect::devices::Store;
use hangar_server::routes::{AppState, guest_router};
use hangar_server::share::ShareState;
use hangar_server::share::gate::GuestConn;
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::Real;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

/// Eco TCP numa porta efêmera; o contador diz quantas conexões chegaram até ele.
async fn echo() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    (port, accepted)
}

/// Porta em que nada escuta: aberta e largada na hora.
fn closed_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
}

async fn owner_server() -> (Arc<Fake>, SocketAddr) {
    let (python, _) = spawn_fake().await;
    let main = spawn_state_for(&python, AppState::new(config("127.0.0.1"))).await;
    (python, main)
}

struct Connect { _python: Arc<Fake>, _dir: tempfile::TempDir, main: SocketAddr, connect: SocketAddr, store: Arc<Store>, id: String, token: String }

/// Porta principal e entrada do Connect com um aparelho aprovado.
async fn connect_server() -> Connect {
    let (python, _) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(dir.path().to_path_buf(), None));
    let (id, token) = store.enroll("teste").unwrap();
    let mut state = AppState::new(config("127.0.0.1"));
    state.connect_devices = store.clone();
    let connect = free_port();
    state.connect = Some(connect);
    let main = spawn_state_for(&python, state).await;
    wait_up(connect).await;
    Connect { _python: python, _dir: dir, main, connect, store, id, token }
}

/// Aperto de mão de WebSocket pelo reqwest, para ler a recusa antes do upgrade.
fn handshake(addr: SocketAddr, path_and_query: &str) -> reqwest::RequestBuilder {
    client().get(format!("http://{addr}{path_and_query}"))
        .header("connection", "upgrade").header("upgrade", "websocket")
        .header("sec-websocket-version", "13").header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
}

/// Código da recusa no corpo, conferido igual no cabeçalho `x-hangar-tunnel`.
async fn code_of(response: reqwest::Response) -> String {
    let header = response.headers().get("x-hangar-tunnel").map(|value| value.to_str().unwrap().to_owned());
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    let code = body["detail"]["code"].as_str().unwrap_or_default().to_owned();
    assert_eq!(header.as_deref(), Some(code.as_str()), "cabeçalho x-hangar-tunnel");
    code
}

/// Próxima mensagem de dados; `None` quando a conexão acabou.
async fn next(ws: &mut Ws, within: Duration) -> Option<Message> {
    loop {
        match tokio::time::timeout(within, ws.next()).await.expect("prazo") {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(message)) => return Some(message),
            _ => return None,
        }
    }
}

async fn round_trip(ws: &mut Ws, bytes: &[u8]) {
    ws.send(Message::Binary(bytes.to_vec().into())).await.unwrap();
    assert_eq!(next(ws, Duration::from_secs(5)).await, Some(Message::Binary(bytes.to_vec().into())));
}

#[tokio::test]
async fn owner_token_echoes_through_the_tunnel() {
    let (_python, main) = owner_server().await;
    let (port, _) = echo().await;
    let (mut by_query, _) = connect_async(format!("ws://{main}/api/port-tunnel?port={port}&token={OWNER}")).await.unwrap();
    round_trip(&mut by_query, b"\x00\x00\x00\x08\x04\xd2\x16\x2f").await;
    let mut request = format!("ws://{main}/api/port-tunnel?port={port}").into_client_request().unwrap();
    request.headers_mut().insert("authorization", format!("Bearer {OWNER}").parse().unwrap());
    let (mut by_bearer, _) = connect_async(request).await.unwrap();
    round_trip(&mut by_bearer, b"SELECT 1;").await;
}

#[tokio::test]
async fn out_of_range_and_hangar_ports_are_refused_with_422() {
    let (_python, main) = owner_server().await;
    // A porta deste servidor é reservada na subida; 8765 e 8766 são do Hangar mesmo sem ninguém nelas.
    for port in ["0".to_owned(), "65536".to_owned(), "abc".to_owned(), main.port().to_string(), "8765".into(), "8766".into()] {
        let response = handshake(main, &format!("/api/port-tunnel?port={port}&token={OWNER}")).send().await.unwrap();
        assert_eq!(response.status(), 422, "porta {port}");
        assert_eq!(code_of(response).await, "port_tunnel_port_refused", "porta {port}");
    }
    let missing = handshake(main, &format!("/api/port-tunnel?token={OWNER}")).send().await.unwrap();
    assert_eq!(missing.status(), 422);
    assert_eq!(code_of(missing).await, "port_tunnel_port_refused");
}

#[tokio::test]
async fn wrong_token_is_refused_with_401_and_the_header() {
    let (_python, main) = owner_server().await;
    let response = handshake(main, &format!("/api/port-tunnel?port={}&token=errado", closed_port())).send().await.unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(code_of(response).await, "erro_nao_autorizado");
}

#[tokio::test]
async fn nothing_listening_is_refused_with_502() {
    let (_python, main) = owner_server().await;
    let response = handshake(main, &format!("/api/port-tunnel?port={}&token={OWNER}", closed_port())).send().await.unwrap();
    assert_eq!(response.status(), 502);
    assert_eq!(code_of(response).await, "port_tunnel_refused");
}

/// A página de outro site do Connect manda o cookie sozinho; só ele não abre o túnel.
#[tokio::test]
async fn login_cookie_alone_is_refused_with_403() {
    let e = connect_server().await;
    let (port, accepted) = echo().await;
    let path = format!("/api/port-tunnel?port={port}");
    let on_main = handshake(e.main, &path).header("cookie", format!("cp_token={OWNER}")).send().await.unwrap();
    assert_eq!(on_main.status(), 403);
    assert_eq!(code_of(on_main).await, "port_tunnel_token_required");
    let on_connect = handshake(e.connect, &path).header("cookie", format!("__Host-cp_token={}", e.token)).send().await.unwrap();
    assert_eq!(on_connect.status(), 403);
    assert_eq!(code_of(on_connect).await, "port_tunnel_token_required");
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "a recusa veio antes de conectar");
}

#[tokio::test]
async fn device_token_opens_the_tunnel_and_revoking_it_closes_the_tunnel() {
    let e = connect_server().await;
    let (port, _) = echo().await;
    let (mut ws, _) = connect_async(format!("ws://{}/api/port-tunnel?port={port}&token={}", e.connect, e.token)).await.unwrap();
    round_trip(&mut ws, b"antes").await;
    e.store.remove(&e.id).unwrap();
    // O vigia do aparelho confere a cada 5 s.
    assert_eq!(next(&mut ws, Duration::from_secs(12)).await, None, "o túnel ficou aberto depois de revogado");
}

/// Toda sessão é viva, com a chave `L-<nome>`.
struct Alive;

impl LifeProbe for Alive {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { None }) }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
    fn sidecar(&self, name: &str) -> Option<(String, bool)> { Some((format!("L-{name}"), false)) }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

/// A 8766 não tem a rota: o porteiro recusa o caminho fora do convite, e nada chega à porta.
#[tokio::test]
async fn guest_port_has_no_tunnel() {
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
    let (port, accepted) = echo().await;
    let response = handshake(guest, &format!("/api/port-tunnel?port={port}")).bearer_auth(&invite).send().await.unwrap();
    assert_eq!(response.status(), 403);
    let with_query = handshake(guest, &format!("/api/port-tunnel?port={port}&token={invite}")).send().await.unwrap();
    assert_eq!(with_query.status(), 403);
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}
