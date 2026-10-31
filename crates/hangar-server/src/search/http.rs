// crates/hangar-server/src/search/http.rs
//! `GET /api/search?q=`: só o dono; o resto segue ao Python, que recusa como hoje.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::json;

use crate::routes::{AppState, cors, gate, pass, route_failed};
use crate::session_write::{detail_body, json_response};

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (forward, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || req.method() != Method::GET {
        return pass(&st, req, &forward).await;
    }
    // Parâmetro repetido: vale o último, como no FastAPI.
    let query: HashMap<String, String> = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    let q = query.get("q").cloned().unwrap_or_default();
    let mut resp = match live_names(&st).await {
        Err(code) => json_response(StatusCode::SERVICE_UNAVAILABLE,
            detail_body("erro_lista_indisponivel", "a lista de sessões está indisponível", json!({"detalhe": code}))),
        Ok(live) => {
            let index = st.search.clone();
            match tokio::task::spawn_blocking(move || super::hits::search(&index, &q, &live, super::hits::MAX_HITS)).await {
                Ok(Ok(hits)) => json_response(StatusCode::OK, json!(hits)),
                Ok(Err(code)) => json_response(StatusCode::SERVICE_UNAVAILABLE,
                    detail_body("erro_busca_contas", "as contas Claude não puderam ser lidas", json!({"detalhe": code}))),
                Err(_) => return route_failed(&st, req.headers(), "rust.search_failed", "", "search_panic", "a busca caiu no servidor"),
            }
        }
    };
    cors(req.headers(), resp.headers_mut());
    resp
}

/// realpath do transcript → nome de cada sessão viva, relido a cada pedido (nome não é identidade).
pub(crate) async fn live_names(st: &AppState) -> Result<HashMap<String, String>, &'static str> {
    let produced = st.list.snapshot().await.map_err(|e| e.code)?;
    Ok(produced.rows.iter()
        .filter_map(|row| Some((crate::list::discover::real(row.jsonl.as_deref()?), row.name.clone())))
        .collect())
}
