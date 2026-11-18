//! Caminhos da instalação e dos logs do Hangar na máquina.
use std::path::{Path, PathBuf};

use crate::account_env::AccountEnvironment;

/// Mesmo lugar do `log_paths.base()` do Python: o log é do Hangar da máquina, não da conta.
pub fn log_base() -> Option<PathBuf> {
    if cfg!(windows) {
        let root = std::env::var_os("LOCALAPPDATA").filter(|dir| !dir.is_empty()).map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join("AppData").join("Local")))?;
        return Some(root.join("hangar").join("logs"));
    }
    Some(std::env::home_dir()?.join(".hangar").join("logs"))
}

/// `log_base` pelo ambiente-base do registro (nos testes, a casa de teste, nunca o diário da máquina).
pub fn log_base_in(env: &AccountEnvironment) -> Option<PathBuf> {
    let home = Some(env.home.clone()).filter(|home| !home.as_os_str().is_empty());
    if cfg!(windows) {
        let root = env.base.get("LOCALAPPDATA").filter(|dir| !dir.is_empty()).map(PathBuf::from)
            .or_else(|| home.map(|home| home.join("AppData").join("Local")))?;
        return Some(root.join("hangar").join("logs"));
    }
    Some(home?.join(".hangar").join("logs"))
}

/// Raiz de uma cópia do Hangar: versão do pacote (`manifest.json`) ou checkout (`.git` e
/// `backend/app`, a mesma marca do `migrate-package` e do modo desenvolvedor). A versão do pacote
/// pode não ter `backend/`, por isso a marca é da raiz.
pub fn is_install_root(dir: &Path) -> bool {
    dir.join("manifest.json").is_file() || (dir.join(".git").exists() && dir.join("backend").join("app").is_dir())
}

/// Pasta `backend/` de uma cópia do Hangar: a que ainda tem o `.env` (máquina não migrada) ou a de
/// uma raiz reconhecida, mesmo que não exista no disco (o `.env` já está em `~/.hangar/config/`).
pub fn is_backend(dir: &Path) -> bool {
    dir.join(".env").is_file()
        || (dir.file_name().is_some_and(|name| name == "backend") && dir.parent().is_some_and(is_install_root))
}

/// A `backend/` da pasta onde o servidor roda: ela mesma (lançamento antigo) ou a da raiz da cópia
/// (o lançador põe o servidor na raiz da versão).
pub fn backend_of(dir: &Path) -> Option<PathBuf> {
    if is_backend(dir) { return Some(dir.to_path_buf()); }
    is_install_root(dir).then(|| dir.join("backend"))
}

/// `~/.hangar/current`: link (symlink no Linux, junction no Windows) para `versions/<v>` ou, no modo
/// desenvolvedor, para um checkout.
pub fn current(home: &Path) -> PathBuf { home.join(".hangar").join("current") }

/// Onde o servidor anota a pasta `backend/` dele, para o comando instalado longe dela achar o `.env`.
/// Continua gravado porque a versão anterior, na volta, só lê este.
pub fn pointer(home: &Path) -> PathBuf { home.join(".hangar").join("backend-dir") }

/// Onde o servidor anota a raiz da cópia de onde roda; vence o `backend-dir`, salvo quando ele foi
/// gravado depois (a versão anterior subiu na volta e só conhece aquele).
pub fn root_pointer(home: &Path) -> PathBuf { home.join(".hangar").join("install-root-dir") }

#[cfg(test)]
mod tests {
    use super::{backend_of, is_backend, is_install_root};

    /// A marca da instalação é a raiz (manifesto ou checkout), não um arquivo dentro de `backend/`;
    /// o servidor na raiz e o lançado do jeito antigo, na `backend/`, acham a mesma `backend/`.
    #[test]
    fn the_install_marker_is_the_root_and_both_cwds_find_the_backend() {
        let dir = tempfile::tempdir().unwrap();
        let (version, checkout, other) = (dir.path().join("v"), dir.path().join("c"), dir.path().join("x"));
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("manifest.json"), "{}").unwrap();
        std::fs::create_dir_all(checkout.join("backend/app")).unwrap();
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::create_dir_all(other.join("backend/app")).unwrap();
        assert!(is_install_root(&version) && is_install_root(&checkout) && !is_install_root(&other));
        assert_eq!(backend_of(&version), Some(version.join("backend")), "a versão sem backend/ ainda tem onde procurar o .env");
        assert_eq!(backend_of(&checkout), Some(checkout.join("backend")));
        assert_eq!(backend_of(&checkout.join("backend")), Some(checkout.join("backend")));
        assert_eq!(backend_of(&other), None);
        assert!(!is_backend(&other.join("backend")), "backend/app sozinha não é marca");
        std::fs::write(other.join("backend/.env"), "").unwrap();
        assert_eq!(backend_of(&other.join("backend")), Some(other.join("backend")), "máquina não migrada: o .env basta");
    }
}
