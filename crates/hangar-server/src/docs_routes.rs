//! `GET /api/news` (histórico das novidades) e `GET /api/manual`: os documentos da versão em uso,
//! lidos a cada pedido de `docs/` da raiz que esta máquina roda.
use crate::routes::AppState;
use crate::session_write::{detail_body, json_response};
use crate::update_arrived::{blocking, owner_gate};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use std::{io, net::SocketAddr, path::Path, sync::Arc};

pub async fn news(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let admission = owner_gate(&state, peer, &request, owner_only);
    serve(&state, admission, "news_unavailable", |root| Ok(json!({"news": hangar_docs::history(root, std::env::consts::OS)?}))).await
}

pub async fn manual(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let admission = owner_gate(&state, peer, &request, owner_only);
    serve(&state, admission, "manual_unavailable", |root| hangar_docs::manual(root, std::env::consts::OS)).await
}

/// 403 com código próprio, como o `/api/update/notes`: o `routes::owner_gate` daria 401 ao convidado.
fn owner_only() -> Response {
    json_response(StatusCode::FORBIDDEN, detail_body("docs_owner_only", "só o dono pode ler as novidades e o manual", json!({})))
}

/// Recebe a admissão já criada: o `&Request` atravessando o `await` tiraria o `Send` do handler.
async fn serve(state: &AppState, admission: impl Future<Output = Result<(), Response>>, code: &'static str,
    read: impl FnOnce(&Path) -> io::Result<Value> + Send + 'static) -> Response {
    if let Err(refused) = admission.await { return refused; }
    match blocking(&crate::update_notes::notes_root(&state.accounts.env.home), read).await {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(error) => {
            tracing::warn!(code, %error, "documentos da versão em uso sem leitura");
            json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body(code, "não deu para ler os documentos desta versão", json!({})))
        }
    }
}
