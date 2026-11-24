//! `settings.json` do CLI: o Hangar acrescenta a pasta da sessão em `trustedWorkspaces`, para o `agy` não abrir
//! a pergunta "Do you trust the contents of this project?" num pane que ninguém está olhando, e as liberações
//! do recado em `permissions.allow`. O resto do arquivo é da pessoa.
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::hooks::Error;

const TRUSTED: &str = "trustedWorkspaces";

/// O que o Hangar libera: as tools de recado do MCP `hangar` e o CLI de recado e do navegador, que só rodam
/// fora do sandbox do terminal (lá dentro o `~/.hangar` some e IP direto é recusado). Criar sessão
/// (`hangar-send --new`) fica de fora, como no Claude: uma sessão que pede aprovação escaparia por ela.
pub const HANGAR_RULES: [&str; 9] = [
    "mcp(hangar/send)",
    "mcp(hangar/group)",
    "mcp(hangar/sessions)",
    "mcp(hangar/who_am_i)",
    "command(hangar-send --list)",
    "command(hangar-send --whoami)",
    "command(hangar-send --group)",
    "command(regex:hangar-send [^-].*)",
    "command(hangar-preview)",
];

/// `true` quando gravou. Arquivo ilegível não é regravado.
pub fn trust_workspace(path: &Path, cwd: &Path) -> Result<bool, Error> {
    let mut doc = read(path)?;
    let wanted = Value::String(cwd.to_string_lossy().into_owned());
    let list = doc.entry(TRUSTED).or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(items) = list else { return Err(Error::Unreadable(format!("{TRUSTED} não é uma lista"))) };
    if items.contains(&wanted) { return Ok(false); }
    items.push(wanted);
    write(path, &doc)?;
    Ok(true)
}

/// Acrescenta em `permissions.allow` as regras que faltam; as da pessoa ficam. `true` quando gravou.
pub fn allow(path: &Path, rules: &[&str]) -> Result<bool, Error> {
    let mut doc = read(path)?;
    let Value::Object(permissions) = doc.entry("permissions").or_insert_with(|| Value::Object(Map::new())) else {
        return Err(Error::Unreadable("permissions não é um objeto".into()));
    };
    let Value::Array(allowed) = permissions.entry("allow").or_insert_with(|| Value::Array(Vec::new())) else {
        return Err(Error::Unreadable("permissions.allow não é uma lista".into()));
    };
    let missing: Vec<Value> = rules.iter().map(|r| Value::String((*r).to_owned())).filter(|r| !allowed.contains(r)).collect();
    if missing.is_empty() { return Ok(false); }
    allowed.extend(missing);
    write(path, &doc)?;
    Ok(true)
}

fn read(path: &Path) -> Result<Map<String, Value>, Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e.into()),
    };
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() { return Ok(Map::new()); }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(Error::Unreadable("não é um objeto".into())),
        Err(e) => Err(Error::Unreadable(e.to_string())),
    }
}

fn write(path: &Path, doc: &Map<String, Value>) -> io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let mut bytes = serde_json::to_vec_pretty(doc).map_err(io::Error::other)?;
    bytes.push(b'\n');
    hangar_core::fs::atomic_write(path, &bytes)
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
        assert!(allow(&path, &HANGAR_RULES).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1,2");
    }

    #[test]
    fn the_rules_are_added_once_after_the_ones_of_the_person() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"permissions":{"allow":["command(agy)","mcp(hangar/send)"],"deny":["command(sudo *)"]}}"#).unwrap();
        assert!(allow(&path, &HANGAR_RULES).unwrap());
        assert!(!allow(&path, &HANGAR_RULES).unwrap(), "segunda vez não regrava");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let allowed: Vec<&str> = doc["permissions"]["allow"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(allowed[..2], ["command(agy)", "mcp(hangar/send)"]);
        assert_eq!(allowed.len(), 1 + HANGAR_RULES.len());
        assert_eq!(doc["permissions"]["deny"][0], "command(sudo *)");
        assert!(!allowed.contains(&"command(hangar-send)"), "criar sessão fica de fora");
    }

    #[test]
    fn a_permissions_field_of_another_shape_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"permissions":{"allow":"tudo"}}"#).unwrap();
        assert!(matches!(allow(&path, &HANGAR_RULES), Err(Error::Unreadable(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"permissions":{"allow":"tudo"}}"#);
    }
}
