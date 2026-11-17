//! Caminhos da instalação e dos logs do Hangar na máquina.
use std::path::{Path, PathBuf};

use crate::account_env::AccountEnvironment;

/// Mesmo lugar do `log_paths.base()` do Python: o log é do Hangar da máquina, não da conta. A regra
/// mora no crate do servidor parado (`hangar_local_server::diary::log_base_from`).
pub fn log_base() -> Option<PathBuf> {
    hangar_local_server::diary::log_base_from(std::env::home_dir().as_deref(), std::env::var_os("LOCALAPPDATA").as_deref())
}

/// `log_base` pelo ambiente-base do registro (nos testes, a casa de teste, nunca o diário da máquina).
pub fn log_base_in(env: &AccountEnvironment) -> Option<PathBuf> {
    let home = Some(env.home.as_path()).filter(|home| !home.as_os_str().is_empty());
    hangar_local_server::diary::log_base_from(home, env.base.get("LOCALAPPDATA").map(std::ffi::OsStr::new))
}

/// Pasta `backend/` de uma cópia do Hangar. O `.env` pode já estar em `~/.hangar/config/`, então um
/// arquivo que só o Hangar tem também a identifica (`app/main.py` casaria com qualquer projeto).
pub fn is_backend(dir: &Path) -> bool {
    dir.join(".env").is_file() || dir.join("app").join("pricing_data.json").is_file()
}

/// `~/.hangar/current`: link (symlink no Linux, junction no Windows) para `versions/<v>` ou, no modo
/// desenvolvedor, para um checkout.
pub fn current(home: &Path) -> PathBuf { home.join(".hangar").join("current") }

/// Onde o servidor anota a pasta `backend/` dele, para o comando instalado longe dela achar o `.env`.
pub fn pointer(home: &Path) -> PathBuf { home.join(".hangar").join("backend-dir") }
