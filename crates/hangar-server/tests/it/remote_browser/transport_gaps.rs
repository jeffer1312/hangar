#![cfg(unix)]
use super::support::{self, OWNER, Scene};
use super::transport::{Client, KEY, seed_session};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "remote-browser-contract-desktop";
const BARRIER: &str = "__transport_gap_barrier__";

#[derive(Default)]
struct Seen {
    cdp: Vec<Value>,
    cmd: Vec<Value>,
    keys: Vec<String>,
    cdp_closes: Vec<Value>,
    peers: Vec<mpsc::UnboundedSender<String>>,
    closed: usize,
    commands_done: usize,
    close_started: bool,
    layout: String,
}

#[derive(Clone)]
struct Context {
    seen: Arc<Mutex<Seen>>,
    changed: Arc<Notify>,
    release: Arc<Notify>,
    inputs: Value,
    measured: Arc<Value>,
}

struct Desktop {
    address: SocketAddr,
    command_address: SocketAddr,
    context: Context,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Desktop {
    async fn new(inputs: &Value) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let commands = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let command_address = commands.local_addr().unwrap();
        let context = Context {
            seen: Arc::new(Mutex::new(Seen { layout: inputs["original"].as_str().unwrap_or("mobile").into(), ..Seen::default() })),
            changed: Arc::default(), release: Arc::default(), inputs: inputs.clone(),
            measured: Arc::new(support::contract("chromium")),
        };
        let serving = context.clone();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let (ending, _) = watch::channel(false);
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    incoming = listener.accept() => {
                        let (stream, _) = incoming.unwrap();
                        let context = serving.clone();
                        let closing = ending.subscribe();
                        handlers.spawn(async move {
                            let mut first = [0];
                            assert_eq!(stream.peek(&mut first).await.unwrap(), 1);
                            if first[0] == b'G' {
                                cdp(stream, context, closing).await;
                            } else {
                                command(stream, context, closing).await;
                            }
                        });
                    }
                    incoming = commands.accept() => {
                        let (stream, _) = incoming.unwrap();
                        handlers.spawn(command(stream, serving.clone(), ending.subscribe()));
                    }
                    result = handlers.join_next(), if !handlers.is_empty() => { result.unwrap().unwrap(); }
                }
            }
            let _ = ending.send(true);
            while let Some(result) = handlers.join_next().await { result.unwrap(); }
        });
        Self { address, command_address, context, stop: Some(stop), task: Some(task) }
    }

    fn publish(&self, scene: &Scene) {
        std::fs::write(scene.nav_dir().join("browser.json"), json!({"chave": KEY, "pid": 123,
            "targetId": null, "url": "http://contract.test/original"}).to_string()).unwrap();
        std::fs::write(scene.nav_dir().join("_srv.json"), json!({"porta": self.address.port(),
            "pid": 123, "token": TOKEN}).to_string()).unwrap();
    }

    async fn wait(&self, predicate: impl Fn(&Seen) -> bool) {
        tokio::time::timeout(Duration::from_secs(55), async {
            loop {
                let changed = self.context.changed.notified();
                if predicate(&self.context.seen.lock().unwrap()) { break; }
                changed.await;
            }
        }).await.expect("a borda externa não recebeu o efeito esperado");
    }

    fn emit(&self, peer: usize, value: Value) {
        self.emit_raw(peer, value.to_string());
    }

    fn emit_raw(&self, peer: usize, raw: String) {
        self.context.seen.lock().unwrap().peers[peer].send(raw).unwrap();
    }

    fn frame(&self, peer: usize, session: u64, width: Value, height: Value) {
        let mut frame = self.context.measured["frame"].clone();
        frame["sessionId"] = json!(session);
        frame["metadata"]["deviceWidth"] = width;
        frame["metadata"]["deviceHeight"] = height;
        self.emit(peer, json!({"method": "Page.screencastFrame", "params": frame}));
    }

    fn result(&self, events: &[Vec<Value>], logs: &Logs, viewer_closes: &[Value]) -> Value {
        let seen = self.context.seen.lock().unwrap();
        json!({"events_by_viewer": events, "cdp": seen.cdp, "cmd": seen.cmd,
            "desktop_layout": seen.layout, "cdp_keys": seen.keys, "log": *logs.0.lock().unwrap(),
            "cdp_close": seen.cdp_closes, "viewer_close": viewer_closes})
    }

    async fn close(mut self) {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(5), self.task.take().unwrap()).await
            .expect("tarefas do desktop de ensaio não terminaram").unwrap();
        crate::assert_closed(self.address, "desktop do complemento de transporte").await;
        crate::assert_closed(self.command_address, "controlador do complemento de transporte").await;
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() { task.abort(); }
    }
}

async fn cdp(stream: tokio::net::TcpStream, context: Context, mut closing: watch::Receiver<bool>) {
    let accepted = context.clone();
    let mut socket = tokio_tungstenite::accept_hdr_async(stream, move |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
        assert_eq!(request.headers()["authorization"].to_str().unwrap(), format!("Bearer {TOKEN}"));
        assert_eq!(request.uri().path(), "/cdp");
        let key = form_urlencoded::parse(request.uri().query().unwrap().as_bytes())
            .find(|(key, _)| key == "chave").unwrap().1.into_owned();
        assert_eq!(key, KEY);
        accepted.seen.lock().unwrap().keys.push(key);
        Ok(response)
    }).await.unwrap();
    let (sender, mut incoming) = mpsc::unbounded_channel();
    let peer = {
        let mut seen = context.seen.lock().unwrap();
        let peer = seen.peers.len();
        seen.peers.push(sender);
        seen.cdp_closes.push(json!({"code": 1006, "reason": ""}));
        peer
    };
    loop {
        let message = tokio::select! {
            _ = closing.changed() => { let _ = socket.close(None).await; break; }
            outgoing = incoming.recv() => {
                if socket.send(Message::Text(outgoing.unwrap().into())).await.is_err() { break; }
                continue;
            }
            message = socket.next() => message,
        };
        let Some(Ok(message)) = message else { break; };
        match message {
            Message::Text(raw) => {
                let message = hangar_server::remote_browser::value::decode(&raw).expect("JSON CDP aceito pelo Python");
                context.seen.lock().unwrap().cdp.push(json!({"peer": peer, "message": message}));
                context.changed.notify_waiters();
                let method = message["method"].as_str().unwrap();
                if method == "Page.captureScreenshot" && context.inputs["hold_capture"] == true { continue; }
                let result = if method == "Page.captureScreenshot" { context.measured["screenshot"].clone() } else { json!({}) };
                let mut response = json!({"id": message["id"], "result": result});
                if method == "Page.enable" {
                    if let Some(size) = context.inputs["response_bytes"].as_u64() {
                        response["padding"] = json!("");
                        let padding = size as usize - response.to_string().len();
                        response["padding"] = json!("x".repeat(padding));
                        assert_eq!(response.to_string().len(), size as usize);
                    }
                }
                if context.inputs["response_bytes"].as_u64().is_some_and(|size| size > 32 * 1024 * 1024) {
                    let (mut outgoing, mut incoming) = socket.split();
                    let sending = outgoing.send(Message::Text(response.to_string().into()));
                    tokio::pin!(sending);
                    let sent = tokio::select! {
                        result = &mut sending => {
                            if result.is_err() { false } else {
                                while let Some(Ok(message)) = incoming.next().await {
                                    if let Message::Close(frame) = message {
                                        context.seen.lock().unwrap().cdp_closes[peer] = close_value(frame.as_ref());
                                        break;
                                    }
                                }
                                true
                            }
                        }
                        message = incoming.next() => {
                            if let Some(Ok(Message::Close(frame))) = message {
                                context.seen.lock().unwrap().cdp_closes[peer] = close_value(frame.as_ref());
                            }
                            false
                        }
                        _ = closing.changed() => false,
                    };
                    let _ = sent;
                    context.seen.lock().unwrap().closed += 1;
                    context.changed.notify_waiters();
                    return;
                }
                if socket.send(Message::Text(response.to_string().into())).await.is_err() { break; }
                let url = if method == "Page.startScreencast" { Some("http://contract.test/ready") }
                    else if method == "Input.insertText" && message["params"]["text"] == BARRIER { Some("http://contract.test/barrier") }
                    else { None };
                if let Some(url) = url {
                    if socket.send(Message::Text(json!({"method": "Page.frameNavigated", "params": {"frame": {"url": url}}}).to_string().into())).await.is_err() { break; }
                }
            }
            Message::Close(frame) => {
                context.seen.lock().unwrap().cdp_closes[peer] = close_value(frame.as_ref());
                if context.inputs["hold_close"] == true {
                    context.seen.lock().unwrap().close_started = true;
                    context.changed.notify_waiters();
                    tokio::select! { _ = context.release.notified() => {}, _ = closing.changed() => {} }
                }
                let _ = socket.flush().await;
                break;
            }
            Message::Ping(_) => { if socket.flush().await.is_err() { break; } }
            _ => {}
        }
    }
    context.seen.lock().unwrap().closed += 1;
    context.changed.notify_waiters();
}

async fn command(mut stream: tokio::net::TcpStream, context: Context, mut closing: watch::Receiver<bool>) {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        header.push(stream.read_u8().await.unwrap());
        assert!(header.len() <= 65536);
    }
    let header = String::from_utf8(header).unwrap();
    assert!(header.starts_with("POST /cmd HTTP/1.1\r\n"));
    let field = |key: &str| header.lines().filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case(key)).unwrap().1.trim().to_owned();
    let mut body = vec![0; field("content-length").parse().unwrap()];
    stream.read_exact(&mut body).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let number = {
        let mut seen = context.seen.lock().unwrap();
        seen.cmd.push(json!({"authorization": field("authorization"), "body": body, "receiver_pid": 123}));
        seen.cmd.len() as u64
    };
    context.changed.notify_waiters();
    if context.inputs["gate_at"].as_u64() == Some(number) {
        if context.inputs["apply_before_gate"] == true {
            context.seen.lock().unwrap().layout = body["args"].as_array().unwrap().iter()
                .map(|value| value.as_str().unwrap()).collect::<Vec<_>>().join("x");
        }
        tokio::select! { _ = context.release.notified() => {}, _ = closing.changed() => return }
    }
    if context.inputs["disconnect_at"].as_u64() != Some(number) {
        if context.inputs["timeout_at"].as_u64() == Some(number) {
            let mut byte = [0];
            tokio::select! { _ = stream.read(&mut byte) => {}, _ = closing.changed() => {} }
        } else {
            let text = {
                let mut seen = context.seen.lock().unwrap();
                let args = body["args"].as_array().unwrap();
                if !args.is_empty() { seen.layout = args.iter().map(|value| value.as_str().unwrap()).collect::<Vec<_>>().join("x"); }
                format!("layout: {}", seen.layout)
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{text}", text.len());
            let delivered = stream.write_all(response.as_bytes()).await;
            assert!(delivered.is_ok() || context.inputs["detach_pending"] == true, "o controlador não entregou a resposta: {delivered:?}");
        }
    }
    let _ = stream.shutdown().await;
    context.seen.lock().unwrap().commands_done += 1;
    context.changed.notify_waiters();
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<Value>>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Logs {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "hangar.navsock" { return; }
        struct Visitor(String);
        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" { self.0 = format!("{value:?}"); }
            }
        }
        let mut visitor = Visitor(String::new());
        event.record(&mut visitor);
        let level = if *event.metadata().level() == tracing::Level::WARN { "WARNING" } else { event.metadata().level().as_str() };
        self.0.lock().unwrap().push(json!({"level": level, "message": visitor.0}));
    }
}

async fn receive(client: &mut Client, events: &mut Vec<Value>, predicate: impl Fn(&Value) -> bool) {
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            let message = client.next().await.expect("WS terminou antes da resposta").unwrap();
            if let Message::Text(raw) = message {
                let event: Value = serde_json::from_str(&raw).unwrap();
                let done = predicate(&event);
                events.push(event);
                if done { break; }
            }
        }
    }).await.expect("o WS não publicou a resposta esperada");
}

async fn send(client: &mut Client, message: Value) {
    client.send(Message::Text(message.to_string().into())).await.unwrap();
}

fn close_value(frame: Option<&tokio_tungstenite::tungstenite::protocol::CloseFrame>) -> Value {
    frame.map_or_else(|| json!({"code": 1005, "reason": ""}), |frame|
        json!({"code": u16::from(frame.code), "reason": frame.reason.as_str()}))
}

async fn close_viewer(client: &mut Client) {
    let _ = client.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
        code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
        reason: "".into(),
    })).await;
}

async fn barrier(client: &mut Client, events: &mut Vec<Value>) {
    send(client, json!({"t": "txt", "s": BARRIER})).await;
    receive(client, events, |event| event["url"] == "http://contract.test/barrier" || event["t"] == "erro").await;
}

async fn open(scene: &Scene, inputs: &Value, desktop: &Desktop) -> (Client, Vec<Value>) {
    std::fs::write(scene.nav_dir().join("_srv.json"), json!({"porta": desktop.address.port(), "pid": 123, "token": TOKEN}).to_string()).unwrap();
    let (mut client, handshake) = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/nav-contract/nav-remoto?token={OWNER}&{}", scene.owner.address,
        inputs["query"].as_str().unwrap_or(""))).await.unwrap();
    assert_eq!(handshake.status(), 101);
    let mut events = vec![];
    receive(&mut client, &mut events, |event| event["url"] == "http://contract.test/ready" || event["t"] == "erro").await;
    std::fs::write(scene.nav_dir().join("_srv.json"), json!({"porta": desktop.command_address.port(), "pid": 123, "token": TOKEN}).to_string()).unwrap();
    (client, events)
}

fn encoded(value: &Value) -> Value {
    if let Some(number) = hangar_server::remote_browser::value::nonfinite(value) {
        return json!({"kind": "float", "bits": format!("{:016x}", number.to_bits())});
    }
    match value {
        Value::Null => json!({"kind": "null"}),
        Value::Bool(value) => json!({"kind": "bool", "value": value}),
        Value::String(value) => json!({"kind": "string", "points": hangar_server::remote_browser::value::codepoints(value)}),
        Value::Number(value) => {
            let text = value.to_string();
            if text.contains('.') || text.contains('e') || text.contains('E') {
                json!({"kind": "float", "bits": format!("{:016x}", value.as_f64().unwrap().to_bits())})
            } else { json!({"kind": "integer", "decimal": text}) }
        }
        Value::Array(values) => json!({"kind": "array", "items": values.iter().map(encoded).collect::<Vec<_>>()}),
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
                json!({"kind": "object", "entries": keys.into_iter().map(|key| vec![encoded(&json!(key)), encoded(values.get(key.as_str()).unwrap())]).collect::<Vec<_>>()})
        }
    }
}

async fn frozen_case(name: &str) {
    if support::isolated_with_timeout(&format!("remote_browser::transport_gaps::{name}"), Duration::from_secs(90),
        |root| assert!(!root.join("git.calls").exists(), "a descoberta iniciou Git")).await { return; }
    use tracing_subscriber::prelude::*;
    let logs = Logs::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(logs.clone())).unwrap();
    let reference = support::contract("transport-gaps");
    assert_eq!(reference["source_commit"], "ba2db1021ab7fb801eca50a50f9509dcb825ac7e");
    let case = reference["public"].as_array().unwrap().iter().find(|case| case["name"] == name).unwrap();
    let inputs = &case["inputs"];
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(inputs).await;
    desktop.publish(&scene);
    let (client, initial) = open(&scene, inputs, &desktop).await;
    let mut clients = vec![client];
    let mut events = vec![initial];
    let mut observations = json!({});
    match name {
        "initial_dimensions_before_frame" => {
            send(&mut clients[0], json!({"t": "m", "acao": "press", "x": 0.5, "y": 0.5})).await;
            barrier(&mut clients[0], &mut events[0]).await;
        }
        "ack_rate_and_false_metadata" => {
            let began = Instant::now();
            desktop.frame(0, 501, json!(1000), json!(500));
            receive(&mut clients[0], &mut events[0], |event| event["t"] == "q").await;
            for session in 502..510 { desktop.frame(0, session, json!(0), json!(false)); }
            desktop.wait(|seen| seen.cdp.iter().filter(|row| row["message"]["method"] == "Page.screencastFrameAck").count() == 9).await;
            assert!(began.elapsed().as_secs_f64() < 1.0 / 24.0, "rajada saiu da janela real de observação; não concluir taxa por um ensaio lento");
            tokio::time::sleep_until(tokio::time::Instant::from_std(began + Duration::from_millis(80))).await;
            desktop.frame(0, 510, json!(0), json!(false));
            desktop.wait(|seen| seen.cdp.iter().filter(|row| row["message"]["method"] == "Page.screencastFrameAck").count() == 10).await;
            assert!((0.079..1.0 / 12.0).contains(&began.elapsed().as_secs_f64()), "a sonda anterior ao limite saiu da janela real de observação");
            tokio::time::sleep_until(tokio::time::Instant::from_std(began + Duration::from_millis(110))).await;
            desktop.frame(0, 511, json!(0), json!(false));
            receive(&mut clients[0], &mut events[0], |event| event["t"] == "q").await;
            assert!((0.1..=0.2).contains(&began.elapsed().as_secs_f64()), "a sonda posterior ao limite saiu da janela real de observação");
            send(&mut clients[0], json!({"t": "m", "acao": "press", "x": 0.5, "y": 0.5})).await;
            barrier(&mut clients[0], &mut events[0]).await;
        }
        name if name.starts_with("screencast_resets_") => {
            receive(&mut clients[0], &mut events[0], |event| event["lento"] == true).await;
            desktop.frame(0, 601, json!(1000), json!(500));
            receive(&mut clients[0], &mut events[0], |event| event["t"] == "q" && event.get("lento").is_none()).await;
            let began = Instant::now();
            receive(&mut clients[0], &mut events[0], |event| event["lento"] == true).await;
            let limits = &reference["comparison"]["after_screencast_seconds"];
            assert!((limits[0].as_f64().unwrap()..=limits[1].as_f64().unwrap()).contains(&began.elapsed().as_secs_f64()), "frame não repôs a espera do screencast");
        }
        "serialized_viewers_apply_and_restore" | "pending_capture_and_rust_layout_keep_other_viewer_input_independent" => {
            let serialized = name == "serialized_viewers_apply_and_restore";
            if !serialized {
                desktop.wait(|seen| seen.cdp.iter().filter(|row| row["message"]["method"] == "Page.captureScreenshot").count() == 1).await;
            }
            let (client, initial) = open(&scene, inputs, &desktop).await;
            clients.push(client); events.push(initial);
            if !serialized {
                desktop.wait(|seen| seen.cdp.iter().filter(|row| row["message"]["method"] == "Page.captureScreenshot").count() == 2).await;
            }
            send(&mut clients[0], if serialized { json!({"t": "layout", "w": 900, "h": 700}) }
                else { json!({"t": "layout", "modo": "desktop"}) }).await;
            desktop.wait(|seen| seen.cmd.len() == if serialized { 2 } else { 1 }).await;
            if serialized {
                send(&mut clients[1], json!({"t": "layout", "w": 1200, "h": 900})).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                observations["requests_while_first_apply_held"] = json!(desktop.context.seen.lock().unwrap().cmd);
                desktop.context.release.notify_one();
                for peer in 0..2 { receive(&mut clients[peer], &mut events[peer], |event| event["t"] == "layout").await; }
            } else {
                send(&mut clients[0], json!({"t": "txt", "s": "same-reader-after-layout"})).await;
                send(&mut clients[1], json!({"t": "txt", "s": "outro espectador 😀"})).await;
                send(&mut clients[1], json!({"t": "txt", "s": BARRIER})).await;
                desktop.wait(|seen| seen.cdp.iter().any(|row| row["peer"] == 1 && row["message"]["method"] == "Input.insertText" && row["message"]["params"]["text"] == BARRIER)).await;
                {
                    let seen = desktop.context.seen.lock().unwrap();
                    observations["cdp_while_capture_and_layout_held"] = json!(seen.cdp);
                    observations["requests_while_layout_held"] = json!(seen.cmd);
                }
                desktop.context.release.notify_one();
                receive(&mut clients[0], &mut events[0], |event| event["t"] == "layout").await;
                send(&mut clients[0], json!({"t": "txt", "s": BARRIER})).await;
                desktop.wait(|seen| seen.cdp.iter().any(|row| row["peer"] == 0 && row["message"]["method"] == "Input.insertText" && row["message"]["params"]["text"] == BARRIER)).await;
            }
        }
        _ if inputs.get("raw").is_some() => {
            clients[0].send(Message::Text(inputs["raw"].as_str().unwrap().to_owned().into())).await.unwrap();
            barrier(&mut clients[0], &mut events[0]).await;
        }
        _ if inputs.get("raw_metadata").is_some() => {
            let mut frame = desktop.context.measured["frame"].clone();
            frame["metadata"] = json!("__RAW_METADATA__");
            let raw = json!({"method": "Page.screencastFrame", "params": frame}).to_string()
                .replace("\"__RAW_METADATA__\"", inputs["raw_metadata"].as_str().unwrap());
            desktop.emit_raw(0, raw);
            receive(&mut clients[0], &mut events[0], |event| event["t"] == "q" || event["t"] == "erro").await;
        }
        _ if inputs.get("response_bytes").is_some() => {}
        _ if inputs["detach_pending"] == true => {
            send(&mut clients[0], json!({"t": "layout", "w": 900, "h": 700})).await;
            desktop.wait(|seen| seen.cmd.len() == 2).await;
            desktop.emit(0, json!({"method": "Inspector.detached", "params": {"reason": "target_closed"}}));
            receive(&mut clients[0], &mut events[0], |event| event["t"] == "erro").await;
            desktop.wait(|seen| seen.closed == 1).await;
            scene.state.remote_browser.shutdown(&scene.nav_dir(), &scene.state.list).await;
            {
                let seen = desktop.context.seen.lock().unwrap();
                observations["layout_after_handler_ended_before_apply_release"] = json!(seen.layout);
                observations["requests_after_handler_ended_before_apply_release"] = json!(seen.cmd);
                observations["unregistered_application"] = json!(seen.cmd.len() == 2);
            }
            desktop.context.release.notify_one();
        }
        _ => {
            send(&mut clients[0], if inputs["mode"] == true { json!({"t": "layout", "modo": "desktop"}) }
                else { json!({"t": "layout", "w": 900, "h": 700}) }).await;
            receive(&mut clients[0], &mut events[0], |event| event["t"] == "layout" || event["t"] == "erro").await;
            if name == "registered_restore_serializes_with_next_viewer_apply" {
                let (client, initial) = open(&scene, inputs, &desktop).await;
                clients.push(client); events.push(initial);
                close_viewer(&mut clients[0]).await;
                desktop.wait(|seen| seen.cmd.len() == 4).await;
                send(&mut clients[1], json!({"t": "layout", "w": 1200, "h": 900})).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                observations["requests_while_registered_restore_held"] = json!(desktop.context.seen.lock().unwrap().cmd);
                desktop.context.release.notify_one();
                receive(&mut clients[1], &mut events[1], |event| event["t"] == "layout").await;
            }
            if name == "restore_query_failure_allows_a_fresh_original_on_reacquisition" {
                close_viewer(&mut clients[0]).await;
                desktop.wait(|seen| seen.cmd.len() == 3 && seen.commands_done == 3).await;
                tokio::time::timeout(Duration::from_secs(4), async {
                    loop {
                        if logs.0.lock().unwrap().iter().any(|line| line["level"] == "WARNING") { break; }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }).await.expect("a restauração falhada não publicou seu diagnóstico");
                observations["requests_after_restore_failure"] = json!(desktop.context.seen.lock().unwrap().cmd);
                let (client, initial) = open(&scene, inputs, &desktop).await;
                clients.push(client); events.push(initial);
                send(&mut clients[1], json!({"t": "layout", "w": 1200, "h": 900})).await;
                receive(&mut clients[1], &mut events[1], |event| event["t"] == "layout").await;
            }
            if name == "failed_error_delivery_still_restores_registered_layout" {
                send(&mut clients[0], json!({"t": "k", "key": "Enter", "mod": "invalid"})).await;
                desktop.wait(|seen| seen.close_started).await;
                let client = clients.pop().unwrap();
                let tokio_tungstenite::MaybeTlsStream::Plain(stream) = client.get_ref() else { panic!("a prova usa somente TCP de loopback") };
                socket2::SockRef::from(stream).set_linger(Some(Duration::ZERO)).unwrap();
                drop(client);
                observations["viewer_reset_before_cdp_close_release"] = json!(true);
                desktop.context.release.notify_one();
            }
        }
    }
    let mut viewer_closes = vec![json!({"code": 1006, "reason": ""}); events.len()];
    for client in &mut clients { close_viewer(client).await; }
    for (peer, client) in clients.iter_mut().enumerate() {
        while let Some(message) = client.next().await {
            match message {
                Ok(Message::Text(raw)) => events[peer].push(serde_json::from_str(&raw).unwrap()),
                Ok(Message::Close(frame)) => viewer_closes[peer] = close_value(frame.as_ref()),
                Ok(_) => {},
                Err(error) => panic!("fechamento externo falhou: {error}"),
            }
        }
    }
    if name == "registered_restore_continues_after_viewer_exit" {
        desktop.wait(|seen| seen.cmd.len() == 4).await;
        {
            let seen = desktop.context.seen.lock().unwrap();
            observations["layout_while_registered_restore_held"] = json!(seen.layout);
        }
        let directory = scene.nav_dir();
        let lifecycle = scene.state.remote_browser.shutdown(&directory, &scene.state.list);
        tokio::pin!(lifecycle);
        let finished = tokio::select! {
            () = &mut lifecycle => true,
            _ = tokio::time::sleep(Duration::from_millis(250)) => false,
        };
        observations["lifecycle_finished_before_restore_release"] = json!(finished);
        desktop.context.release.notify_one();
        if !finished {
            tokio::time::timeout(Duration::from_secs(5), lifecycle).await
                .expect("a limpeza do navegador não terminou depois da devolução");
        }
        observations["lifecycle_finished_after_restore_release"] = json!(true);
    }
    let expected_commands = case["expected"]["cmd"].as_array().map_or(0, Vec::len);
    desktop.wait(|seen| seen.closed == events.len() && seen.commands_done == expected_commands).await;
    scene.state.remote_browser.shutdown(&scene.nav_dir(), &scene.state.list).await;
    let mut actual = desktop.result(&events, &logs, &viewer_closes);
    if inputs["encoded"] == true { assert_eq!(encoded(&actual), case["expected_encoded"], "{name}: efeitos integrais divergentes do Python original"); }
    else {
        let mut expected = case["expected"].clone();
        for result in [&mut actual, &mut expected] {
            for row in result["log"].as_array_mut().unwrap() {
                let message = row["message"].as_str().unwrap();
                if let Some((prefix, cause)) = message.split_once(": layout de antes não voltou: ")
                    && let Some(cause) = cause.strip_prefix("ShellIndisponivel('").and_then(|s| s.strip_suffix("')"))
                {
                    row["message"] = json!(format!("{prefix}: layout de antes não voltou: {cause}"));
                }
            }
        }
        assert_eq!(actual, expected, "{name}: efeitos integrais divergentes do Python original");
    }
    let mut expected_observations = case["observations"].clone();
    if inputs["encoded"] == true {
        assert_eq!(encoded(&observations), expected_observations, "{name}: observação integral divergente");
    } else {
        for measured in ["burst_seconds", "after_screencast_seconds", "before_rate_boundary_seconds", "after_rate_boundary_seconds"] { expected_observations.as_object_mut().unwrap().remove(measured); }
        assert_eq!(observations, expected_observations, "{name}: houve efeito enquanto a borda ainda estava pendente");
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.close().await;
    scene.close().await;
}

macro_rules! cases { ($($name:ident),* $(,)?) => { $(#[tokio::test] async fn $name() { frozen_case(stringify!($name)).await; })* }; }
cases! {
    initial_dimensions_before_frame, ack_rate_and_false_metadata,
    screencast_resets_fallback_wait_q30, screencast_resets_fallback_wait_q45,
    numeric_original_restore, layout_original_disconnect, layout_apply_disconnect,
    layout_mode_disconnect, layout_restore_disconnect, layout_restore_timeout,
    failed_error_delivery_still_restores_registered_layout,
    restore_query_failure_allows_a_fresh_original_on_reacquisition,
    registered_restore_continues_after_viewer_exit,
    registered_restore_serializes_with_next_viewer_apply,
    detached_while_apply_pending_preserves_unregistered_application,
    serialized_viewers_apply_and_restore,
    pending_capture_and_rust_layout_keep_other_viewer_input_independent,
    cdp_response_exactly_32mib, cdp_response_over_32mib,
    special_x_nan, special_x_infinity, special_x_negative_infinity, special_x_huge, special_x_surrogate,
    special_y_nan, special_y_infinity, special_y_negative_infinity, special_y_huge, special_y_surrogate,
    special_mod_nan, special_mod_infinity, special_mod_negative_infinity, special_mod_huge, special_mod_surrogate,
    special_key_mod_nan, special_key_mod_infinity, special_key_mod_negative_infinity, special_key_mod_huge, special_key_mod_surrogate,
    special_dx_nan, special_dx_infinity, special_dx_negative_infinity, special_dx_huge, special_dx_surrogate,
    special_dy_nan, special_dy_infinity, special_dy_negative_infinity, special_dy_huge, special_dy_surrogate,
    special_code_nan, special_code_infinity, special_code_negative_infinity, special_code_huge, special_code_surrogate,
    special_w_nan, special_w_infinity, special_w_negative_infinity, special_w_huge, special_w_surrogate,
    special_h_nan, special_h_infinity, special_h_negative_infinity, special_h_huge, special_h_surrogate,
    special_s_nan, special_s_infinity, special_s_negative_infinity, special_s_huge, special_s_surrogate,
    special_key_nan, special_key_infinity, special_key_negative_infinity, special_key_huge, special_key_surrogate,
    special_t_nan, special_t_infinity, special_t_negative_infinity, special_t_huge, special_t_surrogate,
    special_action_nan, special_action_infinity, special_action_negative_infinity, special_action_huge, special_action_surrogate,
    special_button_nan, special_button_infinity, special_button_negative_infinity, special_button_huge, special_button_surrogate,
    special_mode_nan, special_mode_infinity, special_mode_negative_infinity, special_mode_huge, special_mode_surrogate,
    metadata_nan, metadata_infinity, metadata_negative_infinity, metadata_huge, metadata_surrogate,
    metadata_height_nan, metadata_height_infinity, metadata_height_negative_infinity, metadata_height_huge, metadata_height_surrogate,
    only_height_selects_dimensions_before_mode, fractional_pixels_are_truncated,
    numeric_key_is_stringified_before_dispatch,
}

#[tokio::test]
async fn slow_mux_discovery_does_not_block_another_connection() {
    let exact = "remote_browser::transport_gaps::slow_mux_discovery_does_not_block_another_connection";
    if support::isolated(exact, |root| assert!(!root.join("git.calls").exists())).await { return; }
    let reference = support::contract("transport-gaps");
    let scene = Scene::new().await;
    let program = std::path::PathBuf::from(scene.state.list.env().capture_program.clone());
    let original = std::fs::read(&program).unwrap();
    let delegated = scene.root.path().join("mux-original");
    std::fs::rename(&program, &delegated).unwrap();
    let entered = scene.root.path().join("mux-entered");
    let release = scene.root.path().join("mux-release");
    crate::write_executable(&program, format!("#!/bin/sh\nif [ \"$1\" = list-panes ]; then\n  : > '{}'\n  while [ ! -e '{}' ]; do /bin/sleep 0.01; done\nfi\nexec '{}' \"$@\"\n", entered.display(), release.display(), delegated.display()));
    let address = scene.owner.address;
    let pending = tokio::spawn(async move {
        tokio_tungstenite::connect_async(format!("ws://{address}/api/sessions/missing/nav-remoto?token={OWNER}")).await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.expect("o multiplexador de ensaio não entrou na espera");
    let response = tokio::time::timeout(Duration::from_secs(1), reqwest::Client::builder().no_proxy().build().unwrap()
        .get(format!("http://{}/api/sessions/headless/navegador?token={OWNER}", scene.owner.address)).send()).await
        .expect("a descoberta bloqueou a outra conexão").unwrap();
    let urls = support::contract("url-commands");
    let expected = &urls["cases"].as_array().unwrap().iter().find(|case| case["name"] == "get_empty_directory").unwrap()["expected"]["responses"][0];
    assert_eq!(response.status().as_u16(), expected["status"].as_u64().unwrap() as u16);
    assert_eq!(response.json::<Value>().await.unwrap(), expected["body"]);
    assert_eq!(reference["slow_discovery"]["other_connection_before_release"], json!({"alive": true}));
    assert_eq!(json!(!pending.is_finished()), reference["slow_discovery"]["opening_still_pending"]);
    std::fs::write(&release, "release").unwrap();
    let result = pending.await.unwrap();
    let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = result else { panic!("sessão ausente abriu") };
    assert_eq!(response.status().as_u16(), reference["slow_discovery"]["opening_status"].as_u64().unwrap() as u16);
    std::fs::write(&program, original).unwrap();
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}
