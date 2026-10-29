//! Tela remota do navegador no celular, servida pelo app nativo: o backend (`navsock.py`) abre `GET /cdp` como
//! abriria a porta de depuração do Electron, e aqui só passa a lista fechada de comandos que ele usa. Sem porta de
//! depuração, nada além disso chega ao WebView2.
use std::{sync::atomic::{AtomicU64, Ordering}, time::Duration};

use futures::channel::oneshot;
use serde_json::{Value, json};
use tokio::{io::{AsyncRead, AsyncWrite}, net::TcpStream};

use super::server::{Request, respond};
use crate::ws::{self, Error};

/// O que o `navsock` manda. Fica de fora o que lê a página (`Runtime.evaluate`) e a emulação, que é do controlador.
pub const RELAYED: [&str; 8] = [
    "Page.enable",
    "Page.startScreencast",
    "Page.stopScreencast",
    "Page.screencastFrameAck",
    "Page.captureScreenshot",
    "Input.dispatchMouseEvent",
    "Input.insertText",
    "Input.dispatchKeyEvent",
];
/// Eventos que o espectador recebe.
pub const WATCHED: [&str; 2] = ["Page.screencastFrame", "Page.frameNavigated"];
/// Quadros esperando a escrita no socket; o mais velho sai quando chega um novo.
const QUEUE: usize = 4;
/// O `Page.captureScreenshot` do `navsock` espera 20 s; a resposta daqui não pode desistir antes dele.
const REPLY_SECS: u64 = 25;
/// `reason` do `Inspector.detached` quando outro aparelho assume; o `navsock` o traduz para a tela.
pub const REPLACED: &str = "replaced_by_another_viewer";
const CLOSED: &str = "target_closed";

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub enum ViewerEvent {
    /// Evento CDP pronto para o socket; `frame` é o `sessionId` quando é um quadro do screencast.
    Cdp { text: String, frame: Option<i64> },
    /// Fim da tela remota com o motivo.
    Detached(&'static str),
}

/// Quem está olhando a tela remota de um navegador. Só um por vez: o WebView2 tem uma sessão CDP só.
pub struct Viewer {
    pub id: u64,
    pub events: async_channel::Sender<ViewerEvent>,
}

impl Viewer {
    /// Entrega um evento. Devolve o `sessionId` do quadro que ficou sem entrega (o descartado, ou o próprio sem
    /// ninguém para ler): ele precisa de ack aqui, senão o Chromium para de mandar quadros esperando.
    pub fn deliver(&self, method: &str, params: &Value) -> Option<i64> { deliver(&self.events, method, params) }

    /// Avisa o motivo e solta o canal; a tarefa do socket fecha depois de escrever o aviso.
    pub fn detach(self, reason: &'static str) { let _ = self.events.force_send(ViewerEvent::Detached(reason)); }
}

/// `Viewer::deliver` para quem só tem o canal (a thread do pipe no Linux).
pub fn deliver(events: &async_channel::Sender<ViewerEvent>, method: &str, params: &Value) -> Option<i64> {
    let frame = frame_id(method, params);
    let text = json!({"method": method, "params": params}).to_string();
    match events.force_send(ViewerEvent::Cdp { text, frame }) {
        Ok(Some(ViewerEvent::Cdp { frame, .. })) => frame,
        Ok(_) => None,
        Err(_) => frame,
    }
}

/// `sessionId` de um quadro do screencast; `None` para os outros eventos.
pub fn frame_id(method: &str, params: &Value) -> Option<i64> {
    (method == "Page.screencastFrame").then(|| params["sessionId"].as_i64()).flatten()
}

fn detached(reason: &str) -> String { json!({"method": "Inspector.detached", "params": {"reason": reason}}).to_string() }

/// `GET /cdp` com token já conferido: registra o espectador, completa o upgrade e repassa até alguém fechar.
pub async fn serve(mut stream: TcpStream, ws_key: &str, key: String, requests: async_channel::Sender<Request>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (events, received) = async_channel::bounded(QUEUE);
    let (tx, rx) = oneshot::channel();
    if requests.send(Request::Watch { key: key.clone(), viewer: Viewer { id, events }, reply: tx }).await.is_err() {
        return respond(&mut stream, 500, "erro: o app nativo esta fechando").await;
    }
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(text))) => return respond(&mut stream, 404, &text).await,
        _ => return respond(&mut stream, 500, "erro: o app nao respondeu").await,
    }
    let accept = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        ws::accept_key(ws_key));
    if tokio::io::AsyncWriteExt::write_all(&mut stream, accept.as_bytes()).await.is_ok()
        && let Err(e) = pump(&mut stream, &key, received, &requests).await {
        eprintln!("[nav] tela remota de {key} caiu: {e}");
    }
    let _ = requests.send(Request::Unwatch { key, id }).await;
}

/// Laço do socket: comandos do backend para a interface, respostas e eventos de volta.
async fn pump(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), key: &str, events: async_channel::Receiver<ViewerEvent>,
    requests: &async_channel::Sender<Request>) -> Result<(), Error> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (answer, answers) = async_channel::unbounded::<String>();
    let mut message = ws::Message::default();
    loop {
        let frame = {
            // A leitura parcial sobrevive enquanto saem quadros e respostas.
            let next = ws::read_client_frame(&mut reader);
            tokio::pin!(next);
            loop {
                tokio::select! {
                    frame = &mut next => break frame?,
                    event = events.recv() => {
                        let reason = match event {
                            Ok(ViewerEvent::Cdp { text, .. }) => { ws::write_server_frame(&mut writer, 1, text.as_bytes()).await?; continue; }
                            Ok(ViewerEvent::Detached(reason)) => reason,
                            // O painel caiu sem avisar (o app está saindo).
                            Err(_) => CLOSED,
                        };
                        ws::write_server_frame(&mut writer, 1, detached(reason).as_bytes()).await?;
                        return ws::write_server_frame(&mut writer, 8, &1000u16.to_be_bytes()).await;
                    }
                    Ok(text) = answers.recv() => ws::write_server_frame(&mut writer, 1, text.as_bytes()).await?,
                }
            }
        };
        match frame.opcode {
            8 => { let _ = ws::write_server_frame(&mut writer, 8, &frame.data).await; return Ok(()); }
            9 => ws::write_server_frame(&mut writer, 10, &frame.data).await?,
            10 => {}
            _ => if let Some(bytes) = message.push(frame)?
                && let Some(refused) = call(&bytes, key, requests, &answer).await {
                ws::write_server_frame(&mut writer, 1, refused.as_bytes()).await?;
            },
        }
    }
}

/// Manda o comando à interface sem esperar a resposta, que chega pela fila `answer`: um print lento não pode
/// atrasar o toque seguinte. Devolve a resposta na hora quando o comando é recusado aqui.
async fn call(bytes: &[u8], key: &str, requests: &async_channel::Sender<Request>, answer: &async_channel::Sender<String>) -> Option<String> {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else { return None };
    let id = v["id"].clone();
    let error = |message: String| Some(json!({"id": id, "error": {"code": -32601, "message": message}}).to_string());
    let method = v["method"].as_str().unwrap_or_default().to_owned();
    if !RELAYED.contains(&method.as_str()) { return error(format!("{method} fora do repasse do app nativo")); }
    let params = if v["params"].is_object() { v["params"].clone() } else { json!({}) };
    let (tx, rx) = oneshot::channel();
    if requests.send(Request::Cdp { key: key.to_owned(), method, params, reply: tx }).await.is_err() {
        return error("o app nativo esta fechando".into());
    }
    let answer = answer.clone();
    tokio::spawn(async move {
        let reply = match tokio::time::timeout(Duration::from_secs(REPLY_SECS), rx).await {
            Ok(Ok(Ok(result))) => json!({"id": id, "result": result}),
            Ok(Ok(Err(message))) => json!({"id": id, "error": {"code": -32000, "message": message}}),
            _ => json!({"id": id, "error": {"code": -32000, "message": "o navegador nao respondeu"}}),
        };
        let _ = answer.send(reply.to_string()).await;
    });
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Quadro de texto como o cliente manda: mascarado (máscara zero, o que mantém os bytes legíveis no teste).
    fn client_text(text: &str) -> Vec<u8> {
        assert!(text.len() <= u16::MAX as usize);
        let mut wire = vec![0x81];
        if text.len() < 126 { wire.push(0x80 | text.len() as u8); }
        else { wire.push(0xfe); wire.extend_from_slice(&(text.len() as u16).to_be_bytes()); }
        wire.extend_from_slice(&[0, 0, 0, 0]);
        wire.extend_from_slice(text.as_bytes());
        wire
    }

    async fn server_text(socket: &mut (impl AsyncRead + Unpin)) -> (u8, Value) {
        let mut head = [0; 2];
        socket.read_exact(&mut head).await.unwrap();
        assert_eq!(head[1] & 128, 0, "servidor nunca mascara");
        let len = match head[1] & 127 { 126 => usize::from(socket.read_u16().await.unwrap()), n => usize::from(n) };
        let mut data = vec![0; len];
        socket.read_exact(&mut data).await.unwrap();
        (head[0] & 15, serde_json::from_slice(&data).unwrap_or(Value::Null))
    }

    #[test]
    fn displaced_frame_is_handed_back_for_ack() {
        let (events, received) = async_channel::bounded(1);
        let viewer = Viewer { id: 1, events };
        assert_eq!(viewer.deliver("Page.screencastFrame", &json!({"sessionId": 7})), None);
        // Fila cheia: o quadro 7 sai, e quem chamou confirma o 7.
        assert_eq!(viewer.deliver("Page.screencastFrame", &json!({"sessionId": 8})), Some(7));
        assert_eq!(viewer.deliver("Page.frameNavigated", &json!({"frame": {}})), Some(8));
        drop(received);
        assert_eq!(viewer.deliver("Page.screencastFrame", &json!({"sessionId": 9})), Some(9));
        assert_eq!(viewer.deliver("Page.frameNavigated", &json!({})), None);
    }

    #[tokio::test]
    async fn relays_allowed_refuses_others_and_closes_with_reason() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let (requests, ui) = async_channel::unbounded();
        let (events, received) = async_channel::bounded(QUEUE);
        let viewer = Viewer { id: 1, events };
        let relay = tokio::spawn(async move { pump(&mut server, "s::a", received, &requests).await });

        client.write_all(&client_text(r#"{"id":1,"method":"Runtime.evaluate","params":{}}"#)).await.unwrap();
        let (_, refused) = server_text(&mut client).await;
        assert_eq!(refused["id"], 1);
        assert!(refused["error"]["message"].as_str().unwrap().contains("Runtime.evaluate"));

        client.write_all(&client_text(r#"{"id":2,"method":"Page.enable"}"#)).await.unwrap();
        let Ok(Request::Cdp { key, method, params, reply }) = ui.recv().await else { panic!("esperava Cdp") };
        assert_eq!((key.as_str(), method.as_str(), params), ("s::a", "Page.enable", json!({})));
        reply.send(Ok(json!({}))).unwrap();
        assert_eq!(server_text(&mut client).await.1, json!({"id": 2, "result": {}}));

        assert_eq!(viewer.deliver("Page.screencastFrame", &json!({"sessionId": 3, "data": "x"})), None);
        let (_, frame) = server_text(&mut client).await;
        assert_eq!((frame["method"].as_str(), frame["params"]["sessionId"].as_i64()), (Some("Page.screencastFrame"), Some(3)));

        viewer.detach(REPLACED);
        let (_, gone) = server_text(&mut client).await;
        assert_eq!(gone, json!({"method": "Inspector.detached", "params": {"reason": REPLACED}}));
        assert_eq!(server_text(&mut client).await.0, 8);
        assert!(relay.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn dropped_viewer_reads_as_closed_target() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let (requests, _ui) = async_channel::unbounded();
        let (events, received) = async_channel::bounded::<ViewerEvent>(QUEUE);
        let relay = tokio::spawn(async move { pump(&mut server, "s::a", received, &requests).await });
        drop(events);
        assert_eq!(server_text(&mut client).await.1["params"]["reason"], CLOSED);
        assert_eq!(server_text(&mut client).await.0, 8);
        assert!(relay.await.unwrap().is_ok());
    }

    async fn bounded_text(client: &mut (impl AsyncRead + Unpin)) -> (u8, Value) {
        tokio::time::timeout(Duration::from_secs(2), server_text(client)).await.expect("o repasse precisa responder")
    }

    #[tokio::test]
    async fn every_remote_command_crosses_the_pump_and_other_methods_do_not() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let (requests, ui) = async_channel::unbounded();
        let (events, received) = async_channel::bounded(QUEUE);
        let relay = tokio::spawn(async move { pump(&mut server, "proof::native", received, &requests).await });
        let allowed = [
            ("Page.enable", json!({})),
            ("Page.startScreencast", json!({"format": "jpeg", "quality": 45, "maxWidth": 390, "maxHeight": 390, "everyNthFrame": 5})),
            ("Page.stopScreencast", json!({})),
            ("Page.screencastFrameAck", json!({"sessionId": 1})),
            ("Page.captureScreenshot", json!({"format": "jpeg", "quality": 55})),
            ("Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": 400, "y": 300, "button": "left", "clickCount": 1})),
            ("Input.insertText", json!({"text": "ação 🎯"})),
            ("Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "Backspace", "code": "Backspace", "windowsVirtualKeyCode": 8, "modifiers": 0})),
        ];
        for (index, (method, params)) in allowed.into_iter().enumerate() {
            let id = index + 1;
            client.write_all(&client_text(&json!({"id": id, "method": method, "params": params}).to_string())).await.unwrap();
            let Request::Cdp { key, method: received, params: payload, reply } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
                else { panic!("esperava comando CDP") };
            assert_eq!((key.as_str(), received.as_str(), payload), ("proof::native", method, params));
            reply.send(Ok(json!({"accepted": id}))).unwrap();
            assert_eq!(bounded_text(&mut client).await.1, json!({"id": id, "result": {"accepted": id}}));
        }
        for method in ["Runtime.evaluate", "Emulation.setDeviceMetricsOverride", "Page.navigate", "Browser.close", "Target.attachToTarget", ""] {
            client.write_all(&client_text(&json!({"id": 99, "method": method}).to_string())).await.unwrap();
            let refused = bounded_text(&mut client).await.1;
            assert_eq!(refused, json!({"id": 99, "error": {"code": -32601, "message": format!("{method} fora do repasse do app nativo")}}));
            assert!(ui.is_empty());
        }
        let chromium: Value = serde_json::from_str(include_str!("../../../backend/tests/fixtures/contract/remote-browser/chromium.json")).unwrap();
        for (method, params) in [
            ("Page.screencastFrame", chromium["frame"].clone()),
            ("Page.frameNavigated", json!({"frame": {"id": "main", "url": "http://example.test/"}})),
        ] {
            assert_eq!(deliver(&events, method, &params), None);
            assert_eq!(bounded_text(&mut client).await.1, json!({"method": method, "params": params}));
        }
        drop(events);
        assert_eq!(bounded_text(&mut client).await.1, json!({"method": "Inspector.detached", "params": {"reason": "target_closed"}}));
        assert_eq!(bounded_text(&mut client).await.0, 8);
        assert!(tokio::time::timeout(Duration::from_secs(2), relay).await.unwrap().unwrap().is_ok());
    }

    #[tokio::test]
    async fn pending_screenshot_does_not_hold_mouse_unicode_or_key_responses() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let (requests, ui) = async_channel::unbounded();
        let (events, received) = async_channel::bounded(QUEUE);
        let relay = tokio::spawn(async move { pump(&mut server, "proof::native", received, &requests).await });
        client.write_all(&client_text(r#"{"id":1,"method":"Page.captureScreenshot"}"#)).await.unwrap();
        let Request::Cdp { reply: screenshot, .. } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
            else { panic!("esperava captura") };
        for (id, method, params) in [
            (2, "Input.dispatchMouseEvent", json!({"type": "mouseReleased", "button": "left", "x": 800, "y": 0})),
            (3, "Input.insertText", json!({"text": "ação 🎯"})),
            (4, "Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "Enter", "windowsVirtualKeyCode": 13, "modifiers": 2})),
        ] {
            client.write_all(&client_text(&json!({"id": id, "method": method, "params": params}).to_string())).await.unwrap();
            let Request::Cdp { method: received, params: payload, reply, .. } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
                else { panic!("esperava input enquanto o print aguarda") };
            assert_eq!((received.as_str(), payload), (method, params));
            reply.send(Ok(json!({}))).unwrap();
            assert_eq!(bounded_text(&mut client).await.1, json!({"id": id, "result": {}}));
        }
        screenshot.send(Ok(json!({"data": "completed-later"}))).unwrap();
        assert_eq!(bounded_text(&mut client).await.1, json!({"id": 1, "result": {"data": "completed-later"}}));
        drop(events);
        assert_eq!(bounded_text(&mut client).await.1["params"]["reason"], "target_closed");
        assert_eq!(bounded_text(&mut client).await.0, 8);
        assert!(tokio::time::timeout(Duration::from_secs(2), relay).await.unwrap().unwrap().is_ok());
    }

    #[tokio::test]
    async fn screenshot_response_waits_twenty_five_seconds_and_drops_the_expired_reply() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let (requests, ui) = async_channel::unbounded();
        let (events, received) = async_channel::bounded(QUEUE);
        let relay = tokio::spawn(async move { pump(&mut server, "proof::native", received, &requests).await });
        let since = std::time::Instant::now();
        client.write_all(&client_text(r#"{"id":1,"method":"Page.captureScreenshot"}"#)).await.unwrap();
        let Request::Cdp { reply, .. } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
            else { panic!("esperava captura") };
        let response = tokio::time::timeout(Duration::from_secs(27), server_text(&mut client)).await.unwrap().1;
        assert!(since.elapsed() >= Duration::from_secs(25));
        assert_eq!(response, json!({"id": 1, "error": {"code": -32000, "message": "o navegador nao respondeu"}}));
        assert!(reply.send(Ok(json!({}))).is_err());
        drop(events);
        assert_eq!(bounded_text(&mut client).await.1["params"]["reason"], "target_closed");
        assert_eq!(bounded_text(&mut client).await.0, 8);
        assert!(tokio::time::timeout(Duration::from_secs(2), relay).await.unwrap().unwrap().is_ok());
    }
}
