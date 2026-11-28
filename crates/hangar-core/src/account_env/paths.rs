use std::path::{Path, PathBuf};

pub fn display_account_path(path: &Path) -> String {
    // O prefixo estendido do Windows é detalhe do canonicalize, ausente nos DTOs Python.
    let value = path.to_string_lossy();
    value
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| value.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| value.into_owned())
}
pub fn resolve_account_path(path: &Path) -> PathBuf {
    path.canonicalize()
        .map(|p| PathBuf::from(display_account_path(&p)))
        .unwrap_or_else(|_| path.to_owned())
}
