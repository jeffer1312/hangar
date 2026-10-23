//! Inscrições do Web Push e preferências do aviso no celular, nos mesmos arquivos do `push.py`
//! (`<config Claude>/.hangar-push/subs.json` e `push_prefs.json`): quem já se inscreveu continua.
use chrono::NaiveTime;
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Uma escrita por vez neste processo; o Python ainda grava os mesmos arquivos no modo `python`.
static LOCK: Mutex<()> = Mutex::new(());

pub fn dir(claude: &Path) -> PathBuf { claude.join(".hangar-push") }

fn read_json(path: &Path, empty: Value) -> Value {
    std::fs::read(path).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or(empty)
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    crate::runtime::queue::atomic_write(path, serde_json::to_string(value)?.as_bytes())
}

pub fn subs(dir: &Path) -> Vec<Value> {
    read_json(&dir.join("subs.json"), json!([])).as_array().cloned().unwrap_or_default()
}

fn endpoint(entry: &Value) -> Option<&str> { entry["subscription"]["endpoint"].as_str() }

/// Upsert pelo endpoint: assinar de novo não duplica.
pub fn add(dir: &Path, subscription: Value, label: &str, server_id: &str, locale: &str) -> Result<(), &'static str> {
    let Some(new) = subscription["endpoint"].as_str().filter(|e| !e.is_empty()).map(str::to_owned) else {
        return Err("subscription sem endpoint");
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<Value> = subs(dir).into_iter().filter(|s| endpoint(s) != Some(new.as_str())).collect();
    all.push(json!({"subscription": subscription, "label": label, "serverId": server_id, "locale": locale}));
    write_json(&dir.join("subs.json"), &Value::Array(all)).map_err(|_| "push_store_unwritable")
}

/// Tira as inscrições mortas (404/410) relendo o arquivo: uma inscrição feita durante o envio fica.
pub fn prune(dir: &Path, dead: &[String]) -> io::Result<()> {
    if dead.is_empty() { return Ok(()); }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let all = subs(dir);
    let alive: Vec<Value> = all.iter().filter(|s| !endpoint(s).is_some_and(|e| dead.iter().any(|d| d == e))).cloned().collect();
    if alive.len() == all.len() { return Ok(()); }
    write_json(&dir.join("subs.json"), &Value::Array(alive))
}

pub fn prefs(dir: &Path) -> Value {
    let raw = read_json(&dir.join("push_prefs.json"), json!({}));
    if raw.is_object() { raw } else { json!({}) }
}

/// `GET /api/push/settings`.
pub fn settings(dir: &Path) -> Value {
    let p = prefs(dir);
    json!({"muted": p.get("muted").cloned().unwrap_or(json!([])), "quiet_hours": p.get("quiet_hours").cloned().unwrap_or(Value::Null)})
}

fn update_prefs(dir: &Path, change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = prefs(dir);
    if let Some(obj) = p.as_object_mut() { change(obj); }
    write_json(&dir.join("push_prefs.json"), &p)
}

pub fn set_muted(dir: &Path, session: &str, muted: bool) -> io::Result<()> {
    update_prefs(dir, |p| {
        let mut set: std::collections::BTreeSet<String> = p.get("muted").and_then(Value::as_array).into_iter().flatten()
            .filter_map(|v| v.as_str().map(str::to_owned)).collect();
        if muted { set.insert(session.to_owned()); } else { set.remove(session); }
        p.insert("muted".into(), json!(set));
    })
}

pub fn is_muted(prefs: &Value, session: &str) -> bool {
    prefs["muted"].as_array().is_some_and(|m| m.iter().any(|v| v.as_str() == Some(session)))
}

/// `datetime.time.fromisoformat` para o que a tela manda: `HH`, `HH:MM` ou `HH:MM:SS`.
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let n = |i: usize| parts.get(i).map_or(Some(0), |p| p.parse::<u32>().ok());
    NaiveTime::from_hms_opt(n(0)?, n(1)?, n(2)?)
}

/// Os dois presentes ligam a janela (horário inválido = `Err`); faltando um, ela sai.
pub fn set_quiet_hours(dir: &Path, start: Option<&str>, end: Option<&str>) -> Result<(), &'static str> {
    let window = match (start.filter(|s| !s.is_empty()), end.filter(|s| !s.is_empty())) {
        (Some(s), Some(e)) => {
            if parse_time(s).is_none() || parse_time(e).is_none() { return Err("horario invalido (use HH:MM)"); }
            Some(json!({"start": s, "end": e}))
        }
        _ => None,
    };
    update_prefs(dir, |p| match window {
        Some(w) => { p.insert("quiet_hours".into(), w); }
        None => { p.remove("quiet_hours"); }
    }).map_err(|_| "push_store_unwritable")
}

/// Janela global de silêncio, que pode cruzar a meia-noite (22:00–07:00).
pub fn in_quiet_hours(prefs: &Value, now: NaiveTime) -> bool {
    let qh = &prefs["quiet_hours"];
    let (Some(start), Some(end)) = (qh["start"].as_str().and_then(parse_time), qh["end"].as_str().and_then(parse_time)) else {
        return false;
    };
    if start <= end { start <= now && now < end } else { now >= start || now < end }
}
