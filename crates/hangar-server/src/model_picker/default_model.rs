//! Protege o modelo global que o CLI grava ao trocar o modelo de um motor.
use serde_json::{Map, Value};
use std::{path::Path, time::Duration};

#[derive(Clone, Debug)]
pub enum Snapshot {
    Unreadable,
    Missing,
    Model(Value),
}

fn object(config: &Path) -> Result<Option<Map<String, Value>>, String> {
    let raw = match crate::runtime::queue::read_replaced(&config.join("settings.json")) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(Map::new())),
        Err(error) => {
            tracing::warn!(%error, "Padrão global do modelo ilegível; restauração não aplicada");
            return Ok(None);
        }
    };
    let text = crate::terminal_state::trim(
        std::str::from_utf8(&raw).map_err(|_| "settings.json não é UTF-8")?,
    );
    if text.is_empty() {
        return Ok(Some(Map::new()));
    }
    Ok(match serde_json::from_str(text) {
        Ok(Value::Object(data)) => Some(data),
        _ => {
            tracing::warn!("Padrão global do modelo inválido; restauração não aplicada");
            None
        }
    })
}

pub fn snapshot(config: &Path) -> Result<Snapshot, String> {
    let Some(data) = object(config)? else {
        return Ok(Snapshot::Unreadable);
    };
    Ok(match data.get("model") {
        None => Snapshot::Missing,
        Some(Value::Null) => Snapshot::Unreadable,
        Some(value) => Snapshot::Model(value.clone()),
    })
}

pub fn restore(config: &Path, before: &Snapshot) -> Result<bool, String> {
    if *before == Snapshot::Unreadable {
        return Ok(false);
    }
    let Some(mut data) = object(config)? else {
        return Ok(false);
    };
    let current = data
        .get("model")
        .cloned()
        .map(Snapshot::Model)
        .unwrap_or(Snapshot::Missing);
    if &current == before {
        return Ok(false);
    }
    match before {
        Snapshot::Missing => {
            data.remove("model");
        }
        Snapshot::Model(value) => {
            data.insert("model".into(), value.clone());
        }
        Snapshot::Unreadable => return Ok(false),
    }
    std::fs::create_dir_all(config).map_err(|error| error.to_string())?;
    let text = format!(
        "{}\n",
        crate::transcript::pyjson::dumps_indent(&Value::Object(data), 2, false)
    );
    crate::runtime::queue::atomic_write(&config.join("settings.json"), text.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(true)
}

pub async fn restore_after_write(config: &Path, before: &Snapshot) -> Result<bool, String> {
    if *before == Snapshot::Unreadable {
        return Ok(false);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let config = config.to_owned();
        let current = tokio::task::spawn_blocking(move || snapshot(&config))
            .await
            .map_err(|error| error.to_string())??;
        if current != *before || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let first = restore_async(config, before).await?;
    // O CLI pode gravar de novo depois da primeira restauração.
    tokio::time::sleep(Duration::from_millis(600)).await;
    Ok(restore_async(config, before).await? || first)
}

async fn restore_async(config: &Path, before: &Snapshot) -> Result<bool, String> {
    let (config, before) = (config.to_owned(), before.clone());
    tokio::task::spawn_blocking(move || restore(&config, &before))
        .await
        .map_err(|error| error.to_string())?
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unreadable, Self::Unreadable) | (Self::Missing, Self::Missing) => true,
            (Self::Model(left), Self::Model(right)) => equivalent(left, right),
            _ => false,
        }
    }
}

// A igualdade de valores editados à mão segue o JSON do Python, sem arredondar inteiros grandes.
fn number(value: &Value) -> Option<(num_bigint::BigInt, i32)> {
    match value {
        Value::Bool(value) => Some((num_bigint::BigInt::from(u8::from(*value)), 0)),
        Value::Number(number) => {
            let text = number.to_string();
            if !text.contains(['.', 'e', 'E']) {
                return Some((text.parse().ok()?, 0));
            }
            let value = number.as_f64()?;
            if !value.is_finite() {
                return None;
            }
            let bits = value.to_bits();
            let exponent = ((bits >> 52) & 0x7ff) as i32;
            let mantissa = (bits & ((1u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1u64 << 52 };
            let mut integer = num_bigint::BigInt::from(mantissa);
            if bits >> 63 != 0 {
                integer = -integer;
            }
            Some((
                integer,
                if exponent == 0 {
                    -1074
                } else {
                    exponent - 1075
                },
            ))
        }
        _ => None,
    }
}

fn equivalent(left: &Value, right: &Value) -> bool {
    if let (Some((left, a)), Some((right, b))) = (number(left), number(right)) {
        return if a >= b {
            (left << ((a - b) as usize)) == right
        } else {
            left == (right << ((b - a) as usize))
        };
    }
    match (left, right) {
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| equivalent(a, b))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| equivalent(value, other))
                })
        }
        _ => left == right,
    }
}
