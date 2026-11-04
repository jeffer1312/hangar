//! `/api/device-install/*`: só o dono, pelo token, mesmo em loopback. Nada vai ao Python: a senha
//! da Apple ID não passa por repasse nem pelo diário dele.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroize;

use super::apple::CodeReply;
use super::job::{BAD_REQUEST, BUSY, Failure, FORBIDDEN, NOT_WAITING, SERVICE_MISSING, Start, UNKNOWN_JOB};
use super::DeviceError;
use crate::routes::AppState;

const BODY_LIMIT: usize = 64 * 1024;

fn reply(status: StatusCode, body: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

fn failure(status: StatusCode, f: Failure) -> Response {
    reply(status, json!({"error": f}))
}

fn bad_body() -> Response {
    failure(StatusCode::UNPROCESSABLE_ENTITY, Failure::new(BAD_REQUEST))
}

/// Recusa repasse (Python, convidado), outra origem e quem não traz o token do dono.
fn guard(st: &AppState, peer: SocketAddr, req: &Request)
    -> impl std::future::Future<Output = Result<(), Response>> + Send + use<> {
    let h = req.headers();
    let forbidden = crate::routes::on_guest_port(req) || h.contains_key("x-forwarded-for") || h.contains_key("x-hangar-guest") || !same_origin(h);
    let ip = peer.ip().to_canonical().to_string();
    let token = crate::auth::presented_token(h, req.uri().query(), req.method(), false);
    let auth = st.auth.clone();
    async move {
    if forbidden { return Err(failure(StatusCode::FORBIDDEN, Failure::new(FORBIDDEN))); }
    if !auth.owner(&ip, token.as_deref()).await.map_err(crate::auth::unavailable)? {
        auth.record_failure(&ip).await.map_err(crate::auth::unavailable)?;
        return Err(failure(StatusCode::UNAUTHORIZED, Failure::new("unauthorized")));
    }
    Ok(())
    }
}

/// Sem Origin (app nativo) passa; com ela, só a página servida por este mesmo endereço.
fn same_origin(h: &HeaderMap) -> bool {
    let Some(origin) = h.get(header::ORIGIN) else { return true };
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).map(str::trim);
    let netloc = origin.to_str().ok().and_then(|o| o.split_once("://")).map(|(_, rest)| rest.trim_end_matches('/'));
    netloc.is_some_and(|n| Some(n) == host)
}

async fn body<T: for<'de> Deserialize<'de>>(req: Request) -> Option<T> {
    let bytes = to_bytes(req.into_body(), BODY_LIMIT).await.ok()?;
    // O corpo do `/start` leva a senha: zerado logo depois de lido.
    let mut raw = bytes.try_into_mut().unwrap_or_else(|b| b[..].into());
    let parsed = serde_json::from_slice(&raw).ok();
    raw.as_mut().zeroize();
    parsed
}

fn device_failure(e: DeviceError) -> Response {
    let f = Failure::from(e);
    let status = if f.code == SERVICE_MISSING { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CONFLICT };
    failure(status, f)
}

pub async fn devices(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(r) = guard(&st, peer, &req).await { return r; }
    match st.device_install.link.list().await {
        Ok(devices) => reply(StatusCode::OK, json!({"service": "ok", "devices": devices})),
        Err(DeviceError::ServiceMissing) => reply(StatusCode::OK, json!({"service": "missing", "devices": []})),
        Err(e) => device_failure(e),
    }
}

#[derive(Deserialize)]
struct Udid {
    udid: String,
}

pub async fn trust(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(r) = guard(&st, peer, &req).await { return r; }
    let Some(Udid { udid }) = body(req).await else { return bad_body() };
    match st.device_install.link.request_trust(&udid).await {
        Ok(()) => reply(StatusCode::OK, json!({"ok": true})),
        Err(e) => device_failure(e),
    }
}

pub async fn start(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(r) = guard(&st, peer, &req).await { return r; }
    let Some(start) = body::<Start>(req).await else { return bad_body() };
    if start.udid.is_empty() || start.apple_id.trim().is_empty() || start.password.is_empty() {
        return bad_body();
    }
    match st.device_install.start(start) {
        Ok(job) => reply(StatusCode::OK, json!({"job": job})),
        Err(f) if f.code == BUSY => failure(StatusCode::CONFLICT, f),
        Err(f) => failure(StatusCode::INTERNAL_SERVER_ERROR, f),
    }
}

#[derive(Deserialize)]
struct Code {
    job: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    resend: bool,
}

pub async fn code(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(r) = guard(&st, peer, &req).await { return r; }
    let Some(Code { job, code, resend }) = body(req).await else { return bad_body() };
    let reply_to = if resend {
        CodeReply::Resend
    } else if !code.trim().is_empty() {
        CodeReply::Submit(code.trim().to_string())
    } else {
        return bad_body();
    };
    match st.device_install.answer(&job, reply_to) {
        Ok(()) => reply(StatusCode::OK, json!({"ok": true})),
        Err(f) if f.code == UNKNOWN_JOB => failure(StatusCode::NOT_FOUND, f),
        Err(f) if f.code == NOT_WAITING => failure(StatusCode::CONFLICT, f),
        Err(f) => failure(StatusCode::INTERNAL_SERVER_ERROR, f),
    }
}

#[derive(Deserialize)]
struct Certs {
    job: String,
    #[serde(default)]
    revoke: Vec<String>,
    #[serde(default)]
    cancel: bool,
}

pub async fn certs(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(r) = guard(&st, peer, &req).await { return r; }
    let Some(Certs { job, revoke, cancel }) = body(req).await else { return bad_body() };
    let choice = match (cancel, revoke.is_empty()) {
        (true, true) => None,
        (false, false) => Some(revoke),
        _ => return bad_body(),
    };
    match st.device_install.choose_certs(&job, choice) {
        Ok(()) => reply(StatusCode::OK, json!({"ok": true})),
        Err(f) if f.code == UNKNOWN_JOB => failure(StatusCode::NOT_FOUND, f),
        Err(f) if f.code == NOT_WAITING => failure(StatusCode::CONFLICT, f),
        Err(f) if f.code == BAD_REQUEST => failure(StatusCode::UNPROCESSABLE_ENTITY, f),
        Err(f) => failure(StatusCode::INTERNAL_SERVER_ERROR, f),
    }
}

pub async fn progress(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(r) = guard(&st, peer, &req).await { return r; }
    let job = crate::auth::query_param(req.uri().query(), "job").unwrap_or_default();
    match st.device_install.progress(&job) {
        Some(p) => reply(StatusCode::OK, json!(p)),
        None => failure(StatusCode::NOT_FOUND, Failure::new(UNKNOWN_JOB)),
    }
}
