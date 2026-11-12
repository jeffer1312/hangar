//! Estado privado e escrita comparada; falha nunca confirma uma assinatura nova.
use crate::{Error, Paths, hash, json_bytes};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match fs::read(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !path.is_symlink() => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn private(paths: Paths<'_>, create: bool) -> Result<PathBuf, Error> {
    private_at(paths.home, paths.destination, create)
}
pub(crate) fn private_at(home: &Path, destination: &Path, create: bool) -> Result<PathBuf, Error> {
    let destination = hangar_core::fs::without_verbatim(fs::canonicalize(destination)?);
    let key = hash(destination.to_string_lossy().as_bytes());
    let mut path = home.to_owned();
    for part in [".hangar", "codex-contas", &key] {
        path.push(part);
        if path.is_symlink() {
            return Err(Error::Invalid);
        }
        if create {
            fs::create_dir_all(&path)?;
            restrict(&path, 0o700)?;
        }
    }
    Ok(path)
}
#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}
#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}
pub(crate) fn read_state(paths: Paths<'_>) -> Result<Value, Error> {
    let path = private(paths, false)?.join("estado.json");
    if path.is_symlink() {
        return Err(Error::Invalid);
    }
    let Some(raw) = read(&path)? else {
        return Ok(json!({}));
    };
    let value: Value = serde_json::from_slice(&raw).map_err(|_| Error::Invalid)?;
    if !value.is_object() || !value["public"].is_object() || !value["public"]["status"].is_string()
    {
        return Err(Error::Invalid);
    }
    for key in [
        "config",
        "profiles",
        "resources",
        "source_snapshot",
        "destination_snapshot",
    ] {
        if value.get(key).is_some_and(|v| !v.is_object()) {
            return Err(Error::Invalid);
        }
    }
    Ok(value)
}
pub(crate) fn write_state(paths: Paths<'_>, state: &Value) -> Result<(), Error> {
    let path = private(paths, true)?.join("estado.json");
    if path.is_symlink() {
        return Err(Error::Invalid);
    }
    write(&path, &json_bytes(state), read(&path)?.as_deref(), None)?;
    restrict(&path, 0o600)
}
pub(crate) fn backup(path: &Path, raw: &[u8], directory: &Path) -> Result<(), Error> {
    fs::create_dir_all(directory)?;
    restrict(directory, 0o700)?;
    let key = hash(path.to_string_lossy().as_bytes());
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(directory.join(format!("{key}.json"))) {
        Ok(mut file) => {
            file.write_all(&json_bytes(&json!({"path":path,"conteudo_hex":raw.iter().map(|b|format!("{b:02x}")).collect::<String>(),"symlink":fs::read_link(path).ok()})))?;
            file.sync_all()?;
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub(crate) fn write(
    path: &Path,
    raw: &[u8],
    expected: Option<&[u8]>,
    backups: Option<&Path>,
) -> Result<(), Error> {
    if path.is_symlink() || read(path)?.as_deref() != expected {
        return Err(Error::Changed);
    }
    if expected == Some(raw) {
        return Ok(());
    }
    let parent = path.parent().ok_or(Error::Invalid)?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".hangar-config-")
        .tempfile_in(parent)?;
    tmp.write_all(raw)?;
    tmp.as_file().sync_all()?;
    if let Ok(meta) = fs::metadata(path) {
        fs::set_permissions(tmp.path(), meta.permissions())?;
    }
    if let (Some(backups), Some(raw)) = (backups, expected) {
        backup(path, raw, backups)?;
    }
    if path.is_symlink() || read(path)?.as_deref() != expected {
        return Err(Error::Changed);
    }
    hangar_core::fs::replace(tmp.path(), path)?;
    Ok(())
}
pub(crate) fn write_link(
    path: &Path,
    target: &Path,
    current: Option<&[u8]>,
    previous_link: Option<&Path>,
    backups: &Path,
) -> Result<(), Error> {
    if previous_link == Some(target) {
        return Ok(());
    }
    let parent = path.parent().ok_or(Error::Invalid)?;
    fs::create_dir_all(parent)?;
    let tmp = tempfile::Builder::new()
        .prefix(".hook-")
        .tempdir_in(parent)?;
    let temporary = tmp.path().join("hook");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &temporary)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, &temporary)?;
    if current.is_some() || previous_link.is_some() {
        backup(path, current.unwrap_or_default(), backups)?;
    }
    if fs::read_link(path).ok().as_deref() != previous_link
        || (previous_link.is_none() && read(path)?.as_deref() != current)
    {
        return Err(Error::Changed);
    }
    hangar_core::fs::replace(&temporary, path)?;
    Ok(())
}
pub(crate) fn replace_link_with_file(
    path: &Path,
    raw: &[u8],
    previous_link: Option<&Path>,
    backups: &Path,
) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Invalid)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".hook-")
        .tempfile_in(parent)?;
    tmp.write_all(raw)?;
    tmp.as_file().sync_all()?;
    backup(path, b"", backups)?;
    if fs::read_link(path).ok().as_deref() != previous_link {
        return Err(Error::Changed);
    }
    hangar_core::fs::replace(tmp.path(), path)?;
    Ok(())
}
pub(crate) struct Guard(fs::File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
pub(crate) async fn lock(path: &Path) -> Result<Guard, Error> {
    if path.is_symlink() {
        return Err(Error::Invalid);
    }
    Ok(Guard(hangar_core::fs::lock_exclusive(path).await?))
}
