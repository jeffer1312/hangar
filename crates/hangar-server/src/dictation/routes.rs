use axum::{body::{Body, to_bytes}, extract::{State, ConnectInfo, Request}, http::StatusCode, response::{IntoResponse, Response}};
use serde_json::{Value, json};
use std::{sync::Arc, net::SocketAddr, time::Duration};
use crate::routes::AppState;
use super::{model::*, service};

fn response(value: Value, status: u16) -> Response {
    (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [("content-type", "application/json")], value.to_string()).into_response()
}
fn failure(status: u16, code: &str, detail: &str) -> Response { response(json!({"ok":false,"error":{"status":status,"code":code,"detail":detail}}), status) }

async fn configuration(state: &AppState) -> Result<OrganizationConfig, ()> {
    let request = axum::http::Request::get(format!("http://{}/internal/transcription/config", state.cfg.upstream))
        .header("x-hangar-internal", &state.cfg.internal_secret).body(Body::empty()).map_err(|_| ())?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let response = state.http.request(request).await.map_err(|_| ())?;
        if !response.status().is_success() { return Err(()); }
        let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024).await.map_err(|_| ())?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
        serde_json::from_value(value.get("organization").cloned().unwrap_or_else(|| json!({}))).map_err(|_| ())
    }).await.map_err(|_| ())?
}

pub async fn private(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    if request.uri().path() != "/__hangar_server/dictation/organize" { return StatusCode::NOT_FOUND.into_response(); }
    let body = match to_bytes(request.into_body(), 1024 * 1024).await { Ok(body) => body, Err(_) => return failure(413,"dictation_text_too_large","A transcrição excede o limite de organização.") };
    let request: OrganizationRequest = match serde_json::from_slice(&body) { Ok(request) => request, Err(_) => return failure(400,"dictation_request_invalid","Pedido de organização inválido.") };
    if request.mode == Some(OrganizationMode::None) { return response(json!({"ok":true,"result":OrganizationResult::original(request.raw, OrganizationMode::None)}),200); }
    let config = match configuration(&state).await {
        Ok(config) => config,
        Err(_) => return response(json!({"ok":true,"result":OrganizationResult::original(request.raw, request.mode.unwrap_or_default())
            .failed("dictation_rust_unavailable","A configuração de organização está indisponível; foi mantida a transcrição original.")}),200),
    };
    response(json!({"ok":true,"result":service::organize(request,&config).await}),200)
}
