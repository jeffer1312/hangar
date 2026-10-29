#![cfg(unix)]
use super::support::{self, OWNER, Scene, Server};
use super::transport::{Client, Desktop, barrier, until};
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::{Arc, Mutex}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpSocket}, sync::Notify, task::JoinHandle};
use tokio_tungstenite::tungstenite::Message;

fn case(name: &str) -> Value {
    support::contract("url-commands")["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == name).unwrap().clone()
}

fn unhex(raw: &str) -> Vec<u8> {
    assert_eq!(raw.len() % 2, 0);
    raw.as_bytes().chunks_exact(2)
        .map(|bytes| u8::from_str_radix(std::str::from_utf8(bytes).unwrap(), 16).unwrap())
        .collect()
}

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    if !dir.is_dir() { return BTreeMap::new(); }
    std::fs::read_dir(dir).unwrap().map(|entry| {
        let path = entry.unwrap().path();
        (path.file_name().unwrap().to_str().unwrap().to_owned(), std::fs::read(path).unwrap())
    }).collect()
}

fn write_files(scene: &Scene, source: &Value, port: u16) {
    let dir = scene.nav_dir();
    if dir.exists() { std::fs::remove_dir_all(&dir).unwrap(); }
    let Some(source) = source.as_object() else { return; };
    std::fs::create_dir_all(&dir).unwrap();
    for (name, entry) in source {
        let value = &entry["content"];
        let raw = match value.as_str() {
            Some(raw) => raw.to_owned(),
            None => value.to_string().replace("\"$PORT_A\"", &port.to_string()),
        };
        std::fs::write(dir.join(name), raw).unwrap();
    }
}

async fn ready(scene: &Scene, desktop: &Desktop) -> (Client, Vec<Value>) {
    let dir = scene.home.join(".hangar/claude-headless");
    let mut source: Value = serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
    source["name"] = json!("session");
    source["key"] = json!("command-contract-session-key");
    std::fs::write(dir.join("session.json"), source.to_string()).unwrap();
    desktop.publish(scene, "server::session");
    let (mut client, response) = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/session/nav-remoto?token={OWNER}", scene.owner.address,
    )).await.unwrap();
    assert_eq!(response.status(), 101);
    let mut events = Vec::new();
    until(&mut client, &mut events, |event| event["url"] == "http://contract.test/ready").await;
    (client, events)
}

async fn send_dimensions(client: &mut Client, events: &mut Vec<Value>, event_type: &str) {
    client.send(Message::Text(json!({"t":"layout","w":900,"h":700}).to_string().into())).await.unwrap();
    until(client, events, |event| event["t"] == event_type).await;
}

async fn close_client(mut client: Client, desktop: &Desktop) {
    client.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(message) = client.next().await { message.unwrap(); }
    }).await.expect("o WebSocket público não encerrou");
    desktop.wait_closed(1).await;
}

async fn cleanup(scene: Scene, desktop: Desktop) {
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

async fn finish(scene: Scene, desktop: Desktop, client: Client) {
    close_client(client, &desktop).await;
    cleanup(scene, desktop).await;
}

fn compare_request(actual: &Value, frozen: &Value) {
    assert_eq!(actual["method"], frozen["method"]);
    assert_eq!(actual["path"], frozen["path"]);
    assert_eq!(actual["authorization"], frozen["headers"]["Authorization"]);
    assert_eq!(actual["content_type"], frozen["headers"]["Content-Type"]);
    assert_eq!(actual["body"], frozen["body_json"]);
}

#[derive(Clone)]
struct Replies {
    status: u16,
    bodies: Arc<Vec<Vec<u8>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    changed: Arc<Notify>,
}

struct CommandServer {
    server: Server,
    replies: Replies,
}

impl CommandServer {
    async fn new(status: u16, bodies: Vec<Vec<u8>>) -> Self {
        async fn command(State(replies): State<Replies>, request: Request) -> axum::response::Response {
            let (parts, body) = request.into_parts();
            let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
            let mut requests = replies.requests.lock().unwrap();
            let index = requests.len();
            requests.push(json!({
                "method":parts.method.as_str(), "path":parts.uri.path(),
                "authorization":parts.headers.get("authorization").and_then(|v|v.to_str().ok()),
                "content_type":parts.headers.get("content-type").and_then(|v|v.to_str().ok()),
                "body":serde_json::from_slice::<Value>(&body).unwrap(),
            }));
            let body = replies.bodies.get(index).expect("/cmd recebeu chamada extra").clone();
            drop(requests);
            replies.changed.notify_waiters();
            (StatusCode::from_u16(replies.status).unwrap(), body).into_response()
        }
        let replies = Replies { status, bodies: Arc::new(bodies), requests: Arc::default(), changed: Arc::default() };
        let server = Server::owner(Router::new().route("/cmd", post(command)).with_state(replies.clone())).await;
        Self { server, replies }
    }

    fn requests(&self) -> Vec<Value> { self.replies.requests.lock().unwrap().clone() }

    async fn wait_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let received = self.replies.changed.notified();
                tokio::pin!(received);
                received.as_mut().enable();
                if self.replies.requests.lock().unwrap().len() >= count { break; }
                received.await;
            }
        }).await.expect("o controlador não recebeu todos os comandos de restauração");
    }
}

async fn frozen_failure(name: &str) {
    if support::isolated(&format!("remote_browser::commands::{name}"), |root| {
        assert!(!root.join("git.calls").exists(), "o layout iniciou Git");
    }).await { return; }
    let frozen = case(name);
    let input = &frozen["inputs"];
    let expected = &frozen["expected"];
    let scene = Scene::new().await;
    let desktop = Desktop::new(&json!({})).await;
    let (mut client, mut events) = ready(&scene, &desktop).await;
    let server = CommandServer::new(
        input["response_status"].as_u64().unwrap() as u16,
        vec![unhex(input["response_body_hex"].as_str().unwrap())],
    ).await;
    write_files(&scene, &input["files"], server.server.address.port());
    let before = files(&scene.nav_dir());
    send_dimensions(&mut client, &mut events, "erro").await;
    let message = expected.get("message").or_else(|| expected.get("value")).unwrap();
    assert_eq!(events.last().unwrap(), &json!({"t":"erro","m":message}), "{name}");
    barrier(&mut client, &mut events).await;
    let actual = server.requests();
    let requests = expected["requests"].as_array().unwrap();
    assert_eq!(actual.len(), requests.len(), "{name}: quantidade de comandos");
    for (actual, frozen) in actual.iter().zip(requests) { compare_request(actual, frozen); }
    assert_eq!(files(&scene.nav_dir()), before, "{name}: layout escreveu sidecars");
    assert!(desktop.requests().is_empty(), "o comando usou o endereço antigo do CDP");
    finish(scene, desktop, client).await;
    server.server.close().await;
}

macro_rules! failure_cases {
    ($($name:ident),* $(,)?) => { $(#[tokio::test] async fn $name() { frozen_failure(stringify!($name)).await; })* };
}
failure_cases! {
    cmd_missing_directory, cmd_missing_server, cmd_invalid_server_json, cmd_server_array,
    cmd_server_without_port, cmd_server_without_token, cmd_server_without_browser,
    cmd_ignores_underscore_matching_browser, cmd_http_error_body, cmd_http_auth_error_body,
    cmd_invalid_utf8_response, cmd_empty_response,
}

async fn frozen_success(name: &str) {
    if support::isolated(&format!("remote_browser::commands::{name}"), |root| {
        assert!(!root.join("git.calls").exists(), "o layout iniciou Git");
    }).await { return; }
    let frozen = case(name);
    let dimensions = case("cmd_dimensions");
    let transport = support::contract("transport");
    let layout_size = transport["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == "layout_size").unwrap();
    let imposed = layout_size["expected"]["events"].as_array().unwrap().iter()
        .find(|event| event["t"] == "layout").unwrap().clone();
    let original = unhex(frozen["inputs"]["response_body_hex"].as_str().unwrap());
    let imposed_body = imposed["resposta"].as_str().unwrap().as_bytes().to_vec();
    let server = CommandServer::new(200, vec![original.clone(), imposed_body.clone(), imposed_body, original]).await;
    let scene = Scene::new().await;
    let desktop = Desktop::new(&json!({})).await;
    let (mut client, mut events) = ready(&scene, &desktop).await;
    write_files(&scene, &frozen["inputs"]["files"], server.server.address.port());
    let before = files(&scene.nav_dir());
    send_dimensions(&mut client, &mut events, "layout").await;
    assert_eq!(events.last().unwrap(), &imposed);
    assert_eq!(files(&scene.nav_dir()), before);
    close_client(client, &desktop).await;
    server.wait_requests(4).await;
    let requests = server.requests();
    assert_eq!(requests.len(), 4, "consulta, imposição, conferência e restauração do original");
    let query = &frozen["expected"]["requests"][0];
    compare_request(&requests[0], query);
    compare_request(&requests[2], query);
    let mut impose = dimensions["expected"]["requests"][0].clone();
    impose["body_json"]["chave"] = query["body_json"]["chave"].clone();
    compare_request(&requests[1], &impose);
    let mut restore = query.clone();
    restore["body_json"]["args"] = json!([frozen["expected"]["value"].as_str().unwrap().strip_prefix("layout: ").unwrap()]);
    compare_request(&requests[3], &restore);
    cleanup(scene, desktop).await;
    server.server.close().await;
}

macro_rules! success_cases {
    ($($name:ident),* $(,)?) => { $(#[tokio::test] async fn $name() { frozen_success(stringify!($name)).await; })* };
}
success_cases! { cmd_native_without_target, cmd_electron, cmd_bad_sidecar_warns_before_valid }

#[tokio::test]
async fn cmd_connection_refused() {
    if support::isolated("remote_browser::commands::cmd_connection_refused", |root| {
        assert!(!root.join("git.calls").exists());
    }).await { return; }
    let frozen = case("cmd_connection_refused");
    support::assert_python_platform(&support::contract("url-commands")["sys_platform"]);
    let scene = Scene::new().await;
    let desktop = Desktop::new(&json!({})).await;
    let (mut client, mut events) = ready(&scene, &desktop).await;
    let reserved = TcpSocket::new_v4().unwrap();
    reserved.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    write_files(&scene, &frozen["inputs"]["files"], reserved.local_addr().unwrap().port());
    let before = files(&scene.nav_dir());
    send_dimensions(&mut client, &mut events, "erro").await;
    assert_eq!(events.last().unwrap(), &json!({"t":"erro","m":frozen["expected"]["message"]}));
    barrier(&mut client, &mut events).await;
    assert_eq!(files(&scene.nav_dir()), before);
    assert!(desktop.requests().is_empty());
    finish(scene, desktop, client).await;
    drop(reserved);
}

struct DisconnectedServer {
    address: SocketAddr,
    task: JoinHandle<Value>,
}

impl DisconnectedServer {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut byte = [0];
            while !raw.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.unwrap();
                raw.push(byte[0]);
                assert!(raw.len() < 16 * 1024);
            }
            let headers = String::from_utf8(raw).unwrap();
            let field = |key: &str| headers.lines().filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value.trim());
            let mut body = vec![0; field("content-length").unwrap().parse().unwrap()];
            socket.read_exact(&mut body).await.unwrap();
            let line: Vec<_> = headers.lines().next().unwrap().split_whitespace().collect();
            let request = json!({"method":line[0],"path":line[1],"authorization":field("authorization"),
                "content_type":field("content-type"),"body":serde_json::from_slice::<Value>(&body).unwrap()});
            socket.shutdown().await.unwrap();
            request
        });
        Self { address, task }
    }
}

#[tokio::test]
async fn cmd_disconnect_without_response() {
    if support::isolated("remote_browser::commands::cmd_disconnect_without_response", |root| {
        assert!(!root.join("git.calls").exists());
    }).await { return; }
    let frozen = case("cmd_disconnect_without_response");
    let scene = Scene::new().await;
    let desktop = Desktop::new(&json!({})).await;
    let (mut client, mut events) = ready(&scene, &desktop).await;
    let server = DisconnectedServer::new().await;
    write_files(&scene, &frozen["inputs"]["files"], server.address.port());
    send_dimensions(&mut client, &mut events, "erro").await;
    let request = tokio::time::timeout(Duration::from_secs(4), server.task).await.unwrap().unwrap();
    compare_request(&request, &frozen["expected"]["requests"][0]);
    assert_eq!(events.last().unwrap(), &json!({"t":"erro","m":frozen["expected"]["message"]}));
    barrier(&mut client, &mut events).await;
    assert!(desktop.requests().is_empty());
    finish(scene, desktop, client).await;
    crate::assert_closed(server.address, "controlador sem resposta").await;
}

#[tokio::test]
async fn cmd_reloads_server_after_restart() {
    if support::isolated("remote_browser::commands::cmd_reloads_server_after_restart", |root| {
        assert!(!root.join("git.calls").exists());
    }).await { return; }
    let frozen = case("cmd_reloads_server_after_restart");
    let dimensions = case("cmd_dimensions");
    let transport = support::contract("transport");
    let refusal = transport["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == "layout_refused").unwrap();
    let refusal_response = refusal["expected"]["events"].as_array().unwrap().iter()
        .find(|event| event["t"] == "layout").unwrap().clone();
    let scene = Scene::new().await;
    let desktop = Desktop::new(&json!({})).await;
    let (mut client, mut events) = ready(&scene, &desktop).await;
    let mut servers = Vec::new();
    for (index, body_key, token_key, requests_key) in [
        (0, "server_a_body", "first_token", "requests_a"),
        (1, "server_b_body", "second_token", "requests_b"),
    ] {
        let server = CommandServer::new(200, vec![
            frozen["inputs"][body_key].as_str().unwrap().as_bytes().to_vec(),
            refusal_response["resposta"].as_str().unwrap().as_bytes().to_vec(),
        ]).await;
        write_files(&scene, &frozen["inputs"]["files"], server.server.address.port());
        let path = scene.nav_dir().join("_srv.json");
        let mut source: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        source["token"] = frozen["inputs"][token_key].clone();
        if index == 1 { source["pid"] = frozen["inputs"]["second_pid"].clone(); }
        std::fs::write(path, source.to_string()).unwrap();
        let before = files(&scene.nav_dir());
        send_dimensions(&mut client, &mut events, "layout").await;
        assert_eq!(events.last().unwrap(), &refusal_response);
        assert_eq!(frozen["inputs"][body_key], frozen["expected"]["values"][index]);
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "cada aplicação consulta o original e tenta impor dimensões");
        compare_request(&requests[0], &frozen["expected"][requests_key][0]);
        let mut impose = dimensions["expected"]["requests"][0].clone();
        impose["headers"]["Authorization"] = frozen["expected"][requests_key][0]["headers"]["Authorization"].clone();
        compare_request(&requests[1], &impose);
        assert_eq!(files(&scene.nav_dir()), before);
        servers.push(server);
    }
    assert!(desktop.requests().is_empty());
    finish(scene, desktop, client).await;
    for server in servers { server.server.close().await; }
}
