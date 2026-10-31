//! Apelidos compartilhados entre contas; o arquivo não pertence ao login de nenhuma delas.
use super::environment::AccountEnvironment;
use super::quotas::other::py_strip;
use crate::orchestration::codec;
use serde_json::{Map, Value};
use std::{io, path::PathBuf, sync::Mutex};

static WRITER: Mutex<()> = Mutex::new(());

pub fn path(env: &AccountEnvironment) -> PathBuf {
    env.home.join(".claude/.hangar-apelidos.json")
}

pub fn read(env: &AccountEnvironment) -> Map<String, Value> {
    read_checked(env).unwrap_or_default()
}

fn read_checked(env: &AccountEnvironment) -> io::Result<Map<String, Value>> {
    let path = super::quotas::other::read_path(path(env));
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(error),
    };
    let value = codec::loads(&text).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "arquivo de apelidos contém JSON inválido",
        )
    })?;
    if !value.is_object() {
        codec::discard(value);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "arquivo de apelidos precisa ser um objeto",
        ));
    }
    let result = value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !codec::contains_nonfinite(value))
        .filter_map(|(key, value)| {
            value
                .as_str()
                .map(py_strip)
                .filter(|s| !s.is_empty())
                .map(|s| (key.clone(), Value::String(s.to_owned())))
        })
        .collect();
    codec::discard(value);
    Ok(result)
}

pub fn set(
    env: &AccountEnvironment,
    id: &str,
    value: Option<&str>,
) -> io::Result<Map<String, Value>> {
    let _guard = WRITER
        .lock()
        .map_err(|_| io::Error::other("trava de apelidos indisponível"))?;
    let mut current = read_checked(env)?;
    let clean = codec::truncate_string(py_strip(value.unwrap_or_default()), 40).to_owned();
    if clean.is_empty() {
        current.shift_remove(id);
    } else {
        current.insert(id.into(), Value::String(clean));
    }
    let result = Value::Object(current.clone());
    if codec::contains_surrogate(&result) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "apelido não pode ser gravado em UTF-8",
        ));
    }
    let bytes = crate::transcript::pyjson::dumps_indent(&result, 1, false);
    let path = path(env);
    std::fs::create_dir_all(path.parent().unwrap())?;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(bytes.as_bytes())?;
    tmp.as_file().sync_all()?;
    crate::runtime::queue::replace(tmp.path(), &path)?;
    Ok(current)
}
