//! Herança seletiva da conta Codex padrão, sem copiar identidade ou estado de execução.
#![forbid(unsafe_code)]

use futures_util::future::BoxFuture;
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy)]
pub struct Paths<'a> {
    pub home: &'a Path,
    pub source: &'a Path,
    pub destination: &'a Path,
}

pub type ConfigPlan<'a> = dyn FnMut(&Value) -> Result<Vec<Value>, String> + Send + 'a;

pub trait Native: Send {
    fn parse_toml(&self, raw: &str) -> Result<Value, String>;
    fn edit_config<'a>(
        &'a mut self,
        path: &'a Path,
        backups: &'a Path,
        work: &'a Path,
        plan: &'a mut ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn sync_plugins<'a>(
        &'a mut self,
        paths: Paths<'a>,
        previous: &'a Value,
    ) -> BoxFuture<'a, Result<Value, String>>;
    fn check_trust(&mut self) -> BoxFuture<'_, Result<Option<bool>, String>>;
    fn cli_version(&mut self, force: bool) -> BoxFuture<'_, Result<String, String>>;
    fn now(&self) -> f64;
}

mod preferences;
pub use preferences::{project_preferences, project_trust_edits};

mod common;
mod config;
mod engine;
pub mod plugins;
mod resources;
mod storage;

use common::{Error, blocking, hash, issue, json_bytes, object, public_state, status};
pub use engine::{preparation_status, prepare};

/// `estado.json` da conta, no mesmo diretório que o preparo grava; `None` se a conta não existe.
pub fn state_file(home: &Path, destination: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
    match storage::private_at(home, destination, false) {
        Ok(dir) => Ok(Some(dir.join("estado.json"))),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(Error::Io(error)) => Err(error),
        Err(_) => Err(std::io::Error::other("pasta privada da conta inválida")),
    }
}
use storage::{backup, read, replace_link_with_file, write, write_link};
