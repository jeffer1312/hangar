//! O `.env` que diz porta, token e o id desta máquina: o de `~/.hangar/config/` quando existe,
//! senão o da `backend/` da cópia do atalho (máquina não migrada).
use std::io::Write;
use std::path::PathBuf;

use crate::Invocation;

pub(crate) struct Settings {
    pub token: String,
    pub port: String,
    pub server_id: String,
}

pub(crate) fn env_path(invocation: &Invocation) -> PathBuf {
    let new = invocation.home.join(".hangar").join("config").join(".env");
    match &invocation.root {
        Some(root) if !new.exists() => root.join("backend").join(".env"),
        _ => new,
    }
}

pub(crate) fn read(invocation: &Invocation, err: &mut dyn Write) -> Result<Settings, u8> {
    let path = env_path(invocation);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(err, "erro: .env não encontrado: {}", path.display());
            return Err(2);
        }
        Err(error) => {
            let _ = writeln!(err, "erro: não consegui ler {}: {error}", path.display());
            return Err(2);
        }
    };
    let pairs = hangar_core::user_data::parse_env_file(&String::from_utf8_lossy(&bytes));
    let last = |key: &str| pairs.iter().rev().find(|(name, _)| name == key).map(|(_, value)| value.clone());
    let Some(token) = last("cp_auth_token") else {
        let _ = writeln!(err, "erro: CP_AUTH_TOKEN ausente em {}", path.display());
        return Err(1);
    };
    let port = last("cp_port").filter(|port| !port.is_empty()).unwrap_or_else(|| "8765".into());
    Ok(Settings { token, port, server_id: last("cp_server_id").unwrap_or_default() })
}
