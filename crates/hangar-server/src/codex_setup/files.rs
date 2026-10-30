//! Escritas comparadas e backups restritos; uma colisão conserva o arquivo externo.
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Value, json};

#[cfg(test)]
#[path="files_config_tests.rs"]
mod config_tests;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::Duration,
};

#[cfg(test)]
#[path = "files_contract_tests.rs"]
mod contract_tests;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Invalid(String),
    Changed(String),
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid(e) | Self::Changed(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for Error {}
pub fn read(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !path.is_symlink() => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn object(path: &Path) -> Result<Value, Error> {
    match read(path)? {
        None => Ok(json!({})),
        Some(raw) => {
            let invalid = || Error::Invalid(format!("Objeto JSON esperado em {}", path.display()));
            let value = crate::orchestration::body::parse(&raw).map_err(|_| invalid())?;
            if value.is_object() { Ok(value) }
            else { crate::orchestration::codec::discard(value); Err(invalid()) }
        },
    }
}
pub fn bytes(data: &Value) -> Result<Vec<u8>, Error> {
    let text = crate::orchestration::codec::dumps_indented_utf8(data,2).map_err(|error|Error::Invalid(error.into()))?;
    Ok(format!("{text}\n").into_bytes())
}
pub fn hash(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn restricted_directory(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(error) => Err(error.into()),
    }
}
pub fn backup(path: &Path, raw: &[u8], root: &Path) -> Result<(), Error> {
    restricted_directory(root)?;
    let name = hash(std::path::absolute(path)?.to_string_lossy().as_bytes());
    let data = json!({"path":path,"conteudo_hex":raw.iter().map(|b|format!("{b:02x}")).collect::<String>(),"symlink":fs::read_link(path).ok()});
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(root.join(format!("{name}.json"))) {
        Ok(mut file) => file.write_all(&bytes(&data)?)?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub fn write(
    path: &Path,
    raw: &[u8],
    expected: Option<&[u8]>,
    backups: Option<&Path>,
) -> Result<bool, Error> {
    if expected == Some(raw) {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("arquivo sem diretório".into()))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".hangar-config-")
        .tempfile_in(parent)?;
    temporary.write_all(raw)?;
    temporary.as_file().sync_all()?;
    if let Ok(meta) = fs::metadata(path) {
        fs::set_permissions(temporary.path(), meta.permissions())?;
    }
    let check = || -> Result<(), Error> {
        if read(path)?.as_deref() != expected {
            Err(Error::Changed(format!(
                "Arquivo alterado durante a integração: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )))
        } else {
            Ok(())
        }
    };
    check()?;
    if let (Some(backups), Some(previous)) = (backups, expected) {
        backup(path, previous, backups)?;
    }
    check()?;
    crate::runtime::queue::replace(temporary.path(), path)?;
    Ok(true)
}
pub fn transform(
    path: &Path,
    backups: &Path,
    mut change: impl FnMut(Option<&[u8]>) -> Result<Vec<u8>, Error>,
) -> Result<bool, Error> {
    for _ in 0..3 {
        let previous = read(path)?;
        let raw = change(previous.as_deref())?;
        match write(path, &raw, previous.as_deref(), Some(backups)) {
            Err(Error::Changed(_)) => continue,
            result => return result,
        }
    }
    Err(Error::Changed(format!(
        "Arquivo continua mudando: {}; tentar novamente",
        path.file_name().unwrap_or_default().to_string_lossy()
    )))
}
pub async fn exclusive(path: &Path) -> Result<fs::File, Error> {
    restricted_directory(
        path.parent()
            .ok_or_else(|| Error::Invalid("lock sem diretório".into()))?,
    )?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}
pub fn remap(value: &Value, old: &Path, new: &Path) -> Value {
    match value {
        Value::String(s) => json!(
            s.replace(
                old.to_string_lossy().as_ref(),
                new.to_string_lossy().as_ref()
            )
            .replace(
                &old.to_string_lossy().replace('\\', "/"),
                &new.to_string_lossy().replace('\\', "/")
            )
        ),
        Value::Array(a) => Value::Array(a.iter().map(|v| remap(v, old, new)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), remap(v, old, new)))
                .collect(),
        ),
        v => v.clone(),
    }
}

pub async fn edit_config(
    env: &AccountEnvironment,
    path: &Path,
    backups: &Path,
    work_dir: &Path,
    mut prepare: impl FnMut(&Value) -> Result<Vec<Value>, String>,
) -> Result<(), String> {
    restricted_directory(work_dir).map_err(|e| e.to_string())?;
    for _ in 0..3 {
        let raw = read(path).map_err(|e| e.to_string())?;
        let current = match raw.as_deref().filter(|b| !b.is_empty()) {
            None => json!({}),
            Some(b) => super::instructions_toml::parse(
                std::str::from_utf8(b).map_err(|_| "config.toml não é UTF-8")?,
            )
            .map_err(|_| "config.toml inválido")?,
        };
        let edits = prepare(&current)?;
        if edits.is_empty() {
            return Ok(());
        }
        let home = tempfile::Builder::new()
            .prefix("hangar-config-")
            .tempdir_in(work_dir)
            .map_err(|e| e.to_string())?;
        let codex = home.path().join(".codex");
        fs::create_dir(&codex).map_err(|e| e.to_string())?;
        let copy = codex.join("config.toml");
        write(&copy, raw.as_deref().unwrap_or(b""), None, None).map_err(|e| e.to_string())?;
        let mut native = super::native::Native::open_isolated(env, home).await?;
        let result = native
            .request(
                "config/batchWrite",
                json!({"edits":edits,"reloadUserConfig":false}),
                Duration::from_secs(60),
            )
            .await;
        let updated = result.and_then(|_| fs::read(copy).map_err(|e| e.to_string()));
        native.close().await?;
        let updated = updated?;
        match write(path, &updated, raw.as_deref(), Some(backups)) {
            Ok(_) => return Ok(()),
            Err(Error::Changed(_)) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("config.toml continua mudando; nova leitura necessária".into())
}
