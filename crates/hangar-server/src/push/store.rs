//! Inscrições e preferências persistidas no formato do Python, sem reserva silenciosa para erro de E/S.
use crate::{
    config_sync::value_equality::equal,
    orchestration::{codec, truthy},
};
use chrono::NaiveTime;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub(super) async fn delivery_lock() -> tokio::sync::MutexGuard<'static, ()> {
    LOCK.lock().await
}
pub fn dir(claude: &Path) -> PathBuf {
    claude.join(".hangar-push")
}
type Result<T> = std::result::Result<T, &'static str>;
fn read_json(path: &Path, empty: Value) -> Result<Value> {
    std::fs::create_dir_all(path.parent().ok_or("push_store_unreadable")?)
        .map_err(|_| "push_store_unreadable")?;
    match crate::runtime::queue::read_replaced(path) {
        Ok(raw) => {
            let text = std::str::from_utf8(&raw).map_err(|_| "push_store_unreadable")?;
            Ok(codec::loads(text).unwrap_or(empty))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(empty),
        Err(_) => Err("push_store_unreadable"),
    }
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or("push_store_unwritable")?)
        .map_err(|_| "push_store_unwritable")?;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|_| "push_store_unwritable")?;
    let text = codec::dumps_utf8(value).map_err(|_| "push_store_encoding")?;
    file.write_all(text.as_bytes())
        .map_err(|_| "push_store_unwritable")?;
    drop(file);
    crate::runtime::queue::replace(&tmp, path).map_err(|_| "push_store_unwritable")
}
pub fn subs(dir: &Path) -> Result<Vec<Value>> {
    let value = read_json(&dir.join("subs.json"), json!([]))?;
    match value {
        Value::Array(rows) => Ok(rows),
        Value::Object(map) if map.is_empty() => Ok(vec![]),
        Value::String(text) if text.is_empty() => Ok(vec![]),
        other => {
            codec::discard(other);
            Err("push_store_shape")
        }
    }
}
pub fn prefs(dir: &Path) -> Result<Value> {
    read_json(&dir.join("push_prefs.json"), json!({}))
}
fn endpoint(entry: &Value) -> Result<&Value> {
    let obj = entry.as_object().ok_or("push_store_shape")?;
    match obj.get("subscription") {
        None => Ok(&Value::Null),
        Some(value) => Ok(value
            .as_object()
            .ok_or("push_store_shape")?
            .get("endpoint")
            .unwrap_or(&Value::Null)),
    }
}
pub(super) fn validate_subscription(subscription: &Value) -> Result<()> {
    if truthy(subscription.get("endpoint").unwrap_or(&Value::Null)) {
        Ok(())
    } else {
        Err("subscription sem endpoint")
    }
}
#[cfg(test)]
pub fn add(
    dir: &Path,
    subscription: Value,
    label: &str,
    server_id: &str,
    locale: &str,
) -> Result<()> {
    let guard = LOCK.blocking_lock();
    add_locked(&guard, dir, subscription, label, server_id, locale)
}
pub(super) fn add_locked(
    _guard: &tokio::sync::MutexGuard<'static, ()>,
    dir: &Path,
    mut subscription: Value,
    label: &str,
    server_id: &str,
    locale: &str,
) -> Result<()> {
    let result = (|| {
        validate_subscription(&subscription)?;
        let new = subscription.get("endpoint").unwrap_or(&Value::Null);
        let all = subs(dir)?;
        if all.iter().any(|entry| endpoint(entry).is_err()) {
            codec::discard(Value::Array(all));
            return Err("push_store_shape");
        }
        let mut kept = Vec::new();
        for entry in all {
            if equal(endpoint(&entry).unwrap(), new) {
                codec::discard(entry);
            } else {
                kept.push(entry);
            }
        }
        let mut entry = serde_json::Map::new();
        entry.insert("subscription".into(), std::mem::take(&mut subscription));
        entry.insert("label".into(), json!(label));
        entry.insert("serverId".into(), json!(server_id));
        entry.insert("locale".into(), json!(locale));
        kept.push(Value::Object(entry));
        let value = Value::Array(kept);
        let result = write_json(&dir.join("subs.json"), &value);
        codec::discard(value);
        result
    })();
    codec::discard(subscription);
    result
}
#[cfg(test)]
pub fn prune(dir: &Path, dead: &[String]) -> Result<()> {
    if dead.is_empty() {
        return Ok(());
    }
    let _guard = LOCK.blocking_lock();
    prune_locked(&_guard, dir, dead)
}
pub(super) fn prune_locked(
    _guard: &tokio::sync::MutexGuard<'static, ()>,
    dir: &Path,
    dead: &[String],
) -> Result<()> {
    let all = subs(dir)?;
    if all.iter().any(|entry| endpoint(entry).is_err()) {
        codec::discard(Value::Array(all));
        return Err("push_store_shape");
    }
    let before = all.len();
    let mut kept = Vec::new();
    for entry in all {
        if endpoint(&entry)
            .unwrap()
            .as_str()
            .is_some_and(|s| dead.iter().any(|d| d == s))
        {
            codec::discard(entry);
        } else {
            kept.push(entry);
        }
    }
    let value = Value::Array(kept);
    let result = if value.as_array().unwrap().len() == before {
        Ok(())
    } else {
        write_json(&dir.join("subs.json"), &value)
    };
    codec::discard(value);
    result
}
pub fn settings(dir: &Path) -> Result<Value> {
    let data = prefs(dir)?;
    let result = data
        .as_object()
        .map(|obj| {
            let mut out = serde_json::Map::new();
            out.insert(
                "muted".into(),
                codec::clone_value(obj.get("muted").unwrap_or(&json!([]))),
            );
            out.insert(
                "quiet_hours".into(),
                codec::clone_value(obj.get("quiet_hours").unwrap_or(&Value::Null)),
            );
            Value::Object(out)
        })
        .ok_or("push_store_shape");
    codec::discard(data);
    result
}
fn update_prefs(
    dir: &Path,
    change: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<()>,
) -> Result<()> {
    let mut value = prefs(dir)?;
    let result = match value.as_object_mut() {
        Some(map) => validate_muted(map).and_then(|()| change(map)),
        None => Err("push_store_shape"),
    }
    .and_then(|()| write_json(&dir.join("push_prefs.json"), &value));
    codec::discard(value);
    result
}
fn validate_muted(prefs: &serde_json::Map<String, Value>) -> Result<()> {
    match prefs.get("muted") {
        None => Ok(()),
        Some(Value::Array(values)) if values.iter().all(Value::is_string) => Ok(()),
        _ => Err("push_settings_invalid"),
    }
}
#[cfg(test)]
pub fn set_muted(dir: &Path, session: &str, muted: bool) -> Result<()> {
    let guard = LOCK.blocking_lock();
    set_muted_locked(&guard, dir, session, muted)
}
pub(super) fn set_muted_locked(
    _guard: &tokio::sync::MutexGuard<'static, ()>,
    dir: &Path,
    session: &str,
    muted: bool,
) -> Result<()> {
    update_prefs(dir, |prefs| {
        let mut names = std::collections::BTreeSet::new();
        if let Some(Value::Array(values)) = prefs.get("muted") {
            for value in values {
                names.insert(value.as_str().unwrap().to_owned());
            }
        }
        if muted {
            names.insert(session.to_owned());
        } else {
            names.remove(session);
        }
        prefs.insert("muted".into(), json!(names));
        Ok(())
    })
}
pub fn is_muted(prefs: &Value, session: &str) -> Result<bool> {
    let object = prefs.as_object().ok_or("push_gate_invalid")?;
    validate_muted(object).map_err(|_| "push_gate_invalid")?;
    Ok(object
        .get("muted")
        .and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(session))))
}
fn time(text: &str) -> Option<(NaiveTime, Option<i64>)> {
    let text = text.strip_prefix('T').unwrap_or(text);
    let text = if let Some((prefix, tail)) = text.split_once('\0') {
        let fraction = prefix.rsplit_once(['.', ',']).map(|(_, digits)| digits);
        if fraction.is_some_and(|digits| digits.len() < 6)
            || (!tail.is_empty()
                && !fraction.is_some_and(|digits| {
                    digits.len() >= 6 && digits.bytes().all(|b| b.is_ascii_digit())
                }))
        {
            return None;
        }
        prefix
    } else {
        text
    };
    for (index, ch) in text.char_indices() {
        if matches!(ch, '.' | ',')
            && !text
                .as_bytes()
                .get(index + 1)
                .is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'Z' | b'+' | b'-'))
        {
            return None;
        }
    }
    let (micros, offset) = crate::transcript::py::fromisoformat(&format!("2000-01-01T{text}"))?;
    let day = micros.rem_euclid(86_400_000_000);
    Some((
        NaiveTime::from_num_seconds_from_midnight_opt(
            (day / 1_000_000) as u32,
            ((day % 1_000_000) * 1000) as u32,
        )?,
        offset,
    ))
}
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    time(text).map(|(value, _)| value)
}
#[cfg(test)]
pub fn set_quiet_hours(dir: &Path, start: Option<&str>, end: Option<&str>) -> Result<()> {
    let guard = LOCK.blocking_lock();
    set_quiet_hours_locked(&guard, dir, start, end)
}
pub(super) fn set_quiet_hours_locked(
    _guard: &tokio::sync::MutexGuard<'static, ()>,
    dir: &Path,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<()> {
    update_prefs(dir, |prefs| {
        match (
            start.filter(|s| !s.is_empty()),
            end.filter(|s| !s.is_empty()),
        ) {
            (Some(s), Some(e)) => {
                if time(s).is_none() || time(e).is_none() {
                    return Err("horario invalido (use HH:MM)");
                }
                if let Some(old) = prefs.insert("quiet_hours".into(), json!({"start":s,"end":e})) {
                    codec::discard(old);
                }
            }
            _ => {
                if let Some(old) = prefs.remove("quiet_hours") {
                    codec::discard(old);
                }
            }
        }
        Ok(())
    })
}
pub fn in_quiet_hours(prefs: &Value, now: NaiveTime) -> Result<bool> {
    let object = prefs.as_object().ok_or("push_gate_invalid")?;
    let qh = object.get("quiet_hours").unwrap_or(&Value::Null);
    if !truthy(qh) {
        return Ok(false);
    }
    let parse = |key| {
        qh.get(key)
            .and_then(Value::as_str)
            .and_then(time)
            .ok_or("push_gate_invalid")
    };
    let (start, so) = parse("start")?;
    let (end, eo) = parse("end")?;
    if so.is_some() || eo.is_some() {
        return Err("push_gate_invalid");
    }
    Ok(if start <= end {
        start <= now && now < end
    } else {
        now >= start || now < end
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    proptest::proptest! {
        #[test]
        fn guest_push_external_clock_never_panics(value in ".{0,120}") { let _=time(&value); }
    }
}
