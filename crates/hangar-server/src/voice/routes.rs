//! `WS /api/voice` e `GET`/`PUT /api/voice/settings` do dono; quem não é dono segue ao Python.
use crate::routes::AppState;
use crate::session_write::json_response;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{extract::{ConnectInfo, FromRequestParts, Request, State}, http::{Method, StatusCode, header}, response::{IntoResponse, Response}};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use super::hub::{Attached, Hello};
use super::protocol::{ClientMsg, ServerMsg};
use super::settings::{self, SettingsDto, VOICES, VoiceSettings};

const HELLO_WITHIN: Duration = Duration::from_secs(10);

/// Só o dono pelo `?token=` (como o terminal), com a Origin conferida pelo Python.
pub async fn ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let fwd = crate::proxy::Forward { client_ip, https };
    let owner = crate::auth::query_param(req.uri().query(), "token").filter(|t| !t.is_empty())
        .is_some_and(|t| st.auth.is_owner(&fwd.client_ip, Some(t.as_bytes())));
    if !owner || req.method() != Method::GET { return crate::routes::pass(&st, req, &fwd).await; }
    let (mut parts, body) = req.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(_) => return crate::routes::pass(&st, Request::from_parts(parts, body), &fwd).await,
    };
    if let Some(origin) = parts.headers.get(header::ORIGIN).filter(|o| !o.is_empty()) {
        // Origin que nem é texto não passa como "sem Origin".
        let Ok(origin) = origin.to_str() else { return (StatusCode::FORBIDDEN, "origem recusada").into_response() };
        let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok());
        match crate::term::origin::ask(&st.http, st.cfg.upstream, &st.cfg.internal_secret, origin, host).await {
            Ok(true) => {}
            Ok(false) => { super::log("voice origin refused"); return (StatusCode::FORBIDDEN, "origem recusada").into_response(); }
            Err(code) => {
                st.diag.report("rust.voice_failed", "voice", code, "a origem da voz não foi conferida");
                return json_response(StatusCode::SERVICE_UNAVAILABLE, json!({"ok": false, "error_code": code, "message": "a origem da voz não foi conferida"}));
            }
        }
    }
    ws.on_upgrade(move |socket| serve_socket(st, socket))
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> bool {
    let Ok(text) = serde_json::to_string(msg) else { return false };
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn refuse_socket(mut socket: WebSocket, code: &str) {
    super::log(format!("voice refused code={code}"));
    let _ = send(&mut socket, &ServerMsg::Error { code: code.to_owned(), detail: None }).await;
    let _ = socket.send(Message::Close(None)).await;
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
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
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
    let _ = socket.send(Message::Close(None)).await;
    st.voice.detach(epoch);
}

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

fn refuse(status: StatusCode, code: &str) -> Response { json_response(status, json!({"error_code": code})) }

async fn settings_reply(st: &AppState, req: Request, home: PathBuf, claude_dir: PathBuf) -> Response {
    match *req.method() {
        Method::GET => {
            let read = tokio::task::spawn_blocking(move || (settings::read_gate(&home, &claude_dir), settings::read_settings(&home))).await;
            let Ok((gate, current)) = read else { return refuse(StatusCode::INTERNAL_SERVER_ERROR, "settings_read_failed") };
            let (active, client) = st.voice.call_status();
            json_response(StatusCode::OK, json!({"enabled": gate.enabled, "codex": st.accounts.codex_command().is_some(), "jev": gate.jev.is_some(),
                "voices": VOICES, "settings": SettingsDto::from(&current), "call": {"active": active, "client": client}}))
        }
        Method::PUT => {
            let Ok(bytes) = axum::body::to_bytes(req.into_body(), 64 * 1024).await else { return refuse(StatusCode::BAD_REQUEST, "invalid_body") };
            let Ok(dto) = serde_json::from_slice::<SettingsDto>(&bytes) else { return refuse(StatusCode::BAD_REQUEST, "invalid_body") };
            if dto.voice.as_deref().is_some_and(|v| !VOICES.contains(&v)) { return refuse(StatusCode::BAD_REQUEST, "invalid_voice"); }
            if !st.voice.account_ids(&st.accounts).await.contains(&dto.codex_account) { return refuse(StatusCode::BAD_REQUEST, "unknown_account"); }
            let chosen: VoiceSettings = dto.into();
            let saved = tokio::task::spawn_blocking({ let chosen = chosen.clone(); move || settings::write_settings(&home, &chosen) }).await;
            if let Ok(Err(e)) = &saved { super::log(format!("voice settings write failed kind={:?}", e.kind())); }
            if !matches!(saved, Ok(Ok(()))) { return refuse(StatusCode::INTERNAL_SERVER_ERROR, "settings_write_failed"); }
            st.voice.apply_settings(&chosen);
            json_response(StatusCode::OK, json!({"settings": SettingsDto::from(&chosen)}))
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}
