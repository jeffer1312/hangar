//! Liga a transferência de atalhos (`hangar-shortcut-transfer`) à configuração, aos marcadores de
//! caminho e ao PATH de login deste servidor.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use hangar_shortcut_transfer::{Error, Host, Machine, Snapshot};

use crate::config_sync::runtime::RuntimeConfig;
use crate::routes::AppState;

/// A configuração do servidor como a vê a transferência de atalhos: um escritor só (`WRITES`).
pub struct ServerHost(pub Arc<AppState>);

impl ServerHost {
    fn config(&self) -> Result<RuntimeConfig, Error> {
        RuntimeConfig::from_state(&self.0).map_err(Error::Io)
    }
}

impl Host for ServerHost {
    fn machine(&self) -> Machine {
        let env = &self.0.accounts.env;
        Machine {
            roots: crate::config_sync::http::context(&self.0).roots,
            home: env.home.clone(),
            env: env.base.clone(),
            linux: cfg!(target_os = "linux"),
        }
    }

    fn shortcuts(&self) -> Result<Value, Error> {
        Ok(self.config()?.get("shortcuts"))
    }

    fn validate(&self, text: &str) -> Result<(), String> {
        crate::config_sync::preferences::validate_shortcuts(text)
    }

    fn update(&self, change: &mut dyn FnMut(&Value) -> Result<String, Error>) -> (Option<Snapshot>, Result<(), Error>) {
        let config = match self.config() {
            Ok(config) => config,
            Err(error) => return (None, Err(error)),
        };
        // Leitura, junção e gravação na mesma seção da trava `WRITES` da configuração.
        match config.update_shortcuts(|current| change(current)) {
            Ok(Ok((shortcuts, mode, parent_existed))) => (Some(Snapshot { shortcuts, mode, parent_existed }), Ok(())),
            Ok(Err(refused)) => (None, Err(refused)),
            Err(error) if error.is_validation() => (None, Err(Error::Invalid(error.message))),
            Err(error) => (None, Err(Error::Io(error.message))),
        }
    }

    fn undo(&self, serialized: &str, before: &Snapshot) -> Result<(), String> {
        let config = self.config().map_err(|_| "configuração indisponível".to_owned())?;
        config
            .undo_shortcuts(serialized, before.shortcuts.as_ref(), before.mode, before.parent_existed)
            .map_err(|error| error.message)
    }

    fn login_path(&self) -> Pin<Box<dyn Future<Output = String> + Send + 'static>> {
        let env = self.0.accounts.env.clone();
        Box::pin(async move { crate::provider_discovery::login_path(&env).await })
    }
}
