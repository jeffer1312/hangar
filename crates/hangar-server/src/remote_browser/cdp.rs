//! Uma bomba CDP associa respostas por ID sem parar o fluxo de eventos.
use super::ws::Failure;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Failure>>>>>;
#[derive(Default)]
struct Events {
    queue: Mutex<VecDeque<Value>>,
    ready: Notify,
}
impl Events {
    fn push(&self, value: Value) {
        let mut q = self.queue.lock().unwrap();
        if q.len() == 64 {
            q.pop_front();
        }
        q.push_back(value);
        drop(q);
        self.ready.notify_one();
    }
    async fn next(&self) -> Value {
        loop {
            let ready = self.ready.notified();
            if let Some(value) = self.queue.lock().unwrap().pop_front() {
                return value;
            }
            ready.await;
        }
    }
}
enum Out {
    Command(Value),
    Close,
}
pub(super) struct Cdp {
    next: AtomicU64,
    send: mpsc::UnboundedSender<Out>,
    pending: Pending,
    events: Arc<Events>,
    task: Option<JoinHandle<()>>,
}
struct Waiting {
    pending: Pending,
    id: u64,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}
struct PumpEnd(Pending);
impl Drop for PumpEnd {
    fn drop(&mut self) {
        for (_, reply) in self.0.lock().unwrap().drain() {
            let _ = reply.send(Err(Failure::connection("CDP fechou")));
        }
    }
}
async fn closing(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    frame: Option<CloseFrame>,
) {
    let _ = socket.close(frame).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    })
    .await;
}
impl Cdp {
    pub(super) async fn connect(url: &str, token: Option<&str>) -> Result<Self, Failure> {
        let mut request = url
            .into_client_request()
            .map_err(|_| Failure::connection("o navegador desta sessão fechou"))?;
        if let Some(token) = token {
            let mut header = format!("Bearer {token}")
                .parse::<axum::http::HeaderValue>()
                .map_err(|_| Failure::plain("credencial do navegador inválida"))?;
            header.set_sensitive(true);
            request.headers_mut().insert("authorization", header);
        }
        let cfg = WebSocketConfig::default()
            .max_message_size(Some(32 * 1024 * 1024))
            .max_frame_size(Some(32 * 1024 * 1024));
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async_with_config(request, Some(cfg), false),
        )
        .await
        .map_err(|_| Failure::exception("TimeoutError", "timed out during opening handshake".into()))?
        .map_err(|error| opening_failure(error, url))?;
        let (send, mut receive) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let waiters = pending.clone();
        let events = Arc::new(Events::default());
        let output = events.clone();
        let task = tokio::spawn(async move {
            let _end = PumpEnd(waiters.clone());
            let mut next_ping = tokio::time::Instant::now() + Duration::from_secs(20);
            let mut pong: Option<(Vec<u8>, tokio::time::Instant)> = None;
            let mut ping_id = 0u64;
            loop {
                tokio::select! {
                    outgoing = receive.recv() => match outgoing {
                        Some(Out::Command(value)) => {
                            if socket.send(Message::Text(super::value::encode(&value).into())).await.is_err() {
                                break;
                            }
                        }
                        _ => {
                            closing(&mut socket, Some(CloseFrame {
                                code: CloseCode::Normal,
                                reason: "".into(),
                            })).await;
                            break;
                        }
                    },
                    incoming = socket.next() => {
                        match incoming {
                            Some(Ok(Message::Text(raw))) => {
                                let Some(value) = super::value::decode(&raw) else {
                                    break;
                                };
                                if let Some(id) = value.get("id").filter(|v| !v.is_null()) {
                                    if let Some(id) = id.as_u64() {
                                        if let Some(reply) = waiters.lock().unwrap().remove(&id) {
                                            let _ = reply.send(Ok(value));
                                        }
                                    }
                                } else if value.get("method").is_some_and(super::input::truth) {
                                    output.push(value);
                                }
                            }
                            Some(Ok(Message::Ping(bytes))) => {
                                if socket.send(Message::Pong(bytes)).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Pong(bytes))) => {
                                if pong.as_ref().is_some_and(|(sent, _)| sent.as_slice() == bytes.as_ref()) {
                                    pong = None;
                                }
                            }
                            Some(Ok(Message::Close(_))) => {
                                let _ = socket.flush().await;
                                break;
                            }
                            Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(
                                tokio_tungstenite::tungstenite::error::CapacityError::MessageTooLong { size, max_size }
                            ))) => {
                                drop(PumpEnd(waiters.clone()));
                                let _ = socket.close(Some(CloseFrame {
                                    code: CloseCode::Size,
                                    reason: format!("frame with {size} bytes exceeds limit of {max_size} bytes").into(),
                                })).await;
                                // O Stream termina no erro; drenar o transporte evita perder o Close num reset TCP.
                                let _ = tokio::time::timeout(Duration::from_secs(10), async {
                                    use tokio::io::AsyncReadExt;
                                    let mut discarded = [0; 8192];
                                    while socket.get_mut().read(&mut discarded).await.is_ok_and(|count| count > 0) {}
                                }).await;
                                break;
                            }
                            None | Some(Err(_)) => break,
                            _ => {}
                        }
                    },
                    _ = tokio::time::sleep_until(pong.as_ref().map(|(_, until)| *until).unwrap_or(next_ping)), if pong.is_some() => {
                        closing(&mut socket, Some(CloseFrame {
                            code: CloseCode::Error,
                            reason: "keepalive ping timeout".into(),
                        })).await;
                        break;
                    },
                    _ = tokio::time::sleep_until(next_ping), if pong.is_none() => {
                        ping_id = ping_id.wrapping_add(1);
                        let payload = ping_id.to_be_bytes().to_vec();
                        if socket.send(Message::Ping(payload.clone().into())).await.is_err() {
                            break;
                        }
                        next_ping += Duration::from_secs(20);
                        pong = Some((payload, tokio::time::Instant::now() + Duration::from_secs(20)));
                    }
                }
            }
        });
        Ok(Self {
            next: AtomicU64::new(0),
            send,
            pending,
            events,
            task: Some(task),
        })
    }
    fn command(&self, method: &str, params: Value, id: u64) -> Value {
        json!({"id":id,"method":method,"params":if super::input::truth(&params){params}else{json!({})}})
    }
    pub(super) fn send(&self, method: &str, params: Value) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .send
            .send(Out::Command(self.command(method, params, id)));
    }
    pub(super) async fn cmd(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (send, receive) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, send);
        let _waiting = Waiting {
            pending: self.pending.clone(),
            id,
        };
        self.send
            .send(Out::Command(self.command(method, params, id)))
            .map_err(|_| Failure::closed())?;
        tokio::time::timeout(timeout, receive)
            .await
            .map_err(|_| Failure::timeout())?
            .map_err(|_| Failure::connection("CDP fechou"))?
    }
    pub(super) async fn event(&self) -> Value {
        self.events.next().await
    }
    pub(super) async fn close(&mut self) {
        let _ = self.send.send(Out::Close);
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }
}
fn opening_failure(error: tokio_tungstenite::tungstenite::Error, url: &str) -> Failure {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Http(response) => Failure::exception("InvalidStatus",
            format!("server rejected WebSocket connection: HTTP {}", response.status().as_u16())),
        Error::Io(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            let port = url.parse::<axum::http::Uri>().ok().and_then(|uri| uri.port_u16()).unwrap_or(80);
            let detail = format!("Connect call failed ('127.0.0.1', {port})");
            let code = error.raw_os_error().unwrap_or(0);
            let kind = if cfg!(windows) { "WinError" } else { "Errno" };
            let mut failure = Failure::exception("ConnectionRefusedError", format!("[{kind} {code}] {detail}"));
            failure.repr = format!("ConnectionRefusedError({code}, \"{detail}\")");
            failure
        }
        // O erro de handshake não carrega cabeçalhos/corpo: eles podem conter a credencial.
        Error::Io(error) => Failure::exception("OSError", error.to_string()),
        other => Failure::exception("InvalidHandshake", other.to_string()),
    }
}
impl Drop for Cdp {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    fn reference() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../backend/tests/fixtures/contract/remote-browser/transport-gaps.json");
        serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap()["cdp"].clone()
    }

    async fn connect(address: std::net::SocketAddr) -> Cdp {
        Cdp::connect(&format!("ws://{address}/cdp"), None).await
            .unwrap_or_else(|error| panic!("abertura do ensaio falhou: {}", error.message))
    }

    #[tokio::test]
    async fn complete_out_of_order_replies_ignore_duplicate_and_unknown_ids() {
        let expected = reference();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let first: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            let second: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(first, json!({"id": 1, "method": "Gap.first", "params": {}}));
            assert_eq!(second, json!({"id": 2, "method": "Gap.second", "params": {}}));
            for value in [
                json!({"id": second["id"], "error": {"code": -32001, "message": "second refused", "data": {"full": [1, "ação 😀"]}}, "extra": "kept"}),
                json!({"id": first["id"], "result": {"first": true}, "extra": {"nested": [false, null]}}),
                json!({"id": first["id"], "method": "Must.not.be.event", "result": {"duplicate": true}}),
                json!({"id": 99999, "method": "Must.not.be.event", "result": {"unknown": true}}),
                json!({"method": "Gap.done", "params": {"complete": true}}),
            ] { socket.send(Message::Text(value.to_string().into())).await.unwrap(); }
            while let Some(message) = socket.next().await {
                if matches!(message.unwrap(), Message::Close(_)) { socket.flush().await.unwrap(); break; }
            }
        });
        let mut cdp = connect(address).await;
        let (first, second) = tokio::join!(
            cdp.cmd("Gap.first", json!([]), Duration::from_secs(10)),
            cdp.cmd("Gap.second", json!(false), Duration::from_secs(10)));
        let replies = [first, second].into_iter().map(|value| value.unwrap_or_else(|error| panic!("resposta perdida: {}", error.message))).collect::<Vec<_>>();
        let event = tokio::time::timeout(Duration::from_secs(3), cdp.event()).await.unwrap();
        assert_eq!(json!({"replies": replies, "event": event,
            "events_remaining": cdp.events.queue.lock().unwrap().len(), "pending": cdp.pending.lock().unwrap().len()}), expected["out_of_order"]);
        cdp.close().await;
        peer.await.unwrap();
        assert_eq!(Arc::strong_count(&cdp.pending), 1);
        assert_eq!(Arc::strong_count(&cdp.events), 1);
    }

    #[tokio::test]
    async fn timeout_and_closed_receiver_remove_all_waiters_without_waiting_ten_seconds() {
        let expected = reference();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let changed = Arc::new(Notify::new());
        let (count, notify) = (received.clone(), changed.clone());
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(message) = socket.next().await {
                match message.unwrap() {
                    Message::Text(raw) => {
                        let value: Value = serde_json::from_str(&raw).unwrap();
                        count.fetch_add(1, Ordering::SeqCst); notify.notify_one();
                        if value["method"] == "Gap.finish" { socket.close(None).await.unwrap(); }
                    }
                    Message::Close(_) => { socket.flush().await.unwrap(); break; }
                    _ => {}
                }
            }
        });
        let mut cdp = connect(address).await;
        let error = cdp.cmd("Gap.timeout", json!({}), Duration::from_secs(10)).await.unwrap_err();
        assert_eq!(json!({"message": error.message, "repr": error.repr, "pending": cdp.pending.lock().unwrap().len()}), expected["timeout"]);
        let finish = async {
            loop {
                let change = changed.notified();
                if received.load(Ordering::SeqCst) >= 3 { break; }
                change.await;
            }
            cdp.send("Gap.finish", json!({}));
        };
        let began = tokio::time::Instant::now();
        let (first, second, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(cdp.cmd("Gap.pending", json!({}), Duration::from_secs(10)),
                cdp.cmd("Gap.pending", json!({}), Duration::from_secs(10)), finish)
        }).await.expect("fechar o receptor deixou comandos esperando o prazo próprio");
        assert!(began.elapsed() < Duration::from_secs(2));
        let errors = [first, second].into_iter().map(|result| {
            let error = result.unwrap_err(); json!({"message": error.message, "repr": error.repr})
        }).collect::<Vec<_>>();
        assert_eq!(json!(errors), expected["closed_waiters"]["errors"]);
        assert_eq!(json!(cdp.pending.lock().unwrap().len()), expected["closed_waiters"]["pending"]);
        assert_eq!(json!(cdp.events.queue.lock().unwrap().len()), expected["closed_waiters"]["events_remaining"]);
        let input = cdp.send("Input.insertText", json!({"text": "depois da queda"}));
        let ack = cdp.send("Page.screencastFrameAck", json!({"sessionId": 701}));
        assert_eq!(json!([input, ack]), expected["silent_send_failures"]["results"]);
        cdp.close().await;
        peer.await.unwrap();
        assert!(cdp.task.is_none()); assert!(cdp.send.is_closed());
        assert_eq!(cdp.pending.lock().unwrap().len(), expected["silent_send_failures"]["pending"].as_u64().unwrap() as usize);
        assert_eq!(expected["after_exit"], json!({"pump_done": true, "loose_pending": 0, "pending": 0}));
        assert_eq!(Arc::strong_count(&cdp.pending), 1);
        assert_eq!(Arc::strong_count(&cdp.events), 1);
    }

    #[tokio::test]
    async fn fire_and_forget_input_and_ack_are_written_and_leave_no_receiver_after_exit() {
        let expected = reference();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (written, ready) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut wire = Vec::new();
            for _ in 0..2 { wire.push(serde_json::from_str::<Value>(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap()); }
            written.send(()).unwrap();
            while let Some(message) = socket.next().await {
                if matches!(message.unwrap(), Message::Close(_)) { socket.flush().await.unwrap(); break; }
            }
            wire
        });
        let mut cdp = connect(address).await;
        cdp.send("Input.insertText", json!({"text": "ação 😀"}));
        cdp.send("Page.screencastFrameAck", json!({"sessionId": 702}));
        tokio::time::timeout(Duration::from_secs(3), ready).await.unwrap().unwrap();
        cdp.close().await;
        assert_eq!(json!(peer.await.unwrap()), expected["fire_and_forget_exit"]["wire"]);
        assert!(expected["fire_and_forget_exit"]["pump_done"].as_bool().unwrap());
        assert!(cdp.task.is_none()); assert!(cdp.send.is_closed());
        assert_eq!(cdp.pending.lock().unwrap().len(), expected["fire_and_forget_exit"]["pending"].as_u64().unwrap() as usize);
        assert_eq!(expected["fire_and_forget_exit"]["loose_pending"], 0);
        assert_eq!(Arc::strong_count(&cdp.pending), 1);
        assert_eq!(Arc::strong_count(&cdp.events), 1);
    }
}
