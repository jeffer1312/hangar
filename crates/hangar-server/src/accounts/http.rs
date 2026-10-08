use super::{
    AccountKey, GuardMode, Provider, UsageFacts,
    bridge::AccountsBridge,
    catalog::{AccountError, codex_dto, disconnected_auth, idle_sync},
};
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;

struct Json(Value);
impl IntoResponse for Json {
    fn into_response(self) -> Response {
        (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            self.0.to_string(),
        )
            .into_response()
    }
}

pub fn matches(method: &Method, path: &str) -> bool {
    (*method == Method::GET && path == "/api/claude-configs")
        || (*method == Method::POST && path == "/api/codex-contas")
        || (*method == Method::DELETE
            && ["/api/claude-configs/", "/api/codex-contas/"]
                .iter()
                .any(|prefix| {
                    path.strip_prefix(prefix)
                        .is_some_and(|tail| !tail.is_empty() && !tail.contains('/'))
                }))
}
fn error(error: AccountError) -> Response {
    (
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({"detail":{"code":error.code,"params":error.params,"msg":error.message}})),
    )
        .into_response()
}
pub async fn public(state: Arc<crate::routes::AppState>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let service = state.accounts.clone();
    if method == Method::GET {
        return match tokio::task::spawn_blocking(move || service.claude_catalog()).await {
            Ok(Ok(body)) => Json(body).into_response(),
            Ok(Err(err)) => error(err),
            Err(_) => error(AccountError::io()),
        };
    }
    if method == Method::POST {
        let bytes = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        };
        if bytes.is_empty() || bytes.as_ref() == b"null" {
            return (StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]}))).into_response();
        }
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body, Err(_) => return (StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"detail":[{"type":"json_invalid","loc":["body",0],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting value"}}]}))).into_response(),
        };
        let mut issues = vec![];
        if !body.is_object() {
            issues.push(json!({"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":body}));
        } else {
            if body.get("name").is_none() {
                issues.push(json!({"type":"missing","loc":["body","name"],"msg":"Field required","input":body}));
            } else if !body["name"].is_string() {
                issues.push(json!({"type":"string_type","loc":["body","name"],"msg":"Input should be a valid string","input":body["name"]}));
            }
            for (key, value) in body.as_object().unwrap() {
                if key != "name" {
                    issues.push(json!({"type":"extra_forbidden","loc":["body",key],"msg":"Extra inputs are not permitted","input":value}));
                }
            }
        }
        if !issues.is_empty() {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"detail":issues})),
            )
                .into_response();
        }
        let name = body["name"].as_str().unwrap().to_owned();
        return match tokio::task::spawn_blocking(move || {
            service.create(Provider::Codex, &name, |_| Ok(()))
        })
        .await
        {
            Ok(Ok(account)) => (
                StatusCode::CREATED,
                Json(codex_dto(&account, disconnected_auth(), idle_sync())),
            )
                .into_response(),
            Ok(Err(err)) => error(err),
            Err(_) => error(AccountError::io()),
        };
    }
    let provider = if path.starts_with("/api/codex-contas/") {
        Provider::Codex
    } else {
        Provider::Claude
    };
    let Ok(id) = percent_encoding::percent_decode_str(path.rsplit('/').next().unwrap())
        .decode_utf8()
        .map(|s| s.into_owned())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let account = match service.resolve(provider, &id) {
        Ok(account) => account,
        Err(err) => return error(err),
    };
    let key = match AccountKey::new(provider, &account.home) {
        Ok(key) => key,
        Err(_) => return error(AccountError::io()),
    };
    let guard = match service
        .locks
        .acquire(&key, GuardMode::Exclusive, std::time::Instant::now())
        .await
    {
        Ok(guard) => guard,
        Err(_) => {
            if provider == Provider::Codex {
                return error(AccountError::codex(409, "codex_account_in_use", Some(&id)));
            }
            return error(AccountError::new(
                409,
                "account_busy",
                "a conta está ocupada",
                json!({}),
            ));
        }
    };
    // Releitura sob exclusividade: o retrato anterior só servia para localizar a guarda.
    let account = match service
        .resolve(provider, &id)
        .and_then(|account| service.protect_delete(provider, &account).map(|()| account))
    {
        Ok(account) => account,
        Err(err) => return error(err),
    };
    let instance = match crate::config::Config::runtime_instance() {
        Ok(Some(instance)) => instance,
        _ => {
            return error(AccountError::new(
                409,
                "account_usage_unknown",
                "não foi possível confirmar que a conta está livre",
                json!({}),
            ));
        }
    };
    let bridge = match AccountsBridge::new(
        state.cfg.upstream,
        state.cfg.internal_secret.clone(),
        instance,
    ) {
        Ok(bridge) => bridge,
        Err(_) => return error(AccountError::io()),
    };
    let mut facts = match bridge.facts(std::slice::from_ref(&key)).await {
        Ok(mut rows) => rows.remove(0).facts,
        Err(_) => UsageFacts::default(),
    };
    facts.merge(match state.state.runtime.get() {
        Some(runtime) => runtime.account_usage(&key).await,
        None => UsageFacts::default(),
    });
    match tokio::task::spawn_blocking(move || service.delete(provider, &account, &guard, &facts))
        .await
    {
        Ok(Ok(())) => Json(json!({"ok":true})).into_response(),
        Ok(Err(err)) => error(err),
        Err(_) => error(AccountError::io()),
    }
}
