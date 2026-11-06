//! Python falso das rotas de Git/arquivos: contexto, diário e o resto contado como repasse.
#![allow(dead_code)]
use crate::fake::{SECRET, config};
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
    pub workspace_slots: Arc<tokio::sync::Semaphore>,
    _home: Option<tempfile::TempDir>,
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
    fixture_with_home(root,context_ok,None).await
}

pub async fn fixture_with_home(root: &std::path::Path, context_ok: bool, home: Option<&std::path::Path>) -> Fixture {
    fixture_with_mux(root,context_ok,home,None).await
}

pub async fn fixture_with_mux(root: &std::path::Path, context_ok: bool, home: Option<&std::path::Path>, mux: Option<&std::path::Path>) -> Fixture {
    let hits = Arc::new(AtomicUsize::new(0));
    let diags: Diags = Arc::default();
    let context = json!({"roots":[root],"sessions":[{"name":"fixture","cwd":root}],"session":{"name":"fixture","cwd":root,"jsonl":root.join("fixture.jsonl"),"git_cwd":root}});
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
    let isolated_home=home.is_none().then(||tempfile::tempdir().unwrap());
    let home=home.unwrap_or_else(||isolated_home.as_ref().unwrap().path());
    let mut state=hangar_server::routes::AppState::new(config(upstream,"127.0.0.1"));
    state.origins_home=home.into();
    state.accounts=hangar_server::accounts::AccountService::new(
        hangar_server::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(),home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(),home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(),root.to_string_lossy().into_owned()),
        ].into()));
    let mut env=hangar_server::list::bridge::ListEnv::from_env();
    env.dirs=hangar_server::list::bridge::parse_dirs(&json!({
        "home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),
        "omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")
    }).to_string());
    // Inventário vazio conhecido; a prova não depende do multiplexador instalado na máquina.
    #[cfg(unix)]
    let default_mux={
        let script=home.join("fixture-mux");
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&script,format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\nprintf 'fixture\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\n'\n",root.join("repo-wt").display())).unwrap();
        std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    };
    #[cfg(windows)]
    let default_mux={
        let script=home.join("fixture-mux.cmd");
        std::fs::write(&script,format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\necho fixture\t1\t\t{}\t%%1\t\t\t\t0\t0\r\n",root.join("repo-wt").display())).unwrap();
        script
    };
    env.mux=hangar_server::list::mux::Mux::with_program(mux.unwrap_or(&default_mux),std::time::Duration::from_secs(5));
    state.list=Arc::new(hangar_server::list::bridge::ListBridge::new(env,hangar_server::list::facts::FactsClient::new(upstream,SECRET.into())));
    let workspace_slots=state.workspace_slots.clone();
    let addr=crate::fake::spawn_state(state).await;
    Fixture { addr, hits, diags,workspace_slots,_home:isolated_home }
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
