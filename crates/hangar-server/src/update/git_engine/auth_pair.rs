//! Troca servidor/metadados como par; biblioteca por conteúdo nunca é sobrescrita.
use super::releases::{self, Held};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
type Snapshot = Vec<(PathBuf, Option<Vec<u8>>)>;
fn previous(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.prev",
        path.file_name().unwrap().to_string_lossy()
    ))
}
fn snapshot(paths: &[PathBuf]) -> io::Result<Snapshot> {
    paths
        .iter()
        .map(|p| {
            Ok((
                p.clone(),
                if p.exists() { Some(fs::read(p)?) } else { None },
            ))
        })
        .collect()
}
fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
fn restore_snapshot(before: &Snapshot) -> Vec<String> { restore_snapshot_with(before, &releases::install) }
fn restore_snapshot_with(before: &Snapshot, install:&dyn Fn(&[u8], &Path)->io::Result<()>) -> Vec<String> {
    let mut errors = vec![];
    for (target, data) in before {
        let result = match data {
            None => remove(target),
            Some(data) if !target.is_file() || fs::read(target).ok().as_ref() != Some(data) => {
                install(data, target)
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            errors.push(format!(
                "{}: desfazer falhou ({e})",
                target.file_name().unwrap().to_string_lossy()
            ));
        }
    }
    errors
}
pub fn metadata(
    manifest: &Value,
    platform: &str,
    dest: &Path,
) -> Result<(PathBuf, Vec<u8>), String> {
    let error = || "biblioteca de autenticação ausente ou inválida no manifesto".to_string();
    let sha = manifest["files"][format!("{platform}/hangar-auth")]["sha256"]
        .as_str()
        .ok_or_else(error)?
        .to_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error());
    }
    let server = manifest["files"][format!("{platform}/hangar-server")]["sha256"]
        .as_str()
        .ok_or_else(error)?
        .to_lowercase();
    let library = if platform.starts_with("windows") {
        "hangar_auth.dll"
    } else if platform.starts_with("macos") {
        "libhangar_auth.dylib"
    } else {
        "libhangar_auth.so"
    };
    let relative = format!("auth/{sha}/{library}");
    let fields = json!({"abi":1,"library":relative,"library_sha256":sha,"server_sha256":server,"store_version":1});
    let bytes = format!(
        "{{{}}}",
        fields
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| format!(
                "{}: {}",
                serde_json::to_string(k).unwrap(),
                serde_json::to_string(v).unwrap()
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .into_bytes();
    Ok((dest.join(relative), bytes))
}
pub fn validate(server: &Path) -> Result<(), String> {
    let server = fs::canonicalize(server).map_err(|e| e.to_string())?;
    let root = server.parent().unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(server.with_extension("auth.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let invalid = || "auth_pair_invalid".to_string();
    let map = manifest.as_object().ok_or_else(invalid)?;
    if map.len() != 5
        || ![
            "abi",
            "store_version",
            "server_sha256",
            "library_sha256",
            "library",
        ]
        .iter()
        .all(|k| map.contains_key(*k))
    {
        return Err(invalid());
    }
    if manifest["abi"].as_u64() != Some(1) || manifest["store_version"].as_u64() != Some(1) {
        return Err("auth_abi_mismatch".into());
    }
    let sha = manifest["library_sha256"].as_str().ok_or_else(invalid)?;
    let relative = manifest["library"].as_str().ok_or_else(invalid)?;
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || ![
            "libhangar_auth.so",
            "libhangar_auth.dylib",
            "hangar_auth.dll",
        ]
        .iter()
        .any(|n| relative == format!("auth/{sha}/{n}"))
    {
        return Err(invalid());
    }
    let library = fs::canonicalize(root.join(relative)).map_err(|e| e.to_string())?;
    if !library.starts_with(root) || !library.is_file() {
        return Err(invalid());
    }
    if !releases::matches_file(&server, manifest["server_sha256"].as_str().unwrap_or(""))
        || !releases::matches_file(&library, sha)
    {
        return Err("auth_pair_hash_mismatch".into());
    }
    Ok(())
}
pub fn install_one(item: &Held) -> Option<String> {install_one_with(item,&releases::install)}
fn install_one_with(item: &Held, install:&dyn Fn(&[u8], &Path)->io::Result<()>) -> Option<String> {
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(item.target.parent().unwrap())?;
        if item.name != "hangar-auth" {
            return install(&item.data, &item.target);
        }
        let matches = || releases::matches_file(&item.target, &releases::digest(&item.data));
        if item.target.exists() {
            return if matches() {
                Ok(())
            } else {
                Err(io::Error::other("auth_library_hash_mismatch"))
            };
        }
        let temp = item.target.with_file_name(format!(
            ".{}.{}.tmp",
            item.target.file_name().unwrap().to_string_lossy(),
            releases::nonce()?
        ));
        let result = (|| {
            fs::write(&temp, &item.data)?;
            match fs::hard_link(&temp, &item.target) {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if matches() {
                        Ok(())
                    } else {
                        Err(io::Error::other("auth_library_hash_mismatch"))
                    }
                }
                other => other,
            }
        })();
        let _ = remove(&temp);
        result
    })();
    result.err().map(|e| {
        format!(
            "{}: não consegui gravar em {} ({e})",
            item.name,
            item.target.parent().unwrap().display()
        )
    })
}
fn keep_previous(target: &Path, name: &str) -> Option<String> {
    let prev = previous(target);
    if !target.is_file() {
        return if matches!(name, "hangar-server" | "hangar-auth-pair") {
            fs::write(prev, [])
                .err()
                .map(|e| format!("{name}: não consegui guardar a ausência do par anterior ({e})"))
        } else {
            None
        };
    }
    let result = (|| -> io::Result<()> {
        let temp = prev.with_file_name(format!(
            ".{}.{}.tmp",
            prev.file_name().unwrap().to_string_lossy(),
            releases::nonce()?
        ));
        let result = fs::read(target)
            .and_then(|data| fs::write(&temp, data))
            .and_then(|_| crate::runtime::queue::replace(&temp, &prev));
        let _ = remove(&temp);
        result
    })();
    result.err().map(|e|{let _=remove(&prev);format!("{name}: sem cópia do binário anterior; se a atualização voltar, o Rust não volta junto ({e})")})
}
fn paths(items: &[&Held]) -> (PathBuf, PathBuf) {
    let server = items
        .iter()
        .find(|i| i.name == "hangar-server")
        .map(|i| i.target.clone())
        .unwrap_or_else(|| {
            let root = items
                .iter()
                .find(|i| i.name == "hangar-auth-pair")
                .map(|i| i.target.parent().unwrap())
                .unwrap_or_else(|| items[0].target.ancestors().nth(3).unwrap());
            root.join(if cfg!(windows) {
                "hangar-server.exe"
            } else {
                "hangar-server"
            })
        });
    let metadata = server.with_extension("auth.json");
    (server, metadata)
}
pub fn publish(held: &[Held], preserve: bool) -> Vec<String> {publish_with(held,preserve,&releases::install)}
pub fn publish_with(held: &[Held], preserve: bool, install:&dyn Fn(&[u8], &Path)->io::Result<()>) -> Vec<String> {
    let mut warnings = vec![];
    for item in held.iter().filter(|i| i.name == "hangar-cano") {
        if preserve {
            if let Some(e) = keep_previous(&item.target, &item.name) {
                warnings.push(e);
            }
        }
        if let Some(e) = install_one_with(item,install) {
            warnings.push(e);
            let _ = remove(&previous(&item.target));
        }
    }
    let mut pair = held
        .iter()
        .filter(|i| i.name != "hangar-cano")
        .collect::<Vec<_>>();
    if pair.is_empty() {
        return warnings;
    }
    let (server, metadata) = paths(&pair);
    let targets = [server.clone(), metadata.clone()];
    let previous_paths = targets.iter().map(|p| previous(p)).collect::<Vec<_>>();
    let (before, prev) = match snapshot(&targets).and_then(|b| Ok((b, snapshot(&previous_paths)?)))
    {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!(
                "hangar-auth-pair: não consegui preparar a troca ({e})"
            ));
            return warnings;
        }
    };
    let erased = previous_paths.into_iter().map(|p| (p, None)).collect();
    if pair.iter().any(|i| i.name != "hangar-auth") {
        for (p, n) in [(&server, "hangar-server"), (&metadata, "hangar-auth-pair")] {
            if let Some(e) = keep_previous(p, n) {
                warnings.push(e);
                warnings.extend(restore_snapshot_with(&erased,install));
                return warnings;
            }
        }
    }
    pair.sort_by_key(|i| match i.name.as_str() {
        "hangar-auth" => 0,
        "hangar-server" => 1,
        _ => 2,
    });
    let failure = pair.iter().find_map(|i| install_one_with(i,install)).or_else(|| {
        validate(&server)
            .err()
            .map(|e| format!("hangar-auth-pair: o par publicado não confere ({e})"))
    });
    if let Some(e) = failure {
        warnings.push(e);
        let undo = restore_snapshot_with(&before,install);
        if undo.is_empty() {
            warnings.extend(restore_snapshot_with(&erased,install));
        } else {
            warnings.extend(undo);
        }
    } else if !preserve {
        warnings.extend(restore_snapshot_with(&prev,install));
    }
    warnings
}
pub fn restore(server: &Path) -> Vec<String> {
    let metadata = server.with_extension("auth.json");
    let sp = previous(server);
    let mp = previous(&metadata);
    if !sp.exists() && !mp.exists() {
        return vec![];
    }
    if !sp.is_file() || (!mp.is_file() && metadata.exists()) {
        return vec![
            "hangar-auth-pair: cópias anteriores incompletas; o par atual foi conservado".into(),
        ];
    }
    let captured = (|| -> io::Result<_> {
        let before = snapshot(&[server.to_path_buf(), metadata.clone()])?;
        let data = fs::read(&sp)?;
        let meta = if mp.exists() { fs::read(&mp)? } else { vec![] };
        Ok((
            before,
            vec![
                (server.to_path_buf(), (!data.is_empty()).then_some(data)),
                (metadata.clone(), (!meta.is_empty()).then_some(meta)),
            ],
        ))
    })();
    let (before, restored) = match captured {
        Ok(v) => v,
        Err(e) => {
            return vec![format!(
                "hangar-auth-pair: cópias anteriores ilegíveis ({e})"
            )];
        }
    };
    let mut errors = restore_snapshot(&restored);
    if errors.is_empty() && restored[1].1.is_some() {
        if let Err(e) = validate(server) {
            errors.push(format!(
                "hangar-auth-pair: o par anterior não confere ({e})"
            ));
        }
    }
    if !errors.is_empty() {
        errors.extend(restore_snapshot(&before));
        return errors;
    }
    for (p, n) in [(sp, "hangar-server"), (mp, "hangar-auth-pair")] {
        if let Err(e) = remove(&p) {
            errors.push(format!("{n}: cópia já restaurada não saiu ({e})"));
        }
    }
    errors
}
