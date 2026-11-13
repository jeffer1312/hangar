//! `GET /api/shortcuts/export`, `POST /api/shortcuts/import` e `POST /api/shortcuts/verify`: só o dono.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use hangar_core::edge::Edge;
use hangar_core::guest::{Caller, caller};
use hangar_core::http::{detail, json_response};

use crate::host::{Error, Host};
use crate::transfer::{ImportRequest, export, import};
use crate::verify::{VERIFY_TIMEOUT, run_checks};

pub const ROUTES: &[(&str, &str)] = &[
    ("/api/shortcuts/export", "GET"),
    ("/api/shortcuts/import", "POST"),
    ("/api/shortcuts/verify", "POST"),
];
const BASES: [&str; 3] = ["/api/shortcuts/export", "/api/shortcuts/import", "/api/shortcuts/verify"];
const MAX_VERIFY_SCRIPTS: usize = 50;

struct Http {
    edge: Edge,
    host: Arc<dyn Host>,
}

pub fn owns_path(path: &str) -> bool {
    BASES.iter().any(|base| path == *base || path.strip_prefix(base).is_some_and(|tail| tail.starts_with('/')))
}

pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, host: Arc<dyn Host>) -> Router<S> {
    let mut router = Router::new();
    for base in BASES {
        router = router
            .route(base, axum::routing::any(public))
            .route(&format!("{base}/"), axum::routing::any(public))
            .route(&format!("{base}/{{*rest}}"), axum::routing::any(public));
    }
    router.with_state(Arc::new(Http { edge, host }))
}

async fn public(
    State(state): State<Arc<Http>>,
    peer: Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>,
    request: Request,
) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    let method = request.method().clone();
    let headers = request.headers().clone();
    let mut response = reply(state, peer.ok().map(|peer| peer.0), request).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    hangar_core::http_route_response::suppress_head(&method, &mut response);
    response
}

async fn reply(state: Arc<Http>, peer: Option<SocketAddr>, request: Request) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy().into_owned();
    let socket = peer.unwrap_or_else(|| "0.0.0.0:0".parse().expect("endereço estático válido"));
    let (_, https) = state.edge.trusted.resolve(socket.ip(), request.headers());
    let decision = hangar_core::http_contract::resolve(ROUTES, request.method().as_str(), &path);
    if let Some(response) = hangar_core::http_route_response::response(
        &decision,
        request.method(),
        request.uri(),
        request.headers(),
        https,
        "",
    ) {
        return response;
    }
    // O verify roda comando de arquivo importado: convidado nunca chega aqui, nem pelo porteiro.
    if hangar_core::edge::on_guest_port(&request) || matches!(caller(&request), Some(Caller::Guest(_))) {
        return detail(StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora da sessão compartilhada", json!({}));
    }
    if let Err(response) = state.edge.owner_gate(socket, &request).await {
        return response;
    }
    match path.as_str() {
        "/api/shortcuts/export" => export_route(state, request.uri().query()).await,
        "/api/shortcuts/import" => match body(request).await {
            Ok(value) => import_route(state, value).await,
            Err(response) => response,
        },
        _ => match body(request).await {
            Ok(value) => verify_route(state, value).await,
            Err(response) => response,
        },
    }
}

async fn export_route(state: Arc<Http>, query: Option<&str>) -> Response {
    let pairs: Vec<(String, String)> =
        form_urlencoded::parse(query.unwrap_or("").as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let single: HashMap<String, String> = pairs.iter().cloned().collect();
    let include_scripts = match hangar_core::query::bool_param_or(&single, "include_scripts", true) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let ids: Vec<String> = pairs.into_iter().filter(|(key, _)| key == "ids").map(|(_, value)| value).collect();
    // Sem `ids` exporta todos; `ids=` sozinho é a seleção vazia.
    let ids = match ids.as_slice() {
        [] => None,
        [only] if only.is_empty() => Some(Vec::new()),
        _ => Some(ids),
    };
    let host = state.host.clone();
    let handle = tokio::runtime::Handle::current();
    let result = tokio::task::spawn_blocking(move || {
        export(host.as_ref(), ids.as_deref(), include_scripts, || handle.block_on(host.login_path()))
    })
    .await;
    match result {
        Ok(Ok(value)) => json_response(StatusCode::OK, value),
        Ok(Err(Error::Invalid(motivo))) => invalid("erro_shortcut_export_invalido", &motivo),
        Ok(Err(Error::Io(message))) => internal("shortcut_export_io", &message),
        Err(_) => internal("shortcut_export_task", "tarefa do export interrompida"),
    }
}

async fn import_route(state: Arc<Http>, value: Value) -> Response {
    let Some(request) = import_body(value) else { return invalid_body() };
    let host = state.host.clone();
    match tokio::task::spawn_blocking(move || import(host.as_ref(), request)).await {
        Ok(Ok(value)) => json_response(StatusCode::OK, value),
        Ok(Err(Error::Invalid(motivo))) => invalid("erro_shortcut_import_invalido", &motivo),
        Ok(Err(Error::Io(message))) => internal("shortcut_import_io", &message),
        Err(_) => internal("shortcut_import_task", "tarefa do import interrompida"),
    }
}

async fn verify_route(state: Arc<Http>, value: Value) -> Response {
    let Some((ids, mut scripts)) = verify_body(&value) else { return invalid_body() };
    scripts.truncate(MAX_VERIFY_SCRIPTS);
    let host = state.host.clone();
    // Em tarefa própria: o cliente que desiste no meio não deixa o grupo do comando sem prazo.
    let task = tokio::spawn(async move { run_checks(host.as_ref(), &ids, &scripts, VERIFY_TIMEOUT).await });
    match task.await {
        Ok(Ok(value)) => json_response(StatusCode::OK, value),
        Ok(Err(Error::Invalid(message) | Error::Io(message))) => verify_failed(&message),
        Err(_) => verify_failed("tarefa do verify interrompida"),
    }
}

async fn body(request: Request) -> Result<Value, Response> {
    if let Some(response) = hangar_core::http_body_limit::precheck(request.headers()) {
        return Err(response);
    }
    let json = hangar_core::http_body_input::is_json(request.headers());
    let bytes = match hangar_core::http_body_limit::read(request.into_body()).await {
        Ok(bytes) => bytes,
        Err(error) if hangar_core::http_body_limit::is_limit(&error) => return Err(hangar_core::http_body_limit::rejected()),
        Err(_) => return Err(invalid_body()),
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) if json => Ok(value),
        _ => Err(invalid_body()),
    }
}

/// `{data: objeto|lista, apply = false, secrets = {id: {nome: texto}}}`; campo a mais é ignorado.
fn import_body(value: Value) -> Option<ImportRequest> {
    let Value::Object(mut body) = value else { return None };
    let data = body.remove("data").filter(|data| data.is_object() || data.is_array())?;
    let apply = match body.get("apply") {
        None => false,
        Some(value) => lax_bool(value)?,
    };
    let secrets = match body.remove("secrets") {
        None => Map::new(),
        Some(Value::Object(secrets)) => {
            let valid = secrets.values().all(|values| values.as_object().is_some_and(|v| v.values().all(Value::is_string)));
            valid.then_some(secrets)?
        }
        Some(_) => return None,
    };
    Some(ImportRequest { data, apply, secrets })
}

/// `{ids: [texto], scripts = [texto]}`; campo a mais é ignorado.
fn verify_body(value: &Value) -> Option<(Vec<String>, Vec<String>)> {
    let texts = |value: &Value| -> Option<Vec<String>> {
        value.as_array()?.iter().map(|v| v.as_str().map(str::to_owned)).collect()
    };
    let ids = texts(value.as_object()?.get("ids")?)?;
    let scripts = match value.get("scripts") {
        None => Vec::new(),
        Some(scripts) => texts(scripts)?,
    };
    Some((ids, scripts))
}

/// O `bool` do modelo do pedido: aceita também 0/1 e as palavras de sim e não.
fn lax_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(number) => match number.as_f64() {
            Some(n) if n == 0.0 => Some(false),
            Some(n) if n == 1.0 => Some(true),
            _ => None,
        },
        Value::String(text) => hangar_core::query::fastapi_bool(text),
        _ => None,
    }
}

fn invalid(code: &str, motivo: &str) -> Response {
    detail(StatusCode::BAD_REQUEST, code, motivo, json!({"motivo": motivo}))
}

fn invalid_body() -> Response {
    detail(
        StatusCode::UNPROCESSABLE_ENTITY,
        "erro_corpo_invalido",
        "Pedido em formato inválido para esta rota; nada foi alterado.",
        json!({}),
    )
}

/// Falha do disco no export e no import: o mesmo 500 sem código que essas rotas sempre deram.
fn internal(code: &'static str, message: &str) -> Response {
    tracing::warn!(code, "{message}");
    let mut response = (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}

fn verify_failed(message: &str) -> Response {
    tracing::warn!(code = "erro_shortcut_verify_io", "{message}");
    detail(StatusCode::INTERNAL_SERVER_ERROR, "erro_shortcut_verify_io", message, json!({"motivo": message}))
}
