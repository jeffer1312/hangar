//! Entrega dos avisos do Mission Control à sessão pelo handler da rota de entrada do dono, dentro do processo: a
//! mesma fila e a mesma posse da escrita do `POST /api/sessions/{name}/input`.
use crate::routes::AppState;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::Request;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{OnceLock, Weak};

pub struct SessionNotify {
    /// Referência fraca: o estado guarda este entregador, e uma forte o manteria vivo para sempre.
    pub state: OnceLock<Weak<AppState>>,
}

impl hangar_mission::http::Notify for SessionNotify {
    fn send(&self, session: String, text: String) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let state = self.state.get().and_then(Weak::upgrade);
        Box::pin(async move {
            let Some(state) = state else { return };
            let path = format!("/api/sessions/{}/input", percent_encoding::utf8_percent_encode(&session, percent_encoding::NON_ALPHANUMERIC));
            let body = serde_json::json!({"text": text}).to_string();
            let Ok(request) = Request::post(path).header("authorization", format!("Bearer {}", state.cfg.auth_token))
                .header("content-type", "application/json").header("user-agent", "hangar-mission").body(Body::from(body)) else { return };
            let peer = SocketAddr::from(([127, 0, 0, 1], 0));
            let response = crate::session_write::input::input(State(state), ConnectInfo(peer), request).await;
            if !response.status().is_success() {
                tracing::info!(code = "mission_notice_refused", status = response.status().as_u16());
            }
        })
    }
}
