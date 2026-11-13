//! Rotas do dono para os conectores. A autenticação vem antes de ler corpo, manifesto ou comando;
//! convidado e token errado nunca chegam ao host.
use crate::error::{AdminError, AdminKind, CallError};
use crate::host::Host;
use crate::manifest::Manifest;
use crate::protocol::ErrorCode;
use crate::work_tracking::{GetParams, ListParams, with_fetched_at};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_core::edge::Edge;
use hangar_core::http::{detail, json_response};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;

const MAX_BODY: usize = 64 * 1024;

struct Http { edge: Edge, host: Arc<Host> }

pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, host: Arc<Host>) -> Router<S> {
    Router::new()
        .route("/api/connectors", axum::routing::any(public))
        .route("/api/connectors/{*rest}", axum::routing::any(public))
        .with_state(Arc::new(Http { edge, host }))
}

async fn public(
    State(state): State<Arc<Http>>,
    peer: Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>,
    request: Request,
) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    let headers = request.headers().clone();
    let mut response = reply(&state, peer.ok().map(|peer| peer.0), request).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    response
}

async fn reply(state: &Http, peer: Option<SocketAddr>, request: Request) -> Response {
    let socket = peer.unwrap_or_else(|| "0.0.0.0:0".parse().expect("endereço estático válido"));
    match state.edge.gate(socket, &request).await {
        Ok((_, true)) => {}
        Ok((_, false)) => match state.edge.owner_gate(socket, &request).await {
            // Autenticado, mas não é o dono (convidado): recusa sem contar falha de senha.
            Ok(_) => return detail(StatusCode::FORBIDDEN, "connectors_owner_only", "só o dono administra e consulta conectores", json!({})),
            Err(response) => return response,
        },
        Err(error) => return hangar_core::auth::unavailable(error),
    }
    let segments: Vec<String> = request.uri().path().trim_start_matches('/').split('/')
        .map(|part| percent_encoding::percent_decode_str(part).decode_utf8_lossy().into_owned()).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let method = request.method().clone();
    match (method, &segments[..]) {
        (Method::GET, ["api", "connectors"]) => match state.host.list() {
            Ok(list) => json_response(StatusCode::OK, json!({"connectors": list})),
            Err(error) => admin_error(&error),
        },
        (Method::POST, ["api", "connectors"]) => {
            let body = match body(request).await { Ok(body) => body, Err(response) => return response };
            let Some(path) = body.get("manifest").and_then(Value::as_str) else {
                return detail(StatusCode::BAD_REQUEST, "manifest_invalid", "corpo precisa de \"manifest\": caminho absoluto no servidor", json!({}));
            };
            if !std::path::Path::new(path).is_absolute() {
                return detail(StatusCode::BAD_REQUEST, "manifest_invalid", "o caminho do manifesto precisa ser absoluto", json!({}));
            }
            match Manifest::load(std::path::Path::new(path)).and_then(|manifest| state.host.add(manifest)) {
                Ok(entry) => json_response(StatusCode::CREATED, json!({"connector": entry})),
                Err(error) => admin_error(&error),
            }
        }
        (Method::PATCH, ["api", "connectors", id]) => {
            let body = match body(request).await { Ok(body) => body, Err(response) => return response };
            let Some(enabled) = body.get("enabled").and_then(Value::as_bool) else {
                return detail(StatusCode::BAD_REQUEST, "invalid_params", "corpo precisa de \"enabled\": true ou false", json!({}));
            };
            match state.host.set_enabled(id, enabled).await {
                Ok(entry) => json_response(StatusCode::OK, json!({"connector": entry})),
                Err(error) => admin_error(&error),
            }
        }
        (Method::DELETE, ["api", "connectors", id]) => match state.host.remove(id).await {
            Ok(entry) => json_response(StatusCode::OK, json!({"connector": entry})),
            Err(error) => admin_error(&error),
        },
        (Method::GET, ["api", "connectors", id, "tickets"]) => {
            let pairs = form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes());
            let params = match ListParams::from_pairs(pairs) { Ok(params) => params, Err(error) => return call_error(&error.into()) };
            match state.host.list_tickets(id, params).await {
                Ok(result) => json_response(StatusCode::OK, with_fetched_at(json!(result))),
                Err(error) => call_error(&error),
            }
        }
        (Method::GET, ["api", "connectors", id, "tickets", ticket_id]) => {
            let params = match GetParams::new(*ticket_id) { Ok(params) => params, Err(error) => return call_error(&error.into()) };
            match state.host.get_ticket(id, params).await {
                Ok(detail) => {
                    let mut value = with_fetched_at(json!(detail));
                    // Lido depois da chamada, que já conferiu o cadastro: falhar aqui só tira a sugestão.
                    if let Some(prompt) = state.host.entry(id).ok().and_then(|entry| entry.start_prompt_for(&detail.summary)) {
                        value["start_prompt"] = json!(prompt);
                    }
                    json_response(StatusCode::OK, value)
                }
                Err(error) => call_error(&error),
            }
        }
        (Method::PUT, ["api", "connectors", id, "links", ticket_id]) => {
            let body = match body(request).await { Ok(body) => body, Err(response) => return response };
            let Some(session) = body.get("session").and_then(Value::as_str).filter(|name| valid_session(name)) else {
                return detail(StatusCode::BAD_REQUEST, "invalid_params", "corpo precisa de \"session\": nome de sessão", json!({}));
            };
            link(state, id, ticket_id, Some(session))
        }
        (Method::DELETE, ["api", "connectors", id, "links", ticket_id]) => link(state, id, ticket_id, None),
        (_, ["api", "connectors", ..]) if segments.len() <= 5 => detail(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "método não aceito nesta rota", json!({})),
        _ => detail(StatusCode::NOT_FOUND, "not_found", "rota desconhecida", json!({})),
    }
}

fn link(state: &Http, id: &str, ticket_id: &str, session: Option<&str>) -> Response {
    if let Err(error) = GetParams::new(ticket_id) { return call_error(&error.into()); }
    match state.host.link(id, ticket_id, session) {
        Ok(entry) => json_response(StatusCode::OK, json!({"connector": entry})),
        Err(error) => admin_error(&error),
    }
}

/// Mesmo formato dos nomes de sessão do Hangar.
fn valid_session(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

async fn body(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), MAX_BODY).await
        .map_err(|_| detail(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "corpo acima de 64 KiB", json!({})))?;
    serde_json::from_slice::<Value>(&bytes).ok().filter(Value::is_object)
        .ok_or_else(|| detail(StatusCode::BAD_REQUEST, "invalid_params", "corpo precisa ser um objeto JSON", json!({})))
}

fn admin_error(error: &AdminError) -> Response {
    let status = match error.kind {
        AdminKind::ConnectorNotFound => StatusCode::NOT_FOUND,
        AdminKind::ConnectorExists => StatusCode::CONFLICT,
        AdminKind::RegistryInvalid | AdminKind::RegistryUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        AdminKind::ManifestInvalid | AdminKind::ManifestUnreadable | AdminKind::UnsupportedVersion | AdminKind::UnsupportedCapability => StatusCode::BAD_REQUEST,
    };
    detail(status, error.code(), &error.message, json!({}))
}

fn call_error(error: &CallError) -> Response {
    let status = match error {
        CallError::Admin(error) => return admin_error(error),
        CallError::Disabled(_) => StatusCode::CONFLICT,
        CallError::Connector(error) => match error.code {
            ErrorCode::InvalidParams | ErrorCode::InvalidCursor | ErrorCode::UnsupportedCapability => StatusCode::BAD_REQUEST,
            // Não é 401: o cliente leria como senha do Hangar errada.
            ErrorCode::AuthRequired | ErrorCode::PermissionDenied => StatusCode::FAILED_DEPENDENCY,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::UnsupportedVersion | ErrorCode::ProtocolError | ErrorCode::PayloadTooLarge => StatusCode::BAD_GATEWAY,
        },
    };
    detail(status, error.code(), &error.message(), json!({}))
}
