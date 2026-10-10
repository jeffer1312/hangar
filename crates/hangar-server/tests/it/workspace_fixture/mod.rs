//! Python falso das rotas de Git/arquivos: contexto, diário e o resto contado como repasse.
#![allow(dead_code)]
use crate::fake::{SECRET, config, spawn_server};
use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

pub type Diags = Arc<Mutex<Vec<Value>>>;

pub struct Fixture {
    pub addr: std::net::SocketAddr,
    /// Pedidos de `/api/` que chegaram ao Python.
    pub hits: Arc<AtomicUsize>,
    /// Corpos recebidos em `/internal/diag`.
    pub diags: Diags,
}

impl Fixture {
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    /// O diário sai numa tarefa à parte: espera até 5 s pelo evento.
    pub async fn diag(&self, event: &str) -> Value {
        for _ in 0..500 {
            if let Some(found) = self.diags.lock().unwrap().iter().find(|d| d["evento"] == event) {
                return found.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("diário sem {event}: {:?}", self.diags.lock().unwrap());
    }
}

pub async fn fixture(root: &std::path::Path, context_ok: bool) -> Fixture {
    let hits = Arc::new(AtomicUsize::new(0));
    let diags: Diags = Arc::default();
    let context = json!({"roots":[root],"sessions":[{"name":"fixture","cwd":root}],"session":{"name":"fixture","cwd":root,"jsonl":root.join("fixture.jsonl"),"git_cwd":root}});
    let worktrees = json!({"roots":[root],"cwds":[root.join("repo")],"sessions":[{"name":"fixture","cwd":root.join("repo-wt")}],"project_bases":[]});
    let app = Router::new()
        .route(
            "/internal/workspace/context",
            get(
                move |headers: axum::http::HeaderMap,
                      Query(_): Query<std::collections::HashMap<String, String>>| {
                    let context = context.clone();
                    async move {
                        assert_eq!(headers["x-hangar-internal"], SECRET);
                        let status = if context_ok { 200 } else { 500 };
                        (
                            axum::http::StatusCode::from_u16(status).unwrap(),
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            context.to_string(),
                        )
                    }
                },
            ),
        )
        .route(
            "/internal/worktrees/context",
            get(move || {
                let worktrees = worktrees.clone();
                async move {
                    let status = if context_ok { 200 } else { 500 };
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        worktrees.to_string(),
                    )
                }
            }),
        )
        .route(
            "/internal/diag",
            post(
                |State((_, diags)): State<(Arc<AtomicUsize>, Diags)>, body: Bytes| async move {
                    diags.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
                    "{}"
                },
            ),
        )
        .fallback(
            |State((hits, _)): State<(Arc<AtomicUsize>, Diags)>, uri: axum::http::Uri| async move {
                // A sonda de partida do servidor bate na raiz; repasse é só pedido de `/api/`.
                if uri.path().starts_with("/api/") {
                    hits.fetch_add(1, Ordering::Relaxed);
                }
                "python-reserva"
            },
        )
        .with_state((hits.clone(), diags.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fixture { addr: spawn_server(config(upstream, "127.0.0.1")).await, hits, diags }
}

/// Corpo do 503 de Git/arquivos: mesmo formato das outras rotas do Rust, com o motivo em `params`.
pub async fn refusal(response: reqwest::Response, code: &str) -> Value {
    assert_eq!(response.status(), 503);
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], code);
    assert_eq!(body["detail"]["code"], code);
    assert!(body["detail"]["msg"].as_str().is_some_and(|m| !m.is_empty()), "{body}");
    body
}
