//! O tomllib conserva inteiros maiores que i128; o span do parser mantém a sintaxe validada.
use super::files::Error;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

fn integer(raw: &str) -> Option<String> {
    let (negative, digits) = if let Some(value) = raw.strip_prefix('-') {
        (true, value)
    } else {
        (false, raw.strip_prefix('+').unwrap_or(raw))
    };
    let (base, digits) = if let Some(value) = digits.strip_prefix("0x") {
        (16, value)
    } else if let Some(value) = digits.strip_prefix("0o") {
        (8, value)
    } else if let Some(value) = digits.strip_prefix("0b") {
        (2, value)
    } else {
        (10, digits)
    };
    if base != 10 && (raw.starts_with('-') || raw.starts_with('+')) {
        return None;
    }
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let digits = digits.replace('_', "");
    if base == 10 && digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    let mut decimal = vec![0u8];
    for character in digits.chars() {
        if !character.is_ascii() {
            return None;
        }
        let mut carry = character.to_digit(base)?;
        for digit in &mut decimal {
            let value = u32::from(*digit) * base + carry;
            *digit = (value % 10) as u8;
            carry = value / 10;
        }
        while carry > 0 {
            decimal.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    let mut result: String = decimal
        .iter()
        .rev()
        .map(|digit| char::from(b'0' + digit))
        .collect();
    if negative && result != "0" {
        result.insert(0, '-');
    }
    Some(result)
}

pub(super) fn read(path: &Path) -> Result<Value, Error> {
    let raw = std::fs::read_to_string(path)?;
    parse(&raw)
}

pub(super) fn parse(raw: &str) -> Result<Value, Error> {
    for salt in 0..32 {
        if let Some(value) = parse_attempt(raw, salt)? {
            return Ok(value);
        }
    }
    Err(Error::Invalid("Configuração TOML contém muitas colisões de marcadores de inteiro".into()))
}

fn parse_attempt(raw: &str, salt: u64) -> Result<Option<Value>, Error> {
    let mut text = raw.to_owned();
    let mut wide = BTreeMap::new();
    let mut sequence = 0u64;
    let mut value = loop {
        match toml::from_str::<Value>(&text) {
            Ok(value) => break value,
            Err(error) => {
                if error.message() != "integer number overflowed" {
                    return Err(Error::Invalid(error.to_string()));
                }
                let span = error
                    .span()
                    .ok_or_else(|| Error::Invalid(error.to_string()))?;
                let token = text
                    .get(span.clone())
                    .and_then(integer)
                    .ok_or_else(|| Error::Invalid(error.to_string()))?;
                let number: Value = serde_json::from_str(&token)
                    .map_err(|error| Error::Invalid(error.to_string()))?;
                let marker = loop {
                    sequence += 1;
                    let marker = format!("hangar_toml_integer_{salt}_{sequence}");
                    if !raw.contains(&marker) {
                        break marker;
                    }
                };
                text.replace_range(span, &format!("\"{marker}\""));
                wide.insert(marker, number);
            }
        }
    };
    // A escrita Unicode escapada também pode produzir o marcador: conte a árvore já decodificada.
    let mut counts = BTreeMap::<String, usize>::new();
    let mut pending = vec![&value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) if wide.contains_key(text) => {
                *counts.entry(text.clone()).or_default() += 1;
            }
            Value::Array(values) => pending.extend(values.iter()),
            Value::Object(values) => pending.extend(values.values()),
            _ => {}
        }
    }
    if counts.values().any(|count| *count != 1) {
        return Ok(None);
    }
    let mut pending = vec![&mut value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) => {
                if let Some(number) = wide.get(text) {
                    *value = number.clone();
                }
            }
            Value::Array(values) => pending.extend(values.iter_mut()),
            Value::Object(values) => pending.extend(values.values_mut()),
            _ => {}
        }
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn arbitrary_instruction_toml_does_not_panic(text in ".{0,2048}") {
            if let Ok(value) = parse(&text) {crate::orchestration::codec::discard(value);}
        }
    }
}

pub(super) struct Typed {
    pub value: Value,
    pub dates: BTreeMap<Vec<String>, String>,
}
impl Drop for Typed {
    fn drop(&mut self) {
        crate::orchestration::codec::discard(std::mem::take(&mut self.value));
    }
}
pub(super) fn read_typed(path: &Path) -> Result<Typed, Error> {
    use toml::de::DeValue;
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound && path.is_symlink() {
            Error::Invalid(format!("Link sem destino: {}", path.display()))
        } else {
            Error::Io(error)
        }
    })?;
    let table = toml::de::DeTable::parse(&text)
        .map_err(|e| Error::Invalid(e.to_string()))?
        .into_inner();
    enum Task<'a> {
        Value(DeValue<'a>, Vec<String>),
        Array(usize),
        Object(Vec<String>),
    }
    let mut tasks = vec![Task::Value(DeValue::Table(table), vec![])];
    let mut values = Vec::new();
    let mut dates = BTreeMap::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::Value(value, path) => match value {
                DeValue::String(value) => values.push(Value::String(value.into_owned())),
                DeValue::Boolean(value) => values.push(Value::Bool(value)),
                DeValue::Integer(value) => {
                    let number = integer(&value.to_string())
                        .ok_or_else(|| Error::Invalid("Inteiro TOML inválido".into()))?;
                    values.push(
                        serde_json::from_str(&number).map_err(|e| Error::Invalid(e.to_string()))?,
                    );
                }
                DeValue::Float(value) => {
                    let number = value
                        .as_str()
                        .parse::<f64>()
                        .map_err(|e| Error::Invalid(e.to_string()))?;
                    values.push(if number.is_finite() {
                        serde_json::json!(number)
                    } else {
                        crate::orchestration::codec::loads(if number.is_nan() {
                            "NaN"
                        } else if number.is_sign_negative() {
                            "-Infinity"
                        } else {
                            "Infinity"
                        })
                        .unwrap()
                    });
                }
                DeValue::Datetime(value) => {
                    dates.insert(
                        path,
                        if value.date.is_some() && value.time.is_some() {
                            "datetime"
                        } else if value.date.is_some() {
                            "date"
                        } else {
                            "time"
                        }
                        .into(),
                    );
                    values.push(
                        serde_json::to_value(value).map_err(|e| Error::Invalid(e.to_string()))?,
                    );
                }
                DeValue::Array(items) => {
                    tasks.push(Task::Array(items.len()));
                    for (index, item) in items.into_iter().enumerate().rev() {
                        let mut child = path.clone();
                        child.push(index.to_string());
                        tasks.push(Task::Value(item.into_inner(), child));
                    }
                }
                DeValue::Table(items) => {
                    let keys: Vec<_> = items.keys().map(|key| key.get_ref().to_string()).collect();
                    tasks.push(Task::Object(keys));
                    for (key, item) in items.into_iter().rev() {
                        let mut child = path.clone();
                        child.push(key.into_inner().into_owned());
                        tasks.push(Task::Value(item.into_inner(), child));
                    }
                }
            },
            Task::Array(length) => {
                let start = values.len() - length;
                let children = values.split_off(start);
                values.push(Value::Array(children));
            }
            Task::Object(keys) => {
                let children = values.split_off(values.len() - keys.len());
                values.push(Value::Object(keys.into_iter().zip(children).collect()));
            }
        }
    }
    Ok(Typed {
        value: values.pop().unwrap(),
        dates,
    })
}
#[cfg(test)]
#[test]
fn integer_marker_collisions_have_a_finite_budget() {
    let mut raw = String::from("wide = 9999999999999999999999999999999999999999999999999999999\n");
    for salt in 0..32 { raw.push_str(&format!("collision{salt} = \"\\u0068angar_toml_integer_{salt}_1\"\n")); }
    assert!(matches!(parse(&raw), Err(Error::Invalid(message)) if message.contains("colisões")));
    assert!(parse("wide = 9999999999999999999999999999999999999999999999999999999").is_ok());
}
