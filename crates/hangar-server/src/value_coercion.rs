//! Coerções escalares dos modelos publicados: não estreitam inteiros para 64 bits.
use serde_json::{Value, json};

pub fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_f64()? {
            0.0 => Some(false),
            1.0 => Some(true),
            _ => None,
        },
        Value::String(value) => match value.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

pub fn integer(value: &Value) -> Option<Value> {
    if let Some(b) = value.as_bool() {
        return Some(json!(u8::from(b)));
    }
    let raw = match value {
        Value::String(s) => {
            let s = s.trim();
            let bytes = s.as_bytes();
            if bytes.iter().enumerate().any(|(i, b)| {
                *b == b'_'
                    && (i == 0
                        || i + 1 == bytes.len()
                        || !bytes[i - 1].is_ascii_digit()
                        || !bytes[i + 1].is_ascii_digit())
            }) {
                return None;
            }
            s.replace('_', "")
        }
        Value::Number(n) => {
            let raw = n.to_string();
            if raw.contains(['.', 'e', 'E']) {
                let f = n.as_f64()?;
                if !f.is_finite() || f.fract() != 0.0 || f <= i64::MIN as f64 || f >= -(i64::MIN as f64) {
                    return None;
                }
                format!("{f:.0}")
            } else {
                raw
            }
        }
        _ => return None,
    };
    let (negative, raw) = if let Some(rest) = raw.strip_prefix('-') {
        (true, rest)
    } else { (false, raw.strip_prefix('+').unwrap_or(&raw)) };
    let raw = if let Some((whole, fraction)) = raw.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|b| b == b'0') {
            return None;
        }
        whole
    } else {
        raw
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = raw.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    if value.is_string() && digits.len() + usize::from(negative) > 4300 {
        return None;
    }
    serde_json::from_str(&format!(
        "{}{digits}",
        if negative && digits != "0" { "-" } else { "" }
    ))
    .ok()
}
