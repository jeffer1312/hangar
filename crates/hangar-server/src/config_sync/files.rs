//! Substituição com backup e desfazer; falha de rename nunca vira cópia parcial no mesmo disco.
use super::{
    bundle::{Blob, Error, sha256},
    paths::{HEAVY_DIRS, Roots},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
};

pub fn mode(path: &Path) -> std::io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(path.metadata()?.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0o644)
    }
}
pub fn chmod(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}
pub fn write_file(path: &Path, blob: &Blob) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid("arquivo sem pasta"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::Builder::new()
        .prefix(".hangar-config-")
        .tempfile_in(parent)?;
    file.write_all(&blob.data)?;
    file.as_file().sync_all()?;
    chmod(file.path(), blob.mode & 0o777)?;
    crate::runtime::queue::replace(file.path(), path)?;
    Ok(())
}
pub fn backup_target(path: &Path, roots: &Roots, backups: &Path) -> Result<PathBuf, Error> {
    let absolute = std::path::absolute(path)?;
    let rel = absolute
        .strip_prefix(&roots.home)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| absolute.components().skip(1).collect());
    let target = backups.join("files").join(rel);
    std::fs::create_dir_all(target.parent().unwrap())?;
    chmod(backups, 0o700)?;
    Ok(target)
}
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, dst)?;
        #[cfg(windows)]
        if src.is_dir() {
            std::os::windows::fs::symlink_dir(target, dst)?;
        } else {
            std::os::windows::fs::symlink_file(target, dst)?;
        }
    } else if meta.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let e = entry?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
        std::fs::set_permissions(dst, meta.permissions())?;
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}
pub fn move_path(src: &Path, dst: &Path) -> Result<(), Error> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_tree(src, dst)?;
            crate::accounts::storage::remove_tree(src)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
fn current(path: &Path) -> Result<Option<BTreeMap<String, (Vec<u8>, bool)>>, Error> {
    if !path.exists() {
        return Ok(None);
    }
    let mut found = BTreeMap::new();
    if path.is_file() {
        found.insert(
            String::new(),
            (std::fs::read(path)?, mode(path)? & 0o111 != 0),
        );
        return Ok(Some(found));
    }
    let mut pending = vec![path.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(dir) = pending.pop() {
        if !seen.insert(dir.canonicalize()?) {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let p = entry?.path();
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            if p.is_dir() {
                if !HEAVY_DIRS.contains(&name.as_ref()) {
                    pending.push(p);
                }
            } else if p.is_file() && !name.starts_with(".hangar") {
                found.insert(
                    p.strip_prefix(path)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    (std::fs::read(&p)?, mode(&p)? & 0o111 != 0),
                );
            }
        }
    }
    Ok(Some(found))
}
pub fn write_entry(
    dest: &Path,
    kind: &str,
    files: &BTreeMap<String, Blob>,
    roots: &Roots,
    backups: &Path,
) -> Result<(bool, Option<PathBuf>), Error> {
    if let Some(current) = current(dest)? {
        if current.len() == files.len()
            && files.iter().all(|(rel, b)| {
                current.get(rel).is_some_and(|(bytes, exec)| {
                    bytes == &b.data && (cfg!(windows) || *exec == (b.mode & 0o111 != 0))
                })
            })
        {
            return Ok((false, None));
        }
    }
    let parent = dest
        .parent()
        .ok_or_else(|| Error::invalid("entrada sem pasta"))?;
    std::fs::create_dir_all(parent)?;
    let staging = if kind != "file" {
        let dir = tempfile::Builder::new()
            .prefix(".hangar-config-")
            .tempdir_in(parent)?;
        for (rel, blob) in files {
            write_file(&dir.path().join(rel), blob)?;
        }
        Some(dir)
    } else {
        None
    };
    let link = if dest.is_symlink() {
        Some(std::fs::read_link(dest)?)
    } else {
        None
    };
    let mut done = Vec::<(PathBuf, PathBuf)>::new();
    let mut move_one = |from: &Path, to: PathBuf| -> Result<(), Error> {
        move_path(from, &to)?;
        done.push((to, from.to_owned()));
        Ok(())
    };
    let outcome = (|| -> Result<(), Error> {
        if kind == "file" {
            if link.is_some() || dest.is_dir() {
                move_one(dest, backup_target(dest, roots, backups)?)?;
            } else if dest.exists() {
                std::fs::copy(dest, backup_target(dest, roots, backups)?)?;
            }
            write_file(
                dest,
                files
                    .get("")
                    .ok_or_else(|| Error::invalid("entrada sem conteúdo"))?,
            )?;
        } else {
            let staging = staging.as_ref().unwrap().path();
            if link.is_none() && dest.is_dir() {
                let mut heavy = HEAVY_DIRS;
                heavy.sort();
                for name in heavy {
                    let old = dest.join(name);
                    if old.is_dir() && !old.is_symlink() {
                        move_one(&old, staging.join(name))?;
                    }
                }
            }
            if link.is_some() || dest.exists() {
                move_one(dest, backup_target(dest, roots, backups)?)?;
            }
            move_path(staging, dest)?;
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        for (now, back) in done.into_iter().rev() {
            if let Err(rollback) = move_path(&now, &back) {
                return Err(Error::new(
                    "config_sync_rollback_failed",
                    format!("{error}; restauração falhou: {rollback}"),
                    json!({}),
                ));
            }
        }
        return Err(error);
    }
    Ok((true, link))
}

fn optional_read(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn json_backup(path: &Path, raw: &[u8], backups: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(backups)?;
    chmod(backups, 0o700)?;
    let name = format!(
        "{}.json",
        sha256(std::path::absolute(path)?.to_string_lossy().as_bytes())
    );
    let dst = backups.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(dst) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let link = if path.is_symlink() {
        Some(std::fs::read_link(path)?.to_string_lossy().into_owned())
    } else {
        None
    };
    let data = json!({"path":path.to_string_lossy(),"conteudo_hex":raw.iter().map(|b|format!("{b:02x}")).collect::<String>(),"symlink":link});
    file.write_all(format!("{}\n", serde_json::to_string_pretty(&data).unwrap()).as_bytes())?;
    Ok(())
}
pub fn edit_json(
    path: &Path,
    backups: &Path,
    mut change: impl FnMut(&mut Value) -> Result<(), Error>,
) -> Result<bool, Error> {
    for _ in 0..3 {
        let raw = optional_read(path)?;
        let mut current = match &raw {
            Some(raw) if !raw.is_empty() => serde_json::from_slice(raw)
                .map_err(|e| Error::new("config_sync_invalid_json", e.to_string(), json!({})))?,
            _ => json!({}),
        };
        if !current.is_object() {
            return Err(Error::new(
                "config_sync_invalid_json",
                format!(
                    "{} não é um objeto JSON",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                json!({"file":path.file_name().unwrap_or_default().to_string_lossy()}),
            ));
        }
        let previous = current.clone();
        change(&mut current)?;
        if raw.is_some() && current == previous {
            chmod(path, 0o600)?;
            return Ok(false);
        }
        let data = format!("{}\n", serde_json::to_string_pretty(&current).unwrap()).into_bytes();
        let parent = path
            .parent()
            .ok_or_else(|| Error::invalid("arquivo sem pasta"))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::Builder::new()
            .prefix(".hangar-config-")
            .tempfile_in(parent)?;
        file.write_all(&data)?;
        file.as_file().sync_all()?;
        chmod(file.path(), 0o600)?;
        if optional_read(path)? != raw {
            continue;
        }
        if let Some(raw) = &raw {
            json_backup(path, raw, &backups.join("json"))?;
        }
        if optional_read(path)? != raw {
            continue;
        }
        crate::runtime::queue::replace(file.path(), path)?;
        return Ok(true);
    }
    Err(Error::new(
        "config_sync_changed",
        format!(
            "Arquivo continua mudando: {}; tentar novamente",
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
        json!({}),
    ))
}
