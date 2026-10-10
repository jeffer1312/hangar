use crate::fake;
use crate::mods_support;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::mods::bridge::mint;
use hangar_server::mods::state::Mods;
use hangar_server::routes::AppState;
use mods_support::Probe;
use serde_json::{Value, json};
use tokio::net::TcpListener;

struct Server {
    public: SocketAddr,
    bridge: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Drop for Server {
    fn drop(&mut self) { self.task.abort(); }
}

impl Server {
    /// Sobe e devolve None quando a ponte (a mesma porta em 127.0.0.1) já é de outro processo.
    async fn try_start(listener: TcpListener, state: AppState) -> Option<Self> {
        let public = listener.local_addr().unwrap();
        let bridge = SocketAddr::from((Ipv4Addr::LOCALHOST, public.port()));
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let mut task = tokio::spawn(hangar_server::serve_until_with_state(listener, state, async { let _ = stopping.await; }));
        let http = http();
        let up = match tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if task.is_finished() {
                    return false;
                }
                if let Ok(response) = http.get(format!("http://{public}/__hangar_server/health")).send().await {
                    assert_eq!(response.status().as_u16(), 200);
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await {
            Ok(up) => up,
            Err(_) => {
                let _ = stop.send(());
                task.abort();
                panic!("a saúde pública não respondeu em 5 s");
            }
        };
        if !up {
            let error = (&mut task).await.unwrap().expect_err("o servidor saiu sem erro antes da saúde");
            assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error}");
            return None;
        }
        Some(Self { public, bridge, stop: Some(stop), task })
    }


    /// Porta pública nova a cada tentativa: em paralelo, outro processo pode estar na porta da ponte.
    async fn launch(mut state: impl FnMut() -> AppState) -> Self {
        for _ in 0..5 {
            let listener = TcpListener::bind("127.0.0.2:0").await.unwrap();
            if let Some(server) = Self::try_start(listener, state()).await {
                return server;
            }
        }
        panic!("nenhuma porta livre para a ponte");
    }

    async fn stop(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), &mut self.task).await.unwrap().unwrap().is_ok());
        crate::assert_closed(self.public, "a porta pública").await;
        crate::assert_closed(self.bridge, "a ponte").await;
    }
}

fn http() -> reqwest::Client { reqwest::Client::builder().no_proxy().build().unwrap() }

async fn setup() -> (Arc<Fake>, Server, Mods) {
    let (python, upstream) = spawn_fake().await;
    let mut mods = None;
    let server = Server::launch(|| {
        let state = AppState::new(config(upstream, "127.0.0.1"));
        state.mods.attach_terminal("t", "proc-t", 1, Arc::new(Probe::default()));
        mods = Some(state.mods.clone());
        state
    }).await;
    (python, server, mods.unwrap())
}

async fn post(server: SocketAddr, route: &str, body: Value) -> reqwest::Response {
    http().post(format!("http://{server}/api/plugin/{route}"))
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap()
}

#[tokio::test]
async fn specific_bind_mirrors_terminal_ui_through_the_announced_loopback() {
    let (_python, mut server, mods) = setup().await;
    let response = post(server.bridge, "ui", json!({"sessao":"t", "token":mint(OWNER, "t"),
        "above":{"type":"Text", "children":["ponte"]}, "columns":80, "panes":[]})).await;
    assert_eq!(response.status().as_u16(), 200);
    let (_, data) = mods.replay("t").into_iter().find(|(event, _)| *event == "plugin_ui").unwrap();
    let data: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(data["source"], "terminal");
    assert_eq!(data["above"]["children"][0], "ponte");
    server.stop().await;
}

#[tokio::test]
async fn click_url_uses_the_same_attempt_on_public_and_loopback_entries() {
    let (_python, mut server, mods) = setup().await;
    let attempt = mods.begin_click("t", "band", "test", "link");
    let start = post(server.bridge, "press-start", json!({"sessao":"t", "token":mint(OWNER,"t"),
        "requestId":"band", "plugin":"test", "element":"link"})).await;
    assert_eq!(start.status().as_u16(), 200);
    let started: Value = serde_json::from_str(&start.text().await.unwrap()).unwrap();
    assert_eq!(started["fromApp"], true);
    assert_eq!(started["attempt"], attempt);
    let response = post(server.bridge, "opened", json!({"sessao":"t", "token":mint(OWNER,"t"),
        "attempt":attempt, "url":"https://example.com/proof"})).await;
    assert_eq!(response.status().as_u16(), 200);
    let repeated = post(server.public, "press-start", json!({"sessao":"t", "token":mint(OWNER,"t"),
        "requestId":"band", "plugin":"test", "element":"link"})).await;
    let repeated: Value = serde_json::from_str(&repeated.text().await.unwrap()).unwrap();
    assert_eq!(repeated["fromApp"], false);
    assert_eq!(mods.finish_click("t", &attempt, Duration::ZERO).await, (None, Some("https://example.com/proof".into())));
    server.stop().await;
}

#[tokio::test]
async fn bridge_never_promotes_owner_guest_or_internal_credentials_to_plugin_identity() {
    let (python, mut server, mods) = setup().await;
    for extra_header in [("authorization", format!("Bearer {OWNER}")), ("authorization", "Bearer guest".into()),
        ("x-hangar-internal", SECRET.into())] {
        let response = http().post(format!("http://{}/api/plugin/ui", server.bridge))
            .header(extra_header.0, extra_header.1).header("content-type", "application/json")
            .body(json!({"sessao":"t", "token":"wrong", "above":null, "panes":[]}).to_string()).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 403);
    }
    let missing = post(server.bridge, "ui", json!({"sessao":"t", "above":null, "panes":[]})).await;
    assert!(!missing.status().is_success());
    assert!(mods.replay("t").is_empty());
    assert_eq!(python.hits_to("/api/plugin/ui"), 0);
    server.stop().await;
}

async fn echo(request: axum::extract::Request) -> impl axum::response::IntoResponse {
    let path = request.uri().path_and_query().unwrap().as_str().to_owned();
    let method = request.method().to_string();
    let client_ip = request.headers().get("x-forwarded-for").map(|value| value.to_str().unwrap().to_owned());
    let internal = request.headers().contains_key("x-hangar-internal");
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    ([("content-type", "application/json")], json!({"path":path, "method":method, "client_ip":client_ip, "internal":internal, "body":String::from_utf8(body.to_vec()).unwrap()}).to_string())
}

#[tokio::test]
async fn legacy_bridge_methods_keep_the_path_body_and_loopback_origin() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async { axum::serve(upstream, axum::Router::new().fallback(echo)).await });
    let mut server = Server::launch(|| AppState::new(config(address, "127.0.0.1"))).await;
    for endpoint in ["whoami", "pull", "suggest", "ask", "ask-fim", "filled", "submitted", "state", "rate"] {
        let body = json!({"probe":endpoint}).to_string();
        let response = http().post(format!("http://{}/api/plugin/{endpoint}?check=1", server.bridge))
            .header("x-hangar-internal", SECRET).body(body.clone()).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let received: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        assert_eq!(received, json!({"path":format!("/api/plugin/{endpoint}?check=1"), "method":"POST", "client_ip":"127.0.0.1", "internal":false, "body":body}));
    }
    let response = http().request(reqwest::Method::OPTIONS, format!("http://{}/api/plugin/ui", server.bridge)).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let received: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(received["path"], "/api/plugin/ui");
    assert_eq!(received["method"], "OPTIONS");
    server.stop().await;
    upstream_task.abort();
}

#[tokio::test]
async fn loopback_does_not_publish_the_rest_of_the_public_or_private_api() {
    let (python, mut server, _mods) = setup().await;
    for path in ["/api/sessions", "/internal/list/facts", "/__hangar_server/health", "/__hangar_server/terminal",
        "/api/sessions/t/plugin/press", "/api/plugin-extra/whoami", "/api/plugin/%2e%2e/sessions", "/api/plugin/missing"] {
        let before = python.hits_to(path);
        let response = http().post(format!("http://{}{path}", server.bridge)).header("authorization", format!("Bearer {OWNER}"))
            .header("x-hangar-internal", SECRET).body("{}").send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
        assert_eq!(python.hits_to(path), before, "{path}");
    }
    server.stop().await;
}

#[tokio::test]
async fn occupied_bridge_fails_before_public_health_and_releases_our_sockets() {
    let (_python, upstream) = spawn_fake().await;
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = occupied.local_addr().unwrap();
    let public = SocketAddr::from((Ipv4Addr::new(127,0,0,2), address.port()));
    let listener = TcpListener::bind(public).await.unwrap();
    let result = hangar_server::routes::serve(listener, config(upstream, "127.0.0.1")).await;
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
    crate::assert_closed(public, "a porta pública").await;
    assert!(tokio::net::TcpStream::connect(address).await.is_ok());
}

/// Sobe de novo nas portas que o servidor parado soltou. Um filho de outro teste, entre o fork e o exec,
/// pode carregar por instantes a cópia dos sockets antigos; o exec os solta, e o prazo cobre isso.
async fn restart(address: SocketAddr, upstream: SocketAddr) -> Server {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(listener) = TcpListener::bind(address).await
            && let Some(server) = Server::try_start(listener, AppState::new(config(upstream, "127.0.0.1"))).await
        {
            return server;
        }
        assert!(std::time::Instant::now() < deadline, "o servidor não voltou nas mesmas portas");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn stopping_allows_a_restart_on_the_same_public_and_bridge_port() {
    let (_python, upstream) = spawn_fake().await;
    // A primeira partida sorteia portas livres (a da ponte pode ser de outro teste); a segunda volta nelas.
    let mut server = Server::launch(|| AppState::new(config(upstream, "127.0.0.1"))).await;
    let address = server.public;
    for again in [false, true] {
        if again {
            server = restart(address, upstream).await;
        }
        let response = post(server.bridge, "state", json!({})).await;
        assert_eq!(response.status().as_u16(), 200);
        server.stop().await;
    }
}

#[tokio::test]
async fn start_reports_a_bridge_port_taken_by_someone_else() {
    let (_python, upstream) = spawn_fake().await;
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 2), occupied.local_addr().unwrap().port())).await.unwrap();
    let started = tokio::time::Instant::now();
    assert!(Server::try_start(listener, AppState::new(config(upstream, "127.0.0.1"))).await.is_none());
    assert!(started.elapsed() < Duration::from_secs(2), "esperou a saúde de um servidor que não subiu");
}
