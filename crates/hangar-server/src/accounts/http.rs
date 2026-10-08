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
    (matches!(*method, Method::GET | Method::POST)
        && matches!(path, "/api/claude-configs" | "/api/codex-contas"))
        || (matches!(*method, Method::GET | Method::POST)
            && path
                .strip_prefix("/api/codex-contas/")
                .and_then(|tail| tail.strip_suffix("/prepare"))
                .is_some_and(|id| !id.is_empty() && !id.contains('/')))
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
    if path.ends_with("/prepare") {
        let id = path
            .trim_start_matches("/api/codex-contas/")
            .trim_end_matches("/prepare");
        let query: std::collections::HashMap<String, String> =
            form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        let mut force = false;
        if method == Method::POST {
            if let Some(value) = query.get("forcar") {
                force = match value.to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" | "y" | "on" | "t" => true,
                    "false" | "0" | "no" | "n" | "off" | "f" => false,
                    _ => return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"detail":[{"type":"bool_parsing","loc":["query","forcar"],"msg":"Input should be a valid boolean, unable to interpret input","input":value}]}))).into_response(),
                };
            }
        }
        if method == Method::GET
            && query
                .get("cwd")
                .is_some_and(|value| value.chars().count() > 4096)
        {
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"detail":[{"type":"string_too_long","loc":["query","cwd"],"msg":"String should have at most 4096 characters","input":query["cwd"],"ctx":{"max_length":4096}}]}))).into_response();
        }
        return prepare_response(
            &state,
            id,
            method == Method::POST,
            force,
            query.get("cwd").filter(|value| !value.is_empty()).cloned(),
        )
        .await;
    }
    if method == Method::GET && path == "/api/codex-contas" {
        let bridge = match bridge(&state) {
            Ok(bridge) => bridge,
            Err(err) => return error(err),
        };
        let accounts = match service.visible_codex_accounts() {
            Ok(accounts) => accounts,
            Err(err) => return error(err),
        };
        let mut result = vec![];
        for account in accounts {
            let sync = service.preparation_status(&account, &bridge).await;
            result.push(service.codex_snapshot(&account, sync).await);
        }
        return Json(json!(result)).into_response();
    }
    if method == Method::GET {
        return match tokio::task::spawn_blocking(move || service.claude_catalog()).await {
            Ok(Ok(body)) => Json(body).into_response(),
            Ok(Err(err)) => error(err),
            Err(_) => error(AccountError::io()),
        };
    }
    if method == Method::POST {
        let claude = path == "/api/claude-configs";
        let name_field = if claude { "nome" } else { "name" };
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
            if body.get(name_field).is_none() {
                issues.push(json!({"type":"missing","loc":["body",name_field],"msg":"Field required","input":body}));
            } else if !body[name_field].is_string() {
                issues.push(json!({"type":"string_type","loc":["body",name_field],"msg":"Input should be a valid string","input":body[name_field]}));
            } else if claude {
                let name = body[name_field].as_str().unwrap();
                let size = name.chars().count();
                if size == 0 {
                    issues.push(json!({"type":"string_too_short","loc":["body","nome"],"msg":"String should have at least 1 character","input":name,"ctx":{"min_length":1}}));
                } else if size > 32 {
                    issues.push(json!({"type":"string_too_long","loc":["body","nome"],"msg":"String should have at most 32 characters","input":name,"ctx":{"max_length":32}}));
                } else if !super::catalog::valid_name(name) {
                    let pattern = r"^[a-z0-9][a-z0-9_-]{0,31}\z";
                    issues.push(json!({"type":"string_pattern_mismatch","loc":["body","nome"],"msg":format!("String should match pattern '{pattern}'"),"input":name,"ctx":{"pattern":pattern}}));
                }
            }
            for (key, value) in body.as_object().unwrap() {
                if key != name_field {
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
        let name = body[name_field].as_str().unwrap().to_owned();
        if claude {
            let bridge = match bridge(&state) {
                Ok(bridge) => bridge,
                Err(err) => return error(err),
            };
            let handle = tokio::runtime::Handle::current();
            return match tokio::task::spawn_blocking(move || {
                service.create(Provider::Claude, &name, |path| {
                    handle.block_on(service.seed_claude(path, &name, &bridge))
                })
            })
            .await
            {
                Ok(Ok(account)) => {
                    Json(json!({"path":account.home,"label":account.id,"active":false}))
                        .into_response()
                }
                Ok(Err(err)) if err.code == "account_exists" => {
                    (StatusCode::CONFLICT, Json(json!({"detail":err.message}))).into_response()
                }
                Ok(Err(err)) => error(err),
                Err(_) => error(AccountError::io()),
            };
        }
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

fn bridge(state: &crate::routes::AppState) -> Result<AccountsBridge, AccountError> {
    let instance = crate::config::Config::runtime_instance()
        .ok()
        .flatten()
        .ok_or_else(AccountError::io)?;
    AccountsBridge::new(
        state.cfg.upstream,
        state.cfg.internal_secret.clone(),
        instance,
    )
    .map_err(|_| AccountError::io())
}

async fn prepare_response(
    state: &crate::routes::AppState,
    id: &str,
    start: bool,
    force: bool,
    cwd: Option<String>,
) -> Response {
    let account = match state.accounts.resolve(Provider::Codex, id) {
        Ok(account) => account,
        Err(err) => return error(err),
    };
    let bridge = match bridge(state) {
        Ok(bridge) => bridge,
        Err(err) => return error(err),
    };
    if start {
        return match state.accounts.prepare(&account, bridge, force, None).await {
            Ok(result) => (StatusCode::ACCEPTED, Json(result)).into_response(),
            Err(err) => error(err),
        };
    }
    let result = state.accounts.preparation_status(&account, &bridge).await;
    if let Some(cwd) =
        cwd.filter(|_| matches!(result["status"].as_str(), Some("ready" | "partial")))
    {
        match state.accounts.pretrust(&account, bridge, cwd).await {
            Ok(result) => Json(result).into_response(),
            Err(err) => error(err),
        }
    } else {
        Json(result).into_response()
    }
}

pub async fn private(
    axum::extract::State(state): axum::extract::State<Arc<crate::routes::AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 16384).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        account_id: String,
        prepare: bool,
        force: bool,
        cwd: Option<String>,
    }
    let Ok(body) = serde_json::from_slice::<Input>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    prepare_response(&state, &body.account_id, body.prepare, body.force, body.cwd).await
}
