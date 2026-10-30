//! `~/.hangar/vault.json`: qual máquina é a dona do cofre (nas outras) e para onde vai a cópia
//! (`backup_peer`, na dona). O papel sai daqui e do `vault.bin`: quem tem o arquivo é a dona,
//! mesmo com um `owner` velho gravado.
use serde::{Deserialize, Serialize};
use std::{io, path::{Path, PathBuf}};

use super::store;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_peer: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// O arquivo existe e não lê, ou traz um id torto: nunca tratado como "sem dona".
    Unreadable,
    Io,
}

/// Onde mora o cofre visto desta máquina.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement { Owner, Remote(String), None }

pub fn path(hangar_dir: &Path) -> PathBuf { hangar_dir.join("vault.json") }

/// Id de máquina como o `peers.json` o usa; também vira nome de pasta na cópia da Task 6.
pub fn valid_server_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && id != "." && id != ".."
}

pub fn load(hangar_dir: &Path) -> Result<Config, ConfigError> {
    let raw = match std::fs::read(path(hangar_dir)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(_) => return Err(ConfigError::Io),
    };
    let cfg: Config = serde_json::from_slice(&raw).map_err(|_| ConfigError::Unreadable)?;
    let ok = |id: &Option<String>| id.as_deref().is_none_or(valid_server_id);
    if ok(&cfg.owner) && ok(&cfg.backup_peer) { Ok(cfg) } else { Err(ConfigError::Unreadable) }
}

pub fn save(hangar_dir: &Path, cfg: &Config) -> Result<(), ConfigError> {
    std::fs::create_dir_all(hangar_dir).map_err(|_| ConfigError::Io)?;
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|_| ConfigError::Io)?;
    crate::runtime::queue::atomic_write(&path(hangar_dir), &bytes).map_err(|_| ConfigError::Io)
}

pub fn placement(hangar_dir: &Path) -> Result<Placement, ConfigError> {
    if store::exists(&store::path(hangar_dir)) { return Ok(Placement::Owner); }
    Ok(match load(hangar_dir)?.owner {
        Some(owner) => Placement::Remote(owner),
        None => Placement::None,
    })
}
