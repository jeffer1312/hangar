#![cfg(unix)]
use super::support::{self, OWNER, Scene, Server};
use axum::{
    Router,
    extract::ws::{Message, WebSocketUpgrade},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;

#[tokio::test]
async fn connects_to_desktop_without_python() {
    if support::isolated(
        "remote_browser::transport::connects_to_desktop_without_python",
        |root| {
            assert!(!root.join("git.calls").exists(), "descoberta iniciou Git");
        },
    )
    .await
    {
        return;
    }
    let scene = Scene::new().await;
    let desktop = Server::owner(Router::new().route(
        "/cdp",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut ws| async move {
                while let Some(Ok(Message::Text(raw))) = ws.next().await {
                    let message: serde_json::Value = serde_json::from_str(&raw).unwrap();
                    if ws
                        .send(Message::Text(
                            json!({"id":message["id"],"result":{}}).to_string().into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            })
        }),
    ))
    .await;
    std::fs::write(scene.nav_dir().join("browser.json"), json!({"chave":"contract::headless","url":"http://contract.test/original","targetId":null,"pid":123}).to_string()).unwrap();
    std::fs::write(
        scene.nav_dir().join("_srv.json"),
        json!({"porta":desktop.address.port(),"token":"desktop-contract","pid":123}).to_string(),
    )
    .unwrap();
    let response = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/headless/nav-remoto?token={OWNER}",
        scene.owner.address
    ))
    .await;
    assert!(
        response.is_ok(),
        "rota deve aceitar 101 sem Python: {response:?}"
    );
    let (mut socket, handshake) = response.unwrap();
    assert_eq!(handshake.status(), 101);
    let initial = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(initial.to_text().unwrap()).unwrap(),
        json!({"t":"url","url":"http://contract.test/original"})
    );
    socket.close(None).await.unwrap();
    while socket.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.close().await;
    scene.close().await;
}

use axum::{
    extract::{Request, State},
    response::IntoResponse,
    routing::post,
};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message as ClientMessage, client::IntoClientRequest},
};
pub(super) type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub(super) const KEY: &str = "contract::nav-contract";
const DESKTOP_TOKEN: &str = "remote-browser-contract-desktop";
const BARRIER: &str = "__remote_browser_contract_barrier__";

struct DesktopState {
    commands: Vec<Value>,
    requests: Vec<Value>,
    keys: Vec<String>,
    peers: Vec<mpsc::UnboundedSender<Value>>,
    layout: String,
    pid: u32,
    token: String,
    screenshot: bool,
    capture_errors: u64,
    input_error: bool,
    refuse: String,
    fail_original: bool,
    fail_restore: bool,
    closed: usize,
}
pub(super) struct Desktop {
    state: Arc<Mutex<DesktopState>>,
    changed: Arc<Notify>,
    pub(super) server: Server,
}
#[derive(Clone)]
struct DesktopContext {
    state: Arc<Mutex<DesktopState>>,
    changed: Arc<Notify>,
    measured: Arc<Value>,
}
impl Desktop {
    pub(super) async fn new(inputs: &Value) -> Self {
        let fault = &inputs["desktop_fault"];
        let state = Arc::new(Mutex::new(DesktopState {
            commands: vec![],
            requests: vec![],
            keys: vec![],
            peers: vec![],
            layout: "mobile".into(),
            pid: 123,
            token: DESKTOP_TOKEN.into(),
            screenshot: inputs.get("capture_errors").is_some(),
            capture_errors: inputs["capture_errors"].as_u64().unwrap_or(0),
            input_error: inputs["input_error"] == true,
            refuse: fault["refuse"].as_str().unwrap_or("").into(),
            fail_original: fault["fail_original"] == true,
            fail_restore: fault["fail_restore"] == true,
            closed: 0,
        }));
        let changed = Arc::new(Notify::new());
        let context = DesktopContext {
            state: state.clone(),
            changed: changed.clone(),
            measured: Arc::new(support::contract("chromium")),
        };
        let server = Server::owner(
            Router::new()
                .route("/cdp", get(desktop_cdp))
                .route("/cmd", post(desktop_command))
                .with_state(context),
        )
        .await;
        Self {
            state,
            changed,
            server,
        }
    }
    pub(super) fn publish(&self, scene: &Scene, key: &str) {
        let d = self.state.lock().unwrap();
        std::fs::write(
            scene.nav_dir().join("browser.json"),
            json!({"chave":key,"url":"http://contract.test/original","targetId":null,"pid":d.pid})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            scene.nav_dir().join("_srv.json"),
            json!({"porta":self.server.address.port(),"token":d.token,"pid":d.pid}).to_string(),
        )
        .unwrap();
    }
    async fn wait(&self, predicate: impl Fn(&DesktopState) -> bool) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let notified = self.changed.notified();
                if predicate(&self.state.lock().unwrap()) {
                    break;
                }
                notified.await;
            }
        })
        .await
        .expect("desktop não recebeu o efeito esperado");
    }
    pub(super) fn detach(&self, index: usize, reason: &str) {
        self.state.lock().unwrap().peers[index]
            .send(json!({"method":"Inspector.detached","params":{"reason":reason}}))
            .unwrap();
    }
    pub(super) fn identify(&self, pid: u32, token: &str, layout: &str) {
        let mut state = self.state.lock().unwrap();
        state.pid = pid;
        state.token = token.into();
        state.layout = layout.into();
    }
    pub(super) fn requests(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }
    pub(super) fn current_layout(&self) -> String {
        self.state.lock().unwrap().layout.clone()
    }
    pub(super) async fn wait_closed(&self, count: usize) {
        self.wait(|state| state.closed >= count).await;
    }
    pub(super) async fn wait_requests(&self, count: usize) {
        self.wait(|state| state.requests.len() >= count).await;
    }
    pub(super) async fn assert_no_new_requests(&self, previous: &[Value]) {
        let received = tokio::time::timeout(
            Duration::from_secs(1),
            self.wait(|state| state.requests.len() > previous.len()),
        ).await;
        assert!(received.is_err(), "encerramento consultou ou restaurou um desktop sem posse");
        assert_eq!(self.requests(), previous);
    }
    fn result(&self, events: Vec<Value>, logs: Vec<Value>) -> Value {
        let d = self.state.lock().unwrap();
        json!({"events":events,"cdp":d.commands,"cmd":d.requests,"desktop_layout":d.layout,"cdp_keys":d.keys,"log":logs})
    }
}
async fn desktop_cdp(
    State(ctx): State<DesktopContext>,
    request: Request,
) -> axum::response::Response {
    assert_eq!(
        request
            .headers()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {}", ctx.state.lock().unwrap().token)
    );
    let key = hangar_server::auth::query_param(request.uri().query(), "chave").unwrap();
    let (mut parts, _) = request.into_parts();
    use axum::extract::FromRequestParts;
    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .unwrap();
    ws.on_upgrade(move |mut ws| async move {
        let (send,mut receive)=mpsc::unbounded_channel();
        let index={let mut d=ctx.state.lock().unwrap();let index=d.peers.len();d.peers.push(send);d.keys.push(key);index};
        let mut acknowledged=false;
        loop {
            tokio::select! {
                Some(event)=receive.recv()=>if ws.send(Message::Text(event.to_string().into())).await.is_err(){break},
                message=ws.next()=>{
                    let Some(Ok(Message::Text(raw)))=message else { break; };
                    let message:Value=serde_json::from_str(&raw).unwrap();
                    let method=message["method"].as_str().unwrap();
                    let result={
                        let mut d=ctx.state.lock().unwrap();d.commands.push(json!({"peer":index,"message":message}));
                        if method=="Page.captureScreenshot" {
                            if d.capture_errors>0 {d.capture_errors-=1;json!({"id":message["id"],"error":{"code":-32000,"message":"capture refused by fixture"}})}
                            else{json!({"id":message["id"],"result":ctx.measured["screenshot"]})}
                        } else if method.starts_with("Input.") && d.input_error {json!({"id":message["id"],"error":{"code":-32000,"message":"input refused by fixture"}})}
                        else {json!({"id":message["id"],"result":{}})}
                    };
                    ctx.changed.notify_waiters();
                    if ws.send(Message::Text(result.to_string().into())).await.is_err(){break;}
                    let event=if method=="Page.startScreencast" && !ctx.state.lock().unwrap().screenshot {Some(json!({"method":"Page.screencastFrame","params":ctx.measured["frame"]}))}
                    else if method=="Page.screencastFrameAck" && !acknowledged {acknowledged=true;Some(json!({"method":"Page.frameNavigated","params":{"frame":{"url":"http://contract.test/ready"}}}))}
                    else if method=="Input.insertText" && message["params"]["text"]==BARRIER {Some(json!({"method":"Page.frameNavigated","params":{"frame":{"url":"http://contract.test/barrier"}}}))}else{None};
                    if let Some(event)=event {if ws.send(Message::Text(event.to_string().into())).await.is_err(){break;}}
                }
            }
        }
        ctx.state.lock().unwrap().closed+=1;ctx.changed.notify_waiters();
    })
}
async fn desktop_command(
    State(ctx): State<DesktopContext>,
    request: Request,
) -> axum::response::Response {
    let auth = request
        .headers()
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(request.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let mut d = ctx.state.lock().unwrap();
    let pid = d.pid;
    d.requests
        .push(json!({"authorization":auth,"body":body,"receiver_pid":pid}));
    ctx.changed.notify_waiters();
    if auth != format!("Bearer {}", d.token) {
        return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let args = body["args"].as_array().unwrap();
    if args.is_empty() && d.fail_original {
        return "erro: layout indisponível".into_response();
    }
    if !args.is_empty() {
        let proposed = args
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("x");
        if proposed == d.refuse || (d.fail_restore && proposed == "mobile") {
            return format!("erro: layout {proposed}: recusado").into_response();
        }
        d.layout = proposed;
    }
    format!("layout: {}", d.layout).into_response()
}
async fn next_event(client: &mut Client, events: &mut Vec<Value>) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(4), client.next())
            .await
            .expect("sem evento público")
            .expect("WS terminou")
            .unwrap();
        if let ClientMessage::Text(raw) = message {
            let value: Value = serde_json::from_str(&raw).unwrap();
            events.push(value.clone());
            return value;
        }
    }
}
pub(super) async fn until(client: &mut Client, events: &mut Vec<Value>, predicate: impl Fn(&Value) -> bool) {
    loop {
        if predicate(&next_event(client, events).await) {
            return;
        }
    }
}
pub(super) async fn barrier(client: &mut Client, events: &mut Vec<Value>) {
    client
        .send(ClientMessage::Text(
            json!({"t":"txt","s":BARRIER}).to_string().into(),
        ))
        .await
        .unwrap();
    until(client, events, |v| {
        v["url"] == "http://contract.test/barrier"
    })
    .await;
}
pub(super) async fn connect(
    scene: &Scene,
    query: &str,
) -> Result<(Client, axum::http::Response<Option<Vec<u8>>>), tokio_tungstenite::tungstenite::Error>
{
    tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/nav-contract/nav-remoto?token={OWNER}&{}",
        scene.owner.address,
        query.trim_start_matches('&')
    ))
    .await
}
pub(super) fn seed_session(scene: &Scene) {
    let dir = scene.home.join(".hangar/claude-headless");
    let mut sidecar: Value =
        serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
    sidecar["name"] = json!("nav-contract");
    sidecar["key"] = json!("nav-contract-key");
    std::fs::write(dir.join("nav-contract.json"), sidecar.to_string()).unwrap();
}

#[derive(Clone)]
struct Logs(Arc<Mutex<Vec<Value>>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Logs {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "hangar.navsock"
            || *event.metadata().level() != tracing::Level::WARN
        {
            return;
        }
        struct Visitor(String);
        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut visitor = Visitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap()
            .push(json!({"level":"WARNING","message":visitor.0}));
    }
}
fn d02_expected(case: &Value) -> Value {
    let mut expected = case["expected"].clone();
    let original = expected["cmd"].as_array().unwrap();
    let change = &case["inputs"]["source_change"];
    assert_eq!(original.len(), 4, "D02 precisa continuar preso à restauração antiga");
    assert_eq!(expected["desktop_layout"], "mobile");
    assert_eq!(original[0]["receiver_pid"], change["old_desktop_pid"]);
    assert_eq!(original[0]["body"]["args"], json!([]));
    assert_eq!(original[1]["receiver_pid"], change["old_desktop_pid"]);
    assert_eq!(original[1]["body"]["args"], json!(["900", "700"]));
    assert_eq!(original[2]["receiver_pid"], change["new_desktop_pid"]);
    assert_eq!(original[2]["body"]["chave"], change["key"]);
    assert_eq!(original[2]["body"]["args"], json!([]));
    assert_eq!(original[3]["receiver_pid"], change["new_desktop_pid"]);
    assert_eq!(original[3]["body"]["chave"], change["key"]);
    assert_eq!(original[3]["body"]["args"], json!(["mobile"]));
    // D02 muda somente os dois comandos tardios e o layout que eles alteravam.
    expected["cmd"] = json!(&original[..2]);
    expected["desktop_layout"] = json!("900x700");
    expected
}

async fn frozen_case(name: &str) {
    let exact = format!("remote_browser::transport::{name}");
    if support::isolated(&exact, |root| {
        assert!(!root.join("git.calls").exists(), "descoberta iniciou Git")
    })
    .await
    {
        return;
    }
    let corpus = support::contract("transport");
    let corpus_name = name.replace("_minus_", "_-");
    let case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == corpus_name)
        .unwrap();
    let inputs = &case["inputs"];
    let changes_d02 = matches!(name,
        "same_name_new_browser_receives_old_restore" | "same_key_new_desktop_receives_old_restore");
    let expected_d02;
    let expected = if changes_d02 {
        expected_d02 = d02_expected(case);
        &expected_d02
    } else {
        &case["expected"]
    };
    let logs = Logs(Arc::new(Mutex::new(vec![])));
    use tracing_subscriber::prelude::*;
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(logs.clone()))
        .unwrap();
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(inputs).await;
    desktop.publish(&scene, KEY);
    if name.starts_with("auth_") {
        for token in inputs["token_sequence"].as_array().unwrap() {
            let query = token
                .as_str()
                .map(|token| {
                    form_urlencoded::Serializer::new(String::new())
                        .append_pair("token", token)
                        .finish()
                })
                .unwrap_or_default();
            let mut request = format!(
                "ws://{}/api/sessions/missing/nav-remoto?{query}",
                scene.owner.address
            )
            .into_client_request()
            .unwrap();
            for (key, value) in inputs["headers"].as_object().unwrap() {
                request.headers_mut().insert(
                    axum::http::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                    value.as_str().unwrap().parse().unwrap(),
                );
            }
            request.headers_mut().insert(
                "x-forwarded-for",
                inputs["client_ip"].as_str().unwrap().parse().unwrap(),
            );
            let result = tokio_tungstenite::connect_async(request).await;
            assert!(
                matches!(result,Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status()==403),
                "{name}: recusa prévia deve ser HTTP403: {result:?}"
            );
        }
    } else if name.starts_with("opening_") {
        if inputs["browser_source"] == "absent" {
            std::fs::remove_file(scene.nav_dir().join("browser.json")).unwrap();
        } else {
            let p = scene.nav_dir().join("_srv.json");
            let mut srv: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            srv["pid"] = json!(999);
            std::fs::write(p, srv.to_string()).unwrap();
        }
        let result = connect(&scene, "").await;
        assert!(
            matches!(result,Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status()==403),
            "{name}: {result:?}"
        );
        if let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = result {
            let body = response
                .body()
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert_eq!(
                body,
                support::contract("transport-supplement")["observed"]["preaccept_http"]["body_hex"]
                    .as_str()
                    .unwrap(),
                "recusa anterior ao aceite deve conservar o corpo vazio do uvicorn"
            );
        }
    } else {
        let query = inputs["query"].as_str().unwrap_or("");
        let result = connect(&scene, query).await;
        assert!(result.is_ok(), "{name}: handler WS ausente: {result:?}");
        let (mut client, _) = result.unwrap();
        let mut events = vec![];
        if inputs.get("capture_errors").is_some() {
            let count = if inputs["capture_errors"] == 0 { 2 } else { 1 };
            for _ in 0..count {
                until(&mut client, &mut events, |v| {
                    v["t"] == "q" && v["lento"] == true
                })
                .await;
            }
        } else {
            until(&mut client, &mut events, |v| {
                v["url"] == "http://contract.test/ready"
            })
            .await;
        }
        if let Some(reason) = inputs["detached_reason"].as_str() {
            desktop.detach(0, reason);
            until(&mut client, &mut events, |v| v["t"] == "erro").await;
        } else if name == "old_viewer_leaves_after_same_size_new_viewer" {
            client
                .send(ClientMessage::Text(
                    json!({"t":"layout","w":900,"h":700}).to_string().into(),
                ))
                .await
                .unwrap();
            barrier(&mut client, &mut events).await;
            let (mut second, _) = connect(&scene, "").await.unwrap();
            let mut events_b = vec![];
            until(&mut second, &mut events_b, |v| {
                v["url"] == "http://contract.test/ready"
            })
            .await;
            second
                .send(ClientMessage::Text(
                    json!({"t":"layout","w":900,"h":700}).to_string().into(),
                ))
                .await
                .unwrap();
            barrier(&mut second, &mut events_b).await;
            client.close(None).await.unwrap();
            while client.next().await.is_some() {}
            desktop.wait(|d| d.closed >= 1).await;
            let after = desktop.state.lock().unwrap().layout.clone();
            second.close(None).await.unwrap();
            while second.next().await.is_some() {}
            desktop
                .wait(|d| d.requests.len() >= expected["cmd"].as_array().unwrap().len())
                .await;
            scene.state.remote_browser.shutdown(&scene.nav_dir(), &scene.state.list).await;
            let mut actual = desktop.result(vec![], logs.0.lock().unwrap().clone());
            actual["events_by_viewer"] = json!([events, events_b]);
            actual["layout_after_old_viewer"] = json!(after);
            assert_eq!(actual, *expected, "{name}");
            scene.upstream.assert_no_browser_or_discovery_calls();
            desktop.server.close().await;
            scene.close().await;
            return;
        } else {
            if let Some(raw) = inputs["raw_messages"].as_array() {
                for raw in raw {
                    client
                        .send(ClientMessage::Text(raw.as_str().unwrap().into()))
                        .await
                        .unwrap();
                }
            }
            if let Some(messages) = inputs["messages"].as_array() {
                for message in messages {
                    client
                        .send(ClientMessage::Text(message.to_string().into()))
                        .await
                        .unwrap();
                }
            }
            if name.starts_with("invalid_modifier") || name.starts_with("invalid_wheel") {
                until(&mut client, &mut events, |v| v["t"] == "erro").await;
            } else if !name.starts_with("query_") {
                if !inputs["messages"]
                    .as_array()
                    .is_some_and(|m| m.last().is_some_and(|v| v["s"] == BARRIER))
                {
                    barrier(&mut client, &mut events).await;
                } else {
                    until(&mut client, &mut events, |v| {
                        v["url"] == "http://contract.test/barrier"
                    })
                    .await;
                }
                if let Some(change) = inputs.get("source_change") {
                    {
                        let mut d = desktop.state.lock().unwrap();
                        d.pid = change["new_desktop_pid"].as_u64().unwrap() as u32;
                        d.token = change["new_desktop_token"].as_str().unwrap().into();
                    }
                    desktop.publish(&scene, change["key"].as_str().unwrap());
                    desktop.detach(0, "target_closed");
                    until(&mut client, &mut events, |v| v["t"] == "erro").await;
                }
            }
        }
        let error = events.last().is_some_and(|v| v["t"] == "erro");
        let mut closed = None;
        if !error {
            client.close(None).await.unwrap();
        }
        while let Some(message) = client.next().await {
            if let Ok(ClientMessage::Close(frame)) = message {
                closed =
                    frame.map(|f| json!({"code":u16::from(f.code),"reason":f.reason.to_string()}));
            }
        }
        desktop
            .wait(|d| {
                d.closed >= 1 && d.requests.len() >= expected["cmd"].as_array().unwrap().len()
            })
            .await;
        if changes_d02 {
            desktop.assert_no_new_requests(expected["cmd"].as_array().unwrap()).await;
        }
        scene.state.remote_browser.shutdown(&scene.nav_dir(), &scene.state.list).await;
        let mut actual = desktop.result(events, logs.0.lock().unwrap().clone());
        if expected.get("close").is_some() {
            actual["close"] = closed.unwrap_or(json!({"code":1000,"reason":""}));
        }
        assert_eq!(actual, *expected, "{name}");
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}
macro_rules! frozen_tests {($($name:ident),* $(,)?)=>{$(#[tokio::test]#[allow(non_snake_case)]async fn $name(){frozen_case(stringify!($name)).await;})*};}
frozen_tests! {
mouse_center, mouse_release_clamped, mouse_move_modifiers, mouse_unknown_action, mouse_missing_position,
mouse_numeric_strings, mouse_invalid_position, wheel_fractional, wheel_defaults, text_unicode, text_empty,
text_null, text_number, text_limit_unicode, text_limit_combining, key_custom, key_empty, key_long_unicode,
unknown_message, missing_message_type, mouse_button_right, mouse_button_middle, mouse_button_none,
mouse_button_unknown, mouse_button_None, key_Enter, key_Tab, key_Escape, key_Backspace, key_Delete,
key_ArrowUp, key_ArrowDown, key_ArrowLeft, key_ArrowRight, key_Home, key_End, key_PageUp, key_PageDown,
invalid_json, json_array, json_null, json_number, json_string, message_at_character_limit,
message_over_character_limit, query_default, query_q_0, query_q_minus_1, query_q_invalid, query_q_1,
query_q_30, query_q_80, query_q_100, query_w_0, query_w_minus_1, query_w_invalid, query_w_390, query_w_1401,
layout_size, layout_size_minimum, layout_size_maximum, layout_size_zero, layout_size_above_limit,
layout_missing_height, layout_invalid_type, layout_mode_mobile, layout_mode_desktop, layout_mode_invalid,
layout_dimensions_beat_mode, layout_refused, layout_original_unavailable, layout_restore_refused,
detached_replaced_by_another_viewer, detached_target_closed, screenshots_continue, screenshot_error_is_silent,
cdp_input_error_is_silent, invalid_modifier_closes, invalid_wheel_delta_closes,
same_name_new_browser_receives_old_restore, same_key_new_desktop_receives_old_restore,
old_viewer_leaves_after_same_size_new_viewer, auth_missing, auth_wrong, auth_bearer_only, auth_cookie_only,
auth_owner_foreign_origin, auth_loopback_not_rate_limited, auth_blocked_before_correct_token,
auth_success_does_not_reset_ws_failures, opening_missing_browser, opening_wrong_desktop_pid,
}

#[tokio::test]
async fn arbitrary_external_sidecar_bytes_never_panic_or_consult_python() {
    if support::isolated(
        "remote_browser::transport::arbitrary_external_sidecar_bytes_never_panic_or_consult_python",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    use proptest::{
        prelude::*,
        test_runner::{Config, TestRunner},
    };
    let scene = Scene::new().await;
    seed_session(&scene);
    let mut runner = TestRunner::new(Config {
        cases: 32,
        ..Config::default()
    });
    let strategy = proptest::collection::vec(any::<u8>(), 0..256).prop_filter(
        "o ensaio de bytes não endereça um alvo de depuração existente",
        |sample| {
            !serde_json::from_slice::<Value>(sample).ok()
                .and_then(|value| value["chave"].as_str().map(str::to_owned))
                .is_some_and(|key| key == "nav-contract" || key.ends_with("::nav-contract"))
        },
    );
    let samples = std::cell::RefCell::new(vec![]);
    runner
        .run(&strategy, |sample| {
            samples.borrow_mut().push(sample);
            Ok(())
        })
        .unwrap();
    for sample in samples.into_inner() {
        std::fs::write(scene.nav_dir().join("external.json"), sample).unwrap();
        let response = connect(&scene, "").await;
        assert!(
            matches!(response,Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) if r.status()==403)
        );
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn overflowing_cdp_queue_keeps_latest_64_and_accepts_large_frames() {
    if support::isolated(
        "remote_browser::transport::overflowing_cdp_queue_keeps_latest_64_and_accepts_large_frames",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let commands = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = commands.clone();
    let desktop=Server::owner(Router::new().route("/cdp",get(move |ws:WebSocketUpgrade|{
        let seen=seen.clone();async move{ws.on_upgrade(move |mut ws|async move{
            while let Some(Ok(Message::Text(raw)))=ws.next().await{
                let value:Value=serde_json::from_str(&raw).unwrap();seen.lock().unwrap().push(value.clone());
                if value["method"]=="Page.enable"{
                    for index in 0..100{ws.send(Message::Text(json!({"method":"Page.frameNavigated","params":{"frame":{"url":format!("http://contract.test/{index}")}}}).to_string().into())).await.unwrap();}
                    let measured=support::contract("chromium");
                    ws.send(Message::Text(json!({"method":"Page.screencastFrame","params":measured["frame"],"transport_padding":"x".repeat(1024*1024+1)}).to_string().into())).await.unwrap();
                }
                if ws.send(Message::Text(json!({"id":value["id"],"result":{}}).to_string().into())).await.is_err(){break;}
            }
        })}
    }))).await;
    std::fs::write(
        scene.nav_dir().join("browser.json"),
        json!({"chave":KEY,"pid":123,"url":"http://contract.test/original"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        scene.nav_dir().join("_srv.json"),
        json!({"porta":desktop.address.port(),"pid":123,"token":DESKTOP_TOKEN}).to_string(),
    )
    .unwrap();
    let result = connect(&scene, "").await;
    assert!(result.is_ok(), "fila precisa da rota Rust");
    let (mut client, _) = result.unwrap();
    let mut events = vec![];
    assert_eq!(next_event(&mut client, &mut events).await["t"], "url");
    for index in 37..100 {
        assert_eq!(
            next_event(&mut client, &mut events).await,
            json!({"t":"url","url":format!("http://contract.test/{index}")})
        );
    }
    let frame = next_event(&mut client, &mut events).await;
    assert_eq!(
        frame,
        json!({"t":"q","d":support::contract("chromium")["frame"]["data"],"w":800,"h":600})
    );
    let reference = support::contract("transport-supplement");
    assert_eq!(
        events.len() - 1,
        reference["observed"]["queue_pressure"]["count"]
            .as_u64()
            .unwrap() as usize
    );
    assert_eq!(
        events[1]["url"],
        reference["observed"]["queue_pressure"]["first"]["params"]["frame"]["url"]
    );
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    desktop.close().await;
    assert!(
        commands
            .lock()
            .unwrap()
            .iter()
            .any(|v| v["method"] == "Page.screencastFrameAck")
    );
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn closed_cdp_ends_transport_after_screenshot_fallback() {
    if support::isolated(
        "remote_browser::transport::closed_cdp_ends_transport_after_screenshot_fallback",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Server::owner(Router::new().route(
        "/cdp",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut ws| async move {
                while let Some(Ok(Message::Text(raw))) = ws.next().await {
                    let value: Value = serde_json::from_str(&raw).unwrap();
                    ws.send(Message::Text(
                        json!({"id":value["id"],"result":{}}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                    if value["method"] == "Page.startScreencast" {
                        let _ = ws.send(Message::Close(None)).await;
                        break;
                    }
                }
            })
        }),
    ))
    .await;
    std::fs::write(
        scene.nav_dir().join("browser.json"),
        json!({"chave":KEY,"pid":123,"url":"http://contract.test/original"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        scene.nav_dir().join("_srv.json"),
        json!({"porta":desktop.address.port(),"pid":123,"token":DESKTOP_TOKEN}).to_string(),
    )
    .unwrap();
    let response = connect(&scene, "").await;
    assert!(
        response.is_ok(),
        "rota precisa aceitar o navegador existente"
    );
    let (mut client, _) = response.unwrap();
    let mut events = vec![];
    until(&mut client, &mut events, |v| v["t"] == "erro").await;
    assert_eq!(
        events.last().unwrap(),
        &json!({"t":"erro","m":support::contract("transport-supplement")["observed"]["closed_cdp"]["message"]})
    );
    while client.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.close().await;
    scene.close().await;
}

#[tokio::test]
async fn cdp_keepalive_closes_1011_after_20_seconds_without_pong() {
    if support::isolated(
        "remote_browser::transport::cdp_keepalive_closes_1011_after_20_seconds_without_pong",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let scene = Scene::new().await;
    seed_session(&scene);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ping_tx, ping_rx) = tokio::sync::oneshot::channel();
    let (close_tx, mut close_rx) = tokio::sync::oneshot::channel();
    let (armed_tx, armed_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            match message {
                ClientMessage::Text(raw) => {
                    let value: Value = serde_json::from_str(&raw).unwrap();
                    ws.send(ClientMessage::Text(
                        json!({"id":value["id"],"result":{}}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                    if value["method"] == "Page.startScreencast" {
                        ws.send(ClientMessage::Text(json!({"method":"Page.frameNavigated","params":{"frame":{"url":"http://contract.test/ready"}}}).to_string().into())).await.unwrap();
                    }
                }
                ClientMessage::Ping(_) => {
                    let _ = ping_tx.send(());
                    // Ler o TCP sem voltar ao WebSocket mantém o Pong automático sem envio.
                    let raw = ws.get_mut();
                    let mut armed_tx = Some(armed_tx);
                    loop {
                        let mut head = [0; 2];
                        if raw.read_exact(&mut head).await.is_err() {
                            break;
                        }
                        let mut length = (head[1] & 127) as usize;
                        if length == 126 {
                            length = raw.read_u16().await.unwrap() as usize;
                        } else if length == 127 {
                            length = raw.read_u64().await.unwrap() as usize;
                        }
                        let mut mask = [0; 4];
                        if head[1] & 128 != 0 {
                            raw.read_exact(&mut mask).await.unwrap();
                        }
                        let mut body = vec![0; length];
                        raw.read_exact(&mut body).await.unwrap();
                        if head[1] & 128 != 0 {
                            for (i, b) in body.iter_mut().enumerate() {
                                *b ^= mask[i % 4];
                            }
                        }
                        if head[0] & 15 == 1 {
                            let command: Value = serde_json::from_slice(&body).unwrap();
                            if command["method"] == "Input.insertText" {
                                if let Some(armed) = armed_tx.take() {
                                    let _ = armed.send(());
                                }
                            }
                        }
                        if head[0] & 15 == 8 {
                            let code = u16::from_be_bytes([body[0], body[1]]);
                            let reason = String::from_utf8(body[2..].to_vec()).unwrap();
                            let _ = close_tx.send((code, reason));
                            let mut reply = vec![0x88, body.len() as u8];
                            reply.extend_from_slice(&body);
                            let _ = raw.write_all(&reply).await;
                            break;
                        }
                    }
                    break;
                }
                _ => {}
            }
        }
    });
    std::fs::write(
        scene.nav_dir().join("browser.json"),
        json!({"chave":KEY,"pid":123,"url":"http://contract.test/original"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        scene.nav_dir().join("_srv.json"),
        json!({"porta":address.port(),"pid":123,"token":DESKTOP_TOKEN}).to_string(),
    )
    .unwrap();
    let response = connect(&scene, "").await;
    assert!(response.is_ok(), "rota precisa abrir o CDP");
    let (mut client, _) = response.unwrap();
    let mut events = vec![];
    until(&mut client, &mut events, |v| {
        v["url"] == "http://contract.test/ready"
    })
    .await;
    let keeper = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(20)).await;
    ping_rx.await.unwrap();
    client
        .send(ClientMessage::Text(
            json!({"t":"txt","s":"timing-barrier"}).to_string().into(),
        ))
        .await
        .unwrap();
    armed_rx.await.unwrap();
    tokio::time::advance(Duration::from_secs(19)).await;
    assert!(close_rx.try_recv().is_err(), "Pong expirou cedo");
    tokio::time::advance(Duration::from_millis(1001)).await;
    let mut wall = tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_secs(2)));
    let closed = tokio::select! {result=&mut close_rx=>result.ok(),_=&mut wall=>None};
    keeper.abort();
    let _ = keeper.await;
    tokio::time::resume();
    if !wall.is_finished() {
        let _ = wall.await;
    }
    let reference = support::contract("transport-supplement");
    let expected = &reference["observed"]["ping_timeout"];
    assert_eq!(
        closed,
        Some((
            expected["code"].as_u64().unwrap() as u16,
            expected["reason"].as_str().unwrap().into()
        )),
        "CDP não encerrou no prazo do Pong"
    );
    task.await.unwrap();
    crate::assert_closed(address, "desktop CDP sem Pong").await;
    let _ = client.close(None).await;
    while client.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn screenshot_wait_does_not_hold_unicode_input_or_external_controller() {
    if support::isolated("remote_browser::transport::screenshot_wait_does_not_hold_unicode_input_or_external_controller",|root|assert!(!root.join("git.calls").exists())).await{return;}
    let scene = Scene::new().await;
    seed_session(&scene);
    let (capture_tx, mut capture_rx) = mpsc::unbounded_channel();
    let (inserted_tx, mut inserted_rx) = mpsc::unbounded_channel();
    let controller_started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let started = controller_started.clone();
    let finished = release.clone();
    let desktop = Server::owner(
        Router::new()
            .route(
                "/cdp",
                get(move |ws: WebSocketUpgrade| {
                    let capture_tx = capture_tx.clone();
                    let inserted_tx = inserted_tx.clone();
                    async move {
                        ws.on_upgrade(move |mut ws| async move {
                            while let Some(Ok(Message::Text(raw))) = ws.next().await {
                                let value: Value = serde_json::from_str(&raw).unwrap();
                                if value["method"] == "Page.captureScreenshot" {
                                    capture_tx.send(()).unwrap();
                                    continue;
                                }
                                if ws
                                    .send(Message::Text(
                                        json!({"id":value["id"],"result":{}}).to_string().into(),
                                    ))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                if value["method"] == "Input.insertText" {
                                    inserted_tx.send(value["params"]["text"].clone()).unwrap();
                                }
                            }
                        })
                    }
                }),
            )
            .route(
                "/cmd",
                post(move || {
                    let started = started.clone();
                    let finished = finished.clone();
                    async move {
                        started.notify_one();
                        finished.notified().await;
                        "layout: desktop"
                    }
                }),
            ),
    )
    .await;
    std::fs::write(
        scene.nav_dir().join("browser.json"),
        json!({"chave":KEY,"pid":123,"url":"http://contract.test/original"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        scene.nav_dir().join("_srv.json"),
        json!({"porta":desktop.address.port(),"pid":123,"token":DESKTOP_TOKEN}).to_string(),
    )
    .unwrap();
    let response = connect(&scene, "").await;
    assert!(response.is_ok(), "rota precisa aceitar antes da captura");
    let (mut client, _) = response.unwrap();
    let mut events = vec![];
    next_event(&mut client, &mut events).await;
    tokio::time::timeout(Duration::from_secs(3), capture_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let address = desktop.address;
    let controller = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!("http://{address}/cmd"))
            .body("controle externo")
            .send()
            .await
            .unwrap()
    });
    controller_started.notified().await;
    client
        .send(ClientMessage::Text(
            json!({"t":"txt","s":"ação 😀"}).to_string().into(),
        ))
        .await
        .unwrap();
    let inserted = tokio::time::timeout(Duration::from_secs(1), inserted_rx.recv())
        .await
        .expect("captura ou controlador segurou o input")
        .unwrap();
    assert_eq!(
        inserted,
        support::contract("transport-supplement")["observed"]["concurrent_input"]["text"]
    );
    release.notify_one();
    controller.await.unwrap();
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    desktop.close().await;
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn pending_cdp_command_times_out_after_ten_seconds_and_closes() {
    if support::isolated(
        "remote_browser::transport::pending_cdp_command_times_out_after_ten_seconds_and_closes",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Server::owner(Router::new().route(
        "/cdp",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut ws| async move {
                while let Some(Ok(Message::Text(_))) = ws.next().await {}
            })
        }),
    ))
    .await;
    std::fs::write(
        scene.nav_dir().join("browser.json"),
        json!({"chave":KEY,"pid":123,"url":"http://contract.test/original"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        scene.nav_dir().join("_srv.json"),
        json!({"porta":desktop.address.port(),"pid":123,"token":DESKTOP_TOKEN}).to_string(),
    )
    .unwrap();
    let response = connect(&scene, "").await;
    assert!(
        response.is_ok(),
        "rota precisa aceitar antes do comando CDP"
    );
    let (mut client, _) = response.unwrap();
    let mut events = vec![];
    next_event(&mut client, &mut events).await;
    let began = std::time::Instant::now();
    let event = tokio::time::timeout(Duration::from_secs(12), client.next())
        .await
        .expect("comando CDP não respeitou dez segundos")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(event.to_text().unwrap()).unwrap(),
        json!({"t":"erro","m":support::contract("transport-supplement")["observed"]["command_timeout"]["message"]})
    );
    assert!(
        began.elapsed() >= Duration::from_millis(9900),
        "comando expirou cedo"
    );
    while client.next().await.is_some() {}
    desktop.close().await;
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn electron_target_precedes_native_and_queries_live_url() {
    if support::isolated(
        "remote_browser::transport::electron_target_precedes_native_and_queries_live_url",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let methods = Arc::new(Mutex::new(Vec::new()));
    let commands = methods.clone();
    let desktop = Server::at(
        Router::new().route(
            "/devtools/page/MEASURED",
            get(move |request: Request| {
                let commands = commands.clone();
                async move {
                    assert!(request.headers().get("authorization").is_none());
                    use axum::extract::FromRequestParts;
                    let (mut parts, _) = request.into_parts();
                    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &())
                        .await
                        .unwrap();
                    ws.on_upgrade(move |mut ws| async move {
                        while let Some(Ok(Message::Text(raw))) = ws.next().await {
                            let value: Value = serde_json::from_str(&raw).unwrap();
                            commands.lock().unwrap().push(value["method"].clone());
                            let result = if value["method"] == "Runtime.evaluate" {
                                json!({"result":{"value":"http://electron.test/current"}})
                            } else {
                                json!({})
                            };
                            if ws
                                .send(Message::Text(
                                    json!({"id":value["id"],"result":result}).to_string().into(),
                                ))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                }
            }),
        ),
        "127.0.0.1:9223".parse().unwrap(),
    )
    .await;
    std::fs::write(
        scene.nav_dir().join("native.json"),
        json!({"chave":KEY,"pid":123,"url":"http://native.test/"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        scene.nav_dir().join("electron.json"),
        json!({"chave":KEY,"targetId":"MEASURED","url":"http://electron.test/stale"}).to_string(),
    )
    .unwrap();
    let result = connect(&scene, "").await;
    assert!(
        result.is_ok(),
        "Electron deve vencer sem depender de _srv.json"
    );
    let (mut client, _) = result.unwrap();
    let mut events = vec![];
    assert_eq!(
        next_event(&mut client, &mut events).await,
        json!({"t":"url","url":"http://electron.test/current"})
    );
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    assert_eq!(
        methods.lock().unwrap().first(),
        Some(&json!("Runtime.evaluate"))
    );
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.close().await;
    scene.close().await;
}

#[tokio::test]
async fn private_bridge_requires_secret_and_keeps_trusted_target_after_opaque_query() {
    if support::isolated("remote_browser::transport::private_bridge_requires_secret_and_keeps_trusted_target_after_opaque_query",|root|assert!(!root.join("git.calls").exists())).await{return;}
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let private = Server::owner(hangar_server::routes::terminal_router(scene.state.clone())).await;
    for secret in [None, Some("wrong")] {
        let mut req = format!(
            "ws://{}/__hangar_server/remote-browser?name=nav-contract",
            private.address
        )
        .into_client_request()
        .unwrap();
        if let Some(secret) = secret {
            req.headers_mut()
                .insert("x-hangar-internal", secret.parse().unwrap());
        }
        let result = tokio_tungstenite::connect_async(req).await;
        assert!(
            matches!(result,Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) if r.status()==404)
        );
    }
    let mut request=format!("ws://{}/__hangar_server/remote-browser?name=other&w=390&q=80&token=opaque&name=nav-contract",private.address).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("x-hangar-internal", support::SECRET.parse().unwrap());
    let result = tokio_tungstenite::connect_async(request).await;
    assert!(
        result.is_ok(),
        "ponte privada deve abrir sem credencial do dono"
    );
    let (mut client, _) = result.unwrap();
    let mut events = vec![];
    until(&mut client, &mut events, |v| {
        v["url"] == "http://contract.test/ready"
    })
    .await;
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    {
        let d = desktop.state.lock().unwrap();
        let start = d
            .commands
            .iter()
            .find(|v| v["message"]["method"] == "Page.startScreencast")
            .unwrap();
        assert_eq!(start["message"]["params"]["quality"], 80);
        assert_eq!(start["message"]["params"]["maxWidth"], 390);
        assert_eq!(d.keys, vec![KEY]);
    }
    let mut external = format!(
        "ws://{}/__hangar_server/remote-browser?name=nav-contract",
        private.address
    )
    .into_client_request()
    .unwrap();
    external
        .headers_mut()
        .insert("x-hangar-internal", support::SECRET.parse().unwrap());
    external
        .headers_mut()
        .insert("x-forwarded-for", "198.51.100.2".parse().unwrap());
    let denied = tokio_tungstenite::connect_async(external).await;
    assert!(
        matches!(denied,Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) if r.status()==404)
    );
    scene.upstream.assert_no_browser_or_discovery_calls();
    private.close().await;
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn python_numeric_query_preserves_unbounded_integers_and_underscores() {
    if support::isolated(
        "remote_browser::transport::python_numeric_query_preserves_unbounded_integers_and_underscores",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let reference = support::contract("transport-supplement");
    for (index, case) in reference["observed"]["query_numbers"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let mut query = form_urlencoded::Serializer::new(String::new());
        for (key, value) in case["params"].as_object().unwrap() {
            query.append_pair(key, value.as_str().unwrap());
        }
        let result = connect(&scene, &query.finish()).await;
        assert!(result.is_ok());
        let (mut client, _) = result.unwrap();
        let mut events = vec![];
        until(&mut client, &mut events, |v| {
            v["url"] == "http://contract.test/ready"
        })
        .await;
        client.close(None).await.unwrap();
        while client.next().await.is_some() {}
        desktop.wait(|d| d.closed >= index + 1).await;
        let d = desktop.state.lock().unwrap();
        let start = d
            .commands
            .iter()
            .find(|v| v["peer"] == index && v["message"]["method"] == "Page.startScreencast")
            .unwrap();
        assert_eq!(
            start["message"]["params"]["quality"], case["quality"],
            "{}",
            case["name"]
        );
        let width = case["width"].as_i64().unwrap();
        assert_eq!(
            start["message"]["params"]["maxWidth"],
            if width == 0 { 1400 } else { width },
            "{}",
            case["name"]
        );
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn arbitrary_client_messages_keep_transport_recoverable_without_panicking() {
    if support::isolated(
        "remote_browser::transport::arbitrary_client_messages_keep_transport_recoverable_without_panicking",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    use proptest::test_runner::{Config, TestRunner};
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let samples = std::cell::RefCell::new(Vec::new());
    TestRunner::new(Config { cases: 24, ..Config::default() })
        .run(&(".{0,128}", proptest::sample::select(vec!["m", "w", "k", "txt", "layout", "unknown"])), |sample| {
            samples.borrow_mut().push(sample);
            Ok(())
        }).unwrap();
    for (text, kind) in samples.into_inner() {
        let (mut client, _) = connect(&scene, "").await.unwrap();
        let mut events = Vec::new();
        until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
        client.send(ClientMessage::Text(text.clone().into())).await.unwrap();
        let message = json!({"t": kind, "s": text, "key": text, "mod": text, "dy": text, "w": text, "h": text});
        client.send(ClientMessage::Text(message.to_string().into())).await.unwrap();
        let _ = client.send(ClientMessage::Text(json!({"t": "txt", "s": BARRIER}).to_string().into())).await;
        until(&mut client, &mut events, |value| value["t"] == "erro" || value["url"] == "http://contract.test/barrier").await;
        let _ = client.close(None).await;
        while client.next().await.is_some() {}
    }
    let (mut client, handshake) = connect(&scene, "").await.unwrap();
    assert_eq!(handshake.status(), 101, "entrada anterior não pode derrubar o servidor");
    let mut events = Vec::new();
    until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn arbitrary_cdp_metadata_never_panics_and_next_connection_still_opens() {
    if support::isolated(
        "remote_browser::transport::arbitrary_cdp_metadata_never_panics_and_next_connection_still_opens",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    use proptest::test_runner::{Config, TestRunner};
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let samples = std::cell::RefCell::new(Vec::new());
    TestRunner::new(Config { cases: 24, ..Config::default() })
        .run(&proptest::sample::select(vec![json!(null), json!(true), json!(false), json!(0), json!(-42), json!(1.25), json!("390"), json!("invalid"), json!([]), json!({})]), |sample| {
            samples.borrow_mut().push(sample);
            Ok(())
        }).unwrap();
    for width in samples.into_inner() {
        let (mut client, _) = connect(&scene, "").await.unwrap();
        let mut events = Vec::new();
        until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
        let mut frame = support::contract("chromium")["frame"].clone();
        frame["metadata"]["deviceWidth"] = width;
        let acks_before = desktop.state.lock().unwrap().commands.iter()
            .filter(|command| command["message"]["method"] == "Page.screencastFrameAck").count();
        desktop.state.lock().unwrap().peers.last().unwrap()
            .send(json!({"method": "Page.screencastFrame", "params": frame})).unwrap();
        desktop.wait(|state| state.commands.iter()
            .filter(|command| command["message"]["method"] == "Page.screencastFrameAck").count() > acks_before).await;
        let _ = client.send(ClientMessage::Text(json!({"t": "txt", "s": BARRIER}).to_string().into())).await;
        until(&mut client, &mut events, |value| value["t"] == "erro" || value["url"] == "http://contract.test/barrier").await;
        let _ = client.close(None).await;
        while client.next().await.is_some() {}
    }
    let (mut client, handshake) = connect(&scene, "").await.unwrap();
    assert_eq!(handshake.status(), 101);
    let mut events = Vec::new();
    until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn live_browser_repeats_frozen_auth_sequences_at_the_handshake() {
    if support::isolated(
        "remote_browser::transport::live_browser_repeats_frozen_auth_sequences_at_the_handshake",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let corpus = support::contract("transport");
    for case in corpus["cases"].as_array().unwrap().iter()
        .filter(|case| case["name"].as_str().unwrap().starts_with("auth_")) {
        let input = &case["inputs"];
        let ip = input["client_ip"].as_str().unwrap();
        scene.state.auth.clear_failures(ip);
        for (token, expected) in input["token_sequence"].as_array().unwrap().iter()
            .zip(case["expected"]["closes"].as_array().unwrap()) {
            let query = token.as_str().map(|value| {
                form_urlencoded::Serializer::new(String::new()).append_pair("token", value).finish()
            }).unwrap_or_default();
            let mut request = format!("ws://{}/api/sessions/nav-contract/nav-remoto?{query}", scene.owner.address)
                .into_client_request().unwrap();
            request.headers_mut().insert("x-forwarded-for", ip.parse().unwrap());
            for (key, value) in input["headers"].as_object().unwrap() {
                request.headers_mut().insert(
                    axum::http::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                    value.as_str().unwrap().parse().unwrap(),
                );
            }
            let result = tokio_tungstenite::connect_async(request).await;
            // Na referência, chegar à sessão ausente comprova que a credencial passou.
            let authorized = expected["reason"] == "sessao nao existe";
            assert_eq!(result.is_ok(), authorized, "{}: {expected}", case["name"]);
            match result {
                Ok((mut client, handshake)) => {
                    assert_eq!(handshake.status(), 101);
                    let mut events = Vec::new();
                    until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
                    client.close(None).await.unwrap();
                    while client.next().await.is_some() {}
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => assert_eq!(response.status(), 403),
                Err(error) => panic!("falha de transporte do ensaio: {error}"),
            }
        }
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn six_fallback_screenshots_pay_the_screencast_wait_only_once() {
    if support::isolated(
        "remote_browser::transport::six_fallback_screenshots_pay_the_screencast_wait_only_once",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({"capture_errors": 0})).await;
    desktop.publish(&scene, KEY);
    let reference = support::contract("transport-supplement");
    let expected = &reference["observed"]["screenshot_cadence"];
    let (mut client, _) = connect(&scene, "").await.unwrap();
    let mut events = Vec::new();
    until(&mut client, &mut events, |value| value["t"] == "q").await;
    assert_eq!(events.last().unwrap(), &expected["first_event"]);
    let began = std::time::Instant::now();
    for _ in 1..expected["frames"].as_u64().unwrap() {
        until(&mut client, &mut events, |value| value["t"] == "q").await;
        assert_eq!(events.last().unwrap(), &expected["first_event"]);
    }
    let limits = &reference["comparison"]["five_screenshot_intervals_seconds"];
    assert!((limits[0].as_f64().unwrap()..=limits[1].as_f64().unwrap()).contains(&began.elapsed().as_secs_f64()), "captura repetiu a espera pelo screencast ou perdeu a cadência");
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn python_numeric_separators_are_validated_before_input_dispatch() {
    if support::isolated(
        "remote_browser::transport::python_numeric_separators_are_validated_before_input_dispatch",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let reference = support::contract("transport-supplement");
    let mut differences = Vec::new();
    for (index, case) in reference["observed"]["input_numeric_separators"].as_array().unwrap().iter().enumerate() {
        let (mut client, _) = connect(&scene, "").await.unwrap();
        let mut events = Vec::new();
        until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
        client.send(ClientMessage::Text(case["message"].to_string().into())).await.unwrap();
        let _ = client.send(ClientMessage::Text(json!({"t": "txt", "s": BARRIER}).to_string().into())).await;
        until(&mut client, &mut events, |value| value["t"] == "erro" || value["url"] == "http://contract.test/barrier").await;
        let actual_error = events.last().filter(|value| value["t"] == "erro").map(|value| value["m"].clone());
        let expected_error = case.get("error").map(|error| error["message"].clone());
        let _ = client.close(None).await;
        while client.next().await.is_some() {}
        desktop.wait(|state| state.closed >= index + 1).await;
        let commands: Vec<Value> = desktop.state.lock().unwrap().commands.iter()
            .filter(|command| command["peer"] == index)
            .map(|command| &command["message"])
            .filter(|command| command["method"].as_str().unwrap().starts_with("Input.") && command["params"]["text"] != BARRIER)
            .map(|command| json!({"method": command["method"], "params": command["params"]})).collect();
        if actual_error != expected_error || commands != *case["commands"].as_array().unwrap() {
            differences.push(json!({"case": case["name"], "error": actual_error, "expected_error": expected_error,
                "commands": commands, "expected_commands": case["commands"]}));
        }
    }
    assert!(differences.is_empty(), "entrada divergente do Python original: {differences:?}");
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn native_without_controller_or_key_is_refused_before_accept() {
    if support::isolated(
        "remote_browser::transport::native_without_controller_or_key_is_refused_before_accept",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    for sidecar in [json!({"chave": KEY, "pid": 123}), json!({"chave": KEY}), json!({"pid": 123})] {
        std::fs::write(scene.nav_dir().join("native.json"), sidecar.to_string()).unwrap();
        let result = connect(&scene, "").await;
        assert!(matches!(result, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status() == 403));
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn later_user_layout_choice_is_preserved_on_disconnect() {
    if support::isolated(
        "remote_browser::transport::later_user_layout_choice_is_preserved_on_disconnect",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.state.lock().unwrap().layout = "desktop".into();
    desktop.publish(&scene, KEY);
    let (mut client, _) = connect(&scene, "").await.unwrap();
    let mut events = Vec::new();
    until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
    client.send(ClientMessage::Text(json!({"t": "layout", "w": 600, "h": 500}).to_string().into())).await.unwrap();
    barrier(&mut client, &mut events).await;
    desktop.state.lock().unwrap().layout = "mobile".into();
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    desktop.wait(|state| state.requests.len() >= 3).await;
    {
        let state = desktop.state.lock().unwrap();
        assert_eq!(state.layout, "mobile");
        let arguments: Vec<Value> = state.requests.iter().map(|request| request["body"]["args"].clone()).collect();
        assert_eq!(arguments, vec![json!([]), json!(["600", "500"]), json!([])]);
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn subframe_navigation_never_replaces_the_main_frame_url() {
    if support::isolated(
        "remote_browser::transport::subframe_navigation_never_replaces_the_main_frame_url",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let (mut client, _) = connect(&scene, "").await.unwrap();
    let mut events = Vec::new();
    until(&mut client, &mut events, |value| value["url"] == "http://contract.test/ready").await;
    let before = events.len();
    {
        let state = desktop.state.lock().unwrap();
        let peer = state.peers.last().unwrap();
        peer.send(json!({"method": "Page.frameNavigated", "params": {"frame": {"parentId": "main", "url": "http://contract.test/iframe"}}})).unwrap();
        peer.send(json!({"method": "Page.frameNavigated", "params": {"frame": {"url": "http://contract.test/top"}}})).unwrap();
    }
    until(&mut client, &mut events, |value| value["url"] == "http://contract.test/top").await;
    let reference = support::contract("transport-supplement");
    assert_eq!(&events[before..], reference["observed"]["subframe_urls"].as_array().unwrap());
    client.close(None).await.unwrap();
    while client.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

struct RaceController {
    address: std::net::SocketAddr,
    started: Arc<Notify>,
    release: Arc<Notify>,
    changed: Arc<Notify>,
    completed: Arc<std::sync::atomic::AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RaceController {
    async fn new(outcome: String) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let changed = Arc::new(Notify::new());
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (began, resume, updates, finished, seen) = (
            started.clone(), release.clone(), changed.clone(), completed.clone(), requests.clone(),
        );
        let task = tokio::spawn(async move {
            let mut armed = true;
            let mut current = "mobile".to_owned();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(stream.read_u8().await.unwrap());
                    assert!(headers.len() < 65536);
                }
                let length: usize = String::from_utf8(headers).unwrap().lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .unwrap().1.trim().parse().unwrap();
                let mut bytes = vec![0; length];
                stream.read_exact(&mut bytes).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                seen.lock().unwrap().push(body.clone());
                let arguments = body["args"].as_array().unwrap();
                let gated = armed && !arguments.is_empty();
                if gated {
                    armed = false;
                    began.notify_one();
                    resume.notified().await;
                    if outcome == "error" {
                        let _ = stream.shutdown().await;
                        finished.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        updates.notify_waiters();
                        continue;
                    }
                }
                let proposed = arguments.iter().map(|value| value.as_str().unwrap())
                    .collect::<Vec<_>>().join("x");
                let text = if gated && outcome == "refused" {
                    format!("erro: layout {proposed}: recusado")
                } else {
                    if !arguments.is_empty() {
                        current = proposed;
                    }
                    format!("layout: {current}")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{text}",
                    text.len(),
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                let _ = stream.shutdown().await;
                finished.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                updates.notify_waiters();
            }
        });
        Self { address, started, release, changed, completed, requests, task: Some(task) }
    }

    async fn wait_completed(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let changed = self.changed.notified();
                if self.completed.load(std::sync::atomic::Ordering::SeqCst) >= count {
                    break;
                }
                changed.await;
            }
        }).await.expect("controlador não concluiu os comandos esperados");
    }

    async fn close(mut self) {
        let task = self.task.take().unwrap();
        task.abort();
        let _ = task.await;
        crate::assert_closed(self.address, "controlador de layout do ensaio").await;
    }
}

impl Drop for RaceController {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
async fn layout_wait_only_overwrites_live_dimensions_after_confirmed_size() {
    if support::isolated(
        "remote_browser::transport::layout_wait_only_overwrites_live_dimensions_after_confirmed_size",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let reference = support::contract("transport-review-supplement");
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    for (index, case) in reference["observed"]["layout_races"].as_array().unwrap().iter().enumerate() {
        desktop.publish(&scene, KEY);
        let (mut client, _) = connect(&scene, "").await.unwrap();
        let mut events = Vec::new();
        until(&mut client, &mut events, |event| event["url"] == "http://contract.test/ready").await;
        let controller = RaceController::new(case["outcome"].as_str().unwrap().to_owned()).await;
        std::fs::write(scene.nav_dir().join("_srv.json"), json!({
            "porta": controller.address.port(), "token": DESKTOP_TOKEN, "pid": 123,
        }).to_string()).unwrap();
        client.send(ClientMessage::Text(case["input"].to_string().into())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), controller.started.notified()).await.unwrap();
        let before = desktop.state.lock().unwrap().commands.iter()
            .filter(|command| command["peer"] == index && command["message"]["method"] == "Page.screencastFrameAck")
            .count();
        let mut frame = support::contract("chromium")["frame"].clone();
        frame["metadata"]["deviceWidth"] = case["frame_dimensions"][0].clone();
        frame["metadata"]["deviceHeight"] = case["frame_dimensions"][1].clone();
        desktop.state.lock().unwrap().peers[index]
            .send(json!({"method": "Page.screencastFrame", "params": frame})).unwrap();
        desktop.wait(|state| state.commands.iter()
            .filter(|command| command["peer"] == index && command["message"]["method"] == "Page.screencastFrameAck")
            .count() > before).await;
        controller.release.notify_one();
        until(&mut client, &mut events, |event| event["t"] == "layout" || event["t"] == "erro").await;
        assert_eq!(events.last().unwrap()["t"], case["layout_event"]["t"]);
        client.send(ClientMessage::Text(case["mouse_input"].to_string().into())).await.unwrap();
        barrier(&mut client, &mut events).await;
        {
            let state = desktop.state.lock().unwrap();
            let mouse = state.commands.iter().find(|command|
                command["peer"] == index && command["message"]["method"] == "Input.dispatchMouseEvent"
            ).unwrap();
            assert_eq!(mouse["message"]["params"], case["mouse_params"], "{}", case["outcome"]);
        }
        until(&mut client, &mut events, |event| event["t"] == "q" && event["lento"] == true).await;
        assert_eq!(events.last().unwrap(), &case["screenshot_event"], "{}", case["outcome"]);
        client.close(None).await.unwrap();
        while client.next().await.is_some() {}
        controller.wait_completed(case["cmd"].as_array().unwrap().len()).await;
        assert_eq!(*controller.requests.lock().unwrap(), *case["cmd"].as_array().unwrap());
        controller.close().await;
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn plain_http_get_and_head_keep_the_previous_forwarding_without_discovery() {
    if support::isolated(
        "remote_browser::transport::plain_http_get_and_head_keep_the_previous_forwarding_without_discovery",
        |root| assert!(!root.join("git.calls").exists()),
    ).await {
        return;
    }
    let scene = Scene::new().await;
    let program = std::path::PathBuf::from(scene.state.list.env().capture_program.clone());
    assert!(program.starts_with(scene.root.path()));
    let original = std::fs::read(&program).unwrap();
    let probe = scene.root.path().join("unexpected-discovery.calls");
    std::fs::write(&program, format!("#!/bin/sh\n: > '{}'\nexit 2\n", probe.display())).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let path = format!("/api/sessions/headless/nav-remoto?token={OWNER}&w=390");
    for method in [axum::http::Method::GET, axum::http::Method::HEAD] {
        let response = client.request(method.clone(), format!("http://{}{path}", scene.owner.address))
            .send().await.unwrap();
        assert_eq!(response.status(), 503, "o status deve vir do upstream anterior");
        let body = response.bytes().await.unwrap();
        if method == axum::http::Method::GET {
            assert_eq!(body.as_ref(), b"remote_browser_test_upstream_refused");
        } else {
            assert!(body.is_empty());
        }
    }
    assert!(!probe.exists(), "HTTP comum entrou na descoberta do navegador");
    assert_eq!(scene.upstream.paths(), vec![path.clone(), path]);
    std::fs::write(program, original).unwrap();
    scene.close().await;
}
