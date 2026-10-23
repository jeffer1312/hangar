//! `WS /api/voice` e `GET`/`PUT /api/voice/settings` do dono; quem não é dono segue ao Python.
use crate::routes::AppState;
use crate::session_write::json_response;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{extract::{ConnectInfo, FromRequestParts, Request, State}, http::{Method, StatusCode}, response::{IntoResponse, Response}};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use super::hub::{Attached, Hello};
use super::protocol::{ClientMsg, ServerMsg};
use super::settings::{self, SettingsDto, VoiceSettings};

const HELLO_WITHIN: Duration = Duration::from_secs(10);

/// Só o dono pelo `?token=` (como o terminal); sem Origin: página estranha não tem o token.
pub async fn ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let fwd = crate::proxy::Forward { client_ip, https, guest: None };
    let owner = crate::auth::query_param(req.uri().query(), "token").filter(|t| !t.is_empty())
        .is_some_and(|t| st.auth.is_owner(&fwd.client_ip, Some(t.as_bytes())));
    if !owner || req.method() != Method::GET { return crate::routes::pass(&st, req, &fwd).await; }
    let (mut parts, body) = req.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(_) => return crate::routes::pass(&st, Request::from_parts(parts, body), &fwd).await,
    };
    ws.on_upgrade(move |socket| serve_socket(st, socket))
}

/// Ponte do Connect: o Python já conferiu o dono; aqui só o segredo interno.
pub async fn private_ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let (mut parts, _) = req.into_parts();
    let Ok(ws) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else { return StatusCode::BAD_REQUEST.into_response() };
    ws.on_upgrade(move |socket| serve_socket(st, socket))
}

/// Aparelho que dormiu deixa o socket meio aberto: sem nada dele por este tempo (o cliente pinga a cada 10 s), a conexão
/// cai e o prazo sem dono começa.
const SILENT_FOR: Duration = Duration::from_secs(30);
const SEND_WITHIN: Duration = Duration::from_secs(10);

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> bool {
    let Ok(text) = serde_json::to_string(msg) else { return false };
    matches!(tokio::time::timeout(SEND_WITHIN, socket.send(Message::Text(text.into()))).await, Ok(Ok(())))
}

async fn refuse_socket(mut socket: WebSocket, code: &str) {
    super::log(format!("voice refused code={code}"));
    let _ = send(&mut socket, &ServerMsg::Error { code: code.to_owned(), detail: None }).await;
    close(&mut socket).await;
}

/// A primeira mensagem de texto; controle do WebSocket não conta.
async fn first_text(socket: &mut WebSocket) -> Option<ClientMsg> {
    loop {
        match socket.recv().await? {
            Ok(Message::Text(text)) => return serde_json::from_str(text.as_str()).ok(),
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            _ => return None,
        }
    }
}

async fn serve_socket(st: Arc<AppState>, mut socket: WebSocket) {
    let hello = match tokio::time::timeout(HELLO_WITHIN, first_text(&mut socket)).await {
        Ok(Some(ClientMsg::Hello { client, screen, caps, actions })) => Hello { client, screen, caps, actions },
        _ => return refuse_socket(socket, "bad_hello").await,
    };
    let Attached { epoch, mut from_call } = match st.voice.attach(&st, hello).await {
        Ok(attached) => attached,
        Err(code) => return refuse_socket(socket, code).await,
    };
    let mut heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(heard + SILENT_FOR) => { super::log("voice device silent: closing"); break; }
            incoming = socket.recv() => match { heard = tokio::time::Instant::now(); incoming } {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMsg>(text.as_str()) {
                    Ok(ClientMsg::Ping) => if !send(&mut socket, &ServerMsg::Pong).await { break },
                    // Capacidades e tela do dono só mudam por nova conexão.
                    Ok(ClientMsg::Hello { .. }) => super::log("voice repeated hello ignored"),
                    Ok(msg) => if !st.voice.forward(epoch, msg) { break },
                    Err(_) => super::log("voice device message unreadable"),
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => {}
                _ => break,
            },
            out = from_call.recv() => match out {
                Some(msg) => {
                    let last = matches!(msg, ServerMsg::Taken | ServerMsg::Closed);
                    if !send(&mut socket, &msg).await || last { break; }
                }
                None => break,
            },
        }
    }
    // Antes do Close: num socket meio aberto o envio pode demorar, e o prazo sem dono já tem de estar correndo.
    st.voice.detach(epoch);
    close(&mut socket).await;
}

async fn close(socket: &mut WebSocket) { let _ = tokio::time::timeout(SEND_WITHIN, socket.send(Message::Close(None))).await; }

pub async fn settings_route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let (home, claude_dir) = (st.voice.home().to_path_buf(), st.voice.claude_dir().to_path_buf());
    let headers = req.headers().clone();
    let mut resp = settings_reply(&st, req, home, claude_dir).await;
    // Mesmo CORS das outras rotas do dono: PWA servido por outro servidor lê a resposta.
    crate::routes::cors(&headers, resp.headers_mut());
    resp
}

/// Ponte do Connect para as configurações: mesma resposta do dono, sem CORS (quem lê é o Python).
pub async fn private_settings(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let (home, claude_dir) = (st.voice.home().to_path_buf(), st.voice.claude_dir().to_path_buf());
    settings_reply(&st, req, home, claude_dir).await
}

pub async fn memory_route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let mut resp = memory_reply(&st, req, "/api/voice/memory").await;
    crate::routes::cors(&headers, resp.headers_mut());
    resp
}

/// Ponte do Connect para a memória: mesma resposta do dono, sem CORS (quem lê é o Python).
pub async fn private_memory(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    memory_reply(&st, req, "/__hangar_server/voice/memory").await
}

async fn memory_reply(st: &AppState, req: Request, base: &str) -> Response {
    let file = super::memory::memory_path(&settings::voice_dir(st.voice.home()));
    let id = req.uri().path().strip_prefix(base).and_then(|rest| rest.strip_prefix('/')).filter(|s| !s.is_empty()).map(str::to_owned);
    match (req.method().clone(), id) {
        (Method::GET, None) => match tokio::task::spawn_blocking(move || super::memory::load(&file)).await {
            Ok(Some(entries)) => json_response(StatusCode::OK, json!({"entries": entries})),
            Ok(None) => { super::log("voice memory read failed code=memory_unreadable"); refuse(StatusCode::INTERNAL_SERVER_ERROR, "memory_unreadable") }
            Err(_) => { super::log("voice memory read task failed code=memory_task_failed"); refuse(StatusCode::INTERNAL_SERVER_ERROR, "memory_task_failed") }
        },
        (Method::DELETE, Some(id)) => match tokio::task::spawn_blocking(move || super::memory::forget(&file, &id)).await {
            Ok(Ok(true)) => json_response(StatusCode::OK, json!({"ok": true})),
            Ok(Ok(false)) => refuse(StatusCode::NOT_FOUND, "memory_unknown"),
            Ok(Err(why)) => { super::log(format!("voice memory delete failed code=memory_io kind={}", refusal_kind(&why))); refuse(StatusCode::INTERNAL_SERVER_ERROR, "memory_io") }
            Err(_) => { super::log("voice memory delete task failed code=memory_task_failed"); refuse(StatusCode::INTERNAL_SERVER_ERROR, "memory_task_failed") }
        },
        _ => refuse(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
    }
}

fn refusal_kind(why: &super::memory::Refusal) -> &str { if let super::memory::Refusal::Io(k) = why { k } else { "other" } }

fn refuse(status: StatusCode, code: &str) -> Response { json_response(status, json!({"error_code": code})) }

/// Quanto o `GET` das configurações espera pela lista de vozes do Codex.
const VOICE_LIST_IN_GET: std::time::Duration = std::time::Duration::from_millis(1500);

async fn settings_reply(st: &Arc<AppState>, req: Request, home: PathBuf, claude_dir: PathBuf) -> Response {
    match *req.method() {
        Method::GET => {
            let env_key = st.voice.env_jev_key();
            let read = tokio::task::spawn_blocking(move || (settings::read_gate(&home, &claude_dir, env_key), settings::read_settings(&home))).await;
            let Ok((gate, current)) = read else { return refuse(StatusCode::INTERNAL_SERVER_ERROR, "settings_read_failed") };
            let (active, client) = st.voice.call_status();
            let codex = st.accounts.codex_command().is_some();
            // A voz aparece com o Codex instalado e ao menos uma conta Codex; o microfone só abre quando a pessoa liga a chamada.
            // Catálogo ilegível não esconde a voz como "sem conta": a abertura da chamada mostra o erro de verdade.
            let enabled = codex && st.voice.account_ids(&st.accounts).await.is_none_or(|ids| !ids.is_empty());
            // O botão da voz depende desta resposta, e o aparelho espera pouco: a lista do Codex tem prazo curto e, sem
            // ela, vai a fixa. A leitura segue numa tarefa própria e guarda o resultado para o próximo pedido; cortá-la no
            // prazo a recomeçaria do zero a cada vez.
            let fixed = || settings::VOICES.map(str::to_owned).to_vec();
            let voices = if !enabled { fixed() } else {
                let read = { let (st, account) = (st.clone(), current.codex_account.clone()); tokio::spawn(async move { st.voice.voices(&st, &account).await }) };
                match tokio::time::timeout(VOICE_LIST_IN_GET, read).await {
                    Ok(Ok((list, _))) => list,
                    _ => { super::log("voice list slow in settings read: fixed list"); fixed() }
                }
            };
            json_response(StatusCode::OK, json!({"enabled": enabled, "codex": codex, "jev": gate.jev.is_some(),
                "voices": voices, "settings": SettingsDto::from(&current), "call": {"active": active, "client": client}}))
        }
        Method::PUT => {
            let Ok(bytes) = axum::body::to_bytes(req.into_body(), 64 * 1024).await else { return refuse(StatusCode::BAD_REQUEST, "invalid_body") };
            let Ok(dto) = serde_json::from_slice::<SettingsDto>(&bytes) else { return refuse(StatusCode::BAD_REQUEST, "invalid_body") };
            if let Some(voice) = dto.voice.as_deref()
                && let (list, read) = st.voice.voices(st, &dto.codex_account).await
                && settings::voice_refused(read.then_some(list.as_slice()), voice) {
                return refuse(StatusCode::BAD_REQUEST, "invalid_voice");
            }
            if !st.voice.account_ids(&st.accounts).await.unwrap_or_default().contains(&dto.codex_account) { return refuse(StatusCode::BAD_REQUEST, "unknown_account"); }
            let keep_style = dto.style.is_none();
            let mut chosen: VoiceSettings = dto.into();
            let saved = tokio::task::spawn_blocking({ let mut chosen = chosen.clone(); move || {
                // Cliente que não conhece o estilo não pode apagar o escolhido em outro.
                if keep_style { chosen.style = settings::read_settings(&home).style; }
                settings::write_settings(&home, &chosen).map(|()| chosen.style)
            } }).await;
            if let Ok(Ok(style)) = &saved { chosen.style = *style; }
            if let Ok(Err(e)) = &saved { super::log(format!("voice settings write failed kind={:?}", e.kind())); }
            if !matches!(saved, Ok(Ok(_))) { return refuse(StatusCode::INTERNAL_SERVER_ERROR, "settings_write_failed"); }
            st.voice.apply_settings(&chosen);
            json_response(StatusCode::OK, json!({"settings": SettingsDto::from(&chosen)}))
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}
