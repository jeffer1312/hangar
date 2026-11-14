//! Leitura dos `.claude.json` e da entrada guardada, com os erros que a tela mostra.
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use hangar_core::pycompat::{pyjson, truthy};

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
    parked_entry(paths)
}

/// A entrada guardada ao desligar (`None` = não há).
pub fn parked_entry(paths: &Paths) -> Result<Option<Map<String, Value>>, Error> {
    let parked = read_object(&paths.parked_file(), false)?;
    Ok((!parked.is_empty()).then_some(parked))
}

/// O pacote está instalado: `install.json` com tag e o binário do MCP. A mesma regra para ligar pelo
/// `PUT`, registrar sozinho e o `package_exists` da tela. `install.json` ilegível é erro.
pub fn installed(paths: &Paths) -> Result<bool, Error> {
    Ok(installed_with(paths, &read_object(&paths.install_json(), false)?))
}

/// `installed` com o `install.json` já lido.
pub(crate) fn installed_with(paths: &Paths, install: &Map<String, Value>) -> bool {
    install.get("tag").is_some_and(truthy) && paths.binary().is_file()
}

/// Conta com arquivo de 0 bytes vale `{}`, como conta nunca aberta; o principal vazio é erro.
pub fn read_entry(path: PathBuf, account: bool) -> Result<FileEntry, Error> {
    let data = read_object(&path, account)?;
    let entry = entry_of(&path, &data)?;
    Ok(FileEntry { path, data, entry })
}

/// A entrada do MCP num `.claude.json` já lido; `mcpServers` verdadeiro e não objeto é erro.
pub(crate) fn entry_of(path: &Path, data: &Map<String, Value>) -> Result<Option<Map<String, Value>>, Error> {
    let servers = data.get("mcpServers").filter(|v| truthy(v));
    if servers.is_some_and(|v| !v.is_object()) {
        return Err(Error::read(
            path,
            "mcpServers inválido",
            format!("mcpServers em {} não é um objeto JSON", path.display()),
        ));
    }
    Ok(servers.and_then(|v| v.get(NAME)).and_then(Value::as_object).cloned())
}

/// Ausente = `{}`; ilegível, JSON inválido ou não objeto = `erro_computer_control_read`.
pub(crate) fn read_object(path: &Path, empty_ok: bool) -> Result<Map<String, Value>, Error> {
    parsed(path, read_raw(path)?.as_deref(), empty_ok)
}

/// Os bytes do arquivo; ausente = `None`.
pub(crate) fn read_raw(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    // O Claude Code troca o arquivo por rename: no Windows a leitura no meio da troca falha por um instante.
    match hangar_core::fs::read_replaced(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(path, &error)),
    }
}

/// Ausente = `{}`.
pub(crate) fn parsed(path: &Path, bytes: Option<&[u8]>, empty_ok: bool) -> Result<Map<String, Value>, Error> {
    bytes.map_or_else(|| Ok(Map::new()), |bytes| object_of(path, bytes, empty_ok))
}

fn unreadable(path: &Path, error: &std::io::Error) -> Error {
    let error = crate::strerror(error);
    Error::read(path, &error, format!("não consegui ler {}: {error}", path.display()))
}

/// O objeto JSON de um arquivo já lido, com os mesmos erros de `read_object`.
fn object_of(path: &Path, bytes: &[u8], empty_ok: bool) -> Result<Map<String, Value>, Error> {
    if empty_ok && bytes.is_empty() {
        return Ok(Map::new());
    }
    match parse(bytes) {
        Some(Value::Object(data)) => Ok(data),
        Some(_) => Err(Error::read(path, "", format!("{} não é um objeto JSON", path.display()))),
        None => {
            let error = "JSON ou encoding inválido";
            Err(Error::read(path, error, format!("não consegui ler {}: {error}", path.display())))
        }
    }
}

/// UTF-8 estrito e JSON, como o `json.loads(read_text())` do Python, que aceita o surrogate solto
/// (`\ud800`) que o Claude Code grava. Ele fica como marcador no valor (quem regrava devolve o
/// original); o que vai à tela passa por `py::scrub_*`.
pub(crate) fn parse(bytes: &[u8]) -> Option<Value> {
    pyjson::loads_lossless(std::str::from_utf8(bytes).ok()?)
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

    #[test]
    fn claude_json_with_lone_surrogate_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, r#"{"a": "\ud800", "mcpServers": {"x\udc00": {}}}"#).unwrap();
        assert!(read_entry(path, true).is_ok());
    }
}
