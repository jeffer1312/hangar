//! Admissão do dono e serviços compartilhados da saúde e instalação.
use super::{http, service::Service};
use crate::routes::AppState;
use axum::{extract::Request, response::Response};
use std::{net::SocketAddr, sync::Arc};

pub async fn handle(state: Arc<AppState>, peer: SocketAddr, request: Request) -> Response {
    if let Some(response) = crate::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    let method = request.method().clone();
    let headers = request.headers().clone();
    let mut response = inner(state, peer, request).await;
    crate::http_cors::apply(&headers, &mut response);
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}

async fn inner(state: Arc<AppState>, peer: SocketAddr, request: Request) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
    let decision = crate::http_contract::resolve(http::TABLE, request.method().as_str(), &path);
    let (_, https) = state.cfg.trusted.resolve(peer.ip(), request.headers());
    if let Some(response) = crate::http_route_response::response(
        &decision, request.method(), request.uri(), request.headers(), https,
        &state.cfg.listen.to_string(),
    ) {
        return response;
    }
    if path == "/api/harness/codex/opcoes" { return crate::codex_setup::options_http::handle(state,peer,request).await; }
    if let Err(response) = crate::routes::owner_gate(&state, peer, &request).await {
        return response;
    }
    if state.settings_snapshot().is_err() {
        return crate::http_cors::internal_error();
    }
    if http::integration_path(&path) { return http::integration(&state,request).await; }
    let device = crate::accounts::http::bridge(&state).ok().and_then(|bridge| {
        crate::accounts::codex_device_login::DeviceBridge::new(
            state.cfg.upstream, state.cfg.internal_secret.clone(), bridge.instance().into(),
        ).ok()
    });
    let service = Service {
        accounts: state.accounts.clone(),
        repo: crate::config_sync::http::context(&state).roots.hangar.into(),
        probe: state.harness_probe.clone(),
        runtime: state.state.runtime.get().cloned(),
        device,
    };
    http::serve(service, state.harness_installer.clone(), request).await
}
