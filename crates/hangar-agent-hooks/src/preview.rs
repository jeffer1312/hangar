//! `preview-hook`: a prévia do texto do assistente em voo, `<config>/.hangar-preview/<stem>.json`.
//!
//! O `MessageDisplay` chega com `delta` INCREMENTAL, um processo por evento. Os deltas ficam no próprio
//! sidecar POR `index` e o texto é remontado em ordem a cada evento: a trava impede perder delta, mas
//! não impede o index 2 gravar antes do 1. `message_id` diferente recomeça. O `Stop` grava `""` ("nada
//! em voo") com o id da mensagem que fechou, e um delta atrasado dessa mensagem não a ressuscita.
//! Falha sai 0 com uma linha no stderr, sem o texto da mensagem.
use crate::input::{self, basename};
use crate::{Context, Outcome};
use hangar_core::pycompat::{pyjson, truthy};
use serde_json::{Map, Value, json};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

const FOLDER: &str = ".hangar-preview";

/// A linha de falha, com o nome da exceção que o hook do Python mostrava.
pub fn failure(kind: &str) -> Outcome {
    Outcome { code: 0, stdout: Vec::new(), stderr: format!("hangar: preview_hook falhou ({kind})\n").into_bytes() }
}

pub fn handle(stdin: &[u8], context: &Context) -> Outcome {
    match publish(stdin, context) {
        Ok(()) => Outcome::default(),
        Err(kind) => failure(kind),
    }
}

fn io_kind(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::InvalidInput => "ValueError",
        _ => "OSError",
    }
}

fn publish(stdin: &[u8], context: &Context) -> Result<(), &'static str> {
    let event = input::parse(&input::decode(stdin)).ok_or("JSONDecodeError")?;
    let event = event.as_object().ok_or("AttributeError")?;
    let name = event.get("hook_event_name").and_then(Value::as_str).unwrap_or_default();
    // Chave = stem do transcript VIVO (a do servidor), não o session_id do cmdline: numa sessão
    // retomada os dois divergem. Sempre sem pasta: um session_id com `/` escaparia da pasta.
    let transcript = event.get("transcript_path").filter(|path| truthy(path));
    let stem: Option<String> = match transcript {
        Some(Value::String(path)) if path.ends_with(".jsonl") => {
            let name = basename(path);
            Some(name[..name.len() - ".jsonl".len()].to_owned())
        }
        Some(Value::String(_)) | None => event.get("session_id").and_then(Value::as_str).map(|sid| basename(sid).to_owned()),
        Some(_) => return Err("AttributeError"),
    };
    let Some(stem) = stem.filter(|stem| !stem.is_empty()) else { return Ok(()) };
    let base = context.config_base().ok_or("RuntimeError")?;
    let dir = base.join(FOLDER);
    if name == "Stop" {
        let _lock = lock(&dir, &stem)?;
        let previous = read_previous(&dir, &stem);
        let previous = previous.as_ref().and_then(Value::as_object);
        // Um segundo `Stop` sobre a prévia já fechada guarda o mesmo `closed`: sem ele, um delta
        // atrasado da mensagem fechada voltaria a aparecer.
        let closed = match previous {
            Some(p) if p.get("text") == Some(&json!("")) && p.contains_key("closed") => p["closed"].clone(),
            Some(p) => p.get("message_id").cloned().unwrap_or(Value::Null),
            None => Value::Null,
        };
        let mut payload = Map::new();
        payload.insert("text".into(), json!(""));
        payload.insert("ts".into(), json!(input::now()));
        payload.insert("closed".into(), closed);
        return write(&dir, &stem, &Value::Object(payload));
    }
    // `agent_id` = texto de SUBAGENTE, nunca a prévia da conversa principal.
    if name != "MessageDisplay" || truthy(event.get("agent_id").unwrap_or(&Value::Null)) {
        return Ok(());
    }
    let _lock = lock(&dir, &stem)?;
    let Some(delta) = event.get("delta").and_then(Value::as_str).filter(|delta| !delta.is_empty()) else { return Ok(()) };
    let message = event.get("message_id").cloned().unwrap_or(Value::Null);
    let index = event.get("index").unwrap_or(&Value::Null);
    let indexed = truthy(index);
    let previous = read_previous(&dir, &stem);
    let previous = previous.as_ref().and_then(Value::as_object);
    let mut parts = Map::new();
    if !message.is_null() && previous.is_some_and(|p| p.get("message_id") == Some(&message)) {
        if let Some(Value::Object(kept)) = previous.and_then(|p| p.get("parts")) {
            parts = kept.clone();
        }
    } else if let Some(previous) = previous
        && previous.get("text") == Some(&json!(""))
        && previous.get("closed").unwrap_or(&Value::Null) == &message
        // O "" é a marca do Stop: um delta atrasado da MESMA mensagem republicaria um rabo de texto.
        // Sem id não há como saber se o índice 0 é de mensagem nova; com id, nem o 0 passa.
        && (indexed || !message.is_null())
    {
        return Ok(());
    }
    parts.insert(part_key(index), json!(delta));
    let mut order = Vec::with_capacity(parts.len());
    for (key, value) in &parts {
        order.push((python_int(key).ok_or("ValueError")?, value.as_str().ok_or("TypeError")?));
    }
    order.sort_by(|a, b| a.0.cmp(&b.0));
    let text: String = order.into_iter().map(|(_, piece)| piece).collect();
    let mut payload = Map::new();
    payload.insert("text".into(), json!(text));
    payload.insert("ts".into(), json!(input::now()));
    payload.insert("message_id".into(), message);
    payload.insert("parts".into(), Value::Object(parts));
    write(&dir, &stem, &Value::Object(payload))
}

/// `str(index or 0)`: o índice ausente, nulo ou falso é o 0.
fn part_key(index: &Value) -> String {
    match index {
        _ if !truthy(index) => "0".to_owned(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => hangar_core::pycompat::py::py_str(other),
    }
}

/// `int(chave)` do Python para as chaves de `parts`: espaço em volta, sinal e `_` entre dígitos.
/// Fração (`"1.5"`) não é inteiro e derruba a montagem.
fn python_int(text: &str) -> Option<i128> {
    let text = hangar_core::pycompat::py::strip(text);
    let (negative, digits) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut value: i128 = 0;
    for c in digits.chars().filter(|c| *c != '_') {
        value = value.checked_mul(10)?.checked_add(i128::from(c.to_digit(10)? as u8))?;
    }
    Some(if negative { -value } else { value })
}

fn read_previous(dir: &Path, stem: &str) -> Option<Value> {
    let bytes = hangar_core::fs::read_replaced(&dir.join(format!("{stem}.json"))).ok()?;
    input::parse(std::str::from_utf8(&bytes).ok()?)
}

fn write(dir: &Path, stem: &str, payload: &Value) -> Result<(), &'static str> {
    if input::value_has_surrogate(payload) {
        return Err("UnicodeEncodeError");
    }
    crate::marker::write(dir, stem, pyjson::dumps_unicode(payload, false).as_bytes()).map_err(|error| io_kind(&error))
}

/// Trava em arquivo separado (`<stem>.lock`): o `rename` troca o inode do sidecar. Cobre ler, encaixar e
/// gravar; solta ao sair do processo.
fn lock(dir: &Path, stem: &str) -> Result<File, &'static str> {
    std::fs::create_dir_all(dir).map_err(|error| io_kind(&error))?;
    let path: PathBuf = dir.join(format!("{stem}.lock"));
    let file = OpenOptions::new().read(true).append(true).create(true).open(&path).map_err(|error| io_kind(&error))?;
    wait_lock(&file)?;
    Ok(file)
}

#[cfg(not(windows))]
fn wait_lock(file: &File) -> Result<(), &'static str> {
    file.lock().map_err(|error| io_kind(&error))
}

/// No Windows a espera tem prazo: 10 ms entre tentativas, 5 s ao todo, e o delta que não entra falha.
#[cfg(windows)]
fn wait_lock(file: &File) -> Result<(), &'static str> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err("PermissionError"),
            Err(std::fs::TryLockError::Error(error)) => return Err(io_kind(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_keys_follow_python_str_and_int() {
        assert_eq!(part_key(&json!(null)), "0");
        assert_eq!(part_key(&json!(0)), "0");
        assert_eq!(part_key(&json!(2)), "2");
        assert_eq!(part_key(&json!("1")), "1");
        assert_eq!(python_int("1"), Some(1));
        assert_eq!(python_int(" -2 "), Some(-2));
        assert_eq!(python_int("1_0"), Some(10));
        assert_eq!(python_int("1.5"), None);
        assert_eq!(python_int("True"), None);
        assert_eq!(python_int(""), None);
    }
}
