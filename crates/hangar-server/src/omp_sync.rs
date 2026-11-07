//! A sincronização do omp mora no `hangar-omp-sync`; aqui fica o que ela lê do servidor: ambiente,
//! conta, pasta de instalação e a chave das automações.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{extract::Request, http::StatusCode, response::Response};

pub use hangar_omp_sync::*;
use crate::routes::AppState;

pub fn start(st: Arc<AppState>) -> Arc<Service> {
    let env = crate::push::server_env();
    let home = st.accounts.env.home.clone();
    let claude = st.accounts.env.claude_base.clone();
    let template = installed_template(&home);
    let weak = Arc::downgrade(&st);
    let allowed: Arc<dyn Fn() -> Result<bool, &'static str> + Send + Sync> = Arc::new(move || {
        let Some(st) = weak.upgrade() else {
            return Ok(false);
        };
        let result = crate::loops::automations(&st);
        if let Err(code) = result {
            st.diag.report(
                "omp.sync_config_failed",
                "omp",
                code,
                "configuração inválida interrompeu a sincronização automática",
            );
        }
        result
    });
    Service::launch(env, home, claude, template, allowed)
}

fn installed_template(home: &Path) -> PathBuf {
    hangar_omp_sync::context_template(&crate::install_root::root(home).unwrap_or_default())
}

/// A borda já conferiu o dono, inclusive nas portas repassadas.
pub async fn serve(_st: &Arc<AppState>, service: &Arc<Service>, _req: Request) -> Response {
    crate::session_write::json_response(StatusCode::OK, service.status())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use serde_json::Value;

    use super::*;
    use crate::omp_sync::{Output, Runner, Synchronizer};
    use futures_util::future::BoxFuture;

    struct ForbiddenRunner;
    impl Runner for ForbiddenRunner {
        fn run<'a>(
            &'a self,
            _: Vec<String>,
            _: &'a Path,
            _: &'a BTreeMap<String, String>,
            _: Duration,
        ) -> BoxFuture<'a, Result<Output, String>> {
            Box::pin(async { panic!("serviço desligado não pode iniciar CLI") })
        }
    }

    #[test]
    fn op080_context_rule_targets_current_without_resolving_version() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let real = home.join(".hangar/versions/v1");
        std::fs::create_dir_all(&real).unwrap();
        let current = home.join(".hangar/current");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &current).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real, &current).unwrap();
        assert_eq!(installed_template(home), current.join("scripts").join("omp").join("carregar-claude-md-do-projeto.md"));
    }

    #[tokio::test]
    async fn op099_public_status_requires_owner_token_and_returns_the_published_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let config = crate::config::Config::from_lookup(|key| match key {
            "HANGAR_SERVER_LISTEN" => Some("127.0.0.1:18765".into()),
            "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
            "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
            "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
            _ => None,
        })
        .unwrap();
        let st = Arc::new(crate::routes::AppState::new(config));
        let sync = Synchronizer::new(home.into(), home.join(".claude"), BTreeMap::new(), Arc::new(ForbiddenRunner));
        let service = Service::new(sync, false, 300., Arc::new(|| Ok(true))).unwrap();
        let expected = service.status();
        assert!(st.omp_sync.set(service).is_ok());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, done) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::routes::router(st).into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = done.await;
            })
            .await
            .unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();
        for token in [None, Some("invalid"), Some("fixture-owner")] {
            let mut request = client
                .get(format!("http://{address}/api/omp/plugin-sync"))
                .header("x-forwarded-for", "198.51.100.96");
            if let Some(token) = token {
                request = request.bearer_auth(token)
            }
            let response = request.send().await.unwrap();
            if token == Some("fixture-owner") {
                assert_eq!(response.status(), 200);
                assert_eq!(response.json::<Value>().await.unwrap(), expected)
            } else {
                assert_eq!(response.status(), 401);
            }
        }
        let _ = stop.send(());
        task.await.unwrap();
    }
}
