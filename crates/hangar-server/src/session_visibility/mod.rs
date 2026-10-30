//! "Ocultar da lista": a sessão segue rodando e só sai da lista principal. A marca é do servidor
//! (vale em todos os aparelhos) e a lista a aplica a cada rodada (`fill`).
pub mod auto;
pub mod store;

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::routes::{AppState, cors, gate};
use crate::session_write::{detail_body, json_response, session_name};

fn refuse(status: StatusCode, code: &str, msg: &str) -> Response { json_response(status, detail_body(code, msg, json!({}))) }

async fn blocking<T: Send + 'static>(job: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(job).await.unwrap_or_else(|_| Err(io::Error::other("tarefa interrompida")))
}

/// A casa onde a lista lê as marcas: a mesma pasta que ela usa para os registros.
fn home(st: &AppState) -> Option<PathBuf> { st.list.env().dirs.as_ref().map(|d| d.home.clone()) }

/// Chave com registro: sem ele a limpeza tiraria a marca na rodada seguinte, e a sessão voltaria sozinha.
pub(crate) fn registered(home: &Path, key: &str) -> bool {
    crate::session_open::record::path(home, key).is_ok_and(|p| p.exists())
}

/// `PUT`/`DELETE /api/sessions/{name}/hidden`.
pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let mut response = match gate(&st, peer, &req).await {
        Err(e) => crate::auth::unavailable(e),
        Ok((_, false)) => refuse(StatusCode::FORBIDDEN, "session_hidden_owner_only", "só o dono oculta ou mostra uma sessão"),
        Ok((_, true)) => match session_name(req.uri().path()) {
            None => StatusCode::NOT_FOUND.into_response(),
            Some(name) => set(&st, &name, req.method() == Method::PUT).await,
        },
    };
    cors(&headers, response.headers_mut());
    response
}

/// Sem recusa por "esperando": a linha da descoberta ainda não foi classificada, e a regra da lista
/// tira a marca na rodada seguinte.
async fn set(st: &AppState, name: &str, hidden: bool) -> Response {
    let Some(home) = home(st) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "list_dirs_missing", "as pastas das sessões não foram configuradas");
    };
    let rows = match st.list.discover(Some(crate::state::monitor::wall_now())).await {
        Ok(rows) => rows,
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.code, e.detail),
    };
    let Some(row) = rows.iter().find(|r| r.name == name) else {
        return refuse(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "a sessão não existe");
    };
    let key = crate::dictation::rename_suggestion::key_of(st, name, row.lifecycle_id.as_deref()).await;
    let now = crate::state::monitor::wall_now();
    let written = blocking(move || match key.filter(|k| registered(&home, k)) {
        Some(key) => store::set_hidden(&home, &key, hidden, now).map(|()| true),
        None => Ok(false),
    }).await;
    match written {
        Ok(true) => {
            st.list.invalidate();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => refuse(StatusCode::CONFLICT, "session_hidden_no_key", "esta sessão não tem registro e não pode ser ocultada"),
        Err(error) => {
            tracing::warn!(session = %name, %error, "marca de oculta não gravada");
            st.diag.report("rust.session_visibility", name, "session_hidden_store_failed", "marca de oculta nao gravada");
            refuse(StatusCode::INTERNAL_SERVER_ERROR, "session_hidden_store_failed", "não foi possível guardar a marca de oculta")
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) mod fixture {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::extract::{ConnectInfo, Request, State};
    use axum::http::{Method, StatusCode};
    use axum::response::Response;

    use crate::list::bridge::{ListBridge, ListEnv};
    use crate::list::discover_other::Dirs;
    use crate::list::facts::FactsClient;
    use crate::list::mux::Mux;
    use crate::routes::AppState;

    pub const KEY: &str = "0123456789abcdef0123456789abcdef";

    /// Um pane Pi chamado `fixture`, nascido em 1 s, com a chave `KEY` no ambiente do multiplexador.
    struct Procs(PathBuf);
    impl crate::list::procs::ProcessView for Procs {
        fn children(&self, _: Duration) -> std::io::Result<Arc<crate::list::procs::ChildrenMap>> { Ok(Arc::default()) }
        fn argv(&self, _: i64) -> Vec<String> { vec!["pi".into()] }
        fn cwd(&self, _: i64) -> Option<PathBuf> { Some(self.0.clone()) }
        fn env_var(&self, _: i64, key: &str) -> std::io::Result<Option<std::ffi::OsString>> {
            Ok(match key {
                "CLAUDE_CONFIG_DIR" => Some(self.0.join(".claude").into_os_string()),
                "CP_PROVIDER" => Some("pi".into()),
                _ => None,
            })
        }
        fn start_time(&self, _: i64) -> Option<f64> { Some(1.) }
        fn fds(&self, _: i64) -> Vec<PathBuf> { vec![] }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Com o arquivo `no-key` na raiz, o pane responde sem chave.
    pub fn state(root: &Path) -> Arc<AppState> {
        use std::os::unix::fs::PermissionsExt;
        let shim = root.join("mux");
        let folder = root.display();
        write(&shim, &format!(r#"#!/bin/sh
case "$1" in
list-panes) printf 'fixture\t1\t123\t{folder}\t%%1\t0\tpi\t10\t0\t0\t0\n';;
show-environment) [ -e '{folder}/no-key' ] && exit 1; echo 'CP_SESSION_KEY={KEY}';;
esac
"#));
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dirs = Dirs { home: root.to_owned(), claude: root.join(".claude"), codex_home: root.join(".codex"),
            pi_sessions: root.join(".pi/agent/sessions"), omp_config: root.join(".omp"), omp_agent: root.join(".omp/agent"),
            kimi_home: root.join(".kimi-code") };
        write(&dirs.claude.join(".hangar-pi/1.json"), &serde_json::json!({"file": root.join("life.jsonl"), "ts": 10}).to_string());
        write(&root.join("life.jsonl"), "{}");
        let cfg = crate::config::Config::from_lookup(|key| match key {
            "HANGAR_SERVER_LISTEN" => Some("192.0.2.50:18765".into()),
            "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
            "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
            "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
            _ => None,
        }).unwrap();
        let facts = FactsClient::new(cfg.upstream, cfg.internal_secret.clone());
        let mut state = AppState::new(cfg);
        state.origins_home = root.to_owned();
        state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(shim.as_os_str(), Duration::from_secs(2)),
            capture_program: shim.into_os_string(), procs: Arc::new(Procs(root.to_owned())), dirs: Some(dirs) }, facts));
        Arc::new(state)
    }

    pub fn register(root: &Path) { write(&root.join(".hangar/session-specs").join(format!("{KEY}.json")), "{}"); }

    pub async fn call<F, Fut>(st: &Arc<AppState>, handler: F, method: Method, path: &str, token: &str, body: &str) -> (StatusCode, serde_json::Value)
    where F: FnOnce(State<Arc<AppState>>, ConnectInfo<std::net::SocketAddr>, Request) -> Fut, Fut: std::future::Future<Output = Response> {
        let req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json").body(Body::from(body.to_owned())).unwrap();
        let response = handler(State(st.clone()), ConnectInfo("127.0.0.1:5000".parse().unwrap()), req).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::fixture::{KEY, call, register, state};
    use super::*;

    #[tokio::test]
    async fn the_owner_hides_and_shows_a_registered_session() {
        let root = tempfile::tempdir().unwrap();
        let st = state(root.path());
        register(root.path());
        let (status, _) = call(&st, route, Method::PUT, "/api/sessions/fixture/hidden", "fixture-owner", "").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(store::read(root.path()).unwrap()[KEY].hidden);
        let (status, _) = call(&st, route, Method::DELETE, "/api/sessions/fixture/hidden", "fixture-owner", "").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let mark = store::read(root.path()).unwrap()[KEY];
        assert!(!mark.hidden && mark.shown_at.is_some(), "mostrar grava quando: {mark:?}");
    }

    #[tokio::test]
    async fn refusals_carry_their_codes() {
        let root = tempfile::tempdir().unwrap();
        let st = state(root.path());
        register(root.path());
        let (status, body) = call(&st, route, Method::PUT, "/api/sessions/fixture/hidden", "errado", "").await;
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("session_hidden_owner_only")));
        let (status, body) = call(&st, route, Method::PUT, "/api/sessions/ausente/hidden", "fixture-owner", "").await;
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("erro_sessao_inexistente")));
        std::fs::write(root.path().join("no-key"), "").unwrap();
        let (status, body) = call(&st, route, Method::PUT, "/api/sessions/fixture/hidden", "fixture-owner", "").await;
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("session_hidden_no_key")));
        assert!(store::read(root.path()).unwrap().is_empty(), "nada gravado");
    }

    #[tokio::test]
    async fn a_pane_key_without_a_registry_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let st = state(root.path());
        let (status, body) = call(&st, route, Method::PUT, "/api/sessions/fixture/hidden", "fixture-owner", "").await;
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("session_hidden_no_key")));
    }
}
