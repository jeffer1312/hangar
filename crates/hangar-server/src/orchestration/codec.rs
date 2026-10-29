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
    let raw = if sort_keys {
        crate::transcript::pyjson::dumps_ordered(value, &|left, right| {
            codepoints(left).cmp(&codepoints(right))
        })
    } else {
        crate::transcript::pyjson::dumps_literal(value)
    };
    restore_markers(raw, true)
}

/// Escrita UTF-8 do Python: não finitos permanecem números, surrogate isolado recusa o arquivo.
pub fn dumps_indented_utf8(value: &Value, indent: usize) -> Result<String, &'static str> {
    if contains_surrogate(value) { return Err("surrogate Unicode isolado"); }
    Ok(restore_markers(crate::transcript::pyjson::dumps_indent(value, indent, false), false))
}

fn restore_markers(mut raw: String, ascii: bool) -> String {
    let quote = |value: &Value| if ascii { crate::transcript::pyjson::dumps_literal(value) } else { crate::transcript::pyjson::dumps_unicode(value, false) };
    for token in ["NaN", "Infinity", "-Infinity"] {
        raw = raw.replace(
            &quote(&json!(format!("{}{token}", *MARK))),
            token,
        );
    }
    let quoted = quote(&json!(format!("{}surrogate:", *MARK)));
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

pub fn string_length(text: &str) -> usize {
    let prefix = format!("{}surrogate:", *MARK);
    let mut rest = text;
    let mut count = 0;
    while !rest.is_empty() {
        let length = if let Some(tail) = rest.strip_prefix(&prefix)
            && tail.len() >= 5 && tail.as_bytes()[4] == b';'
            && tail.as_bytes()[..4].iter().all(u8::is_ascii_hexdigit) {
            prefix.len() + 5
        } else { rest.chars().next().unwrap().len_utf8() };
        rest = &rest[length..];
        count += 1;
    }
    count
}

/// Normalização dos sidecars que historicamente usam models.dumps_safe.
pub fn scrub_surrogates(mut value: Value) -> Value {
    fn text(value: &str, prefix: &str) -> String {
        let mut output = String::with_capacity(value.len());
        let mut rest = value;
        while let Some(at) = rest.find(prefix) {
            output.push_str(&rest[..at]);
            let tail = &rest[at + prefix.len()..];
            if tail.as_bytes().get(4) == Some(&b';')
                && tail.as_bytes().get(..4).is_some_and(|bytes| bytes.iter().all(u8::is_ascii_hexdigit))
                && u32::from_str_radix(&tail[..4], 16).is_ok_and(|code| (0xd800..=0xdfff).contains(&code))
            {
                output.push('\u{fffd}');
                rest = &tail[5..];
            } else {
                output.push_str(prefix);
                rest = tail;
            }
        }
        output.push_str(rest);
        output
    }
    let prefix = format!("{}surrogate:", *MARK);
    let mut pending = vec![&mut value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(value) => *value = text(value, &prefix),
            Value::Array(values) => pending.extend(values.iter_mut()),
            Value::Object(values) => {
                let original = std::mem::take(values);
                for (key, value) in original {
                    // A última chave normalizada vence, inclusive quando substitui uma árvore.
                    if let Some(previous) = values.insert(text(&key, &prefix), value) { discard(previous); }
                }
                pending.extend(values.values_mut());
            }
            _ => {}
        }
    }
    value
}
