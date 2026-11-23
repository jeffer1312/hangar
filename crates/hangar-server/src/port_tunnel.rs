//! `GET /api/port-tunnel?port=N`: quem abre e até qual porta; os bytes são do `hangar-port-tunnel`.
//! Só em `public_routes` (8765 e Connect): a 8766 do convidado não tem esta rota.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;

use crate::routes::{AppState, cors, owner_gate, pass};
use crate::share::gate::Watcher;

/// O cookie vai sozinho em pedido de qualquer página do mesmo site, até a prévia de projeto de outra
/// máquina do Connect: tira o cookie e diz se sobrou credencial explícita (`?token=` ou Bearer).
pub(crate) fn explicit_token(st: &AppState, peer: SocketAddr, req: &mut Request) -> bool {
    req.headers_mut().remove(header::COOKIE);
    let https = st.cfg.trusted.resolve(peer.ip(), req.headers()).1;
    crate::auth::presented_token(req.headers(), req.uri().query(), req.method(), https).is_some()
}

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    if !explicit_token(&st, peer, &mut req) {
        return refusal(req.headers(), StatusCode::FORBIDDEN, "port_tunnel_token_required", "credencial só por ?token= ou Bearer");
    }
    let fwd = match owner_gate(&st, peer, &req).await {
        Ok(fwd) => fwd,
        Err(response) => return response,
    };
    // O aparelho do Connect traz o vigia: revogado, o túnel aberto cai.
    let watcher = req.extensions().get::<Watcher>().cloned();
    let (mut parts, body) = req.into_parts();
    let Ok(ws) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
        return pass(&st, Request::from_parts(parts, body), &fwd).await;
    };
    let port = hangar_port_tunnel::port_of(parts.uri.query()).filter(|port| !st.connect_previews.refused(*port));
    let Some(port) = port else {
        return refusal(&parts.headers, StatusCode::UNPROCESSABLE_ENTITY, "port_tunnel_port_refused", "porta fora da faixa ou do Hangar");
    };
    let Ok(tcp) = hangar_port_tunnel::connect(port).await else {
        return refusal(&parts.headers, StatusCode::BAD_GATEWAY, "port_tunnel_refused", "nada escutando nessa porta");
    };
    ws.max_message_size(hangar_port_tunnel::MAX_MESSAGE).on_upgrade(move |socket| hangar_port_tunnel::relay(socket, tcp, async move { Watcher::ended(watcher.as_ref()).await }))
}

fn refusal(headers: &HeaderMap, status: StatusCode, code: &str, msg: &str) -> Response {
    let mut response = hangar_core::http::detail(status, code, msg, serde_json::json!({}));
    cors(headers, response.headers_mut());
    response
}
