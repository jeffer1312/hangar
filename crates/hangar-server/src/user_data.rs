//! Dados do usuário (`.env`, `peers.json`, `projects.json`) em `~/.hangar/config/`, fora da pasta da
//! versão. Máquina ainda não migrada os tem na `backend/`: vale o lugar novo quando o arquivo está lá;
//! senão, o antigo. A mesma regra do `app/user_data.py`, mais uma: máquina migrada (`~/.hangar/current`)
//! usa sempre o lugar novo, porque a `backend/` do pacote é da versão e some na troca seguinte, e a do
//! checkout do modo desenvolvedor deixa de ser lida quando o modo desliga.
use std::io;
use std::path::{Path, PathBuf};

use crate::accounts::environment::AccountEnvironment;

/// `None` só quando o arquivo não está no lugar novo, a máquina não migrou e não há `backend/`
/// conhecida. Disco que não deixa conferir o lugar novo é erro, nunca "não existe".
pub fn path(home: &Path, backend: Option<&Path>, name: &str) -> io::Result<Option<PathBuf>> {
    let new = home.join(".hangar").join("config").join(name);
    if new.try_exists()? || crate::install_root::current(home).try_exists()? {
        return Ok(Some(new));
    }
    Ok(backend.map(|dir| dir.join(name)))
}

/// O `.env` do servidor, com o home do processo.
pub fn env_file(backend: Option<&Path>) -> io::Result<Option<PathBuf>> {
    path(&AccountEnvironment::capture().home, backend, ".env")
}

/// O valor de `key` no `.env` dos dados do usuário: a última linha vale; aspas, comentário (` #`) e
/// espaço saem. `None` sem o arquivo ou sem a chave; arquivo que não se lê é erro.
pub fn env_value(home: &Path, backend: Option<&Path>, key: &str) -> Result<Option<String>, String> {
    let env = path(home, backend, ".env").map_err(|e| format!("não consegui conferir o .env dos dados do usuário: {e}"))?;
    let Some(env) = env else { return Ok(None) };
    let text = match std::fs::read_to_string(&env) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", env.display())),
    };
    Ok(text.lines().filter_map(|l| l.trim().strip_prefix(key)?.trim_start().strip_prefix('='))
        .map(|v| v.split(" #").next().unwrap_or(v).trim().trim_matches(['"', '\'']).to_owned()).last())
}

/// Uma porta (`CP_PORT`, `CP_FRONT_PORT`) do `.env`; vazia é ausente, e valor que não é porta é erro.
pub fn env_port(home: &Path, backend: Option<&Path>, key: &str) -> Result<Option<u16>, String> {
    env_value(home, backend, key)?.filter(|v| !v.is_empty())
        .map(|v| v.parse::<u16>().map_err(|_| format!("{key}={v:?} no .env não é uma porta")))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_place_wins_and_the_backend_is_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        std::fs::write(backend.join(".env"), "").unwrap();
        assert_eq!(path(&home, Some(&backend), ".env").unwrap(), Some(backend.join(".env")));
        assert_eq!(path(&home, None, ".env").unwrap(), None);
        assert!(!home.join(".hangar").exists(), "ler não cria a pasta nova");
        let new = home.join(".hangar/config/.env");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, "").unwrap();
        assert_eq!(path(&home, Some(&backend), ".env").unwrap(), Some(new.clone()));
        assert_eq!(path(&home, None, ".env").unwrap(), Some(new));
        // Arquivo a arquivo: o que ainda não migrou continua na `backend/`.
        assert_eq!(path(&home, Some(&backend), "peers.json").unwrap(), Some(backend.join("peers.json")));
    }

    /// `~/.hangar/current` numa versão com manifesto de pacote.
    #[cfg(unix)]
    fn package_home(home: &std::path::Path) {
        let version = home.join(".hangar/versions/0.1.0.5");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("manifest.json"), r#"{"version": "0.1.0.5", "commit": "c", "channel": "latest"}"#).unwrap();
        std::os::unix::fs::symlink(&version, home.join(".hangar/current")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_package_install_uses_the_new_place_even_without_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        std::fs::write(backend.join("peers.json"), "{}").unwrap();
        package_home(&home);
        assert_eq!(path(&home, Some(&backend), "peers.json").unwrap(), Some(home.join(".hangar/config/peers.json")));
        assert_eq!(path(&home, None, "projects.json").unwrap(), Some(home.join(".hangar/config/projects.json")));
    }

    /// Modo desenvolvedor: o arquivo que não existia na migração nasce no lugar novo, não no checkout,
    /// senão some quando o modo desenvolvedor desliga.
    #[cfg(unix)]
    #[test]
    fn a_developer_mode_machine_uses_the_new_place_even_without_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (home, checkout) = (dir.path().join("home"), dir.path().join("checkout"));
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::create_dir_all(checkout.join("backend")).unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::os::unix::fs::symlink(&checkout, home.join(".hangar/current")).unwrap();
        assert_eq!(path(&home, Some(&checkout.join("backend")), "peers.json").unwrap(), Some(home.join(".hangar/config/peers.json")));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_config_dir_is_an_error_not_the_old_place() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        let config = home.join(".hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (read, root) = (path(&home, Some(&backend), ".env"), std::fs::read_dir(&config).is_ok());
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700)).unwrap();
        if root { return; } // root lê tudo: o caso não se monta
        assert!(read.is_err(), "{read:?}");
    }

    #[test]
    fn env_value_strips_quotes_comments_and_spaces_and_a_bad_port_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        assert_eq!(env_port(&home, Some(&backend), "CP_PORT").unwrap(), None, "sem .env");
        std::fs::write(backend.join(".env"), "CP_PORT=1\n  CP_PORT = 2\nCP_PORT=\"9100\"  # a porta\nCP_FRONT_PORT='abc'\nCP_EMPTY=\nCP_X=a #b\n").unwrap();
        assert_eq!(env_port(&home, Some(&backend), "CP_PORT").unwrap(), Some(9100), "a última vale, sem aspas nem comentário");
        assert_eq!(env_value(&home, Some(&backend), "CP_X").unwrap().as_deref(), Some("a"));
        assert_eq!(env_port(&home, Some(&backend), "CP_EMPTY").unwrap(), None, "vazio é ausente");
        assert_eq!(env_port(&home, Some(&backend), "CP_NADA").unwrap(), None);
        let error = env_port(&home, Some(&backend), "CP_FRONT_PORT").unwrap_err();
        assert!(error.contains("CP_FRONT_PORT") && error.contains("abc"), "{error}");
    }
}
