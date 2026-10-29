//! WebSocket do navegador existente, servido diretamente nas portas Rust.
use super::{
    access,
    cdp::Cdp,
    frames::{self, View},
    input, shell,
};
use crate::{
    routes::AppState,
    share::{
        caller::{Caller, caller},
        gate::Watcher,
        store::Kind,
    },
};
use axum::{
    extract::{
        ConnectInfo, FromRequestParts, Path, Request, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

pub(super) struct Failure {
    pub message: String,
    pub repr: String,
    pub transport_closed: bool,
}
use super::value::quoted;
impl Failure {
    pub(super) fn exception(kind: &str, message: String) -> Self {
        let repr = format!("{kind}({})", quoted(&message));
        Self { message, repr, transport_closed: false }
    }
    pub(super) fn plain(message: impl Into<String>) -> Self {
        let message = message.into();
        let repr = format!("RuntimeError({})", quoted(&message));
        Self {
            message,
            repr,
            transport_closed: false,
        }
    }
    pub(super) fn connection(message: &str) -> Self {
        Self {
            message: message.into(),
            repr: format!("ConnectionError({})", quoted(message)),
            transport_closed: false,
        }
    }
    pub(super) fn value(message: String) -> Self {
        let repr = format!("ValueError({})", quoted(&message));
        Self {
            message,
            repr,
            transport_closed: false,
        }
    }
    pub(super) fn closed() -> Self {
        let mut error = Self::connection("CDP fechou");
        error.transport_closed = true;
        error
    }
    pub(super) fn timeout() -> Self {
        Self {
            message: String::new(),
            repr: "TimeoutError()".into(),
            transport_closed: false,
        }
    }
}
pub(super) type Output = Arc<Mutex<SplitSink<WebSocket, Message>>>;
pub(super) async fn send(output: &Output, value: Value) -> Result<(), Failure> {
    output
        .lock()
        .await
        .send(Message::Text(super::value::encode(&value).into()))
        .await
        .map_err(|_| Failure::plain("WebSocket fechou"))
}

pub async fn public(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if !crate::share::gate::is_websocket(&req) {
        return crate::routes::pass_any(State(st), ConnectInfo(peer), req).await;
    }
    let (ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let guest = caller(&req)
        .and_then(Caller::grant)
        .map(|grant| grant.sessions.get(&name) == Some(&Kind::Share));
    if (guest.is_none() && super::login::refused(&st, req.headers(), req.uri().query(), https).await)
        || guest == Some(false)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match access::authorize_websocket(
            &st.auth,
            &ip,
            req.uri().query(),
            guest == Some(true),
        ).await {
        Ok(()) => {},
        Err(access::AccessError::Unavailable(error)) => return crate::auth::unavailable(error),
        Err(_) => return StatusCode::FORBIDDEN.into_response(),
    }
    serve(st, name, req, false).await
}
pub async fn private(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(name) = crate::auth::query_param(req.uri().query(), "name") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    serve(st, name, req, true).await
}
fn refuse(reason: &'static str, forwarded: bool) -> Response {
    if forwarded {
        (StatusCode::FORBIDDEN, reason).into_response()
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}
async fn serve(st: Arc<AppState>, name: String, req: Request, forwarded: bool) -> Response {
    let life = match st.list.discovered_session(&name).await {
        Ok(Some((row, _))) => row.lifecycle_id,
        Ok(None) => return refuse("sessao nao existe", forwarded),
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "remote_browser_discovery_unavailable",
            )
                .into_response();
        }
    };
    let dir = st.hangar_dir.join("nav");
    let lookup = dir.clone();
    let lookup_name = name.clone();
    let target = tokio::task::spawn_blocking(move || {
        let sc = shell::sidecar(&lookup, &lookup_name, true)
            .or_else(|| shell::sidecar(&lookup, &lookup_name, false))?;
        if input::truth(&sc["targetId"]) {
            return Some((
                format!(
                    "ws://127.0.0.1:9223/devtools/page/{}",
                    shell::string_field(&sc, "targetId")
                ),
                None,
                sc,
            ));
        }
        if !input::truth(&sc["pid"]) || !input::truth(&sc["chave"]) {
            return None;
        }
        let srv = shell::server(&lookup).ok()?;
        if !shell::same_pid(&srv["pid"], &sc["pid"]) {
            return None;
        }
        let port = input::integer(&srv["porta"]).ok()?;
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("chave", &shell::string_field(&sc, "chave"))
            .finish();
        Some((
            format!("ws://127.0.0.1:{port}/cdp?{query}"),
            Some(shell::string_field(&srv, "token")),
            sc,
        ))
    })
    .await
    .ok()
    .flatten();
    let Some((url, token, sc)) = target else {
        return refuse("sessao sem navegador", forwarded);
    };
    let width = number(req.uri().query(), "w", 0, 1400, 0);
    let quality = number(req.uri().query(), "q", 30, 90, 45);
    let watcher = req.extensions().get::<Watcher>().cloned();
    let (mut parts, _) = req.into_parts();
    let Ok(upgrade) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(connection) = st.remote_browser.connections.enter() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "remote_browser_stopping").into_response();
    };
    upgrade
        .max_message_size(32 * 1024 * 1024)
        .on_upgrade(move |socket| run(st, name, life, socket, url, token, sc, width, quality, watcher, connection))
}
fn number(query: Option<&str>, key: &str, minimum: i64, maximum: i64, default: i64) -> i64 {
    let Some(raw) = crate::auth::query_param(query, key) else {
        return default;
    };
    let Some(raw) = input::normalize_numeric_separators(&raw) else {
        return default;
    };
    let mut digits = raw.trim().chars().peekable();
    let negative = digits.peek() == Some(&'-');
    if matches!(digits.peek().copied(), Some('+' | '-')) {
        digits.next();
    }
    let mut magnitude = 0i64;
    let mut has_digit = false;
    while let Some(character) = digits.next() {
        let Some(digit) = character.to_digit(10) else {
            return default;
        };
        // O valor fica limitado, mas o restante da entrada ainda precisa ser validado.
        magnitude = magnitude
            .saturating_mul(10)
            .saturating_add(i64::from(digit))
            .min(maximum);
        has_digit = true;
    }
    if !has_digit {
        return default;
    }
    if key == "q" && (negative || magnitude == 0) {
        default
    } else if negative {
        minimum
    } else {
        magnitude.clamp(minimum, maximum)
    }
}
#[allow(clippy::too_many_arguments)]
async fn run(
    st: Arc<AppState>,
    name: String,
    life: Option<String>,
    socket: WebSocket,
    url: String,
    token: Option<String>,
    sc: Value,
    width: i64,
    quality: i64,
    watcher: Option<Watcher>,
    mut connection: super::shutdown::Connection,
) {
    let (writer, mut reader) = socket.split();
    let output = Arc::new(Mutex::new(writer));
    let view = Arc::new(Mutex::new(View {
        size: (json!(1280), json!(800)),
        width,
        quality,
    }));
    let mut owner = None;
    let mut stopping = false;
    let connected = tokio::select! {
        connected = Cdp::connect(&url, token.as_deref()) => Some(connected),
        () = connection.stopped() => { stopping = true; None },
    };
    let mut cdp = match connected {
        None => None,
        Some(Ok(cdp)) => Some(cdp),
        Some(Err(error)) => {
            report(&output, &name, error).await;
            None
        }
    };
    let mut restoring = None;
    if let Some(cdp) = cdp.as_mut() {
        let result = tokio::select! {
        result = async {
            let mut current = sc["url"].clone();
            if input::truth(&sc["targetId"]) {
                match cdp.cmd(
                    "Runtime.evaluate",
                    json!({"expression": "location.href", "returnByValue": true}),
                    Duration::from_secs(5),
                ).await {
                    Ok(answer) => {
                        if answer["result"]["result"]["value"].as_str().is_some_and(|value| !value.is_empty()) {
                            current = answer["result"]["result"]["value"].clone();
                        }
                    }
                    Err(error) => tracing::debug!(target:"hangar.navsock", "navsock: url atual falhou: {}", error.repr),
                }
            }
            send(&output, json!({"t": "url", "url": current})).await?;
            tokio::select! {
                result = frames::run(cdp, &output, &view) => result,
                result = read(&st, &name, life.as_deref(), cdp, &mut reader, &output, &view, &mut owner) => result,
                _ = Watcher::ended(watcher.as_ref()) => {
                    let _ = output.lock().await.send(Message::Close(Some(CloseFrame {
                        code: 4410,
                        reason: "convite encerrado".into(),
                    }))).await;
                    Ok(())
                }
            }
        } => result,
        () = connection.stopped() => { stopping = true; Ok(()) },
        };
        if stopping {
            // O Supervisor pode sair antes do prazo de Close do CDP; a devolução começa já.
            restoring = Some(restore(&st, &name, owner));
        }
        cdp.close().await;
        if let Err(error) = result {
            report(&output, &name, error).await;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        let mut writer = output.lock().await;
        let _ = writer.send(Message::Close(Some(CloseFrame {
            code: if connection.is_stopping() { 1012 } else { 1000 },
            reason: "".into(),
        }))).await;
        // O Close recebido já enfileira a resposta, mesmo quando outro envio é recusado.
        let _ = writer.close().await;
    }).await;
    // A devolução pode esperar o controlador depois que o socket do espectador já terminou.
    drop(reader);
    drop(output);
    // A devolução sobrevive ao cancelamento da conexão que impôs o tamanho.
    let restoring = restoring.unwrap_or_else(|| restore(&st, &name, owner));
    if restoring.await.is_err() {
        tracing::warn!(target:"hangar.navsock", "navsock {name}: tarefa de devolver layout não terminou");
    }
}
fn restore(st: &AppState, name: &str, owner: Option<u64>) -> tokio::task::JoinHandle<()> {
    let restore = st.remote_browser.clone();
    let dir = st.hangar_dir.join("nav");
    let list = st.list.clone();
    let name = name.to_owned();
    tokio::spawn(async move { restore.restore(&dir, &name, owner, &list).await })
}
async fn report(output: &Output, name: &str, error: Failure) {
    tracing::warn!(target:"hangar.navsock","navsock {name}: {}",error.repr);
    let _ = send(output, json!({"t":"erro","m":error.message})).await;
}
#[allow(clippy::too_many_arguments)]
async fn read(
    st: &AppState,
    name: &str,
    life: Option<&str>,
    cdp: &Cdp,
    reader: &mut SplitStream<WebSocket>,
    output: &Output,
    view: &Arc<Mutex<View>>,
    owner: &mut Option<u64>,
) -> Result<(), Failure> {
    while let Some(message) = reader.next().await {
        let raw = match message {
            Ok(Message::Text(raw)) => raw,
            Ok(Message::Close(_)) | Err(_) => return Ok(()),
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Binary(_)) => return Err(Failure::plain("text")),
        };
        if raw.chars().count() > 65536 {
            continue;
        }
        let Some(message) = super::value::decode(&raw) else {
            continue;
        };
        if !message.is_object() {
            continue;
        }
        if message["t"] == "layout" {
            let result = st
                .remote_browser
                .apply(
                    &st.hangar_dir.join("nav"),
                    name,
                    life,
                    &message,
                    owner,
                )
                .await;
            match result {
                Ok(Some(change)) => {
                    if let Some(dimensions) = change.dimensions {
                        view.lock().await.size = (json!(dimensions.0), json!(dimensions.1));
                    }
                    send(output, change.event).await?;
                }
                Ok(None) => {}
                Err(error) if error.repr.starts_with("OverflowError(") => return Err(error),
                Err(error) => send(output, json!({"t":"erro","m":error.message})).await?,
            }
        } else {
            let size = view.lock().await.size.clone();
            input::apply(cdp, &message, &size.0, &size.1)?;
        }
    }
    Ok(())
}
