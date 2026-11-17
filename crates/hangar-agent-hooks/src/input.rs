//! O evento do agente como o `json.loads` do Python o lia: bytes decodificados em UTF-8 com troca
//! (nunca pelo encoding do sistema), chave repetida vale a última, surrogate solto aceito e `NaN`/
//! `Infinity` aceitos como valor.
use hangar_core::pycompat::{py, pyjson};
use serde_json::Value;

/// `bytes.decode("utf-8", "replace")`.
pub fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `json.loads`; `None` onde o Python levantava `ValueError`.
pub fn parse(text: &str) -> Option<Value> {
    pyjson::loads_lossless(text).or_else(|| {
        let finite = without_nonfinite(text)?;
        pyjson::loads_lossless(&finite)
    })
}

/// Os não finitos do Python (`NaN`, `Infinity`, `-Infinity`) fora de texto viram `null`: nenhum hook
/// lê esses campos, e sem a troca o evento inteiro seria recusado. `None` quando não há o que trocar.
fn without_nonfinite(text: &str) -> Option<String> {
    if !text.contains("NaN") && !text.contains("Infinity") {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let (mut at, mut quoted, mut changed) = (0, false, false);
    'scan: while at < bytes.len() {
        let byte = bytes[at];
        if quoted {
            if byte == b'\\' && at + 1 < bytes.len() {
                out.extend_from_slice(&bytes[at..at + 2]);
                at += 2;
                continue;
            }
            quoted = byte != b'"';
        } else if byte == b'"' {
            quoted = true;
        } else {
            for token in ["-Infinity", "Infinity", "NaN"] {
                if bytes[at..].starts_with(token.as_bytes()) {
                    out.extend_from_slice(b"null");
                    at += token.len();
                    changed = true;
                    continue 'scan;
                }
            }
        }
        out.push(byte);
        at += 1;
    }
    // Só bytes ASCII foram trocados: o resto continua UTF-8 válido.
    changed.then(|| String::from_utf8(out).ok()).flatten()
}

/// Texto com surrogate solto: o Python não conseguia gravá-lo em UTF-8 (`UnicodeEncodeError`).
pub fn has_surrogate(text: &str) -> bool {
    text.chars().any(py::is_marker)
}

/// Algum texto (chave ou valor) com surrogate solto.
pub fn value_has_surrogate(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) if has_surrogate(text) => return true,
            Value::Array(items) => pending.extend(items),
            Value::Object(map) => {
                if map.keys().any(|key| has_surrogate(key)) {
                    return true;
                }
                pending.extend(map.values());
            }
            _ => {}
        }
    }
    false
}

/// `os.path.basename` da plataforma: no POSIX só `/` separa, no Windows também `\`.
pub fn basename(text: &str) -> &str {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    text.rsplit(separators).next().unwrap_or(text)
}

/// Chave de arquivo vinda do evento: só texto, sem pasta (nada escapa da pasta do marcador) e não vazia.
pub fn file_key(value: &Value) -> Option<&str> {
    let key = basename(value.as_str()?);
    (!key.is_empty() && key != "." && key != "..").then_some(key)
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs_f64()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn non_finite_numbers_become_null_outside_strings_only() {
        let value = parse(r#"{"a":NaN,"b":-Infinity,"c":"NaN Infinity","d":[Infinity]}"#).unwrap();
        assert_eq!(value, json!({"a": null, "b": null, "c": "NaN Infinity", "d": [null]}));
    }

    #[test]
    fn duplicate_keys_keep_the_last_value_like_python() {
        assert_eq!(parse(r#"{"k":"1","k":"2"}"#).unwrap()["k"], "2");
    }

    #[test]
    fn a_lone_surrogate_is_accepted_and_detected() {
        let value = parse(r#"{"p":"a\ud800b"}"#).unwrap();
        assert!(value_has_surrogate(&value));
        assert!(!value_has_surrogate(&parse(r#"{"p":"a😀b"}"#).unwrap()));
    }

    #[test]
    fn bom_and_garbage_are_refused_like_json_loads() {
        assert!(parse("\u{feff}{}").is_none());
        assert!(parse("{x").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn file_keys_never_carry_a_directory() {
        assert_eq!(file_key(&json!("../x")), Some("x"));
        assert_eq!(file_key(&json!("a/b")), Some("b"));
        assert_eq!(file_key(&json!("a/")), None);
        assert_eq!(file_key(&json!("..")), None);
        assert_eq!(file_key(&json!(12)), None);
    }
}
