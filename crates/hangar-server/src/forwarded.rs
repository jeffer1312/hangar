//! Pedido do dono, do convidado ou do Connect que entrou pelas portas do Python, já autenticado e
//! filtrado lá, para uma rota que o Rust atende. O caminho original vem em `x-hangar-path`.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::routes::AppState;

pub async fn private(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // O Starlette manda o caminho já decodificado: cada segmento volta a ser escapado, senão um
    // nome de sessão com espaço não vira URI e um `%` seria decodificado duas vezes.
    let Some(path) = req.headers().get("x-hangar-path").and_then(|v| std::str::from_utf8(v.as_bytes()).ok())
        .filter(|p| p.starts_with("/api/"))
        .map(|p| p.split('/').map(crate::groups::exit::segment).collect::<Vec<_>>().join("/"))
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let method = req.method();
    let owned = if method == Method::GET && path == crate::claude_customizations::CATALOG_PATH {
        Owned::Catalog
    } else if matches!(*method, Method::GET | Method::PUT) && path == "/api/session-defaults" {
        Owned::SessionDefaults
    } else if method == Method::POST && path == "/api/push/expo/subscribe" {
        Owned::PushExpo
    } else if method == Method::GET && path == "/api/jev/delivery" {
        Owned::JevDelivery
    } else if method == Method::GET && path == "/api/client-api" {
        Owned::ClientApi
    } else if crate::orq_conductor::matches(method, &path) {
        Owned::Conductor
    } else if method == Method::GET && crate::commands::matches(&path) {
        Owned::Commands
    } else if method == Method::POST && path == crate::mermaid::PATH {
        Owned::Mermaid
    } else if crate::prewarm::http::matches(method, &path) {
        Owned::Prewarm
    } else if crate::nav_requests::matches(method, &path) {
        Owned::Nav
    } else if crate::worktree_routes::matches(method, &path) {
        Owned::Worktrees
    } else if crate::plans::routes::matches(method, &path) {
        Owned::Plans
    } else if let Some((project, sid)) = archive_resume(method, &path) {
        Owned::ArchiveResume(project, sid)
    } else if let Some(name) = live_resume(method, &path) {
        Owned::Resume(name)
    } else if crate::groups::federation::forwarded_route(method, &path) {
        Owned::Groups(req.headers().get("x-hangar-forwarded-guest").is_some_and(|v| v == "1"))
    } else if crate::session_media::is_media(method, &path) {
        Owned::Media(path.clone(), req.headers().get("x-hangar-forwarded-guest").is_some_and(|v| v == "1"))
    } else {
        return crate::session_write::json_response(StatusCode::NOT_FOUND, json!({"code": "forwarded_route_not_owned"}));
    };
    let target = match req.uri().query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let Ok(uri) = target.parse() else { return StatusCode::BAD_REQUEST.into_response() };
    *req.uri_mut() = uri;
    req.headers_mut().remove("x-hangar-internal");
    req.headers_mut().remove("x-hangar-path");
    req.headers_mut().remove("x-hangar-forwarded-guest");
    match owned {
        Owned::Media(path, guest) => crate::session_media::serve_forwarded(&st, &path, req, guest).await
            .unwrap_or_else(|| crate::session_write::json_response(StatusCode::NOT_FOUND, json!({"code": "forwarded_route_not_owned"}))),
        Owned::JevDelivery => crate::session_write::delivery_timing::availability_reply(&st).await,
        Owned::Groups(guest) => crate::groups::federation::serve_forwarded(st, peer, req, guest).await,
        Owned::ClientApi => crate::routes::client_api_reply(),
        Owned::Catalog => crate::claude_customizations::public_catalog(&st, req).await,
        Owned::SessionDefaults => crate::session_defaults::serve(&st, req).await,
        Owned::PushExpo => crate::push_expo::serve(&st, req).await,
        Owned::Conductor => crate::orq_conductor::serve(&st, req).await,
        Owned::Commands => crate::commands::serve(&st, req).await,
        Owned::Mermaid => crate::mermaid::serve(&st, req).await,
        Owned::Prewarm => crate::prewarm::http::serve(&st, req).await,
        Owned::Nav => crate::nav_requests::serve(&st, req).await,
        Owned::Worktrees => crate::worktree_routes::public(st, req).await,
        Owned::Plans => crate::plans::routes::serve(&st, req).await,
        Owned::ArchiveResume(project, sid) => {
            let Ok(bytes) = axum::body::to_bytes(req.into_body(), crate::session_write::BODY_LIMIT).await else {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            crate::session_open::server::serve_archive_resume(&st, &project, &sid, &bytes).await
        }
        Owned::Resume(name) => {
            let Ok(bytes) = axum::body::to_bytes(req.into_body(), crate::session_write::BODY_LIMIT).await else {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            let (status, body) = crate::session_write::resume::serve(&st, &name, &bytes).await;
            crate::session_write::json_response(status, body)
        }
    }
}

/// `/api/archive/<projeto>/<conversa>/resume`, com os dois segmentos já decodificados.
fn archive_resume(method: &Method, path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/api/archive/")?.strip_suffix("/resume")?;
    let (project, sid) = rest.split_once('/')?;
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8().ok().map(|s| s.into_owned());
    (method == Method::POST && !sid.contains('/')).then(|| Some((decode(project)?, decode(sid)?))).flatten()
}

/// `/api/sessions/<nome>/resume`, com o nome já decodificado.
fn live_resume(method: &Method, path: &str) -> Option<String> {
    let name = path.strip_prefix("/api/sessions/")?.strip_suffix("/resume")?;
    (method == Method::POST && !name.is_empty() && !name.contains('/'))
        .then(|| percent_encoding::percent_decode_str(name).decode_utf8().ok().map(|s| s.into_owned())).flatten()
}

/// As rotas do Rust que aceitam o repasse.
enum Owned { Groups(bool), Conductor, Catalog, SessionDefaults, PushExpo, Commands, Mermaid, Prewarm, Nav, Plans, Worktrees, JevDelivery, ClientApi, ArchiveResume(String, String), Resume(String), Media(String, bool) }
