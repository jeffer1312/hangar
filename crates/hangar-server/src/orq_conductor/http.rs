use super::feed_bytes;
use crate::routes::AppState;
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
pub fn matches(method: &Method, path: &str) -> bool {
    *method == Method::GET && exec_id(path).is_some()
}
fn exec_id(path: &str) -> Option<String> {
    let raw = path.strip_prefix("/api/orq/")?.strip_suffix("/conductor")?;
    let id = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .ok()?
        .into_owned();
    (!id.is_empty() && !id.contains('/')).then_some(id)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && !matches!(id, "." | "..") && !id.contains(['/', '\\', ':', '\0'])
}
pub async fn public(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if let Some(mut response) = crate::provider_info::wrong_method(req.method(), req.uri().path(), req.uri().query()) {
        crate::routes::cors(req.headers(), response.headers_mut());
        return response;
    }
    let (forward, owner) = crate::routes::gate(&st, peer, &req);
    if !owner || req.method() != Method::GET {
        return crate::routes::pass(&st, req, &forward).await;
    }
    let headers = req.headers().clone();
    let mut response = serve(&st, req).await;
    crate::routes::cors(&headers, response.headers_mut());
    response
}
fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    crate::session_write::json_response(
        status,
        crate::session_write::detail_body(code, message, json!({})),
    )
}
fn missing() -> Response {
    failure(
        StatusCode::NOT_FOUND,
        "erro_nao_encontrado",
        "execução não encontrada",
    )
}
pub async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    let Some(id) = exec_id(req.uri().path()).filter(|id| valid_id(id)) else {
        return missing();
    };
    let path = crate::orchestration::runs::root(&st.accounts.env.home).join(&id);
    if !path.is_dir() {
        return missing();
    }
    let root = path.clone();
    let watchdog = match tokio::task::spawn_blocking(move || {
        let units = crate::orchestration::runs::units();
        serde_json::to_vec(&crate::orchestration::runs::watchdog(
            &root,
            units.as_ref(),
            crate::orchestration::runs::now(),
        ))
    })
    .await
    {
        Ok(Ok(value)) => value,
        _ => {
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "orq_conductor_watchdog_failed",
                "Não foi possível consultar o condutor.",
            );
        }
    };
    match tokio::task::spawn_blocking(move || feed_bytes(&path, watchdog)).await {
        Ok(Ok(bytes)) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap(),
        outcome => {
            let code = match outcome {
                Ok(Err(code)) => code,
                _ => "orq_conductor_feed_failed",
            };
            st.diag.report(
                "rust.orq_conductor_failed",
                &id,
                code,
                "feed do condutor ilegível",
            );
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                code,
                "Não foi possível ler o feed do condutor.",
            )
        }
    }
}
