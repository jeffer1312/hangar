//! `settings.json` do CLI: o Hangar acrescenta a pasta da sessão em `trustedWorkspaces`, para o `agy` não abrir
//! a pergunta "Do you trust the contents of this project?" num pane que ninguém está olhando, e as liberações
//! do recado em `permissions.allow`, e ocupa o `statusLine` vazio para receber a cota. O resto do arquivo é da pessoa.
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::hooks::Error;

const TRUSTED: &str = "trustedWorkspaces";
const STATUS_LINE: &str = "statusLine";

/// O que coube ao `statusLine` da conta.
#[derive(Debug, PartialEq, Eq)]
pub enum StatusLine {
    Written,
    Unchanged,
    /// A pessoa tem statusline própria ou desligou a dela: fica como está, e a cota não chega ao Hangar.
    Taken,
}

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

/// Põe `command` no `statusLine` vazio ou já do Hangar (`ours` reconhece o comando antigo, de outra versão).
/// `stack_with_default` mantém a barra padrão do `agy`, de onde o pane lê modo e modelo.
pub fn status_line(path: &Path, command: &str, ours: impl Fn(&str) -> bool) -> Result<StatusLine, Error> {
    let mut doc = read(path)?;
    let mut line = match doc.get(STATUS_LINE) {
        None => Map::new(),
        Some(Value::Object(line)) => line.clone(),
        Some(_) => return Ok(StatusLine::Taken),
    };
    let current = line.get("command").and_then(Value::as_str).unwrap_or("").trim();
    if (!current.is_empty() && !ours(current)) || line.get("enabled") == Some(&Value::Bool(false)) {
        return Ok(StatusLine::Taken);
    }
    let before = line.clone();
    line.insert("type".into(), "command".into());
    line.insert("command".into(), command.into());
    line.insert("stack_with_default".into(), true.into());
    if doc.contains_key(STATUS_LINE) && line == before { return Ok(StatusLine::Unchanged); }
    doc.insert(STATUS_LINE.into(), Value::Object(line));
    write(path, &doc)?;
    Ok(StatusLine::Written)
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
    // `settings.json` ligado a um dotfile: grava no alvo, senão o rename trocaria o link por um arquivo comum.
    let real = std::fs::canonicalize(path);
    let path = match &real { Ok(real) => real.as_path(), Err(_) => path };
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

    #[cfg(unix)]
    #[test]
    fn a_linked_file_stays_a_link_and_the_target_gets_the_rules() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles-settings.json");
        std::fs::write(&target, "{}").unwrap();
        let path = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(allow(&path, &HANGAR_RULES).unwrap());
        assert!(trust_workspace(&path, Path::new("/home/u/proj")).unwrap());
        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(doc["permissions"]["allow"].as_array().unwrap().len(), HANGAR_RULES.len());
        assert_eq!(doc[TRUSTED], serde_json::json!(["/home/u/proj"]));
    }

    const OURS: &str = "\"/opt/hangar/hangar-server\" agy-statusline";
    fn ours(command: &str) -> bool { command.ends_with(" agy-statusline") }

    #[test]
    fn an_empty_status_line_is_taken_and_keeps_the_default_bar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model":"x","statusLine":{"type":"","command":"","enabled":true}}"#).unwrap();
        assert_eq!(status_line(&path, OURS, ours).unwrap(), StatusLine::Written);
        assert_eq!(status_line(&path, OURS, ours).unwrap(), StatusLine::Unchanged, "segunda vez não regrava");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc[STATUS_LINE], serde_json::json!({"type":"command","command":OURS,"enabled":true,"stack_with_default":true}));
        assert_eq!(doc["model"], "x");
    }

    #[test]
    fn a_missing_status_line_is_created_and_an_old_hangar_command_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(status_line(&path, OURS, ours).unwrap(), StatusLine::Written);
        let newer = "\"/novo/hangar-server\" agy-statusline";
        assert_eq!(status_line(&path, newer, ours).unwrap(), StatusLine::Written);
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc[STATUS_LINE]["command"], newer);
    }

    #[test]
    fn the_status_line_of_the_person_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for own in [r#"{"statusLine":{"type":"command","command":"~/minha-barra.sh"}}"#,
                    r#"{"statusLine":{"type":"","command":"","enabled":false}}"#,
                    r#"{"statusLine":true}"#] {
            std::fs::write(&path, own).unwrap();
            assert_eq!(status_line(&path, OURS, ours).unwrap(), StatusLine::Taken, "{own}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), own);
        }
        std::fs::write(&path, "[1,2").unwrap();
        assert!(status_line(&path, OURS, ours).is_err());
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
