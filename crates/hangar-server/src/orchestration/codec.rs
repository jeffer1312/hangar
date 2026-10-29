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
    let mut raw = if sort_keys {
        crate::transcript::pyjson::dumps_ordered(value, &|left, right| {
            codepoints(left).cmp(&codepoints(right))
        })
    } else {
        crate::transcript::pyjson::dumps(value, false)
    };
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
    let body = dumps(&value, false);
    discard(value);
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

pub fn contains_surrogate(value: &Value) -> bool {
    fn in_text(text: &str, prefix: &str) -> bool {
        text.split(prefix).skip(1).any(|tail| {
            let bytes = tail.as_bytes();
            bytes.get(4) == Some(&b';')
                && bytes.get(..4).is_some_and(|code| {
                    code.iter().all(u8::is_ascii_hexdigit)
                        && std::str::from_utf8(code).ok()
                            .and_then(|s| u32::from_str_radix(s, 16).ok())
                            .is_some_and(|n| (0xd800..=0xdfff).contains(&n))
                })
        })
    }
    let prefix = format!("{}surrogate:", *MARK);
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) if in_text(text, &prefix) => return true,
            Value::Array(items) => pending.extend(items),
            Value::Object(items) => {
                if items.keys().any(|key| in_text(key, &prefix)) { return true; }
                pending.extend(items.values());
            }
            _ => {}
        }
    }
    false
}

/// JSONResponse recusa números não finitos, embora o cofre possa conservá-los.
pub fn contains_nonfinite(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) if ["NaN", "Infinity", "-Infinity"].iter().any(|token| text == &format!("{}{token}", *MARK)) => return true,
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => pending.extend(values.values()),
            _ => {}
        }
    }
    false
}

/// Resposta tipada do FastAPI converte números não finitos em null, mas não corrige Unicode inválido.
pub fn normalize_response(value: Value) -> Value {
    let result = crate::transcript::json_tree::map_leaves(&value, |leaf| {
        if contains_nonfinite(leaf) { Value::Null } else { leaf.clone() }
    });
    discard(value);
    result
}

pub fn clone_value(value: &Value) -> Value {
    crate::transcript::json_tree::map_leaves(value, Value::clone)
}

pub fn discard(value: Value) {
    crate::transcript::json_tree::discard(value);
}

/// Fatia texto por codepoints Python sem partir um surrogate representado internamente.
pub fn truncate_string(text: &str, count: usize) -> &str {
    let prefix = format!("{}surrogate:", *MARK);
    let mut end = 0;
    for _ in 0..count {
        let rest = &text[end..];
        if rest.is_empty() { break; }
        if let Some(tail) = rest.strip_prefix(&prefix)
            && tail.len() >= 5
            && tail.as_bytes()[4] == b';'
            && tail.as_bytes()[..4].iter().all(u8::is_ascii_hexdigit)
        {
            end += prefix.len() + 5;
        } else {
            end += rest.chars().next().unwrap().len_utf8();
        }
    }
    &text[..end]
}
