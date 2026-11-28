//! Canal anfitrião (`GET /api/browser-host`) e a rota do CLI sem app local
//! (`POST /api/sessions/{name}/browser-cmd`). Registro, foco e prazos são do `hangar-browser-host`;
//! aqui ficam a credencial, o socket e a gravação do print nesta máquina.
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use hangar_browser_host::{Refusal, Reply};
use serde_json::{Value, json};

use crate::routes::{AppState, cors, owner_gate};
use crate::share::gate::Watcher;

/// O cabeçalho em que o app nativo diz por que o verbo respondeu `erro:`; esta rota repassa o do app de lá.
pub(crate) const BROWSER_CODE: &str = "x-hangar-browser-code";
const BODY_MAX: usize = 1024 * 1024;
const PNG_MAX: usize = 32 * 1024 * 1024;
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(25);
const SEND_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Página de outra máquina do Connect não pode dirigir o navegador de quem usa: só credencial explícita.
async fn owner(st: &AppState, peer: SocketAddr, req: &mut Request) -> Result<(), Response> {
    if !crate::port_tunnel::explicit_token(st, peer, req) {
        return Err(hangar_core::http::detail(StatusCode::FORBIDDEN, "browser_host_token_required", "credencial só por ?token= ou Bearer", json!({})));
    }
    owner_gate(st, peer, req).await.map(|_| ())
}

pub async fn host_ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    if let Err(refusal) = owner(&st, peer, &mut req).await {
        return refusal;
    }
    let watcher = req.extensions().get::<Watcher>().cloned();
    let (mut parts, _) = req.into_parts();
    let Ok(upgrade) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
        return hangar_core::http::detail(StatusCode::BAD_REQUEST, "browser_host_websocket_required", "o canal anfitrião é um WebSocket", json!({}));
    };
    // O quadro tem teto próprio (16 MiB no tungstenite): um PNG maior que ele derrubaria o canal.
    upgrade.max_message_size(PNG_MAX).max_frame_size(PNG_MAX).on_upgrade(move |socket| serve(st, socket, watcher))
}

async fn serve(st: Arc<AppState>, socket: WebSocket, watcher: Option<Watcher>) {
    let (mut connection, mut outgoing) = st.browser_host.attach();
    let (mut sink, mut stream) = socket.split();
    // Fora do laço: recriado a cada volta, o vigia nunca chegaria ao fim.
    let ended = Watcher::ended(watcher.as_ref());
    tokio::pin!(ended);
    // App que dormiu ou trocou de rede some sem FIN e seguraria o foco: sem ouvir nada, o canal cai.
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut heard = tokio::time::Instant::now();
    let close = loop {
        tokio::select! {
            _ = &mut ended => break Some(CloseFrame { code: crate::share::gate::CLOSE_ENDED, reason: "aparelho revogado".into() }),
            _ = ping.tick() => {
                if heard.elapsed() > PING_EVERY * 3 || !sent(&mut sink, Message::Ping(Default::default())).await { break None }
            }
            next = outgoing.recv() => match next {
                // O `Zeroizing` zera o quadro (o do cofre leva a senha) quando sai daqui.
                Some(text) => if !sent(&mut sink, Message::Text(text.as_str().into())).await { break None },
                None => break None,
            },
            incoming = stream.next() => {
                heard = tokio::time::Instant::now();
                match incoming {
                    Some(Ok(Message::Text(text))) => connection.text(text.as_str()),
                    Some(Ok(Message::Binary(png))) => connection.binary(png.into()),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => break None,
                }
            }
        }
    };
    // Antes do Close: os pedidos em curso já saem com `browser_host_gone`.
    drop(connection);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), sink.send(Message::Close(close))).await;
}

/// App que sumiu sem FIN enche o buffer e o envio nunca volta: preso, o laço não veria revogação nem silêncio.
async fn sent(sink: &mut SplitSink<WebSocket, Message>, message: Message) -> bool {
    matches!(tokio::time::timeout(SEND_WITHIN, sink.send(message)).await, Ok(Ok(())))
}

pub async fn browser_cmd(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(name): Path<String>, mut req: Request) -> Response {
    let headers = req.headers().clone();
    let mut response = match owner(&st, peer, &mut req).await {
        Err(refusal) => refusal,
        Ok(_) => command(&st, &name, req).await,
    };
    cors(&headers, response.headers_mut());
    response
}

async fn command(st: &AppState, name: &str, req: Request) -> Response {
    let invalid = || hangar_core::http::detail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo invalido", json!({}));
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), BODY_MAX).await else { return invalid() };
    let Ok(Value::Object(mut body)) = serde_json::from_slice::<Value>(&bytes) else { return invalid() };
    if !body.get("verbo").is_some_and(Value::is_string) || body.get("args").is_some_and(|a| !a.is_array()) {
        return invalid();
    }
    let verb = body["verbo"].as_str().unwrap_or_default().to_owned();
    let shot = (verb == "shot").then(|| shot_path(st, name, &mut body));
    match st.browser_host.command(name, body).await {
        Ok(Reply::Text(text)) => text_response(text),
        // A causa vai no cabeçalho do `/cmd` local, para o MCP e o CLI lerem igual; o texto não muda.
        Ok(Reply::Error(code, text)) => {
            let mut response = text_response(text);
            if let Ok(value) = header::HeaderValue::from_str(&code) { response.headers_mut().insert(BROWSER_CODE, value); }
            response
        }
        Ok(Reply::Png(png)) => {
            let path = shot.unwrap_or_else(|| default_shot(st, name));
            let shown = path.display().to_string();
            // Fora do executor: disco lento não segura as outras rotas.
            let written = tokio::task::spawn_blocking(move || {
                if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
                std::fs::write(&path, png)
            }).await;
            match written.map_err(std::io::Error::other).and_then(|r| r) {
                Ok(()) => text_response(format!("ok: shot {shown}")),
                Err(e) => text_response(format!("erro: shot {shown}: {e}")),
            }
        }
        Err(refusal) => refused(refusal, &verb),
    }
}

fn text_response(text: String) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

fn refused(refusal: Refusal, verb: &str) -> Response {
    let status = StatusCode::from_u16(refusal.status()).unwrap_or(StatusCode::BAD_GATEWAY);
    hangar_core::http::detail(status, refusal.code(), &refusal.message(verb), json!({}))
}

/// O caminho do print nesta máquina, a da sessão: o que o agente cita tem de existir aqui. Um `.png`
/// absoluto do CLI (já resolvido no cwd do agente) vale; senão, a pasta de prints da sessão.
fn shot_path(st: &AppState, name: &str, body: &mut serde_json::Map<String, Value>) -> PathBuf {
    let mut args: Vec<Value> = body.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
    let given = args.first().and_then(Value::as_str)
        .filter(|a| !a.starts_with("--") && a.ends_with(".png") && std::path::Path::new(a).is_absolute())
        .map(PathBuf::from);
    let path = match given {
        Some(path) => path,
        None => {
            let path = default_shot(st, name);
            args.insert(0, json!(path.display().to_string()));
            path
        }
    };
    body.insert("args".into(), Value::Array(args));
    path
}

/// `~/.hangar/nav/shots/<sessão>/<epoch ms>.png`, como o `pastaShots()` do `hangar-preview`.
fn default_shot(st: &AppState, name: &str) -> PathBuf {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    st.hangar_dir.join("nav").join("shots").join(shot_folder(name)).join(format!("{ms}.png"))
}

/// O `replace(/[^\p{L}\p{N}_]+/gu, '-')` do CLI: o nome não sai da pasta de prints.
fn shot_folder(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_alphanumeric() || c == '_' {
            if gap { out.push('-'); gap = false; }
            out.push(c);
        } else {
            gap = true;
        }
    }
    if gap { out.push('-'); }
    if out.is_empty() { "nav".into() } else { out }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shot_folder_matches_the_cli() {
        assert_eq!(super::shot_folder("minha sessão.2"), "minha-sessão-2");
        assert_eq!(super::shot_folder("../x"), "-x");
        assert_eq!(super::shot_folder("a__b"), "a__b");
    }
}
