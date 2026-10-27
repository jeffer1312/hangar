//! Linha da extensão Pi/omp. Pergunta de editor e confirmação de entrega nunca compartilham future.
use crate::routes::AppState;
use axum::{
    extract::{
        FromRequestParts, Request,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

pub fn message_fits(text: &str) -> bool {
    text.chars().count() <= MAX_MESSAGE
}

pub const QUESTION_TIMEOUT: Duration = Duration::from_secs(1);
pub const ACK_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_MESSAGE: usize = 256 * 1024;
const HEARTBEAT: Duration = Duration::from_secs(20);

type Questions = HashMap<String, oneshot::Sender<Option<String>>>;
type Deliveries = HashMap<String, oneshot::Sender<bool>>;
pub struct Line {
    pub outgoing: mpsc::UnboundedSender<Value>,
    questions: Mutex<Questions>,
    deliveries: Mutex<Deliveries>,
    serial: tokio::sync::Mutex<()>,
}

struct Pending {
    line: Arc<Line>,
    id: String,
    question: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if self.question {
            self.line
                .questions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.id);
        } else {
            self.line
                .deliveries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.id);
        }
    }
}
#[derive(Default)]
pub struct Store {
    lines: Mutex<HashMap<String, Arc<Line>>>,
}
impl Store {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn register(&self, key: &str, outgoing: mpsc::UnboundedSender<Value>) -> Arc<Line> {
        let line = Arc::new(Line {
            outgoing,
            questions: Mutex::default(),
            deliveries: Mutex::default(),
            serial: tokio::sync::Mutex::new(()),
        });
        if let Some(old) = self
            .lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.into(), line.clone())
        {
            Self::cancel(&old);
        }
        line
    }
    fn cancel(line: &Line) {
        for (_, send) in line
            .questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
        {
            let _ = send.send(None);
        }
        for (_, send) in line
            .deliveries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
        {
            let _ = send.send(false);
        }
    }
    pub fn remove(&self, key: &str, line: &Arc<Line>) {
        let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        if lines
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, line))
        {
            lines.remove(key);
        }
        drop(lines);
        Self::cancel(line);
    }
    fn get(&self, key: &str) -> Option<Arc<Line>> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }
    pub fn has_line(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    pub fn line_for(&self, name: Option<&str>, pane: Option<&str>) -> Option<String> {
        name.filter(|v| !v.is_empty() && self.has_line(v))
            .or_else(|| pane.filter(|v| !v.is_empty() && self.has_line(v)))
            .map(str::to_owned)
    }
    pub fn respond(&self, key: &str, message: &Value) {
        let Some(line) = self.get(key) else { return };
        let Some(id) = message["id"].as_str().filter(|v| !v.is_empty()) else {
            return;
        };
        if message.get("resposta").is_some() {
            if let Some(send) = line
                .questions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id)
            {
                let _ = send.send(message["resposta"].as_str().map(str::to_owned));
            }
        } else if let Some(send) = line
            .deliveries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        {
            let _ = send.send(super::pure::truthy(&message["ok"]));
        }
    }
    pub async fn question(&self, key: &str, what: &str) -> Option<String> {
        self.question_for(key, what, QUESTION_TIMEOUT).await
    }
    pub async fn question_for(&self, key: &str, what: &str, deadline: Duration) -> Option<String> {
        let line = self.get(key)?;
        let id = crate::mods::state::random_hex(16);
        let (send, recv) = oneshot::channel();
        line.questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), send);
        let _pending = Pending {
            line: line.clone(),
            id: id.clone(),
            question: true,
        };
        if line.outgoing.send(json!({"id":id,"pedir":what})).is_err() {
            self.remove(key, &line);
            return None;
        }
        let result = tokio::time::timeout(deadline, recv)
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        line.questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        result
    }
    pub async fn deliver(&self, key: &str, text: &str, id: Option<&str>) -> &'static str {
        self.deliver_for(key, text, id, ACK_TIMEOUT).await
    }
    pub async fn deliver_for(
        &self,
        key: &str,
        text: &str,
        id: Option<&str>,
        deadline: Duration,
    ) -> &'static str {
        let Some(line) = self.get(key) else {
            return "sem-linha";
        };
        let _serial = line.serial.lock().await;
        let id = id
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| crate::mods::state::random_hex(16));
        let (send, recv) = oneshot::channel();
        line.deliveries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), send);
        let _pending = Pending {
            line: line.clone(),
            id: id.clone(),
            question: false,
        };
        if line
            .outgoing
            .send(json!({"id":id,"text":text,"deliverAs":"steer"}))
            .is_err()
        {
            self.remove(key, &line);
            return "deferred";
        }
        let accepted = matches!(tokio::time::timeout(deadline, recv).await, Ok(Ok(true)));
        line.deliveries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if accepted {
            "sent"
        } else {
            tracing::warn!(
                code = "pi_inbox_delivery_unconfirmed",
                "entrega da extensão sem confirmação; permanece na fila, sem repetir por tecla"
            );
            "deferred"
        }
    }
}
pub static INBOX: LazyLock<Arc<Store>> = LazyLock::new(Store::new);
static ACCESS_WARNINGS: LazyLock<Mutex<HashSet<(IpAddr, &'static str)>>> =
    LazyLock::new(Mutex::default);

fn refuse_once(host: IpAddr, cause: &'static str) {
    // A extensão tenta reconectar em laço; só uma mudança de estado merece outro aviso.
    if ACCESS_WARNINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((host, cause))
    {
        if cause == "origin" {
            tracing::warn!(code="pi_inbox_origin_refused",%host,"origem recusada da extensão; aviso único até conectar");
        } else {
            tracing::warn!(code="pi_inbox_token_refused",%host,"token recusado da extensão; aviso único até conectar");
        }
    }
}

/// A borda pública mantém a mesma restrição de self-connect da extensão, além do bearer.
pub async fn websocket(st: &Arc<AppState>, peer: SocketAddr, request: Request) -> Response {
    use subtle::ConstantTimeEq;
    let params: HashMap<_, _> =
        form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let local = peer.ip().is_loopback()
        || (!st.cfg.listen.ip().is_unspecified() && peer.ip() == st.cfg.listen.ip());
    if !local {
        refuse_once(peer.ip(), "origin");
        return StatusCode::FORBIDDEN.into_response();
    }
    if st.cfg.auth_token.is_empty()
        || !bool::from(
            params
                .get("token")
                .map(String::as_bytes)
                .unwrap_or_default()
                .ct_eq(st.cfg.auth_token.as_bytes()),
        )
    {
        refuse_once(peer.ip(), "token");
        return StatusCode::FORBIDDEN.into_response();
    }
    {
        let mut warned = ACCESS_WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
        warned.remove(&(peer.ip(), "origin"));
        warned.remove(&(peer.ip(), "token"));
    }
    let (mut parts, _) = request.into_parts();
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    upgrade
        .max_message_size(4 * MAX_MESSAGE)
        .on_upgrade(|socket| connection(INBOX.clone(), socket))
        .into_response()
}
async fn close(socket: &mut WebSocket, code: u16) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: "".into(),
        })))
        .await;
}
async fn connection(store: Arc<Store>, mut socket: WebSocket) {
    let first = match tokio::time::timeout(HEARTBEAT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(s)))) if message_fits(&s) => s,
        _ => {
            close(&mut socket, 1008).await;
            return;
        }
    };
    let first: Value = match serde_json::from_str(&first) {
        Ok(v) => v,
        Err(_) => {
            close(&mut socket, 1008).await;
            return;
        }
    };
    let key = first
        .get("chave")
        .filter(|v| super::pure::truthy(v))
        .or_else(|| first.get("pane"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if key.is_empty() {
        close(&mut socket, 1008).await;
        return;
    }
    let (outgoing, mut recv) = mpsc::unbounded_channel();
    let line = store.register(&key, outgoing);
    let mut unanswered = 0;
    let mut heartbeat = tokio::time::Instant::now() + HEARTBEAT;
    loop {
        tokio::select! {
          value=recv.recv()=>{let Some(value)=value else{break};if socket.send(Message::Text(value.to_string().into())).await.is_err(){break;}}
          inbound=tokio::time::timeout_at(heartbeat,socket.recv())=>{
            match inbound{
              Err(_)=>{heartbeat=tokio::time::Instant::now()+HEARTBEAT;unanswered+=1;if unanswered>2{close(&mut socket,1000).await;break;}if socket.send(Message::Text(json!({"ping":true}).to_string().into())).await.is_err(){break;}}
              Ok(Some(Ok(Message::Text(text))))=>{heartbeat=tokio::time::Instant::now()+HEARTBEAT;unanswered=0;if !message_fits(&text){close(&mut socket,1009).await;break;}let Ok(value)=serde_json::from_str::<Value>(&text)else{break};if !super::pure::truthy(&value["ping"])&&!super::pure::truthy(&value["pong"]){store.respond(&key,&value);}}
              Ok(Some(Ok(Message::Ping(data))))=>{heartbeat=tokio::time::Instant::now()+HEARTBEAT;unanswered=0;if socket.send(Message::Pong(data)).await.is_err(){break;}}
              Ok(Some(Ok(Message::Pong(_))))=>{heartbeat=tokio::time::Instant::now()+HEARTBEAT;unanswered=0;}
              _=>break,
            }
          }
        }
    }
    store.remove(&key, &line);
}

/// Transporte privado para os emissores que ainda vivem no Python; o estado da linha só existe aqui.
pub async fn private(st: &Arc<AppState>, peer: SocketAddr, request: Request) -> Response {
    if !crate::terminal_routes::trusted_internal(st, peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_MESSAGE).await {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    match call(value).await {
        Ok(value) => crate::session_write::json_response(StatusCode::OK, value),
        Err(error) => error.response(),
    }
}

/// A ponte existente da lista chama este dispatcher no processo do Rust.
pub async fn call(value: Value) -> Result<Value, super::ModelError> {
    let s = |key: &str| value[key].as_str().unwrap_or("");
    let result = match s("op") {
        "line" => json!({"line":INBOX.line_for(value["name"].as_str(),value["pane"].as_str())}),
        "has_line" => json!({"present":INBOX.has_line(s("key"))}),
        "question" => json!({"value":INBOX.question(s("key"),s("what")).await}),
        "deliver" => {
            json!({"disposition":INBOX.deliver(s("key"),s("text"),value["msg_id"].as_str()).await})
        }
        _ => {
            return Err(super::ModelError::text(
                400,
                "operação da extensão desconhecida",
            ));
        }
    };
    Ok(result)
}

pub fn write_endpoint(
    dirs: &[PathBuf],
    address: SocketAddr,
    token: &str,
    now: f64,
) -> Vec<PathBuf> {
    let ip = if address.ip().is_unspecified() {
        "127.0.0.1".to_owned()
    } else {
        address.ip().to_string()
    };
    let host = if ip.contains(':') {
        format!("[{ip}]")
    } else {
        ip
    };
    let value =
        json!({"url":format!("ws://{host}:{}/api/pi/inbox",address.port()),"token":token,"ts":now});
    let mut written = vec![];
    for dir in dirs {
        let result = (|| -> std::io::Result<PathBuf> {
            std::fs::create_dir_all(dir)?;
            let target = dir.join(".hangar-conn.json");
            crate::runtime::queue::atomic_write(&target, value.to_string().as_bytes())?;
            Ok(target)
        })();
        match result {
            Ok(path) => written.push(path),
            Err(error) => {
                tracing::warn!(code="pi_inbox_endpoint_failed",io_kind=?error.kind(),"endereço da extensão não publicado nesta conta")
            }
        }
    }
    written
}
pub async fn install_endpoint(st: Arc<AppState>) {
    let address = st.cfg.listen;
    let token = st.cfg.auth_token.clone();
    let list = st.list.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let dirs = list
            .state_dirs_blocking()
            .map(|(v, _)| v.as_ref().clone())
            .unwrap_or_default();
        write_endpoint(
            &dirs,
            address,
            &token,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|v| v.as_secs_f64())
                .unwrap_or(0.),
        );
    })
    .await;
}
