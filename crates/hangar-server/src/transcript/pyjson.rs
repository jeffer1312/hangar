// crates/hangar-server/src/transcript/pyjson.rs
//! `json.dumps` e `json.loads` do Python na medida que os ids exigem: separadores ", " e ": ",
//! `repr` de float, `ensure_ascii` e surrogate solto aceito na leitura.

use std::fmt::Write;

use serde_json::{Number, Value};

use super::py;

/// `json.dumps(v, sort_keys=sort_keys)` (ensure_ascii=True).
pub fn dumps(v: &Value, sort_keys: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, v, sort_keys, true);
    out
}

/// `json.dumps(v, ensure_ascii=False, sort_keys=sort_keys)`. Surrogate solto sai cru, como no
/// Python, e só vira U+FFFD na borda do `ChatEvent`.
pub fn dumps_unicode(v: &Value, sort_keys: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, v, sort_keys, false);
    out
}

/// `json.loads` que aceita surrogate solto como o Python. O valor guarda o surrogate como marcador;
/// para o valor final, sem marcador, use `transcript::decode_line`.
// ponytail: inteiro além de u64 vira f64 e NaN/Infinity não são aceitos (o Python aceita os dois);
// `arbitrary_precision` do serde_json se aparecer num transcript.
pub fn loads_lossless(s: &str) -> Option<Value> {
    match serde_json::from_str(s) {
        Ok(v) => Some(v),
        Err(_) if s.contains("\\u") => serde_json::from_str(&mark_lone_surrogates(s)).ok(),
        Err(_) => None,
    }
}

/// Troca cada `\uD8xx`..`\uDFxx` sem par, dentro de string JSON, pelo marcador cru.
fn mark_lone_surrogates(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied, mut in_str) = (0, 0, false);
    while i < b.len() {
        match b[i] {
            b'"' => {
                in_str = !in_str;
                i += 1;
            }
            b'\\' if in_str => match (b.get(i + 1) == Some(&b'u')).then(|| hex4(b, i + 2)).flatten() {
                // Par válido: o serde_json junta sozinho.
                Some(0xD800..=0xDBFF) if low_follows(b, i + 6) => i += 12,
                Some(code @ 0xD800..=0xDFFF) => {
                    out.push_str(&s[copied..i]);
                    out.push(py::mark(code));
                    i += 6;
                    copied = i;
                }
                _ => i += 2,
            },
            _ => i += 1,
        }
    }
    out.push_str(&s[copied..]);
    out
}

fn low_follows(b: &[u8], at: usize) -> bool {
    b.get(at) == Some(&b'\\') && b.get(at + 1) == Some(&b'u') && hex4(b, at + 2).is_some_and(|lo| (0xDC00..=0xDFFF).contains(&lo))
}

fn hex4(b: &[u8], at: usize) -> Option<u32> {
    let digits = b.get(at..at + 4)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

fn write_value(out: &mut String, v: &Value, sort_keys: bool, ascii: bool) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number_repr(n)),
        Value::String(s) => write_str(out, s, ascii),
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, x, sort_keys, ascii);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut pairs: Vec<_> = m.iter().collect();
            if sort_keys {
                // Ordem de ponto de código do Python, com o surrogate marcado no lugar dele.
                pairs.sort_by(|a, b| a.0.chars().map(py::py_code).cmp(b.0.chars().map(py::py_code)));
            }
            out.push('{');
            for (i, (k, x)) in pairs.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_str(out, k, ascii);
                out.push_str(": ");
                write_value(out, x, sort_keys, ascii);
            }
            out.push('}');
        }
    }
}

fn write_str(out: &mut String, s: &str, ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            // O Python escreve o surrogate solto como \udXXX; com ensure_ascii=False ele sai cru.
            c if ascii && py::is_marker(c) => {
                let _ = write!(out, "\\u{:04x}", py::surrogate_of(c));
            }
            c if ascii && c as u32 > 0x7e => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{u:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `repr` de um número do `json.loads`: inteiro em dígitos, float como o Python.
pub(crate) fn number_repr(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        float_repr(n.as_f64().unwrap_or(f64::NAN))
    }
}

/// `float.__repr__`: dígitos mais curtos que voltam ao mesmo valor; notação científica quando o
/// expoente decimal fica abaixo de -4 ou passa de 15 (1e-05, 1e+16), senão fixa com ".0".
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').expect("formato {:e}");
    let exp: i32 = exp.parse().expect("expoente do {:e}");
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1;
    let body = if decpt <= -4 || decpt > 16 {
        let mut m = digits[..1].to_string();
        if digits.len() > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat(decpt.unsigned_abs() as usize))
    } else if decpt as usize >= digits.len() {
        format!("{digits}{}.0", "0".repeat(decpt as usize - digits.len()))
    } else {
        let (int, frac) = digits.split_at(decpt as usize);
        format!("{int}.{frac}")
    };
    if neg { format!("-{body}") } else { body }
}
