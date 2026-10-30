//! Os consumidores compartilham as tarefas e o encerramento da mesma integração.
use super::service::Service;
use crate::{config_sync, routes::AppState};
use std::sync::Arc;

impl AppState {
    pub fn codex_setup_service(&self) -> Result<Arc<Service>, String> {
        self.codex_setup.get_or_init(|| {
            let context = config_sync::http::context(self);
            Ok(Service::new(
                self.accounts.clone(),
                config_sync::runtime::RuntimeConfig::from_state(self)?,
                context.roots.hangar.into(),
                self.side.runtime.get().cloned(),
            ))
        }).clone()
    }
}
