//! Resgate do bilhete e aparelhos do dono. Falha aqui é resposta com código, nunca repasse ao Python.
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::devices::{Poll, Refusal, Store};
use super::gate::DeviceCaller;
use crate::routes::{AppState, cors, on_connect_port, owner_gate};
use crate::session_write::json_response;
use crate::voice::routes::preflight;

const BODY_LIMIT: usize = 16 * 1024;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/connect/redeem", post(redeem).options(preflight))
        .route("/api/connect/redeem/status", post(status).options(preflight))
        .route("/api/connect/devices", get(list).options(preflight))
        .route("/api/connect/devices/self", post(enroll).options(preflight))
        .route("/api/connect/devices/{id}", delete(remove).options(preflight))
        .route("/api/connect/devices/{id}/approve", post(approve).options(preflight))
}

pub fn matches(method: &Method, path: &str) -> bool {
    let id = path.strip_prefix("/api/connect/devices/");
    match method.as_str() {
        "GET" => path == "/api/connect/devices",
        "POST" => matches!(path, "/api/connect/redeem" | "/api/connect/redeem/status" | "/api/connect/devices/self")
            || id.and_then(|rest| rest.strip_suffix("/approve")).is_some_and(|id| !id.is_empty() && !id.contains('/')),
        "DELETE" => id.is_some_and(|id| !id.is_empty() && !id.contains('/')),
        _ => false,
    }
}

fn reply(headers: &HeaderMap, status: StatusCode, body: Value) -> Response {
    let mut response = json_response(status, body);
    cors(headers, response.headers_mut());
    response
}

fn error(headers: &HeaderMap, status: StatusCode, code: &str, msg: &str) -> Response {
    reply(headers, status, json!({"detail": {"code": code, "params": {}, "msg": msg}}))
}

fn refused(headers: &HeaderMap, refusal: Refusal) -> Response {
    match refusal {
        Refusal::NotLinked => error(headers, StatusCode::CONFLICT, "connect_not_linked", "esta máquina não está ligada ao Connect"),
        Refusal::Ticket => error(headers, StatusCode::UNAUTHORIZED, "connect_ticket_refused", "bilhete recusado"),
        Refusal::TooManyPending => error(headers, StatusCode::TOO_MANY_REQUESTS, "connect_too_many_pending", "muitos aparelhos esperando aprovação"),
        Refusal::Credential => error(headers, StatusCode::UNAUTHORIZED, "connect_device_refused", "aparelho ou segredo recusado"),
        Refusal::Unknown => error(headers, StatusCode::NOT_FOUND, "connect_device_unknown", "aparelho desconhecido"),
        Refusal::Io(cause) => {
            tracing::warn!(code = "connect_store_failed", "os aparelhos do Connect não foram gravados: {cause}");
            error(headers, StatusCode::SERVICE_UNAVAILABLE, "connect_store_failed", "os aparelhos do Connect não foram gravados")
        }
    }
}

async fn parse<T: DeserializeOwned>(req: Request) -> Result<T, Response> {
    let headers = req.headers().clone();
    let invalid = || error(&headers, StatusCode::BAD_REQUEST, "connect_body_invalid", "corpo inválido");
    let bytes = to_bytes(req.into_body(), BODY_LIMIT).await.map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

/// A loja grava com fsync: fora do executor assíncrono.
async fn blocking<T: Send + 'static>(st: &AppState, op: impl FnOnce(&Store) -> Result<T, Refusal> + Send + 'static) -> Result<T, Refusal> {
    let store = st.connect_devices.clone();
    tokio::task::spawn_blocking(move || op(&*store)).await.unwrap_or_else(|cause| Err(Refusal::Io(io::Error::other(cause))))
}

// `device_name` do corpo fica de fora de propósito: quem aparece na aprovação é o nome assinado no bilhete.
#[derive(Deserialize)]
struct RedeemBody { ticket: String, signature: String, secret: String }

async fn redeem(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let headers = req.headers().clone();
    let body: RedeemBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    match blocking(&st, move |store| store.redeem(&body.ticket, &body.signature, &body.secret)).await {
        Ok(device_id) => reply(&headers, StatusCode::ACCEPTED, json!({"device_id": device_id, "status": "pending"})),
        Err(refusal) => refused(&headers, refusal),
    }
}

#[derive(Deserialize)]
struct StatusBody { device_id: String, secret: String }

async fn status(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let headers = req.headers().clone();
    let body: StatusBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    let body = match blocking(&st, move |store| store.status(&body.device_id, &body.secret)).await {
        Ok(Poll::Pending) => json!({"status": "pending"}),
        Ok(Poll::Denied) => json!({"status": "denied"}),
        Ok(Poll::Approved(Some(token))) => json!({"status": "approved", "token": token}),
        Ok(Poll::Approved(None)) => json!({"status": "approved"}),
        Err(refusal) => return refused(&headers, refusal),
    };
    reply(&headers, StatusCode::OK, body)
}

async fn list(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(mut response) = owner_gate(&st, peer, &req).await {
        cors(&headers, response.headers_mut());
        return response;
    }
    reply(&headers, StatusCode::OK, json!(st.connect_devices.list()))
}

async fn approve(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(mut response) = owner_gate(&st, peer, &req).await {
        cors(&headers, response.headers_mut());
        return response;
    }
    let by = req.extensions().get::<DeviceCaller>().map(|device| device.id.clone());
    match blocking(&st, move |store| store.approve(&id, by.as_deref())).await {
        Ok(()) => reply(&headers, StatusCode::OK, json!({"ok": true})),
        Err(refusal) => refused(&headers, refusal),
    }
}

async fn remove(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(mut response) = owner_gate(&st, peer, &req).await {
        cors(&headers, response.headers_mut());
        return response;
    }
    match blocking(&st, move |store| store.remove(&id)).await {
        Ok(()) => reply(&headers, StatusCode::OK, json!({"ok": true})),
        Err(refusal) => refused(&headers, refusal),
    }
}

#[derive(Deserialize)]
struct EnrollBody { device_name: String }

/// Sem aprovação porque quem pede apresentou a senha. Aparelho não entra aqui: quem for revogado
/// sairia com outra credencial guardada.
async fn enroll(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if on_connect_port(&req) {
        return error(&headers, StatusCode::UNAUTHORIZED, "erro_nao_autorizado", "unauthorized");
    }
    if req.extensions().get::<DeviceCaller>().is_some() {
        return error(&headers, StatusCode::FORBIDDEN, "connect_owner_secret_refused", "rota só do dono");
    }
    if let Err(mut response) = owner_gate(&st, peer, &req).await {
        cors(&headers, response.headers_mut());
        return response;
    }
    let body: EnrollBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    match blocking(&st, move |store| store.enroll(&body.device_name)).await {
        Ok((device_id, token)) => reply(&headers, StatusCode::OK, json!({"device_id": device_id, "token": token})),
        Err(refusal) => refused(&headers, refusal),
    }
}
