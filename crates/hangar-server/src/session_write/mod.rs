//! Rotas de escrita de sessão Claude. O Rust as reivindica na tabela (`table`) e decide por pedido;
//! `/input` e `/steer` (`input`), o controle (`control`) e `/answer` (`answer`) têm corpo no Rust;
//! as rotas só do Codex sem terminal moram em `codex`; o que o Rust admite mas não atende (corpo que o
//! FastAPI recusa) volta ao Python por `relay`.
//!
//! Ordem fixa de `admit`: dono → corpo → porta (`enter`) → entrada (`writable`) → decisão. A entrada
//! é procurada DEPOIS da porta: a achada antes de esperar pode ser a que o relançamento parou.
//! Repasse nunca acontece com o passe na mão: o `freeze` do Python fecha a porta e esperaria o
//! nosso próprio passe.
pub mod answer;
pub mod codex;
pub mod control;
pub mod input;
pub mod table;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::Request;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};

use crate::proxy::Forward;
use crate::routes::{AppState, cors, gate, pass};
use crate::runtime::gateway::{RuntimeRegistry, WriteTarget};
use crate::runtime::ingress::{GateClosed, IngressPass};
pub use table::{Owner, Provider, WriteRoute, decide};

/// Mesmo teto do middleware de corpo do Python (`uploads.MAX_BYTES`), que recusa antes da rota.
pub(crate) const BODY_LIMIT: usize = 100 * 1024 * 1024;
const BUSY_MSG: &str = "A sessão está trocando de agente; tente novamente quando terminar.";

/// O que a rota precisa para escrever: o passe mantém a porta aberta até o fim da escrita.
pub(crate) struct Ctx {
    pub(crate) st: Arc<AppState>,
    pub(crate) name: String,
    pub(crate) target: WriteTarget,
    pub(crate) pass: IngressPass,
    parts: Parts,
    fwd: Forward,
}

impl Ctx {
    pub(crate) fn headers(&self) -> &axum::http::HeaderMap { &self.parts.headers }
}

/// Envelope do `HTTPException(detail=erro(...))` do Python (`mensagens.py`).
pub(crate) fn detail(status: StatusCode, code: &str, msg: &str, params: Value) -> Response {
    json_response(status, detail_body(code, msg, params))
}

pub fn detail_body(code: &str, msg: &str, params: Value) -> Value {
    json!({"detail": {"code": code, "params": params, "msg": msg}})
}

pub(crate) fn json_response(status: StatusCode, body: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// Entra na porta e SÓ ENTÃO procura a entrada; sem entrada o passe volta a sair (solto aqui).
pub async fn enter_then_find(runtime: &RuntimeRegistry, name: &str, wait: Duration) -> Result<Option<(IngressPass, WriteTarget)>, GateClosed> {
    let pass = runtime.ingress().enter(name, wait).await?;
    Ok(runtime.writable(name).await.map(|target| (pass, target)))
}

/// Nome da sessão em `/api/sessions/{name}/...`, já sem o escape da URL.
pub(crate) fn session_name(path: &str) -> Option<String> {
    let raw = path.strip_prefix("/api/sessions/")?.split('/').next().filter(|n| !n.is_empty())?;
    percent_decode_str(raw).decode_utf8().ok().map(|n| n.into_owned())
}

async fn forward_whole(st: &AppState, parts: Parts, bytes: Bytes, fwd: &Forward) -> Response {
    pass(st, Request::from_parts(parts, Body::from(bytes)), fwd).await
}

/// Decide quem atende. `Ok`: o Rust atende, com o passe na mão e o corpo já lido. `Err`: a resposta
/// pronta (repasse ao Python com o corpo intacto, ou a recusa da porta fechada).
pub(crate) async fn admit(st: &Arc<AppState>, peer: SocketAddr, req: Request, route: WriteRoute,
    early: impl Fn(&Bytes) -> bool) -> Result<(Ctx, Bytes), Response> {
    let (fwd, owner) = gate(st, peer, &req);
    let runtime = st.state.runtime.get().cloned();
    let (Some(runtime), true, Some(name)) = (runtime, owner, session_name(req.uri().path())) else {
        return Err(pass(st, req, &fwd).await);
    };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else { return Err(too_large(&parts.headers)) };
    if !table::body_ok(route, &bytes) || early(&bytes) {
        return Err(forward_whole(st, parts, bytes, &fwd).await);
    }
    match route_write(&runtime, &name, route, st.write_gate_wait).await {
        Write::Rust(pass_in, target) => Ok((Ctx { st: st.clone(), name, target, pass: pass_in, parts, fwd }, bytes)),
        Write::Python => Err(forward_whole(st, parts, bytes, &fwd).await),
        Write::Busy => {
            let mut response = json_response(StatusCode::CONFLICT, busy_body());
            cors(&parts.headers, response.headers_mut());
            Err(response)
        }
    }
}

/// Igual ao `_BodySizeLimitMiddleware`: texto puro, e o CORS por fora dele.
pub(crate) fn too_large(headers: &axum::http::HeaderMap) -> Response {
    let mut response = (StatusCode::PAYLOAD_TOO_LARGE, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "request body too large").into_response();
    cors(headers, response.headers_mut());
    response
}

/// Quem atende uma escrita de corpo já aceito.
pub(crate) enum Write { Rust(IngressPass, WriteTarget), Python, Busy }

/// Porta, entrada e tabela, nessa ordem. O passe só volta com `Rust`: repasse nunca o segura.
pub(crate) async fn route_write(runtime: &RuntimeRegistry, name: &str, route: WriteRoute, wait: Duration) -> Write {
    match enter_then_find(runtime, name, wait).await {
        Err(GateClosed) => Write::Busy,
        Ok(None) => Write::Python,
        Ok(Some((pass_in, target))) => {
            let rust = Provider::from_str(&target.provider).is_some_and(|p| decide(route, p, target.terminal, target.healthy) == Owner::Rust);
            if rust { Write::Rust(pass_in, target) } else { Write::Python }
        }
    }
}

pub(crate) fn busy_body() -> Value { detail_body("session_transfer_busy", BUSY_MSG, json!({})) }

/// Repassa ao Python o que o Rust admitiu mas não atende (corpo que o FastAPI recusa), soltando
/// antes o passe.
pub(crate) async fn relay(ctx: Ctx, bytes: Bytes) -> Response {
    let Ctx { st, pass: held, parts, fwd, .. } = ctx;
    drop(held);
    forward_whole(&st, parts, bytes, &fwd).await
}
