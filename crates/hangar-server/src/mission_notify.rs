//! Entrega dos avisos do Mission Control à sessão pela rota de entrada do dono, dentro do processo, como as tools
//! do MCP chamam as rotas: a mesma fila, a mesma posse da escrita.
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::OnceLock;
use tower_service::Service;

pub struct RouterNotify {
    /// O roteador público inteiro, gravado quando fica pronto (ele contém as rotas do Mission Control).
    pub router: OnceLock<Router>,
    pub token: String,
}

impl hangar_mission::http::Notify for RouterNotify {
    fn send(&self, session: String, text: String) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (router, token) = (self.router.get().cloned(), self.token.clone());
        Box::pin(async move {
            let Some(mut router) = router else { return };
            let path = format!("/api/sessions/{}/input", percent_encoding::utf8_percent_encode(&session, percent_encoding::NON_ALPHANUMERIC));
            let body = serde_json::json!({"text": text}).to_string();
            let Ok(mut request) = Request::post(path).header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json").header("user-agent", "hangar-mission").body(Body::from(body)) else { return };
            request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
            match router.call(request).await {
                Ok(response) if response.status().is_success() => {}
                Ok(response) => tracing::info!(code = "mission_notice_refused", status = response.status().as_u16()),
                Err(never) => match never {},
            }
        })
    }
}
