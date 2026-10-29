//! JSON do cofre e da CLI: inteiros largos, não finitos e surrogates conservam sua identidade.
use serde_json::{Value, json};
use std::sync::LazyLock;

// O marcador só existe em memória; a saída recompõe o JSON original antes de cruzar o protocolo.
static MARK: LazyLock<String> = LazyLock::new(|| {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("aleatoriedade do sistema");
    format!(
        "\u{10ffff}orq:{}:",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
});
pub fn loads(text: &str) -> Option<Value> {
    crate::orq_conductor::load(text, &MARK)
}
pub(super) fn surrogate_marker(code: u32) -> String {
    format!("{}surrogate:{code:04x};", *MARK)
}
pub fn infinite(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s == format!("{}Infinity", *MARK) || s == format!("{}-Infinity", *MARK))
}
pub fn python_string(value: &Value) -> String {
    crate::orq_conductor::py_str(value, &MARK)
}
pub fn dumps(value: &Value, sort_keys: bool) -> String {
    fn codepoints(key: &str) -> Vec<u32> {
        let prefix = format!("{}surrogate:", *MARK);
        let mut rest = key;
        let mut out = Vec::new();
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix(&prefix)
                && tail.len() >= 5
                && tail.as_bytes()[4] == b';'
                && let Ok(code) = u32::from_str_radix(&tail[..4], 16)
            {
                out.push(code);
                rest = &tail[5..];
            } else {
                let c = rest.chars().next().unwrap();
                out.push(c as u32);
                rest = &rest[c.len_utf8()..];
            }
        }
        out
    }
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut pairs: Vec<_> = map.iter().collect();
                pairs.sort_by_cached_key(|(key, _)| codepoints(key));
                Value::Object(
                    pairs
                        .into_iter()
                        .map(|(key, value)| (key.clone(), sorted(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(sorted).collect()),
            _ => value.clone(),
        }
    }
    let mut raw = crate::transcript::pyjson::dumps(
        &if sort_keys {
            sorted(value)
        } else {
            value.clone()
        },
        false,
    );
    for token in ["NaN", "Infinity", "-Infinity"] {
        raw = raw.replace(
            &crate::transcript::pyjson::dumps(&json!(format!("{}{token}", *MARK)), false),
            token,
        );
    }
    let quoted = crate::transcript::pyjson::dumps(&json!(format!("{}surrogate:", *MARK)), false);
    let prefix = &quoted[1..quoted.len() - 1];
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    while let Some(at) = rest.find(prefix) {
        out.push_str(&rest[..at]);
        let tail = &rest[at + prefix.len()..];
        if tail.len() >= 5
            && tail.as_bytes()[4] == b';'
            && tail.as_bytes()[..4].iter().all(u8::is_ascii_hexdigit)
        {
            out.push_str("\\u");
            out.push_str(&tail[..4]);
            rest = &tail[5..];
        } else {
            out.push_str(prefix);
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}
pub fn response(status: axum::http::StatusCode, value: Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        dumps(&value, false),
    )
        .into_response()
}
