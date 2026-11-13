use std::{net::SocketAddr, sync::Arc};
use axum::{Router, extract::{State, Request, ConnectInfo}, response::Response, http::{Method, StatusCode}};
use hangar_core::{edge::Edge, http::{detail, json_response}};
use serde_json::json;
use crate::{Engine, Failure};

struct Http { edge: Edge, engine: Arc<Engine> }

pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, engine: Arc<Engine>) -> Router<S> {
    Router::new().route("/api/mission/actions", axum::routing::any(public))
        .route("/api/mission/actions/{id}", axum::routing::any(public))
        .with_state(Arc::new(Http { edge, engine }))
}

async fn public(State(state): State<Arc<Http>>, peer: Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>, request: Request) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(request.method(), request.headers()) { return response; }
    let headers = request.headers().clone();
    let mut response = reply(&state, peer.ok().map(|p| p.0), request).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    response
}

async fn reply(state: &Http, peer: Option<SocketAddr>, request: Request) -> Response {
    if let Err(response) = state.edge.owner_only(peer.unwrap_or_else(|| SocketAddr::from(([0,0,0,0],0))), &request,
        ("ticket_actions_owner_only", "só o dono aciona os tickets")).await { return response; }
    let path = request.uri().path();
    match (request.method(), path.strip_prefix("/api/mission/actions")) {
        (&Method::POST, Some("")) => {
            let bytes = match axum::body::to_bytes(request.into_body(), 64*1024).await {
                Ok(bytes) => bytes,
                Err(_) => return error(Failure::new(413, "ticket_action_invalid", "pedido acima de 64 KiB")),
            };
            let request = match serde_json::from_slice(&bytes) {
                Ok(request) => request,
                Err(_) => return error(Failure::new(422, "ticket_action_invalid", "pedido de ação inválido")),
            };
            match state.engine.start(request) {
                Ok(attempt) => json_response(StatusCode::ACCEPTED, json!({"attempt": attempt})),
                Err(failure) => error(failure),
            }
        }
        (&Method::GET, Some("")) => {
            let pairs: Vec<_> = form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes()).collect();
            let connector = pairs.iter().find(|(k,_)| k == "connector").map(|(_,v)| v.as_ref()).unwrap_or_default();
            let ticket = pairs.iter().find(|(k,_)| k == "ticket_id").map(|(_,v)| v.as_ref()).unwrap_or_default();
            match state.engine.latest(connector, ticket) {
                Ok(attempt) => match state.engine.latest_execution(connector,ticket) {
                    Ok(execution)=>json_response(StatusCode::OK,json!({"attempt":attempt,"execution":execution})),
                    Err(failure)=>error(failure),
                },
                Err(failure) => error(failure),
            }
        }
        (&Method::GET, Some(id)) if id.starts_with('/') => match state.engine.read(&id[1..]) {
            Ok(attempt) => json_response(StatusCode::OK, json!({"attempt": attempt})),
            Err(failure) => error(failure),
        },
        _ => error(Failure::new(405, "method_not_allowed", "método não aceito nesta rota")),
    }
}

fn error(failure: Failure) -> Response {
    detail(StatusCode::from_u16(failure.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), &failure.code, &failure.message, json!({}))
}
