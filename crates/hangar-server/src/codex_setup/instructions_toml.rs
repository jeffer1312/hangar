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
    let mut salt = 0;
    loop {
        if let Some(value) = parse_attempt(raw, salt)? {
            return Ok(value);
        }
        salt += 1;
    }
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
