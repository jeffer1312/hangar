//! `GET`/`PUT /api/placement/config`: só o dono, nunca repassadas (o Python não tem a rota).
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use hangar_placement::{store::{self, SaveError}, PlacementConfig};
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
use crate::{routes::{AppState, owner_gate}, session_write::{detail_body, json_response}};

fn failure(status: StatusCode, code: &str, message: &str, params: serde_json::Value) -> Response {
    json_response(status, detail_body(code, message, params))
}

pub async fn config(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(response) = owner_gate(&state, peer, &request).await { return response; }
    let home = state.accounts.env.home.clone();
    if request.method() != Method::PUT {
        return match tokio::task::spawn_blocking(move || store::load(&home)).await {
            Ok(Ok(config)) => json_response(StatusCode::OK, json!(config)),
            _ => failure(StatusCode::SERVICE_UNAVAILABLE, "placement_config_unreadable",
                "não foi possível ler as regras do balanceamento", json!({})),
        };
    }
    let parsed = to_bytes(request.into_body(), 64 * 1024).await.ok()
        .and_then(|raw| serde_json::from_slice::<PlacementConfig>(&raw).ok());
    let Some(config) = parsed else {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "regras do balanceamento inválidas", json!({}));
    };
    match tokio::task::spawn_blocking(move || store::save(&home, &config).map(|()| config)).await {
        Ok(Ok(config)) => json_response(StatusCode::OK, json!(config)),
        Ok(Err(SaveError::Invalid(invalid))) => failure(StatusCode::UNPROCESSABLE_ENTITY, "placement_config_invalid",
            "regra do balanceamento inválida", json!({"field": invalid.field})),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "placement_config_unavailable",
            "não foi possível guardar as regras do balanceamento", json!({})),
    }
}
