//! Texto e JSON com o comportamento do Python que os aparelhos e o backend esperam.
use serde_json::Value;

pub mod json_tree;
mod json_writer;
pub mod py;
pub mod pyjson;

/// Linha crua do transcript como JSON, com surrogate solto trocado por U+FFFD (`scrub_surrogates`,
/// models.py:22). None em linha em branco ou que não é JSON.
pub fn decode_line(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let mut v = pyjson::loads_lossless(py::strip(&text))?;
    py::scrub_value(&mut v);
    Some(v)
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
