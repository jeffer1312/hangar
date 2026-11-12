//! Cookie compartilhado do painel OpenCode, usado pela leitura de cotas.
#![forbid(unsafe_code)]

use std::{
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{Map, Value, json};

use hangar_core::orq_codec::codec;

static WRITER: Mutex<()> = Mutex::new(());

/// Arquivo compartilhado, independente da conta ativa do Claude.
pub fn path(home: &Path) -> PathBuf {
    home.join(".claude/.hangar-opencode.json")
}

/// Conserva apenas cookies completos; erro de arquivo ou JSON continua distinguível pelo leitor.
///
/// # Errors
/// Retorna o erro de leitura, ou InvalidData se o conteúdo não for um objeto JSON.
pub fn read(home: &Path) -> io::Result<Map<String, Value>> {
    let bytes =
        match hangar_core::fs::read_replaced(&hangar_core::fs::legacy_sidecar_path(path(home))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(error) => return Err(error),
        };
    let text =
        std::str::from_utf8(&bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let value = codec::loads(text).ok_or(io::ErrorKind::InvalidData)?;
    let Value::Object(entries) = value else {
        return Err(io::ErrorKind::InvalidData.into());
    };
    Ok(entries
        .into_iter()
        .filter(|(_, entry)| {
            ["workspace_id", "auth_cookie"].iter().all(|key| {
                entry
                    .get(key)
                    .filter(|value| !codec::contains_nonfinite(value))
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            })
        })
        .collect())
}

/// Grava ou remove uma entrada, preservando as outras e relendo o resultado antes de confirmar.
///
/// # Errors
/// Retorna erro de leitura, trava, criação de diretório, escrita ou troca atômica. Não confirma gravação que falhou.
pub fn write(home: &Path, id: &str, workspace: &str, cookie: &str) -> io::Result<bool> {
    let _guard = WRITER
        .lock()
        .map_err(|_| io::Error::other("trava dos cookies indisponível"))?;
    let mut entries: Map<String, Value> = read_for_write(home)?
        .into_iter()
        .map(|(id, entry)| {
            (
                id,
                json!({"workspace_id":entry["workspace_id"], "auth_cookie":entry["auth_cookie"]}),
            )
        })
        .collect();
    let (workspace, cookie) = (
        hangar_core::pycompat::py::strip(workspace),
        hangar_core::pycompat::py::strip(cookie),
    );
    if workspace.is_empty() || cookie.is_empty() {
        entries.remove(id);
    } else {
        entries.insert(
            id.to_owned(),
            json!({"workspace_id":workspace, "auth_cookie":cookie}),
        );
    }
    let target = path(home);
    std::fs::create_dir_all(
        target
            .parent()
            .ok_or_else(|| io::Error::other("arquivo sem diretório"))?,
    )?;
    let value = Value::Object(entries);
    let serialized = codec::dumps_indented_utf8(&value, 1)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    hangar_core::fs::atomic_write(&target, serialized.as_bytes())?;
    Ok(read_for_write(home)?.contains_key(id))
}

fn read_for_write(home: &Path) -> io::Result<Map<String, Value>> {
    match read(home) {
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(Map::new()),
        result => result,
    }
}
