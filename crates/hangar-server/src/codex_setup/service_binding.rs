//! Os consumidores compartilham as tarefas e o encerramento da mesma integração.
use super::service::Service;
use crate::{config_sync, routes::AppState};
use std::sync::Arc;

impl AppState {
    pub(crate) fn initialize_codex_setup(&self) {
        if self.codex_setup_service().is_err() {
            tracing::warn!(code="codex_setup_unavailable", "integração Codex indisponível; as demais rotas continuam disponíveis");
        }
    }

    pub fn codex_setup_service(&self) -> Result<Arc<Service>, String> {
        self.codex_setup.get_or_init(|| {
            let context = config_sync::http::context(self);
            let service = Service::new(
                self.accounts.clone(),
                config_sync::runtime::RuntimeConfig::from_state(self)?,
                context.roots.hangar.into(),
                self.side.runtime.get().cloned(),
            );
            let preparation: Arc<dyn crate::accounts::preparation::CodexSetup> = service.clone();
            *self.accounts.codex_setup.lock().unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(&preparation));
            Ok(service)
        }).clone()
    }
}

impl Service {
    pub async fn prepare_project(&self, account: &std::path::Path, cwd: &std::path::Path) -> Result<(), String> {
        super::instructions::prepare_project(&self.config, account, cwd, false).await.map_err(|error| error.to_string())?;
        let work = super::status::root(&self.accounts.env.home, account).map_err(|error| error.message)?;
        super::files::edit_config(&self.accounts.env, &account.join("config.toml"), &work.join("backups"), &work, |current| {
            let limit = super::instructions::limit(account).map_err(|error| error.to_string())?;
            if current.get("project_doc_max_bytes") == Some(&limit) { return Ok(Vec::new()); }
            Ok(vec![serde_json::json!({"keyPath":"project_doc_max_bytes","value":limit,"mergeStrategy":"replace"})])
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config};
    use axum::{body::Body, extract::ConnectInfo, http::{Request, StatusCode}};
    use tower::ServiceExt;

    #[tokio::test]
    async fn invalid_runtime_setting_keeps_health_and_preserves_setup_error() {
        for setting in ["CP_FINISH_MIN_SECONDS", "CP_STALL_SECONDS", "CP_AUTOMATIONS"] {
            let home = tempfile::tempdir().unwrap();
            let address = "127.0.0.1:0".parse().unwrap();
            let mut state = AppState::new(Config {
                listen: address, upstream: address, internal_secret: "fixture-internal".into(),
                auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
            });
            state.accounts = AccountService::new(AccountEnvironment::from_map([
                ("HOME".into(), home.path().display().to_string()),
                ("USERPROFILE".into(), home.path().display().to_string()),
                (setting.into(), "invalid".into()),
            ].into()));
            state.initialize_codex_setup();
            assert!(matches!(state.codex_setup.get(), Some(Err(_))));
            assert!(state.codex_setup_service().is_err());
            assert!(crate::accounts::http::bridge(&state).is_err());
            let response = crate::routes::router(Arc::new(state)).oneshot(
                Request::builder().uri("/__hangar_server/health")
                    .extension(ConnectInfo(address)).body(Body::empty()).unwrap()
            ).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{setting}");
        }
    }
}
