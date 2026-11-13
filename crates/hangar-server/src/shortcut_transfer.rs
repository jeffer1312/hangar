//! Liga a transferência de atalhos (`hangar-shortcut-transfer`) à configuração, aos marcadores de
//! caminho e ao PATH de login deste servidor.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value, json};

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

    fn snapshot(&self) -> Result<Snapshot, Error> {
        let config = self.config()?;
        let mode = match std::fs::metadata(&config.path) {
            Ok(_) => Some(crate::config_sync::files::mode(&config.path).map_err(|e| Error::Io(e.to_string()))?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(Error::Io(error.to_string())),
        };
        Ok(Snapshot {
            shortcuts: config.read().get("shortcuts").cloned(),
            mode,
            parent_existed: config.path.parent().is_some_and(std::path::Path::exists),
        })
    }

    fn save(&self, serialized: &str) -> Result<(), Error> {
        let changes: Map<String, Value> = [("shortcuts".to_owned(), json!(serialized))].into_iter().collect();
        match self.config()?.apply_checked(&changes, &Default::default()) {
            Ok(_) => Ok(()),
            Err(error) if error.is_validation() => Err(Error::Invalid(error.message)),
            Err(error) => Err(Error::Io(error.message)),
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
