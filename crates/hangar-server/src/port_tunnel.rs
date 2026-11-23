//! `GET /api/port-tunnel?port=N`: quem abre e até qual porta; os bytes são do `hangar-port-tunnel`.
//! Só em `public_routes` (8765 e Connect): a 8766 do convidado não tem esta rota.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;

use crate::routes::{AppState, cors, owner_gate, pass};
use crate::share::gate::Watcher;

/// Toda recusa do túnel leva o código também aqui: o 502 do Caddy ou do frp com o servidor fora não
/// o traz, e o cliente separa "servidor fora" de "nada escutando" sem ler o corpo.
const TUNNEL_HEADER: &str = "x-hangar-tunnel";

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
        Err(response) => return tagged(response).await,
    };
    // O aparelho do Connect traz o vigia: revogado, o túnel aberto cai.
    let watcher = req.extensions().get::<Watcher>().cloned();
    let (mut parts, body) = req.into_parts();
    let Ok(ws) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
        return tagged(pass(&st, Request::from_parts(parts, body), &fwd).await).await;
    };
    let port = hangar_port_tunnel::port_of(parts.uri.query()).filter(|port| !st.connect_previews.refused(*port));
    let Some(port) = port else {
        return refusal(&parts.headers, StatusCode::UNPROCESSABLE_ENTITY, "port_tunnel_port_refused", "porta fora da faixa ou do Hangar");
    };
    let tcp = match hangar_port_tunnel::connect(port).await {
        Ok(tcp) => tcp,
        Err(e) => {
            let (status, code, msg) = connect_failure(&e);
            if status != StatusCode::BAD_GATEWAY {
                tracing::warn!(code, kind = ?e.kind(), "port tunnel connect failed");
            }
            return refusal(&parts.headers, status, code, &msg);
        }
    };
    ws.max_message_size(hangar_port_tunnel::MAX_MESSAGE).on_upgrade(move |socket| hangar_port_tunnel::relay(socket, tcp, async move { Watcher::ended(watcher.as_ref()).await }))
}

/// Só conexão recusada é "nada escutando"; outra falha é da máquina, não da porta.
pub fn connect_failure(e: &std::io::Error) -> (StatusCode, &'static str, String) {
    if e.kind() == std::io::ErrorKind::ConnectionRefused {
        return (StatusCode::BAD_GATEWAY, "port_tunnel_refused", "nada escutando nessa porta".into());
    }
    (StatusCode::SERVICE_UNAVAILABLE, "port_tunnel_unavailable", format!("não foi possível abrir a porta: {:?}", e.kind()))
}

fn refusal(headers: &HeaderMap, status: StatusCode, code: &str, msg: &str) -> Response {
    let mut response = hangar_core::http::detail(status, code, msg, serde_json::json!({}));
    cors(headers, response.headers_mut());
    response.headers_mut().insert(TUNNEL_HEADER, HeaderValue::from_str(code).expect("código ASCII"));
    response
}

/// Recusa de outro portão: o código sai do corpo (`detail.code`, ou `detail.error_code` do 503).
async fn tagged(response: Response) -> Response {
    let (mut parts, body) = response.into_parts();
    // ponytail: recusa é JSON curto; o teto só barra corpo que não é recusa.
    let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(status = %parts.status, error = %e, "port tunnel refusal body unreadable");
            return Response::from_parts(parts, Body::empty());
        }
    };
    let detail = serde_json::from_slice::<serde_json::Value>(&bytes).ok().map(|value| value["detail"].clone());
    let code = detail.as_ref().and_then(|d| d["code"].as_str().or(d["error_code"].as_str()));
    if let Some(value) = code.and_then(|code| HeaderValue::from_str(code).ok()) {
        parts.headers.insert(TUNNEL_HEADER, value);
    }
    Response::from_parts(parts, Body::from(bytes))
}
