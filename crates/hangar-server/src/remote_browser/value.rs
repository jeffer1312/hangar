//! Valores JSON conservam as conversões e os escapes aceitos pelo navegador Python.
use super::ws::Failure;
use num_bigint::BigInt;
use serde_json::{Value, json};
use std::sync::LazyLock;

static MARKER: LazyLock<String> = LazyLock::new(|| {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).expect("aleatoriedade do sistema");
    format!("browser-value:{}:", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
});

pub fn decode(raw: &str) -> Option<Value> {
    crate::orq_conductor::load(raw, MARKER.as_str())
}

pub fn nonfinite(value: &Value) -> Option<f64> {
    let text = value.as_str()?.strip_prefix(MARKER.as_str())?;
    match text {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

pub(super) fn float_value(number: f64) -> Value {
    if number.is_finite() {
        json!(number)
    } else {
        let kind = if number.is_nan() { "NaN" } else if number.is_sign_negative() { "-Infinity" } else { "Infinity" };
        Value::String(format!("{}{kind}", MARKER.as_str()))
    }
}

pub(super) fn python_string(value: &Value) -> String {
    crate::orq_conductor::py_str(value, MARKER.as_str())
}

pub(super) fn quoted(text: &str) -> String {
    let repr = python_string(&json!([text]));
    repr[1..repr.len() - 1].to_owned()
}

fn unit(text: &str, at: usize) -> (usize, u32) {
    let prefix = format!("{}surrogate:", MARKER.as_str());
    if let Some(tail) = text[at..].strip_prefix(&prefix)
        && tail.as_bytes().get(4) == Some(&b';')
        && let Some(code) = tail.get(..4).and_then(|raw| u32::from_str_radix(raw, 16).ok())
        && (0xd800..=0xdfff).contains(&code)
    {
        return (at + prefix.len() + 5, code);
    }
    let character = text[at..].chars().next().unwrap();
    (at + character.len_utf8(), character as u32)
}

pub fn codepoints(text: &str) -> Vec<u32> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let (end, code) = unit(text, at);
        result.push(code);
        at = end;
    }
    result
}

pub(super) fn truncate(text: &str, count: usize) -> String {
    let mut at = 0;
    for _ in 0..count {
        if at == text.len() { break; }
        at = unit(text, at).0;
    }
    text[..at].to_owned()
}

fn write_string(output: &mut String, text: &str) {
    use std::fmt::Write;
    if !text.contains(MARKER.as_str()) {
        output.push_str(&serde_json::to_string(text).unwrap());
        return;
    }
    output.push('"');
    let mut at = 0;
    while at < text.len() {
        let (end, code) = unit(text, at);
        if (0xd800..=0xdfff).contains(&code) {
            write!(output, "\\u{code:04x}").unwrap();
        } else {
            let escaped = serde_json::to_string(&char::from_u32(code).unwrap().to_string()).unwrap();
            output.push_str(&escaped[1..escaped.len() - 1]);
        }
        at = end;
    }
    output.push('"');
}

fn write(output: &mut String, value: &Value) {
    if let Some(number) = nonfinite(value) {
        output.push_str(if number.is_nan() { "NaN" } else if number.is_sign_negative() { "-Infinity" } else { "Infinity" });
        return;
    }
    match value {
        Value::String(text) => write_string(output, text),
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 { output.push(','); }
                write(output, item);
            }
            output.push(']');
        }
        Value::Object(items) => {
            output.push('{');
            for (index, (key, item)) in items.iter().enumerate() {
                if index != 0 { output.push(','); }
                write_string(output, key);
                output.push(':');
                write(output, item);
            }
            output.push('}');
        }
        other => output.push_str(&other.to_string()),
    }
}

pub fn encode(value: &Value) -> String {
    let mut output = String::new();
    write(&mut output, value);
    output
}

fn float_integer(number: f64) -> Result<Value, Failure> {
    if number.is_nan() {
        return Err(Failure::value("cannot convert float NaN to integer".into()));
    }
    if !number.is_finite() {
        return Err(Failure::exception("OverflowError", "cannot convert float infinity to integer".into()));
    }
    let bits = number.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, shift) = if exponent == 0 { (fraction, -1074) } else { (fraction | (1u64 << 52), exponent - 1075) };
    let mut integer = BigInt::from(mantissa);
    if shift >= 0 { integer <<= shift as usize; } else { integer >>= (-shift) as usize; }
    if number.is_sign_negative() { integer = -integer; }
    Ok(Value::Number(integer.to_string().parse().expect("inteiro decimal")))
}

fn numeric_type_error(value: &Value, expected: &str) -> Failure {
    let kind = match value {
        Value::Null => "NoneType",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
        _ => unreachable!("tipo numérico já convertido"),
    };
    Failure::exception("TypeError", format!("{expected}, not '{kind}'"))
}

pub(super) fn integer(value: &Value) -> Result<Value, Failure> {
    if let Some(number) = nonfinite(value) {
        return float_integer(number);
    }
    match value {
        Value::Bool(flag) => Ok(json!(u8::from(*flag))),
        Value::Number(number) if !number.is_f64() => Ok(value.clone()),
        Value::Number(number) => float_integer(number.as_f64().unwrap()),
        Value::String(text) => {
            let normalized = super::input::normalize_numeric_separators(text).map(|text| {
                text.trim().chars().map(|c| crate::orq_conductor::decimal(c).map_or(c, |n| char::from(b'0' + n))).collect::<String>()
            });
            normalized.and_then(|text| text.parse::<BigInt>().ok())
                .map(|number| Value::Number(number.to_string().parse().expect("inteiro decimal")))
                .ok_or_else(|| Failure::value(format!("invalid literal for int() with base 10: {}", quoted(text))))
        }
        _ => Err(numeric_type_error(value, "int() argument must be a string, a bytes-like object or a real number")),
    }
}

pub(super) fn number(value: &Value) -> Result<f64, Failure> {
    if let Some(number) = nonfinite(value) { return Ok(number); }
    match value {
        Value::Bool(flag) => Ok(if *flag { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64().filter(|n| n.is_finite()).ok_or_else(|| {
            Failure::exception("OverflowError", "int too large to convert to float".into())
        }),
        Value::String(text) => super::input::normalize_numeric_separators(text)
            .map(|text| text.trim().chars().map(|c| crate::orq_conductor::decimal(c).map_or(c, |n| char::from(b'0' + n))).collect::<String>())
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| Failure::value(format!("could not convert string to float: {}", quoted(text)))),
        _ => Err(numeric_type_error(value, "float() argument must be a string or a real number")),
    }
}

pub(super) fn pixel(value: &Value, size: &Value) -> Result<Value, Failure> {
    let number = match number(value) {
        Ok(number) => number,
        Err(error) if error.repr.starts_with("OverflowError(") => return Err(error),
        Err(_) => return Ok(json!(0)),
    };
    let clamped = if number.is_nan() { 1.0 } else { number.clamp(0.0, 1.0) };
    let dimension = self::number(size)?;
    float_integer(clamped * dimension)
}
