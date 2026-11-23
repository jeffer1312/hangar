//! A entrada do Hangar no `hooks.json` da conta (`<gemini>/config/hooks.json`). O arquivo é da pessoa: só a
//! entrada `"hangar"` é nossa, e as outras ficam como estão.
//!
//! Nunca `PreToolUse` nem `PostToolUse`: um `PreToolUse` que devolve `{}` bloqueia a ferramenta (provado no agy 1.5.15).
use std::io;
use std::path::Path;

use serde_json::{json, Map, Value};

pub const ENTRY: &str = "hangar";
pub const EVENTS: [&str; 4] = ["SessionStart", "PreInvocation", "PostInvocation", "Stop"];
const TIMEOUT_S: u64 = 10;

#[derive(Debug)]
pub enum Error {
    /// O arquivo existe e não é um objeto JSON: gravar por cima apagaria os hooks da pessoa.
    Unreadable(String),
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "hooks.json ilegível: {why}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self { Self::Io(e) }
}

pub fn entry(command: &str) -> Value {
    let handler = json!([{ "type": "command", "command": command, "timeout": TIMEOUT_S }]);
    Value::Object(EVENTS.iter().map(|e| ((*e).to_owned(), handler.clone())).collect())
}

/// `true` quando gravou. Arquivo ausente nasce só com a entrada do Hangar.
pub fn install(path: &Path, command: &str) -> Result<bool, Error> {
    let mut doc = read(path)?;
    let wanted = entry(command);
    if doc.get(ENTRY) == Some(&wanted) { return Ok(false); }
    doc.insert(ENTRY.to_owned(), wanted);
    write(path, &doc)?;
    Ok(true)
}

/// Tira só a entrada do Hangar; `true` quando havia.
pub fn uninstall(path: &Path) -> Result<bool, Error> {
    let mut doc = read(path)?;
    if doc.remove(ENTRY).is_none() { return Ok(false); }
    write(path, &doc)?;
    Ok(true)
}

fn read(path: &Path) -> Result<Map<String, Value>, Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e.into()),
    };
    // Arquivo salvo com BOM (Windows) também vale, como no próprio agy.
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
    fn installs_only_the_four_state_events_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/hooks.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"lint":{"PostToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"./lint.sh"}]}]}}"#).unwrap();
        assert!(install(&path, "/opt/hangar/hangar-server agy-state-hook").unwrap());
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["lint"]["PostToolUse"][0]["matcher"], "run_command");
        let ours = doc[ENTRY].as_object().unwrap();
        assert_eq!(ours.keys().map(String::as_str).collect::<Vec<_>>(), EVENTS);
        assert!(!ours.contains_key("PreToolUse") && !ours.contains_key("PostToolUse"));
        assert_eq!(ours["Stop"][0]["command"], "/opt/hangar/hangar-server agy-state-hook");
        assert!(!install(&path, "/opt/hangar/hangar-server agy-state-hook").unwrap(), "segunda vez não regrava");
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "{ quebrado").unwrap();
        assert!(matches!(install(&path, "x"), Err(Error::Unreadable(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ quebrado");
    }

    #[test]
    fn a_missing_file_is_created_and_uninstall_leaves_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/hooks.json");
        assert!(install(&path, "cmd").unwrap());
        assert!(uninstall(&path).unwrap());
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc, json!({}));
        assert!(!uninstall(&path).unwrap());
    }
}
