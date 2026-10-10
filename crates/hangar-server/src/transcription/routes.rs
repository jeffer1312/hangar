use axum::{extract::{State, ConnectInfo, Request}, response::{Response, IntoResponse}, http::StatusCode, body::{Body, to_bytes}};
use std::{net::SocketAddr, sync::Arc};
use crate::routes::AppState;
use super::model::{ConfigSnapshot, Profile, TranscriptionError};
use serde_json::{Value, json};

fn response(value: Value, status: u16) -> Response {
    (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [("content-type", "application/json")], value.to_string()).into_response()
}

fn failure(error: TranscriptionError) -> Response {
    let status = error.status;
    response(json!({"ok":false,"error":error}), status)
}

async fn configuration(state: &AppState) -> Result<ConfigSnapshot, TranscriptionError> {
    let failed = || TranscriptionError { status: 503, code: "transcription_config_unavailable".into(),
        detail: "A configuração de transcrição não está disponível.".into() };
    let request = axum::http::Request::get(format!("http://{}/internal/transcription/config", state.cfg.upstream))
        .header("x-hangar-internal", &state.cfg.internal_secret).body(Body::empty()).map_err(|_| failed())?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let response = state.http.request(request).await.map_err(|_| failed())?;
        if !response.status().is_success() { return Err(failed()); }
        let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024).await.map_err(|_| failed())?;
        serde_json::from_slice(&bytes).map_err(|_| failed())
    }).await.map_err(|_| failed())?
}

pub async fn private(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let operation = request.uri().path().strip_prefix("/__hangar_server/transcription/").unwrap_or("").to_owned();
    if !matches!(operation.as_str(), "transcribe" | "status" | "test") { return StatusCode::NOT_FOUND.into_response(); }
    let Ok(_slot) = state.transcription_slots.clone().try_acquire_owned() else {
        return failure(TranscriptionError { status: 503, code: "transcription_busy".into(), detail: "As vagas de transcrição estão ocupadas. Tente novamente.".into() });
    };
    let query: std::collections::HashMap<String, String> = request.uri().query()
        .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default();
    let profile = match query.get("profile").map(String::as_str).unwrap_or("dictation") {
        "dictation" => Profile::Dictation, "file" => Profile::File, "video" => Profile::Video,
        _ => return failure(TranscriptionError { status: 400, code: "transcription_profile_invalid".into(), detail: "Perfil de transcrição inválido.".into() }),
    };
    let content = match to_bytes(request.into_body(), 100 * 1024 * 1024).await {
        Ok(content) => content,
        Err(_) => return failure(TranscriptionError { status: 413, code: "transcription_audio_too_large".into(), detail: "O áudio excede o limite de 100 MiB.".into() }),
    };
    let snapshot = match configuration(&state).await { Ok(snapshot) => snapshot, Err(error) => return failure(error) };
    state.transcription.configure(snapshot).await;
    if operation == "status" { return response(json!({"ok":true,"result":{"providers":state.transcription.status().await}}), 200); }
    let filename = query.get("filename").cloned();
    let result = if operation == "test" {
        match query.get("provider_id") {
            Some(id) => state.transcription.transcribe_provider(id, content, filename).await,
            None => Err(TranscriptionError { status: 400, code: "transcription_provider_missing".into(), detail: "Informe o serviço que será testado.".into() }),
        }
    } else { state.transcription.transcribe(content, filename, profile).await };
    match result {
        Ok(result) => response(json!({"ok":true,"result":result}), 200),
        Err(error) => { state.diag.report("transcription.failed", "", &error.code, "A transcrição falhou."); failure(error) }
    }
}
