use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde_json::json;

use hangar_core::edge::Edge;
use hangar_core::http::{detail, json_response};

use crate::reader::DesktopTheme;

pub const ROUTES: &[(&str, &str)] = &[
    ("/api/desktop/palette", "GET"),
    ("/api/desktop/wallpaper", "GET"),
];

struct Http {
    edge: Edge,
    theme: Arc<DesktopTheme>,
}

pub fn owns_path(path: &str) -> bool {
    ["/api/desktop/palette", "/api/desktop/wallpaper"]
        .iter()
        .any(|base| {
            path == *base
                || path
                    .strip_prefix(base)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
}

pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, theme: Arc<DesktopTheme>) -> Router<S> {
    Router::new()
        .route("/api/desktop/palette", axum::routing::any(public))
        .route("/api/desktop/palette/", axum::routing::any(public))
        .route("/api/desktop/palette/{*rest}", axum::routing::any(public))
        .route("/api/desktop/wallpaper", axum::routing::any(public))
        .route("/api/desktop/wallpaper/", axum::routing::any(public))
        .route("/api/desktop/wallpaper/{*rest}", axum::routing::any(public))
        .with_state(Arc::new(Http { edge, theme }))
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
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
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
    let forward = match state.edge.owner_gate(socket, &request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if peer.is_none() || !["127.0.0.1", "::1", "localhost"].contains(&forward.client_ip.as_str()) {
        return detail(
            StatusCode::FORBIDDEN,
            "erro_so_loopback",
            "só na máquina do backend",
            json!({}),
        );
    }
    let theme = state.theme.clone();
    if path == "/api/desktop/palette" {
        return match tokio::task::spawn_blocking(move || theme.palette()).await {
            Ok(Some(value)) => json_response(StatusCode::OK, value),
            Ok(None) => detail(
                StatusCode::NOT_FOUND,
                "erro_sem_paleta",
                "sem paleta",
                json!({}),
            ),
            Err(_) => hangar_core::http_cors::internal_error(),
        };
    }
    let path = match tokio::task::spawn_blocking(move || theme.wallpaper()).await {
        Ok(Ok(Some(path))) => path,
        Ok(Ok(None)) => {
            return detail(
                StatusCode::NOT_FOUND,
                "erro_sem_papel_de_parede",
                "sem papel de parede",
                json!({}),
            );
        }
        _ => return hangar_core::http_cors::internal_error(),
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => return hangar_core::http_cors::internal_error(),
    };
    let metadata = match file.metadata().await {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return hangar_core::http_cors::internal_error(),
    };
    let media = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    let mut response =
        match hangar_http_file::legacy_file(file, metadata, &path, request.headers(), &media, true)
            .await
        {
            Ok(response) => response,
            Err(_) => return hangar_core::http_cors::internal_error(),
        };
    if response.status().is_success() {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            "no-store".parse().expect("cabeçalho estático válido"),
        );
    }
    response
}
