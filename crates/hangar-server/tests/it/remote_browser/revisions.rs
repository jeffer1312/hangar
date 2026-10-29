#![cfg(unix)]
use super::support::{self, OWNER, SECRET, Scene, Server};
use super::transport::{Desktop, KEY, barrier, connect, seed_session, until};
use axum::{Router, routing::get, http::{HeaderMap, HeaderValue}};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{sync::{Arc, Mutex}, time::Duration};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

async fn isolated(name: &str) -> bool {
    support::isolated(&format!("remote_browser::revisions::{name}"), |_| {}).await
}
fn login_guest(scene: &Scene) {
    let token_hash: String = ring::digest::digest(&ring::digest::SHA256, b"login-guest").as_ref()
        .iter().map(|byte| format!("{byte:02x}")).collect();
    std::fs::write(scene.home.join(".claude/guests.json"), json!({"guests":[{
        "id":"browser-login-guest", "name":"Convidado", "token_hash":token_hash,
        "root":scene.home.join("guest-root"), "sees_owner":true, "owner_sees":true,
    }],"sessions":{}}).to_string()).unwrap();
}
#[tokio::test]
async fn public_login_guest_is_refused_before_owner_failures() {
    if isolated("public_login_guest_is_refused_before_owner_failures").await { return; }
    let scene = Scene::new().await;
    login_guest(&scene);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let corpus = support::contract("url-commands");
    let expected = &corpus["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == "get_login_guest_is_blocked").unwrap()["expected"]["responses"][0]["body"];
    let ip = "198.51.100.230";
    for _ in 0..8 {
        let response = client.get(format!("http://{}/api/sessions/missing/navegador?token=login-guest", scene.owner.address))
            .header("x-forwarded-for", ip).send().await.unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(response.json::<Value>().await.unwrap(), *expected);
        let mut request = format!("ws://{}/api/sessions/missing/nav-remoto?token=login-guest", scene.owner.address)
            .into_client_request().unwrap();
        request.headers_mut().insert("x-forwarded-for", ip.parse().unwrap());
        let denied = tokio_tungstenite::connect_async(request).await;
        assert!(matches!(denied, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status()==403));
        assert!(!scene.state.auth.is_blocked_at(ip, std::time::Instant::now()), "login válido virou palpite errado");
    }
    let owner = client.get(format!("http://{}/api/sessions/missing/navegador?token={OWNER}", scene.owner.address))
        .header("x-forwarded-for", ip).send().await.unwrap();
    assert_eq!(owner.status(), 200);
    assert!(!scene.root.path().join("calls.log").exists());
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene,KEY);
    for (header, value) in [("authorization",format!("Bearer {OWNER}")),("cookie",format!("cp_token={OWNER}"))] {
        let mut request = format!("ws://{}/api/sessions/nav-contract/nav-remoto",scene.owner.address).into_client_request().unwrap();
        request.headers_mut().insert(axum::http::HeaderName::from_bytes(header.as_bytes()).unwrap(),value.parse().unwrap());
        let denied = tokio_tungstenite::connect_async(request).await;
        assert!(matches!(denied,Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status()==403),"o WS do dono continua exigindo query token");
    }
    let (mut valid, _) = connect(&scene, "").await.unwrap();
    let mut events = Vec::new();
    until(&mut valid,&mut events,|event|event["url"]=="http://contract.test/ready").await;
    valid.close(None).await.unwrap();
    while let Some(message) = valid.next().await { message.unwrap(); }
    desktop.wait_closed(1).await;
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}
#[tokio::test]
async fn login_guest_precedence_matches_the_original_gate() {
    if isolated("login_guest_precedence_matches_the_original_gate").await { return; }
    let scene = Scene::new().await;
    login_guest(&scene);
    let reference = support::contract("final-review");
    use tower::ServiceExt;
    for case in reference["observed"]["guest_tokens"].as_array().unwrap() {
        let query = case["query"].as_str().unwrap().replace("owner", OWNER);
        let mut headers = HeaderMap::new();
        for pair in case["headers"].as_array().unwrap() {
            let name = axum::http::HeaderName::from_bytes(pair[0].as_str().unwrap().as_bytes()).unwrap();
            headers.append(name, HeaderValue::from_str(&pair[1].as_str().unwrap().replace("owner", OWNER)).unwrap());
        }
        if case["https"] == true { headers.insert("x-forwarded-proto", HeaderValue::from_static("https")); }
        // O roteador público recebe os bytes do cabeçalho como o scope ASGI capturado.
        let mut request = axum::http::Request::builder().uri(format!("/api/sessions/missing/navegador?{query}"))
            .body(axum::body::Body::empty()).unwrap();
        *request.headers_mut() = headers;
        request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap()));
        let response = hangar_server::routes::router(scene.state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status()==403, case["token"]=="login-guest", "{}", case["name"]);
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}
#[tokio::test]
async fn absent_session_life_preserves_the_original_between_viewers() {
    if isolated("absent_session_life_preserves_the_original_between_viewers").await { return; }
    let scene = Scene::new().await;
    seed_session(&scene);
    let source = scene.home.join(".hangar/claude-headless/nav-contract.json");
    let mut sidecar: Value = serde_json::from_slice(&std::fs::read(&source).unwrap()).unwrap();
    sidecar.as_object_mut().unwrap().remove("key");
    std::fs::write(source, sidecar.to_string()).unwrap();
    scene.state.list.invalidate();
    let (row, _) = scene.state.list.discovered_session("nav-contract").await.unwrap().unwrap();
    assert_eq!(row.lifecycle_id, None, "o caso precisa de uma vida não demonstrada");
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let reference = support::contract("transport");
    let case = reference["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == "old_viewer_leaves_after_same_size_new_viewer").unwrap();
    let mut clients = Vec::new();
    for _ in 0..2 {
        let (mut client, _) = connect(&scene, "").await.unwrap();
        let mut events = Vec::new();
        until(&mut client, &mut events, |event| event["url"] == "http://contract.test/ready").await;
        client.send(Message::Text(json!({"t":"layout","w":900,"h":700}).to_string().into())).await.unwrap();
        barrier(&mut client, &mut events).await;
        clients.push(client);
    }
    for mut client in clients {
        client.close(None).await.unwrap();
        while let Some(message) = client.next().await { message.unwrap(); }
    }
    desktop.wait_closed(2).await;
    desktop.wait_requests(case["expected"]["cmd"].as_array().unwrap().len()).await;
    assert_eq!(desktop.requests(), *case["expected"]["cmd"].as_array().unwrap());
    assert_eq!(desktop.current_layout(), "mobile");
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[derive(Clone)]
struct Logs(Arc<Mutex<Vec<Value>>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Logs {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "hangar.navsock" { return; }
        struct Visitor(String);
        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name()=="message" { self.0 = format!("{value:?}"); }
            }
        }
        let mut visitor = Visitor(String::new()); event.record(&mut visitor);
        self.0.lock().unwrap().push(json!({"level":event.metadata().level().as_str(), "message":visitor.0}));
    }
}
async fn opening(name: &str) {
    if isolated(name).await { return; }
    use tracing_subscriber::prelude::*;
    let logs = Logs(Arc::default());
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(logs.clone())).unwrap();
    let scene = Scene::new().await;
    seed_session(&scene);
    let reference = support::contract("final-review");
    let case = reference["observed"]["opening"].as_array().unwrap().iter().find(|case|case["name"]==name).unwrap();
    if name=="connection_refused" {
        // macOS descarta o SYN de porta presa sem listen (vira prazo); porta liberada recusa nos dois.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        opening_request(&scene, case, &logs, port).await;
        scene.close().await;
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let status = if name=="http_401" { "401 Unauthorized" } else { "403 Forbidden" };
    let timeout = name=="handshake_timeout";
    let serving = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut data = [0;4096]; socket.read(&mut data).await.unwrap();
        if timeout { let _ = socket.read(&mut data).await; }
        else { socket.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap(); }
    });
    opening_request(&scene, case, &logs, port).await;
    serving.await.unwrap();
    scene.close().await;
}
async fn opening_request(scene: &Scene, case: &Value, logs: &Logs, port: u16) {
    std::fs::write(scene.nav_dir().join("browser.json"),json!({"chave":KEY,"pid":123,"targetId":null}).to_string()).unwrap();
    std::fs::write(scene.nav_dir().join("_srv.json"),json!({"porta":port,"pid":123,"token":"never-print-this-desktop-secret"}).to_string()).unwrap();
    let (mut client, response) = connect(scene, "").await.unwrap();
    assert_eq!(response.status(), 101);
    let mut events = Vec::new();
    let event = tokio::time::timeout(Duration::from_secs(7), client.next()).await.unwrap().unwrap().unwrap();
    let Message::Text(raw) = event else { panic!("abertura não publicou a causa") };
    events.push(serde_json::from_str::<Value>(&raw).unwrap());
    // A captura é do Linux: o número do errno é o da plataforma (ECONNREFUSED é 61 no macOS).
    let expected = case["message"].as_str().unwrap().replace("<PORT>",&port.to_string())
        .replace("[Errno 111]", &format!("[Errno {}]", libc::ECONNREFUSED));
    assert_eq!(events, vec![json!({"t":"erro","m":expected})]);
    while let Some(message) = client.next().await { message.unwrap(); }
    let lines = logs.0.lock().unwrap();
    assert!(lines.iter().any(|line| line["level"]=="WARN" && line["message"].as_str().unwrap().contains(case["type"].as_str().unwrap())), "tipo da causa foi perdido");
    assert!(!serde_json::to_string(&*lines).unwrap().contains("never-print-this-desktop-secret"));
    scene.upstream.assert_no_browser_or_discovery_calls();
}
macro_rules! opening_cases {($($name:ident),* $(,)?)=>{$(#[tokio::test] async fn $name(){opening(stringify!($name)).await;})*};}
opening_cases! {http_401,http_403,connection_refused,handshake_timeout}

fn held_cdp(path: &str, held: &'static str) -> Router {
    Router::new().route(path, get(move |upgrade: axum::extract::ws::WebSocketUpgrade| async move {
        upgrade.on_upgrade(move |mut socket| async move {
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    axum::extract::ws::Message::Text(raw) => {
                        let command: Value = serde_json::from_str(&raw).unwrap();
                        if command["method"] != held {
                            if socket.send(axum::extract::ws::Message::Text(json!({"id":command["id"],"result":{}}).to_string().into())).await.is_err() { break; }
                        }
                    }
                    axum::extract::ws::Message::Close(_) => { let _ = socket.flush().await; break; }
                    axum::extract::ws::Message::Ping(_) => { if socket.flush().await.is_err() { break; } }
                    _ => {}
                }
            }
        })
    }))
}

async fn debug_failure(name: &str, evaluate: bool) {
    if support::isolated_with_timeout(&format!("remote_browser::revisions::{name}"), Duration::from_secs(30), |_| {}).await { return; }
    use tracing_subscriber::prelude::*;
    let lines = Logs(Arc::default());
    let changed = Arc::new(tokio::sync::Notify::new());
    #[derive(Clone)]
    struct Debugs { lines: Logs, changed: Arc<tokio::sync::Notify> }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Debugs {
        fn on_event(&self, event: &tracing::Event<'_>, context: tracing_subscriber::layer::Context<'_, S>) {
            self.lines.on_event(event, context);
            self.changed.notify_one();
        }
    }
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Debugs { lines:lines.clone(), changed:changed.clone() })).unwrap();
    let scene = Scene::new().await;
    seed_session(&scene);
    let peer = if evaluate {
        // O Electron usa esta porta fixa; falha de bind impede tocar outro processo.
        Server::at(held_cdp("/devtools/page/debug-target", "Runtime.evaluate"), "127.0.0.1:9223".parse().unwrap()).await
    } else { Server::owner(held_cdp("/cdp", "Page.captureScreenshot")).await };
    std::fs::write(scene.nav_dir().join("browser.json"),json!({"chave":KEY,"pid":123,"targetId":if evaluate { json!("debug-target") } else { Value::Null },"url":"http://contract.test/fallback"}).to_string()).unwrap();
    std::fs::write(scene.nav_dir().join("_srv.json"),json!({"porta":peer.address.port(),"pid":123,"token":"debug-desktop-secret"}).to_string()).unwrap();
    let (mut client, _) = connect(&scene, "").await.unwrap();
    let reference = support::contract("final-review");
    let expected = reference["observed"]["debug"][if evaluate {1} else {0}]["message"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(24), async {
        loop {
            let ready = changed.notified();
            if lines.0.lock().unwrap().iter().any(|line|line["level"]=="DEBUG" && line["message"]==expected) { break; }
            ready.await;
        }
    }).await.expect("o diagnóstico DEBUG original desapareceu");
    let mut events = Vec::new();
    until(&mut client, &mut events, |event|event["t"]=="url").await;
    assert_eq!(events.last().unwrap()["url"], reference["observed"]["fallback_url"]);
    client.close(None).await.unwrap();
    while let Some(message) = client.next().await {
        if let Message::Text(raw) = message.unwrap() { events.push(serde_json::from_str(&raw).unwrap()); }
    }
    assert!(!events.iter().any(|event|event["t"]=="erro"));
    assert!(!lines.0.lock().unwrap().iter().any(|line|line["level"]=="WARN"));
    scene.upstream.assert_no_browser_or_discovery_calls();
    peer.close().await;
    scene.close().await;
}
#[tokio::test]
async fn screenshot_timeout_keeps_original_debug_without_ui_error() {
    debug_failure("screenshot_timeout_keeps_original_debug_without_ui_error", false).await;
}
#[tokio::test]
async fn evaluate_timeout_keeps_original_debug_and_sidecar_url() {
    debug_failure("evaluate_timeout_keeps_original_debug_and_sidecar_url", true).await;
}

async fn stopped_server(name: &str, entry: &str) {
    if isolated(name).await { return; }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let program = scene.state.list.env().capture_program.clone();
    let isolated_root = std::path::PathBuf::from(std::env::var_os("HANGAR_REMOTE_BROWSER_ROOT").unwrap());
    crate::write_executable(&isolated_root.join("bin/tmux"),format!("#!/bin/sh\nexec '{}' \"$@\"\n",std::path::Path::new(&program).display()));
    let public_reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let public = public_reservation.local_addr().unwrap();
    let guest_reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let guest = guest_reservation.local_addr().unwrap();
    let share_token = scene.guest_token("nav-contract",hangar_server::share::store::Kind::Share).await;
    let log = scene.root.path().join("owned-server.log");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.env_clear().current_dir(&isolated_root).kill_on_drop(true)
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
        "CLAUDE_CONFIG_DIR", "CODEX_HOME", "CP_CLAUDE_CONFIG_DIRS", "HANGAR_LIST_DIRS", "PATH"] {
        command.env(key,std::env::var_os(key).expect("ambiente isolado ausente"));
    }
    command.env("HANGAR_SERVER_LISTEN",public.to_string())
        .env("HANGAR_SERVER_UPSTREAM",scene.upstream.server.address.to_string())
        .env("HANGAR_SERVER_GUEST_LISTEN",guest.to_string())
        .env("HANGAR_SHARES_FILE",scene.home.join("shares.json"))
        .env("HANGAR_INTERNAL_SECRET",SECRET).env("CP_AUTH_TOKEN",OWNER)
        .env("CP_RUST_NO_ORPHAN_SWEEP","1").env("HANGAR_SERVER_LOG",&log);
    drop(public_reservation); drop(guest_reservation);
    let mut serving = command.spawn().unwrap();
    let pid = serving.id().unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let health = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Ok(response) = http.get(format!("http://{public}/__hangar_server/health")).send().await {
                if response.status()==200 {
                    let health = response.json::<Value>().await.unwrap();
                    assert_eq!(health["pid"],pid,"a porta foi ocupada por outro processo; não abrir espectador");
                    if health["guest_port_bound"]==true { break health; }
                }
            }
            assert!(serving.try_wait().unwrap().is_none(),"servidor saiu antes da abertura: {}",std::fs::read_to_string(&log).unwrap_or_default());
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let request = match entry {
        "private" => {
            let private = health["terminal_address"].as_str().expect("porta privada não anunciada");
            let mut request = format!("ws://{private}/__hangar_server/remote-browser?name=nav-contract").into_client_request().unwrap();
            request.headers_mut().insert("x-hangar-internal", SECRET.parse().unwrap());
            request
        }
        "guest" => {
            format!("ws://{guest}/api/sessions/nav-contract/nav-remoto?token={share_token}").into_client_request().unwrap()
        }
        _ => format!("ws://{public}/api/sessions/nav-contract/nav-remoto?token={OWNER}").into_client_request().unwrap(),
    };
    let (mut client, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut events = Vec::new();
    until(&mut client, &mut events, |event|event["url"]=="http://contract.test/ready").await;
    client.send(Message::Text(json!({"t":"layout","w":900,"h":700}).to_string().into())).await.unwrap();
    barrier(&mut client,&mut events).await;
    assert_eq!(desktop.current_layout(),"900x700");
    assert!(events.iter().any(|event|event["t"]=="layout" && event["resposta"]=="layout: 900x700"));
    // O espectador permanece aberto: a queda do cano do pai deve iniciar a devolução.
    drop(serving.stdin.take());
    let output = tokio::time::timeout(Duration::from_secs(5), serving.wait_with_output()).await
        .expect("a parada não respeitou o prazo do Supervisor").unwrap();
    assert!(output.status.success(),"servidor saiu em falha: {}",String::from_utf8_lossy(&output.stderr));
    assert_eq!(desktop.current_layout(),"mobile");
    let reference = support::contract("shutdown-reference");
    let expected = &reference["observed"];
    assert_eq!(desktop.requests(),*expected["cmd"].as_array().unwrap());
    assert_eq!(desktop.current_layout(),expected["desktop_layout"].as_str().unwrap());
    let mut closed = None;
    while let Some(message) = client.next().await {
        if let Message::Close(frame) = message.unwrap() {
            closed = frame.map(|frame|json!({"code":u16::from(frame.code),"reason":frame.reason.to_string()}));
        }
    }
    assert_eq!(closed.expect("a parada não enviou Close"),expected["client_close"]);
    crate::assert_closed(public,"porta do servidor parado").await;
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}
#[tokio::test]
async fn parent_eof_restores_active_public_viewer() { stopped_server("parent_eof_restores_active_public_viewer","public").await; }
#[tokio::test]
async fn parent_eof_restores_active_private_viewer() { stopped_server("parent_eof_restores_active_private_viewer","private").await; }
#[tokio::test]
async fn parent_eof_restores_active_share_viewer() { stopped_server("parent_eof_restores_active_share_viewer","guest").await; }

#[tokio::test]
async fn arbitrary_login_headers_and_query_do_not_panic_or_consult_python() {
    use proptest::{prelude::*, strategy::ValueTree, test_runner::{Config, TestRunner}};
    if isolated("arbitrary_login_headers_and_query_do_not_panic_or_consult_python").await { return; }
    let scene = Scene::new().await;
    login_guest(&scene);
    let mut runner = TestRunner::new(Config { cases:32, ..Config::default() });
    let strategy = (proptest::collection::vec(0x20u8..0x7f,0..256), proptest::collection::vec(any::<u8>(),0..128));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for case in 0..32 {
        let (cookie, query) = strategy.new_tree(&mut runner).unwrap().current();
        let query = if case%2==0 { String::new() } else {
            form_urlencoded::Serializer::new(String::new()).append_pair("token",&String::from_utf8_lossy(&query)).finish()
        };
        let response = client.get(format!("http://{}/api/sessions/missing/navegador?{query}",scene.owner.address))
            .header("cookie",HeaderValue::from_bytes(&cookie).unwrap()).send().await.unwrap();
        assert!([200,401,403].contains(&response.status().as_u16()));
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn cookie_ascii_preserves_the_valid_login_guest_prefix() {
    if isolated("cookie_ascii_preserves_the_valid_login_guest_prefix").await { return; }
    use tower::ServiceExt;
    let scene = Scene::new().await;
    login_guest(&scene);
    let reference = support::contract("cookie-ascii");
    assert_eq!(reference["observed"]["token"], "login-guest");
    let raw = reference["observed"]["raw_cookie_hex"].as_str().unwrap();
    let bytes: Vec<_> = (0..raw.len()).step_by(2).map(|index| u8::from_str_radix(&raw[index..index+2],16).unwrap()).collect();
    let mut request = axum::http::Request::builder().uri("/api/sessions/missing/navegador")
        .body(axum::body::Body::empty()).unwrap();
    request.headers_mut().insert("cookie", HeaderValue::from_bytes(&bytes).unwrap());
    request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap()));
    let response = hangar_server::routes::router(scene.state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), 403, "a chave não ASCII depois do cookie não descarta o convidado válido");
    let corpus = support::contract("url-commands");
    let expected = &corpus["cases"].as_array().unwrap().iter()
        .find(|case|case["name"]=="get_login_guest_is_blocked").unwrap()["expected"]["responses"][0]["body"];
    let body = axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),*expected);
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}
