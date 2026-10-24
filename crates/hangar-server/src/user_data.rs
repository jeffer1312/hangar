//! Dados do usuário (`.env`, `peers.json`, `projects.json`) em `~/.hangar/config/`, fora da pasta da
//! versão. Máquina ainda não migrada os tem na `backend/`: vale o lugar novo quando o arquivo está lá;
//! senão, o antigo. A mesma regra do `app/user_data.py`.
use std::path::{Path, PathBuf};

use crate::accounts::environment::AccountEnvironment;

/// `None` só quando o arquivo não está no lugar novo e não há `backend/` conhecida.
pub fn path(home: &Path, backend: Option<&Path>, name: &str) -> Option<PathBuf> {
    let new = home.join(".hangar").join("config").join(name);
    if new.exists() { Some(new) } else { backend.map(|dir| dir.join(name)) }
}

/// O `.env` do servidor, com o home do processo.
pub fn env_file(backend: Option<&Path>) -> Option<PathBuf> {
    path(&AccountEnvironment::capture().home, backend, ".env")
}

#[cfg(test)]
mod tests {
    use super::path;

    #[test]
    fn the_new_place_wins_and_the_backend_is_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        std::fs::write(backend.join(".env"), "").unwrap();
        assert_eq!(path(&home, Some(&backend), ".env"), Some(backend.join(".env")));
        assert_eq!(path(&home, None, ".env"), None);
        assert!(!home.join(".hangar").exists(), "ler não cria a pasta nova");
        let new = home.join(".hangar/config/.env");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, "").unwrap();
        assert_eq!(path(&home, Some(&backend), ".env"), Some(new.clone()));
        assert_eq!(path(&home, None, ".env"), Some(new));
        // Arquivo a arquivo: o que ainda não migrou continua na `backend/`.
        assert_eq!(path(&home, Some(&backend), "peers.json"), Some(backend.join("peers.json")));
    }
}
