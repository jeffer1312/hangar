//! Rotas do dono para a revisão da sessão. A autenticação vem antes de ler corpo ou sessão; convidado
//! e token errado nunca chegam ao conector.
use crate::engine::{DecideBody, Engine, Failure};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hangar_core::edge::Edge;
use hangar_core::http::{detail, json_response};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

/// 50 decisões com motivo de 2.000 caracteres cabem com folga.
const MAX_BODY: usize = 512 * 1024;

struct Http { edge: Edge, engine: Arc<Engine> }

pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, engine: Arc<Engine>) -> Router<S> {
    Router::new()
        .route("/api/sessions/{name}/work-review", axum::routing::any(public))
        .route("/api/sessions/{name}/work-review/{*rest}", axum::routing::any(public))
        .with_state(Arc::new(Http { edge, engine }))
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
    let socket = peer.unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
    if let Err(response) = state.edge.owner_only(socket, &request, ("work_review_owner_only", "só o dono vê a revisão do trabalho")).await {
        return response;
    }
    let segments: Vec<String> = request.uri().path().trim_start_matches('/').split('/')
        .map(|part| percent_encoding::percent_decode_str(part).decode_utf8_lossy().into_owned()).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let method = request.method().clone();
    match (method, &segments[..]) {
        (Method::GET, ["api", "sessions", name, "work-review"]) => match state.engine.session_review(name).await {
            Ok(None) => json_response(StatusCode::OK, json!({"linked": false})),
            Ok(Some(linked)) => {
                let mut value = json!(linked);
                value["linked"] = json!(true);
                json_response(StatusCode::OK, value)
            }
            Err(failure) => error(&failure),
        },
        (Method::GET, ["api", "sessions", name, "work-review", "assets", asset]) => {
            let Some(review) = form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .find(|(key, _)| key == "review_id").map(|(_, value)| value.into_owned()) else {
                return detail(StatusCode::BAD_REQUEST, "invalid_params", "falta o review_id", json!({}));
            };
            match state.engine.asset(name, asset, &review).await {
                Ok(image) => {
                    let mut response = (StatusCode::OK, image.bytes).into_response();
                    let headers = response.headers_mut();
                    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&image.mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
                    // Imutável dentro do `review_id` (contrato): o app guarda sem perguntar de novo.
                    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400, immutable"));
                    response
                }
                Err(failure) => error(&failure),
            }
        }
        (Method::POST, ["api", "sessions", name, "work-review", "decisions"]) => {
            let name = (*name).to_owned();
            let Ok(bytes) = to_bytes(request.into_body(), MAX_BODY).await else {
                return detail(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "corpo acima de 512 KiB", json!({}));
            };
            let body: DecideBody = match serde_json::from_slice(&bytes) {
                Ok(body) => body,
                Err(error) => return detail(StatusCode::BAD_REQUEST, "invalid_params", &format!("corpo inválido: {error}"), json!({})),
            };
            match state.engine.decide(&name, body).await {
                Ok(review) => json_response(StatusCode::OK, json!({"ok": true, "review": review})),
                Err(failure) => error(&failure),
            }
        }
        (_, ["api", "sessions", _, "work-review"]) | (_, ["api", "sessions", _, "work-review", "decisions"])
        | (_, ["api", "sessions", _, "work-review", "assets", _]) => detail(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "método não aceito nesta rota", json!({})),
        _ => detail(StatusCode::NOT_FOUND, "not_found", "rota desconhecida", json!({})),
    }
}

fn error(failure: &Failure) -> Response {
    match failure {
        Failure::Call(error) => hangar_connectors::http::call_error(error),
        Failure::Mission(error) => hangar_mission::http::failure(error),
        Failure::NotLinked => detail(StatusCode::NOT_FOUND, "work_review_not_linked", "esta sessão não está ligada a um ticket", json!({})),
    }
}
