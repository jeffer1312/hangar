//! `GET /api/news` (histórico das novidades) e `GET /api/manual`: os documentos da versão em uso,
//! lidos a cada pedido de `docs/` da raiz que esta máquina roda.
use crate::auth::AuthError;
use crate::proxy::Forward;
use crate::routes::{AppState, gate};
use crate::session_write::{detail_body, json_response};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use std::{io, net::SocketAddr, path::Path, sync::Arc};

pub async fn news(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let admission = gate(&state, peer, &request);
    serve(&state, admission, "news_unavailable", |root| Ok(json!({"news": hangar_docs::history(root, std::env::consts::OS)?}))).await
}

pub async fn manual(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let admission = gate(&state, peer, &request);
    serve(&state, admission, "manual_unavailable", |root| hangar_docs::manual(root, std::env::consts::OS)).await
}

/// Recebe a admissão já criada: o `&Request` atravessando o `await` tiraria o `Send` do handler.
async fn serve(state: &AppState, admission: impl Future<Output = Result<(Forward, bool), AuthError>>, code: &'static str,
    read: impl FnOnce(&Path) -> io::Result<Value> + Send + 'static) -> Response {
    // 403 com código próprio, como o `/api/update/notes`: o `routes::owner_gate` daria 401 ao convidado.
    match admission.await {
        Ok((_, true)) => {}
        Ok(_) => return json_response(StatusCode::FORBIDDEN, detail_body("docs_owner_only", "só o dono pode ler as novidades e o manual", json!({}))),
        Err(error) => return crate::auth::unavailable(error),
    }
    let root = crate::update_notes::notes_root(&state.accounts.env.home);
    match tokio::task::spawn_blocking(move || read(&root)).await.unwrap_or_else(|join| Err(io::Error::other(join))) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(error) => {
            tracing::warn!(code, %error, "documentos da versão em uso sem leitura");
            json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body(code, "não deu para ler os documentos desta versão", json!({})))
        }
    }
}
