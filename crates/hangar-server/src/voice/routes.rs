//! `GET`/`PUT /api/voice/settings` do dono; quem não é dono segue ao Python.
use crate::routes::AppState;
use crate::session_write::json_response;
use axum::{extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::{IntoResponse, Response}};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use super::settings::{self, SettingsDto, VOICES, VoiceSettings};

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
            if !st.voice.account_ids(&st.accounts).contains(&dto.codex_account) { return refuse(StatusCode::BAD_REQUEST, "unknown_account"); }
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
