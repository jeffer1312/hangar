//! Caminhos dos agentes e escrita atômica com o modo do arquivo existente.
use crate::accounts::environment::AccountEnvironment;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn directory(env: &AccountEnvironment, target: &str) -> PathBuf {
    match target {
        "codex" => env.codex_default.clone(),
        "kimi" => env
            .base
            .get("KIMI_CODE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| env.home.join(".kimi-code")),
        _ => env.home.join(".pi/agent"),
    }
}

pub fn write_preserving(path: &Path, text: &str) -> Result<(), String> {
    let old = fs::metadata(path).ok().map(|m| m.permissions());
    crate::runtime::queue::atomic_write(path, text.as_bytes()).map_err(|e| e.to_string())?;
    if let Some(mode) = old {
        fs::set_permissions(path, mode).map_err(|e| e.to_string())?;
    }
    Ok(())
}
