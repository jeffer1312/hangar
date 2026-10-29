#![cfg(unix)]
use super::support::{self, OWNER, Scene, Server};
use super::transport::{Client, barrier, until};
use axum::{Router, extract::{FromRequestParts, Request, State, ws::{Message as PeerMessage, WebSocketUpgrade}}, routing::{get, post}};
use futures_util::{FutureExt, SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::{Arc, Mutex}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, sync::Notify};
use tokio_tungstenite::tungstenite::{Error, Message};

const SOURCE: &str = "ba2db1021ab7fb801eca50a50f9509dcb825ac7e";
const DESKTOP_TOKEN: &str = "remote-browser-contract-desktop";
const BARRIER: &str = "__remote_browser_contract_barrier__";

fn reference(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backend/tests/fixtures/contract/remote-browser/url-gaps.json");
    assert!(path.is_file(), "captura original ainda pendente: {}", path.display());
    let corpus = support::contract("url-gaps");
    assert_eq!(corpus["source_commit"], SOURCE);
    assert_eq!(corpus["platform"], std::env::consts::OS);
    corpus["cases"].as_array().unwrap().iter().find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("caso não capturado pelo Python original: {name}")).clone()
}

async fn isolated(name: &str, deadline: u64) -> bool {
    support::isolated_with_timeout(&format!("remote_browser::url_gaps::{name}"), Duration::from_secs(deadline), |root| {
        assert!(!root.join("git.calls").exists(), "a prova de navegador iniciou Git");
    }).await
}

fn write_files(scene: &Scene, files: &Value, port: u16) {
    let dir = scene.nav_dir();
    if dir.exists() { std::fs::remove_dir_all(&dir).unwrap(); }
    if files.is_null() { return; }
    std::fs::create_dir_all(&dir).unwrap();
    for entry in files.as_array().unwrap() {
        let path = dir.join(entry["name"].as_str().unwrap());
        if entry["directory"] == true {
            std::fs::create_dir(path).unwrap();
        } else if let Some(raw) = entry["bytes_hex"].as_str() {
            let bytes: Vec<_> = raw.as_bytes().chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect();
            std::fs::write(path, bytes).unwrap();
        } else {
            std::fs::write(path, entry["text"].as_str().unwrap().replace("\"$CONTROLLER_PORT\"", &port.to_string())).unwrap();
        }
    }
}

fn snapshot(dir: &Path) -> Option<BTreeMap<String, Value>> {
    if !dir.exists() { return None; }
    Some(std::fs::read_dir(dir).unwrap().map(|entry| {
        let path = entry.unwrap().path();
        let value = if path.is_dir() { json!({"directory":true}) } else {
            let bytes: String = std::fs::read(&path).unwrap().iter().map(|byte| format!("{byte:02x}")).collect();
            json!({"bytes_hex":bytes})
        };
        (path.file_name().unwrap().to_str().unwrap().to_owned(), value)
    }).collect())
}

fn assert_order(scene: &Scene, expected: &Value) {
    let actual = if scene.nav_dir().is_dir() {
        json!(std::fs::read_dir(scene.nav_dir()).unwrap().map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
            .filter(|name| name.ends_with(".json")).collect::<Vec<_>>())
    } else { Value::Null };
    assert_eq!(actual, *expected, "enumeração diferente da captura original; conservar o tmpfs e a ordem, sem ordenar ou ajustar esperado");
}

fn seed(scene: &Scene, name: &str) {
    let dir = scene.home.join(".hangar/claude-headless");
    let mut source: Value = serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
    source["name"] = json!(name);
    source["key"] = json!(format!("{name}-key"));
    std::fs::write(dir.join(format!("{name}.json")), source.to_string()).unwrap();
    scene.state.list.invalidate();
}

async fn connect(scene: &Scene, name: &str) -> Result<(Client, axum::http::Response<Option<Vec<u8>>>), Error> {
    let encoded = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    tokio_tungstenite::connect_async(format!("ws://{}/api/sessions/{encoded}/nav-remoto?token={OWNER}", scene.owner.address)).await
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<Value>>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Logs {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "hangar.navsock" || *event.metadata().level() != tracing::Level::WARN { return; }
        struct Visitor(String);
        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" { self.0 = format!("{value:?}"); }
            }
        }
        let mut visitor = Visitor(String::new());
        event.record(&mut visitor);
        self.0.lock().unwrap().push(json!({"level":"WARNING", "message":visitor.0}));
    }
}
impl Logs {
    fn install() -> Self {
        use tracing_subscriber::prelude::*;
        let logs = Self::default();
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(logs.clone())).unwrap();
        logs
    }
    fn clear(&self) { self.0.lock().unwrap().clear(); }
    fn assert_warnings(&self, expected: &Value) {
        let root = std::env::var("HANGAR_REMOTE_BROWSER_ROOT").unwrap();
        let actual: Vec<_> = self.0.lock().unwrap().iter().map(|row| json!({"level":row["level"],
            "message":row["message"].as_str().unwrap().replace(&root, "$ROOT")})).collect();
        assert_eq!(json!(actual), *expected, "warning diferente do original; nenhuma normalização do parser/SO foi aprovada");
    }
}

#[derive(Default)]
struct Observed {
    handshakes: Vec<Value>,
    requests: Vec<Value>,
    native_cdp: Vec<Value>,
    electron_cdp: Vec<Value>,
    closed: usize,
}
#[derive(Clone)]
struct Context {
    observed: Arc<Mutex<Observed>>,
    changed: Arc<Notify>,
    measured: Arc<Value>,
    evaluate: Value,
}
struct Peer {
    context: Context,
    native: Server,
    electron: Option<Server>,
}
impl Peer {
    async fn new(evaluate: Value, electron: bool) -> Self {
        let context = Context { observed: Arc::default(), changed: Arc::default(),
            measured: Arc::new(support::contract("chromium")), evaluate };
        let app = Router::new().route("/cdp", get(native_cdp)).route("/devtools/page/GAP-TARGET", get(electron_cdp))
            .route("/cmd", post(command)).with_state(context.clone());
        let native = Server::owner(app.clone()).await;
        // A porta fixa só é usada depois de um bind nosso; nunca reutiliza o navegador do usuário.
        let electron = if electron { Some(Server::at(app, "127.0.0.1:9223".parse().unwrap()).await) } else { None };
        Self { context, native, electron }
    }
    async fn close(self) {
        self.native.close().await;
        if let Some(server) = self.electron { server.close().await; }
    }
    async fn wait_closed(&self) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let changed = self.context.changed.notified();
                if self.context.observed.lock().unwrap().closed == 1 { break; }
                changed.await;
            }
        }).await.expect("o CDP de ensaio ficou aberto");
    }
}
async fn native_cdp(State(context): State<Context>, request: Request) -> axum::response::Response {
    cdp(context, request, false).await
}
async fn electron_cdp(State(context): State<Context>, request: Request) -> axum::response::Response {
    cdp(context, request, true).await
}
async fn cdp(context: Context, request: Request, electron: bool) -> axum::response::Response {
    let authorization = request.headers().get("authorization").map(|value| value.to_str().unwrap());
    assert_eq!(authorization, if electron { None } else { Some(concat!("Bearer ", "remote-browser-contract-desktop")) });
    context.observed.lock().unwrap().handshakes.push(json!({"transport":if electron {"electron"} else {"native"},
        "path":request.uri().path(), "authorization":authorization,
        "key":hangar_server::auth::query_param(request.uri().query(), "chave")}));
    let (mut parts, _) = request.into_parts();
    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &()).await.unwrap();
    upgrade.on_upgrade(move |mut socket| async move {
        let mut acknowledged = false;
        while let Some(Ok(message)) = socket.next().await {
            let PeerMessage::Text(raw) = message else {
                if matches!(message, PeerMessage::Close(_)) { let _ = socket.flush().await; break; }
                if matches!(message, PeerMessage::Ping(_)) { let _ = socket.flush().await; }
                continue;
            };
            let message: Value = serde_json::from_str(&raw).unwrap();
            let method = message["method"].as_str().unwrap();
            {
                let mut observed = context.observed.lock().unwrap();
                if electron { observed.electron_cdp.push(message.clone()); }
                else { observed.native_cdp.push(json!({"peer":0, "message":message})); }
            }
            let mut reply = if electron && method == "Runtime.evaluate" { context.evaluate.clone() } else { json!({"result":{}}) };
            if method == "Page.captureScreenshot" { reply = json!({"result":context.measured["screenshot"]}); }
            reply["id"] = message["id"].clone();
            if socket.send(PeerMessage::Text(reply.to_string().into())).await.is_err() { break; }
            let event = if method == "Page.startScreencast" {
                Some(json!({"method":"Page.screencastFrame", "params":context.measured["frame"]}))
            } else if method == "Page.screencastFrameAck" && !acknowledged {
                acknowledged = true;
                Some(json!({"method":"Page.frameNavigated", "params":{"frame":{"url":"http://contract.test/ready"}}}))
            } else if method == "Input.insertText" && message["params"]["text"] == BARRIER {
                Some(json!({"method":"Page.frameNavigated", "params":{"frame":{"url":"http://contract.test/barrier"}}}))
            } else { None };
            if let Some(event) = event {
                if socket.send(PeerMessage::Text(event.to_string().into())).await.is_err() { break; }
            }
        }
        context.observed.lock().unwrap().closed += 1;
        context.changed.notify_one();
    })
}
async fn command(State(context): State<Context>, request: Request) -> String {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parts.headers["authorization"].to_str().unwrap(), format!("Bearer {DESKTOP_TOKEN}"));
    assert_eq!(body["verbo"], "layout");
    context.observed.lock().unwrap().requests.push(json!({"method":parts.method.as_str(), "path":parts.uri.path(),
        "authorization":parts.headers["authorization"].to_str().unwrap(), "content_type":parts.headers["content-type"].to_str().unwrap(), "body":body}));
    "layout: mobile".to_owned()
}

async fn finish(mut client: Client, peer: &Peer) {
    client.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(message) = client.next().await { message.unwrap(); }
    }).await.expect("o WebSocket público ficou aberto");
    peer.wait_closed().await;
}

async fn selection(name: &str) {
    if isolated(name, 25).await { return; }
    let case = reference(name);
    let logs = Logs::install();
    let scene = Scene::new().await;
    assert_eq!(case["inputs"]["phases"].as_array().unwrap().len(), case["expected"]["phases"].as_array().unwrap().len());
    for (input, expected) in case["inputs"]["phases"].as_array().unwrap().iter().zip(case["expected"]["phases"].as_array().unwrap()) {
        let session = input["session"].as_str().unwrap();
        seed(&scene, session);
        let electron = input["files"].as_array().is_some_and(|files| files.iter().any(|file| file["name"] == "electron.json"));
        let peer = Peer::new(input["evaluate"].clone(), electron).await;
        write_files(&scene, &input["files"], peer.native.address.port());
        assert_order(&scene, &input["directory_order"]);
        let before = snapshot(&scene.nav_dir());
        logs.clear();
        let mut events = Vec::new();
        match connect(&scene, session).await {
            Ok((mut client, response)) => {
                assert_eq!(response.status().as_u16(), expected["status"].as_u64().unwrap() as u16);
                until(&mut client, &mut events, |event| event["url"] == "http://contract.test/ready").await;
                finish(client, &peer).await;
            },
            Err(Error::Http(response)) => {
                assert_eq!(json!(response.status().as_u16()), expected["status"]);
                let body: String = response.body().as_deref().unwrap_or_default().iter().map(|byte| format!("{byte:02x}")).collect();
                assert_eq!(json!(body), expected["body_hex"]);
            },
            Err(error) => panic!("a abertura não corresponde à referência: {error}"),
        }
        assert_eq!(json!(events), expected["events"]);
        {
            let actual = peer.context.observed.lock().unwrap();
            assert_eq!(json!(actual.handshakes), expected["handshakes"]);
            assert_eq!(json!(actual.requests), expected["requests"]);
            assert_eq!(json!(actual.native_cdp), expected["native_cdp"]);
            assert_eq!(json!(actual.electron_cdp), expected["electron_cdp"]);
        }
        if name == "ws_unreadable_and_non_objects_before_valid" {
            assert!(!expected["warnings"].as_array().unwrap().is_empty(), "a captura original precisa conter os avisos dos arquivos ilegíveis");
        }
        if input["directory_order"].is_null() { assert!(logs.0.lock().unwrap().is_empty()); }
        else { logs.assert_warnings(&expected["warnings"]); }
        assert_eq!(expected["files_unchanged"], true);
        assert_eq!(snapshot(&scene.nav_dir()), before);
        peer.close().await;
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

macro_rules! selection_cases { ($($name:ident),* $(,)?) => {$(
    #[tokio::test] async fn $name() { selection(stringify!($name)).await; }
)*}; }
selection_cases! {
    ws_directory_absent, ws_underscore_match_is_ignored, ws_unreadable_and_non_objects_before_valid,
    ws_first_match_creation_order_a_b, ws_first_match_creation_order_b_a,
    ws_number_key, ws_boolean_key, ws_null_key, ws_array_key_before_valid, ws_object_key_before_valid,
    ws_false_key_before_valid, ws_native_missing_only_key, ws_electron_precedes_usable_native,
    ws_evaluate_error_keeps_sidecar_url, ws_evaluate_number_keeps_sidecar_url, ws_evaluate_empty_keeps_sidecar_url,
}

async fn ready(scene: &Scene, peer: &Peer, input: &Value) -> Client {
    seed(scene, "nav-contract");
    write_files(scene, &input["initial_files"], peer.native.address.port());
    assert_order(scene, &input["initial_directory_order"]);
    let (mut client, response) = connect(scene, "nav-contract").await.unwrap();
    assert_eq!(response.status(), 101);
    until(&mut client, &mut Vec::new(), |event| event["url"] == "http://contract.test/ready").await;
    client
}
async fn outcome(client: &mut Client) -> Value {
    tokio::time::timeout(Duration::from_secs(65), async {
        loop {
            let message = client.next().await.expect("a conversa fechou antes da resposta de layout").unwrap();
            if let Message::Text(raw) = message {
                let event: Value = serde_json::from_str(&raw).unwrap();
                if event["t"] == "layout" || event["t"] == "erro" { break event; }
            }
        }
    }).await.expect("o comando de layout não terminou")
}
async fn commands(name: &str) {
    if isolated(name, 25).await { return; }
    let case = reference(name);
    let logs = Logs::install();
    let scene = Scene::new().await;
    let peer = Peer::new(json!({}), false).await;
    let mut client = ready(&scene, &peer, &case["inputs"]).await;
    let mut outcomes = Vec::new();
    logs.clear();
    for stage in case["inputs"]["stages"].as_array().unwrap() {
        write_files(&scene, &stage["files"], peer.native.address.port());
        assert_order(&scene, &stage["directory_order"]);
        let before = snapshot(&scene.nav_dir());
        client.send(Message::Text(json!({"t":"layout", "modo":"mobile"}).to_string().into())).await.unwrap();
        outcomes.push(outcome(&mut client).await);
        barrier(&mut client, &mut Vec::new()).await;
        assert_eq!(snapshot(&scene.nav_dir()), before);
    }
    finish(client, &peer).await;
    assert_eq!(json!(outcomes), case["expected"]["outcomes"]);
    assert_eq!(json!(peer.context.observed.lock().unwrap().requests), case["expected"]["requests"]);
    if name == "cmd_warning_and_non_objects_before_valid" {
        assert!(!case["expected"]["warnings"].as_array().unwrap().is_empty(), "a captura original precisa conter os avisos do leitor de comandos");
    }
    logs.assert_warnings(&case["expected"]["warnings"]);
    scene.upstream.assert_no_browser_or_discovery_calls();
    peer.close().await;
    scene.close().await;
}
#[tokio::test]
async fn cmd_warning_and_non_objects_before_valid() { commands("cmd_warning_and_non_objects_before_valid").await; }
#[tokio::test]
async fn cmd_reloads_changed_removed_and_exact_key() { commands("cmd_reloads_changed_removed_and_exact_key").await; }

#[derive(Default)]
struct HeldRequests {
    accepted: usize,
    requests: Vec<Value>,
    closed: usize,
}

#[tokio::test]
async fn cmd_default_read_timeout_45_seconds() {
    if isolated("cmd_default_read_timeout_45_seconds", 80).await { return; }
    let case = reference("cmd_default_read_timeout_45_seconds");
    let logs = Logs::install();
    let scene = Scene::new().await;
    let peer = Peer::new(json!({}), false).await;
    let mut client = ready(&scene, &peer, &case["inputs"]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let observed = Arc::new(Mutex::new(HeldRequests::default()));
    let requests = observed.clone();
    let body_ready = Arc::new(Notify::new());
    let ready = body_ready.clone();
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let serving = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = tokio::select! {
                biased;
                accepted = listener.accept() => accepted.unwrap(),
                _ = stopped.changed() => break,
            };
            requests.lock().unwrap().accepted += 1;
            let requests = requests.clone();
            let ready = ready.clone();
            let mut closed = stopped.clone();
            connections.spawn(async move {
                let receive = async {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        head.push(socket.read_u8().await.unwrap());
                        assert!(head.len() < 16384);
                    }
                    let head = String::from_utf8(head).unwrap();
                    let field = |key: &str| head.lines().filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value.trim());
                    let mut raw = vec![0; field("content-length").unwrap().parse().unwrap()];
                    socket.read_exact(&mut raw).await.unwrap();
                    let first: Vec<_> = head.lines().next().unwrap().split_whitespace().collect();
                    requests.lock().unwrap().requests.push(json!({"method":first[0], "path":first[1], "authorization":field("authorization"),
                        "content_type":field("content-type"), "body":serde_json::from_slice::<Value>(&raw).unwrap()}));
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nlayout:").await.unwrap();
                    ready.notify_one();
                    let mut tail = [0];
                    match socket.read(&mut tail).await {
                        Ok(0) => {},
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {},
                        other => panic!("o cliente não encerrou a leitura pendente: {other:?}"),
                    }
                    requests.lock().unwrap().closed += 1;
                };
                tokio::select! {
                    _ = receive => {},
                    _ = closed.changed() => {},
                }
            });
        }
        drop(listener);
        while let Some(result) = connections.join_next().await { result.unwrap(); }
    });
    write_files(&scene, &case["inputs"]["files"], address.port());
    assert_order(&scene, &case["inputs"]["directory_order"]);
    let before = snapshot(&scene.nav_dir());
    logs.clear();
    let keeper = tokio::spawn(async {
        loop { tokio::task::yield_now().await; }
    });
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    client.send(Message::Text(json!({"t":"layout", "modo":"mobile"}).to_string().into())).await.unwrap();
    body_ready.notified().await;
    let deadline = Duration::from_secs_f64(case["inputs"]["deadline_seconds"].as_f64().unwrap());
    assert_eq!(started.elapsed(), Duration::ZERO, "o relógio avançou antes da barreira HTTP");
    let actual = {
        let pending = outcome(&mut client);
        tokio::pin!(pending);
        assert!(pending.as_mut().now_or_never().is_none());
        tokio::time::advance(deadline - Duration::from_secs(1)).await;
        assert!(pending.as_mut().now_or_never().is_none(), "o comando terminou antes dos 45 s");
        assert_eq!(observed.lock().unwrap().closed, 0, "o controlador foi encerrado antes dos 45 s");
        tokio::time::advance(Duration::from_secs(1)).await;
        // A vigia existente só permite entregar I/O: o relógio continua parado nos 45 s.
        let mut wall = tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_secs(2)));
        let actual = tokio::select! { event = &mut pending => Some(event), _ = &mut wall => None };
        let elapsed = started.elapsed();
        assert_eq!(elapsed, deadline);
        keeper.abort();
        let _ = keeper.await;
        tokio::time::resume();
        if !wall.is_finished() { wall.await.unwrap(); }
        actual.expect("o comando não terminou com o relógio parado no prazo original de 45 s")
    };
    eprintln!("prazo de leitura: Python original={} s; Rust virtual={} s", case["expected"]["elapsed_seconds"], deadline.as_secs_f64());
    assert_eq!(json!([actual]), case["expected"]["outcomes"]);
    assert!(peer.context.observed.lock().unwrap().requests.is_empty());
    barrier(&mut client, &mut Vec::new()).await;
    assert_eq!(snapshot(&scene.nav_dir()), before);
    finish(client, &peer).await;
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(4), serving).await.unwrap().unwrap();
    let received = observed.lock().unwrap();
    assert_eq!(json!(received.accepted), case["expected"]["accepted_connections"]);
    assert_eq!(json!(received.requests), case["expected"]["requests"]);
    drop(received);
    logs.assert_warnings(&case["expected"]["warnings"]);
    crate::assert_closed(address, "controlador de corpo retido").await;
    scene.upstream.assert_no_browser_or_discovery_calls();
    peer.close().await;
    scene.close().await;
}
