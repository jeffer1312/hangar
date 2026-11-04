//! `PUT /api/sessions/prewarm` e `DELETE /api/sessions/prewarm/{id}`: só o dono. Na 8765 o Rust
//! atende; o dono do Connect chega pela ponte `forwarded`; quem não é o dono segue ao Python, que recusa.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::routes::AppState;
use crate::session_open::server::reply;
use crate::session_open::{OpenError, SessionOpenRequest};

pub const PATH: &str = "/api/sessions/prewarm";
const BODY_LIMIT: usize = 1024 * 1024;

pub async fn put(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = match crate::routes::gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    // Como o Mermaid: o Python recusa o convidado (403, fora do bloqueio) e quem não tem credencial (401, que conta).
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let mut response = reserve(&st, req).await;
    crate::routes::cors(&headers, response.headers_mut());
    response
}

pub async fn delete(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>,
                    req: Request) -> Response {
    let (fwd, owner) = match crate::routes::gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let mut response = discard(&st, &id, req.uri()).await;
    crate::routes::cors(req.headers(), response.headers_mut());
    response
}

/// As duas rotas, no caminho que o Python repassa (segmentos escapados pela ponte).
pub(crate) fn matches(method: &Method, path: &str) -> bool {
    (method == Method::PUT && path == PATH) || (method == Method::DELETE && reserve_id(path).is_some())
}

/// O dono do Connect, já autenticado pelo Python.
pub(crate) async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    match reserve_id(req.uri().path()) {
        Some(id) if req.method() == Method::DELETE => discard(st, &id, req.uri()).await,
        _ => reserve(st, req).await,
    }
}

fn reserve_id(path: &str) -> Option<String> {
    let id = path.strip_prefix(PATH)?.strip_prefix('/').filter(|id| !id.is_empty() && !id.contains('/'))?;
    percent_encoding::percent_decode_str(id).decode_utf8().ok().map(|id| id.into_owned())
}

async fn reserve(st: &Arc<AppState>, req: Request) -> Response {
    let headers: HeaderMap = req.headers().clone();
    let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else { return crate::session_write::too_large(&headers) };
    let (request, client_id) = match parse(&bytes) {
        Ok(parsed) => parsed,
        Err(error) => return reply(&error),
    };
    let st = st.clone();
    // Fora da conexão, como a abertura: o cliente que desiste no meio não interrompe a subida do cano.
    let result = tokio::spawn(async move {
        let launcher = super::ServerLauncher::new(st.clone());
        st.prewarm.reserve(&launcher, &request, &client_id).await
    }).await.unwrap_or_else(|_| Err(OpenError::new(500, "erro_criacao_sessao", "a reserva não terminou")));
    match result {
        Ok(ticket) => crate::session_write::json_response(StatusCode::OK,
            json!({"id": ticket.id, "fingerprint": ticket.fingerprint, "name": ticket.name})),
        Err(error) => reply(&error),
    }
}

/// Solta `id` (`-` quando o cliente ainda não tem) e tudo do `?client_id=`. Sempre 204.
async fn discard(st: &Arc<AppState>, id: &str, uri: &Uri) -> Response {
    let client_id = Query::<HashMap<String, String>>::try_from_uri(uri).ok()
        .and_then(|Query(mut query)| query.remove("client_id")).filter(|c| !c.is_empty());
    let (st, id) = (st.clone(), id.to_owned());
    // Destacado, como o `PUT`: o cliente que cai no meio não deixa o processo vivo.
    if let Err(error) = tokio::spawn(async move { st.prewarm.release(&id, client_id.as_deref()).await }).await {
        tracing::warn!(%error, "prewarm: soltar a reserva caiu");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// O corpo da abertura mais `client_id`; `name` e `initial_prompt` são recusados pelo `scope` da reserva.
fn parse(bytes: &[u8]) -> Result<(SessionOpenRequest, String), OpenError> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(|e| OpenError::invalid_body(&e.to_string()))?;
    let client_id = value.as_object_mut().and_then(|body| body.remove("client_id"));
    let Some(Value::String(client_id)) = client_id.filter(|id| id.as_str().is_some_and(|id| !id.is_empty())) else {
        return Err(OpenError::invalid_body("client_id: texto obrigatório"));
    };
    Ok((SessionOpenRequest::parse(value.to_string().as_bytes())?, client_id))
}
