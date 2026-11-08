//! Onde fica o repositório de uma pasta, lido da `.git` sem abrir o git: o que o
//! `git rev-parse --git-common-dir --show-toplevel` responde nos formatos comuns.
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Layout {
    /// `common` é a pasta git comum (a `.git` do checkout principal); `toplevel`, a raiz da cópia.
    Repo { common: PathBuf, toplevel: PathBuf },
    /// Nenhuma `.git` subindo a partir da pasta.
    NotRepo,
    /// Formato que só o git sabe ler (variável `GIT_*`, `.git` torta): pergunte a ele.
    Unknown,
}

/// Variáveis que mudam onde o git procura o repositório.
pub const DISCOVERY_VARS: &[&str] = &["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_CEILING_DIRECTORIES", "GIT_DISCOVERY_ACROSS_FILESYSTEM"];

/// Pasta git de verdade: o git exige `HEAD`; sem ele a `.git` não é repositório para ele.
fn git_dir(path: &Path) -> bool { path.join("HEAD").is_file() }

/// Caminho escrito num arquivo da pasta git (`gitdir`, `commondir`): relativo a `base`.
fn pointer(base: &Path, text: &str) -> PathBuf {
    let target = Path::new(text.trim_end_matches(['\r', '\n']));
    if target.is_absolute() { target.to_path_buf() } else { base.join(target) }
}

pub fn layout(cwd: &Path) -> Layout {
    for dir in crate::real(cwd).ancestors() {
        let dot = dir.join(".git");
        let Ok(meta) = std::fs::metadata(&dot) else { continue };
        let gitdir = if meta.is_dir() {
            dot
        } else if meta.is_file() {
            let Ok(text) = std::fs::read_to_string(&dot) else { return Layout::Unknown };
            let Some(target) = text.strip_prefix("gitdir: ") else { return Layout::Unknown };
            pointer(dir, target)
        } else {
            return Layout::Unknown;
        };
        if !git_dir(&gitdir) { return Layout::Unknown; }
        let common = match std::fs::read_to_string(gitdir.join("commondir")) {
            Ok(text) => pointer(&gitdir, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => gitdir,
            Err(_) => return Layout::Unknown,
        };
        if !git_dir(&common) { return Layout::Unknown; }
        return Layout::Repo { common, toplevel: dir.to_path_buf() };
    }
    Layout::NotRepo
}

#[cfg(test)]
mod tests;
