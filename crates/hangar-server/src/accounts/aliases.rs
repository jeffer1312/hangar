//! Apelidos compartilhados entre contas; o arquivo não pertence ao login de nenhuma delas.
use super::environment::AccountEnvironment;
use serde_json::{Map, Value};
use std::{io, path::PathBuf, sync::Mutex};

static WRITER: Mutex<()> = Mutex::new(());

pub fn path(env: &AccountEnvironment) -> PathBuf {
    env.home.join(".claude/.hangar-apelidos.json")
}

pub fn read(env: &AccountEnvironment) -> Map<String, Value> {
    let path = super::quotas::other::read_path(path(env));
    let value = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    value
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            value
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| (key.clone(), Value::String(s.to_owned())))
        })
        .collect()
}

pub fn set(
    env: &AccountEnvironment,
    id: &str,
    value: Option<&str>,
) -> io::Result<Map<String, Value>> {
    let _guard = WRITER
        .lock()
        .map_err(|_| io::Error::other("trava de apelidos indisponível"))?;
    let mut current = read(env);
    let clean: String = value.unwrap_or_default().trim().chars().take(40).collect();
    if clean.is_empty() {
        current.shift_remove(id);
    } else {
        current.insert(id.into(), Value::String(clean));
    }
    let bytes = crate::transcript::pyjson::dumps_indent(&Value::Object(current.clone()), 1, false);
    let path = path(env);
    std::fs::create_dir_all(path.parent().unwrap())?;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(bytes.as_bytes())?;
    tmp.as_file().sync_all()?;
    crate::runtime::queue::replace(tmp.path(), &path)?;
    Ok(current)
}
