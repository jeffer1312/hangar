//! `settings.json` do CLI: o Hangar só acrescenta a pasta da sessão em `trustedWorkspaces`, para o `agy` não abrir
//! a pergunta "Do you trust the contents of this project?" num pane que ninguém está olhando.
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::hooks::Error;

const TRUSTED: &str = "trustedWorkspaces";

/// `true` quando gravou. Arquivo ilegível não é regravado.
pub fn trust_workspace(path: &Path, cwd: &Path) -> Result<bool, Error> {
    let mut doc = match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = text.trim_start_matches('\u{feff}');
            if text.trim().is_empty() { Map::new() } else {
                match serde_json::from_str::<Value>(text) {
                    Ok(Value::Object(map)) => map,
                    Ok(_) => return Err(Error::Unreadable("não é um objeto".into())),
                    Err(e) => return Err(Error::Unreadable(e.to_string())),
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e.into()),
    };
    let wanted = Value::String(cwd.to_string_lossy().into_owned());
    let list = doc.entry(TRUSTED).or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(items) = list else { return Err(Error::Unreadable(format!("{TRUSTED} não é uma lista"))) };
    if items.contains(&wanted) { return Ok(false); }
    items.push(wanted);
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let mut bytes = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
    bytes.push(b'\n');
    hangar_core::fs::atomic_write(path, &bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_folder_once_and_keeps_the_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"permissions":{"allow":["command(agy)"]},"trustedWorkspaces":["/home/u"]}"#).unwrap();
        assert!(trust_workspace(&path, Path::new("/home/u/proj")).unwrap());
        assert!(!trust_workspace(&path, Path::new("/home/u/proj")).unwrap());
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["permissions"]["allow"][0], "command(agy)");
        assert_eq!(doc[TRUSTED], serde_json::json!(["/home/u", "/home/u/proj"]));
    }

    #[test]
    fn a_broken_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "[1,2").unwrap();
        assert!(trust_workspace(&path, Path::new("/x")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1,2");
    }
}
