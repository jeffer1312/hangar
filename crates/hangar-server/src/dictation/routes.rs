use super::{model::*, service};
use crate::routes::AppState;
use axum::{
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc, time::Duration};

fn response(value: Value, status: u16) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [("content-type", "application/json")],
        value.to_string(),
    )
        .into_response()
}
fn failure(status: u16, code: &str, detail: &str) -> Response {
    response(
        json!({"ok":false,"error":{"status":status,"code":code,"detail":detail}}),
        status,
    )
}

/// A causa vai ao diário (nunca o corpo, que leva chaves); quem chama só preserva o cru.
pub(crate) async fn configuration(state: &AppState) -> Result<OrganizationConfig, ()> {
    let failed = |cause: &str, status: Option<u16>| {
        tracing::warn!(
            code = "dictation_rust_unavailable",
            cause,
            status,
            "configuração da organização do ditado indisponível"
        );
    };
    let request = axum::http::Request::get(format!(
        "http://{}/internal/transcription/config",
        state.cfg.upstream
    ))
    .header("x-hangar-internal", &state.cfg.internal_secret)
    .body(Body::empty())
    .map_err(|_| failed("request", None))?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let response = state
            .http
            .request(request)
            .await
            .map_err(|_| failed("network", None))?;
        let status = response.status();
        if !status.is_success() {
            return Err(failed("status", Some(status.as_u16())));
        }
        let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024)
            .await
            .map_err(|_| failed("too_large_or_body", Some(status.as_u16())))?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| failed("invalid_json", None))?;
        // Sem o objeto, assumir "desligado" esconderia um Python incompatível; campos vazios valem.
        let organization = value
            .get("organization")
            .cloned()
            .ok_or_else(|| failed("organization_missing", None))?;
        serde_json::from_value(organization).map_err(|_| failed("organization_invalid", None))
    })
    .await
    .map_err(|_| failed("timeout", None))?
}

pub async fn private(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let operation = request
        .uri()
        .path()
        .strip_prefix("/__hangar_server/dictation/")
        .unwrap_or("")
        .to_owned();
    if !matches!(
        operation.as_str(),
        "parse_models"
            | "validate_model"
            | "catalog_models"
            | "invalidate_models"
            | "raw_model"
    ) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let body = match to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => {
            return failure(
                413,
                "dictation_text_too_large",
                "A transcrição excede o limite de organização.",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return failure(
                400,
                "dictation_request_invalid",
                "Pedido de catálogo inválido.",
            );
        }
    };
    if matches!(
        operation.as_str(),
        "catalog_models" | "invalidate_models" | "raw_model"
    ) {
        let provider = value["provider"].as_str().unwrap_or("codex");
        let home = value["home"]
            .as_str()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                if provider == "claude" {
                    state.accounts.env.claude_base.clone()
                } else {
                    state.accounts.env.codex_default.clone()
                }
            });
        if operation == "invalidate_models" {
            state
                .dictation_models
                .invalidate(value["home"].as_str().map(std::path::Path::new));
            return response(json!({"ok":true,"result":{}}), 200);
        }
        if operation == "raw_model" {
            return match state
                .dictation_models
                .raw_model(
                    &state.accounts,
                    &home,
                    &value["config"],
                    value["model"].as_str().unwrap_or(""),
                    value["version"].as_str().unwrap_or(""),
                )
                .await
            {
                Ok(model) => response(json!({"ok":true,"result":{"model":model}}), 200),
                Err(code) => failure(
                    502,
                    code,
                    "Não foi possível comprovar a capacidade do modelo.",
                ),
            };
        }
        return match state
            .dictation_models
            .get(
                &state.accounts,
                &state.dictation_slots,
                provider,
                &home,
                value["fresh"].as_bool().unwrap_or(false),
            )
            .await
        {
            Ok((models, raw)) => {
                response(json!({"ok":true,"result":{"models":models,"raw":raw}}), 200)
            }
            // O código vai no texto: quem mostra é o `erro` do Python, que não carrega o código do Rust.
            Err(code) => failure(
                502,
                code,
                &format!("Não foi possível consultar o catálogo da conta selecionada ({code})."),
            ),
        };
    }
    if operation == "parse_models" {
        return match super::catalog::parse(
            value["provider"].as_str().unwrap_or("codex"),
            &value["catalog"],
            value["current"].as_str(),
        ) {
            Ok(models) => response(json!({"ok":true,"result":{"models":models}}), 200),
            Err(detail) => failure(502, "dictation_catalog_unavailable", detail),
        };
    }
    match super::catalog::validate(
        value["models"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        value["model"].as_str(),
        value["effort"].as_str(),
        value["service_tier"].as_str(),
    ) {
        Ok(()) => response(json!({"ok":true,"result":{}}), 200),
        Err(detail) => failure(400, "dictation_model_unavailable", &detail),
    }
}

/// A organização de um pedido já lido; falha nunca vira erro, só o texto cru com o motivo.
pub(crate) async fn organize_request(
    state: &AppState,
    request: OrganizationRequest,
    started: tokio::time::Instant,
) -> OrganizationResult {
    if request.mode == Some(OrganizationMode::None) {
        return OrganizationResult::original(request.raw, OrganizationMode::None);
    }
    match tokio::time::timeout_at(started + std::time::Duration::from_secs(120), configuration(state)).await {
        Ok(Ok(config)) => service::organize_started(state, request, &config, started).await,
        Err(_) => OrganizationResult::original(request.raw, request.mode.unwrap_or_default())
            .failed("dictation_organization_timeout", super::harness::detail("dictation_organization_timeout")),
        Ok(Err(())) => OrganizationResult::original(request.raw, request.mode.unwrap_or_default())
            .failed("dictation_rust_unavailable", "A configuração de organização está indisponível; foi mantida a transcrição original."),
    }
}
