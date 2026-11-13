//! Leitura dos `.claude.json` e da entrada guardada, com os erros que a tela mostra.
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use hangar_core::pycompat::truthy;

use crate::{Error, NAME, Paths};

/// Um `.claude.json` lido e a entrada do MCP nele (`None` = ausente ou não objeto).
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub data: Map<String, Value>,
    pub entry: Option<Map<String, Value>>,
}

/// O principal e cada conta que já tem `.claude.json`; o primeiro ilegível recusa tudo.
pub fn read_entries(paths: &Paths) -> Result<Vec<FileEntry>, Error> {
    let main = paths.main_file();
    paths.config_files().into_iter().map(|path| {
        let account = path != main;
        read_entry(path, account)
    }).collect()
}

/// A entrada ativa no principal ou, desligado, a guardada.
pub fn known_entry(paths: &Paths) -> Result<Option<Map<String, Value>>, Error> {
    let main = read_entry(paths.main_file(), false)?;
    known_from(paths, main.entry)
}

pub(crate) fn known_from(paths: &Paths, active: Option<Map<String, Value>>) -> Result<Option<Map<String, Value>>, Error> {
    if active.is_some() {
        return Ok(active);
    }
    let parked = read_object(&paths.parked_file(), false)?;
    Ok((!parked.is_empty()).then_some(parked))
}

/// Conta com arquivo de 0 bytes vale `{}`, como conta nunca aberta; o principal vazio é erro.
pub(crate) fn read_entry(path: PathBuf, account: bool) -> Result<FileEntry, Error> {
    let data = read_object(&path, account)?;
    let servers = data.get("mcpServers").filter(|v| truthy(v));
    if servers.is_some_and(|v| !v.is_object()) {
        return Err(Error::read(
            &path,
            "mcpServers inválido",
            format!("mcpServers em {} não é um objeto JSON", path.display()),
        ));
    }
    let entry = servers.and_then(|v| v.get(NAME)).and_then(Value::as_object).cloned();
    Ok(FileEntry { path, data, entry })
}

/// Ausente = `{}`; ilegível, JSON inválido ou não objeto = `erro_computer_control_read`.
pub(crate) fn read_object(path: &Path, empty_ok: bool) -> Result<Map<String, Value>, Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => {
            let error = crate::strerror(&error);
            return Err(Error::read(path, &error, format!("não consegui ler {}: {error}", path.display())));
        }
    };
    if empty_ok && bytes.is_empty() {
        return Ok(Map::new());
    }
    match parse(&bytes) {
        Some(Value::Object(data)) => Ok(data),
        Some(_) => Err(Error::read(path, "", format!("{} não é um objeto JSON", path.display()))),
        None => {
            let error = "JSON ou encoding inválido";
            Err(Error::read(path, error, format!("não consegui ler {}: {error}", path.display())))
        }
    }
}

/// UTF-8 estrito e JSON, como o `json.loads(read_text())` do Python.
pub(crate) fn parse(bytes: &[u8]) -> Option<Value> {
    serde_json::from_str(std::str::from_utf8(bytes).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn claude_json_reader_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256), account: bool) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".claude.json");
            std::fs::write(&path, &bytes).unwrap();
            let _ = read_entry(path, account);
        }

        #[test]
        fn claude_json_text_reader_never_panics(text in "\\PC{0,200}", account: bool) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".claude.json");
            std::fs::write(&path, format!("{{\"mcpServers\": {text}}}")).unwrap();
            let _ = read_entry(path, account);
        }
    }
}
