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

async fn configuration(state: &AppState) -> Result<OrganizationConfig, ()> {
    let request = axum::http::Request::get(format!(
        "http://{}/internal/transcription/config",
        state.cfg.upstream
    ))
    .header("x-hangar-internal", &state.cfg.internal_secret)
    .body(Body::empty())
    .map_err(|_| ())?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let response = state.http.request(request).await.map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024)
            .await
            .map_err(|_| ())?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
        serde_json::from_value(
            value
                .get("organization")
                .cloned()
                .unwrap_or_else(|| json!({})),
        )
        .map_err(|_| ())
    })
    .await
    .map_err(|_| ())?
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
        "organize"
            | "catalog"
            | "parse_models"
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
    if operation != "organize" {
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
        if operation == "catalog" {
            let request: OrganizationRequest = match serde_json::from_value(value) {
                Ok(request) => request,
                Err(_) => return failure(400, "dictation_request_invalid", "Destino inválido."),
            };
            let context = match super::context::resolve(&state, &request).await {
                Ok(context) => context,
                Err(code) => return failure(409, code, super::harness::detail(code)),
            };
            if context.provider == "claude" && !context.compatible {
                return failure(
                    409,
                    "dictation_claude_oauth_unsupported",
                    super::harness::detail("dictation_claude_oauth_unsupported"),
                );
            }
            return match state
                .dictation_models
                .get_context(&state.accounts, &context, false)
                .await
            {
                Ok((models, _)) => response(
                    json!({"ok":true,"result":{"provider":context.provider,"models":models,"generation":context.generation}}),
                    200,
                ),
                Err(code) => failure(502, code, super::harness::detail(code)),
            };
        }
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
                    provider,
                    &home,
                    value["fresh"].as_bool().unwrap_or(false),
                )
                .await
            {
                Ok((models, raw)) => {
                    response(json!({"ok":true,"result":{"models":models,"raw":raw}}), 200)
                }
                Err(code) => failure(
                    502,
                    code,
                    "Não foi possível consultar o catálogo da conta selecionada.",
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
        return match super::catalog::validate(
            value["models"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            value["model"].as_str(),
            value["effort"].as_str(),
            value["service_tier"].as_str(),
        ) {
            Ok(()) => response(json!({"ok":true,"result":{}}), 200),
            Err(detail) => failure(400, "dictation_model_unavailable", &detail),
        };
    }
    let request: OrganizationRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return failure(
                400,
                "dictation_request_invalid",
                "Pedido de organização inválido.",
            );
        }
    };
    if request.mode == Some(OrganizationMode::None) {
        return response(
            json!({"ok":true,"result":OrganizationResult::original(request.raw, OrganizationMode::None)}),
            200,
        );
    }
    let config = match configuration(&state).await {
        Ok(config) => config,
        Err(_) => {
            return response(
                json!({"ok":true,"result":OrganizationResult::original(request.raw, request.mode.unwrap_or_default())
            .failed("dictation_rust_unavailable","A configuração de organização está indisponível; foi mantida a transcrição original.")}),
                200,
            );
        }
    };
    response(
        json!({"ok":true,"result":service::organize(&state,request,&config).await}),
        200,
    )
}
