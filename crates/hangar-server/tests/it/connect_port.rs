//! Entrada do Connect pelo listener de verdade, contra o Python falso.
use crate::fake;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake::*;
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::config::connect_listen;
use hangar_server::login_guests::{Service, Settings};
use hangar_server::routes::{AppState, front_address, serve_connect};

pub(crate) const GUEST: &str = "convidado-token";

pub(crate) fn free_port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
}

pub(crate) async fn wait_up(addr: SocketAddr) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while client().get(format!("http://{addr}/__hangar_server/health")).send().await.is_err() {
        assert!(tokio::time::Instant::now() < deadline, "a entrada {addr} não abriu");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Pela entrada só passa aparelho: a pasta fica de pé até o fim do processo, junto com a loja.
pub(crate) fn device(state: &mut AppState) -> String {
    let dir: &'static tempfile::TempDir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let store = hangar_server::connect::devices::Store::new(dir.path().to_path_buf(), None);
    let token = store.enroll("teste").unwrap().1;
    state.connect_devices = Arc::new(store);
    token
}

async fn start(addr: SocketAddr, trusted: &str) -> (Arc<Fake>, SocketAddr, String) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, trusted));
    state.connect = Some(addr);
    let token = device(&mut state);
    (python, spawn_state(state).await, token)
}

/// Python falso, porta principal, entrada do Connect já aberta, servidas pelo `select!` do `serve`, e uma credencial de aparelho.
async fn server(trusted: &str) -> (Arc<Fake>, SocketAddr, SocketAddr, String) {
    let connect = free_port();
    let (python, main, token) = start(connect, trusted).await;
    wait_up(connect).await;
    (python, main, connect, token)
}

pub(crate) fn get(addr: SocketAddr, path: &str) -> reqwest::RequestBuilder {
    client().get(format!("http://{addr}{path}"))
}

fn mcp(addr: SocketAddr, token: &str) -> reqwest::RequestBuilder {
    client().post(format!("http://{addr}/mcp")).bearer_auth(token).header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream").body("{}")
}

#[tokio::test]
async fn local_only_route_is_refused_through_connect() {
    let (_python, main, connect, device) = server("127.0.0.1").await;
    let path = "/api/desktop/palette";
    assert_eq!(get(main, path).bearer_auth(OWNER).send().await.unwrap().status(), 200, "na máquina a rota responde");
    let refused = get(connect, path).bearer_auth(&device).send().await.unwrap();
    assert_eq!(refused.status(), 403);
    assert!(refused.text().await.unwrap().contains("erro_so_loopback"));
}

#[tokio::test]
async fn forged_forwarded_for_does_not_become_local() {
    // `*` é o pior caso: com ele o vizinho de qualquer endereço reescreve o cliente.
    for trusted in ["127.0.0.1", "*"] {
        let (python, _main, connect, device) = server(trusted).await;
        let forged = get(connect, "/api/desktop/palette").bearer_auth(&device).header("x-forwarded-for", "127.0.0.1");
        assert_eq!(forged.send().await.unwrap().status(), 403, "{trusted}");
        assert_eq!(python.last_hit().1["x-forwarded-for"], hangar_server::devices::CONNECT_PEER, "{trusted}");
        assert_eq!(mcp(connect, &device).header("x-forwarded-for", "127.0.0.1").send().await.unwrap().status(), 403, "{trusted}");
    }
}

#[tokio::test]
async fn connect_requests_count_as_https() {
    let (_python, main, connect, device) = server("127.0.0.1").await;
    let with_cookie = |addr, cookie: &str| get(addr, "/api/client-api").header("cookie", cookie.to_owned());
    let host = format!("__Host-cp_token={device}");
    let plain = format!("cp_token={device}");
    assert_eq!(with_cookie(connect, &host).send().await.unwrap().status(), 200);
    // O esquema sai da porta: forjar `http` não devolve o cookie sem prefixo.
    let forged = with_cookie(connect, &plain).header("x-forwarded-proto", "http");
    assert_eq!(forged.send().await.unwrap().status(), 401, "em https o cookie sem prefixo não vale");
    assert_eq!(with_cookie(main, &plain).send().await.unwrap().status(), 200, "na porta principal é http");
}

#[tokio::test]
async fn python_receives_connect_peer_on_pass() {
    let (python, main, connect, device) = server("127.0.0.1").await;
    let forged = get(connect, "/api/qualquer").bearer_auth(&device).header("x-forwarded-for", "198.51.100.7")
        .header("x-forwarded-proto", "http").header("x-forwarded-host", "evil.example");
    assert_eq!(forged.send().await.unwrap().status(), 200);
    let (_, headers) = python.last_hit();
    assert_eq!(headers["x-forwarded-for"], "192.0.2.1");
    assert_eq!(headers["x-forwarded-proto"], "https");
    assert!(!headers.contains_key("x-forwarded-host"));
    // A porta principal mantém o cliente real.
    assert_eq!(get(main, "/api/qualquer").send().await.unwrap().status(), 200);
    let (_, headers) = python.last_hit();
    assert_eq!(headers["x-forwarded-for"], "127.0.0.1");
    assert_eq!(headers["x-forwarded-proto"], "http");
}

#[tokio::test]
async fn mcp_is_refused_through_connect() {
    let (_python, main, connect, device) = server("127.0.0.1").await;
    assert_ne!(mcp(main, OWNER).send().await.unwrap().status(), 403, "na máquina o MCP atende");
    let refused = mcp(connect, &device).send().await.unwrap();
    assert_eq!(refused.status(), 403);
    assert!(refused.text().await.unwrap().contains("so na maquina do backend"));
}

/// A camada apaga o `X-Forwarded-For` que esta guarda lê; quem recusa aqui é a entrada, não o cabeçalho.
#[tokio::test]
async fn device_install_is_refused_through_connect() {
    let (python, main, connect, device) = server("127.0.0.1").await;
    let path = "/api/device-install/progress";
    assert_eq!(get(main, path).bearer_auth(OWNER).send().await.unwrap().status(), 404, "na porta principal passa da guarda");
    let plain = format!("cp_token={device}");
    let host = format!("__Host-cp_token={device}");
    for request in [
        get(connect, path).bearer_auth(&device),
        get(connect, path).header("cookie", host),
        client().post(format!("http://{connect}/api/device-install/start")).bearer_auth(&device).body(r#"{"password":"segredo"}"#),
    ] {
        let refused = request.send().await.unwrap();
        assert_eq!(refused.status(), 403);
        let body = refused.text().await.unwrap();
        assert!(body.contains("device_install_forbidden") && !body.contains("segredo"), "{body}");
    }
    assert_eq!(get(connect, path).header("cookie", plain).send().await.unwrap().status(), 401, "em https o cookie sem prefixo não vale");
    assert_eq!(python.hits_to(path), 0);
}

/// Entrada do Connect com um convidado de login (`GUEST`) que vê as sessões sem dono.
pub(crate) async fn guest_server() -> (tempfile::TempDir, Arc<Fake>, SocketAddr) {
    let home = crate::physical_tempdir();
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    let root = home.path().to_str().unwrap();
    state.accounts = AccountService::new(AccountEnvironment::from_pairs([("HOME", root), ("USERPROFILE", root)]));
    let service = Service::new(Settings { guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
        runtime_config: home.path().join("config.json"), sync_enabled: false, bootstrap: String::new(),
        secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into() });
    service.store.create("Ana", root, true, true, "ana".into(), GUEST.into()).unwrap();
    state.share = None;
    state.login_guests = Arc::new(service);
    let connect = free_port();
    state.connect = Some(connect);
    spawn_state(state).await;
    wait_up(connect).await;
    (home, python, connect)
}

/// O convidado vai como loopback, que não conta tentativa: acertar a senha do dono não pode mudar a resposta.
#[tokio::test]
async fn login_guest_cannot_probe_the_owner_password_through_the_terminal() {
    let (_home, _python, connect) = guest_server().await;
    let attempt = |token: &str| get(connect, &format!("/api/sessions/conexao-teste/term?token={token}")).bearer_auth(GUEST)
        .header("origin", "https://evil.example").header("connection", "upgrade").header("upgrade", "websocket")
        .header("sec-websocket-version", "13").header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    let wrong = attempt("errado").send().await.unwrap();
    let right = attempt(OWNER).send().await.unwrap();
    assert_eq!(wrong.status(), 403);
    assert_eq!(right.status(), 403);
    let (wrong, right) = (wrong.text().await.unwrap(), right.text().await.unwrap());
    assert_eq!(wrong, "origem não autorizada");
    assert_eq!(right, wrong);
}

#[tokio::test]
async fn login_guest_reaches_python_as_https() {
    let (_home, python, connect) = guest_server().await;
    let passed = get(connect, "/api/sessions/conexao-teste/term").bearer_auth(GUEST);
    assert_eq!(passed.send().await.unwrap().status(), 200);
    let (path, headers) = python.last_hit();
    assert!(path.ends_with("/term"), "{path}");
    assert_eq!(headers["x-forwarded-proto"], "https");
}

#[test]
fn front_port_never_takes_the_connect_port() {
    let home = tempfile::tempdir().unwrap();
    let hangar = home.path().join(".hangar");
    std::fs::create_dir_all(hangar.join("config")).unwrap();
    std::fs::create_dir(hangar.join("current")).unwrap();
    let listen = "127.0.0.1:8765".parse().unwrap();
    std::fs::write(hangar.join("config/.env"), "CP_FRONT_PORT=5173\n").unwrap();
    assert_eq!(front_address(home.path(), listen), Some("127.0.0.1:5173".parse().unwrap()));
    std::fs::write(hangar.join("config/.env"), "CP_FRONT_PORT=8768\n").unwrap();
    assert_eq!(front_address(home.path(), listen), None);
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[tokio::test]
async fn busy_connect_port_keeps_the_owner_listeners_up() {
    let log = Log::default();
    let writer = log.clone();
    // O teste roda numa thread só: as tarefas do servidor veem este assinante.
    let _log = tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(move || writer.clone()).finish());
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let connect = busy.local_addr().unwrap();
    let (_python, main, device) = start(connect, "127.0.0.1").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&log.0.lock().unwrap()).contains("connect_port_failed") {
        assert!(tokio::time::Instant::now() < deadline, "a falha da porta não foi registrada");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(get(main, "/api/client-api").bearer_auth(OWNER).send().await.unwrap().status(), 200);
    drop(busy);
    wait_up(connect).await;
    assert_eq!(get(connect, "/api/desktop/palette").bearer_auth(&device).send().await.unwrap().status(), 403, "a porta solta abre já como Connect");
    assert_eq!(get(main, "/api/client-api").bearer_auth(OWNER).send().await.unwrap().status(), 200);
}

#[tokio::test]
async fn connect_entrance_opens_only_on_loopback() {
    let (_python, upstream) = spawn_fake().await;
    let state = Arc::new(AppState::new(config(upstream, "127.0.0.1")));
    let refused = serve_connect("0.0.0.0:0".parse().unwrap(), state, None).await.unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(connect_listen("0.0.0.0:8765".parse().unwrap()), Some("127.0.0.1:8768".parse().unwrap()));
}

#[test]
fn main_port_on_8768_leaves_connect_off() {
    assert_eq!(connect_listen("0.0.0.0:8768".parse().unwrap()), None);
}
