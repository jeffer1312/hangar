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

/// Separadores e Unicode da resposta HTTP, sem copiar a árvore recebida.
pub fn dumps_compact_unicode(v: &Value) -> String {
    let mut out = String::new();
    super::json_writer::write(&mut out, v, false, None, None, true, true);
    out
}

/// `json.dumps(v, indent=indent, ensure_ascii=ascii)`: um item por linha, separadores "," e ": ", `{}` e
/// `[]` vazios compactos.
pub fn dumps_indent(v: &Value, indent: usize, ascii: bool) -> String {
    let mut out = String::new();
    super::json_writer::write(&mut out, v, ascii, Some(indent), None, false, true);
    out
}

/// `json.loads` que aceita surrogate solto como o Python. O valor guarda o surrogate como marcador;
/// para o valor final, sem marcador, use `transcript::decode_line`.
// Não finitos continuam recusados neste leitor; o cofre ORQ tem o próprio codec para eles.
pub fn loads_lossless(s: &str) -> Option<Value> {
    decode_raw(s).or_else(|| s.contains("\\u").then(||decode_raw(&mark_lone_surrogates(s))).flatten())
}

pub(crate) fn decode_raw(raw: &str) -> Option<Value> {
    super::json_tree::decode(raw)
}

/// Troca cada `\uD8xx`..`\uDFxx` sem par, dentro de string JSON, pelo marcador cru.
fn mark_lone_surrogates(s: &str) -> String {
    mark_lone_surrogates_using(s, |code| py::mark(code).to_string())
}

/// Permite que outro leitor preserve a identidade dos surrogates sem colidir com texto legítimo.
pub(crate) fn mark_lone_surrogates_using(s: &str, mark: impl Fn(u32) -> String) -> String {
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
                    out.push_str(&mark(code));
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
    let compare = |left: &str, right: &str| {
        left.chars().map(py::py_code).cmp(right.chars().map(py::py_code))
    };
    super::json_writer::write(out, v, ascii, None, if sort_keys { Some(&compare) } else { None }, false, true);
}

/// O codec do cofre usa marcadores próprios e fornece a ordem dos pontos de código.
pub(crate) fn dumps_ordered(v: &Value, compare: &dyn Fn(&str, &str) -> std::cmp::Ordering) -> String {
    let mut out = String::new();
    super::json_writer::write(&mut out, v, true, None, Some(compare), false, false);
    out
}

/// O codec com marcadores próprios precisa conservar os caracteres Unicode legítimos.
pub(crate) fn dumps_literal(v: &Value) -> String {
    let mut out = String::new();
    super::json_writer::write(&mut out, v, true, None, None, false, false);
    out
}

pub(super) fn write_str(out: &mut String, s: &str, ascii: bool, legacy_surrogates: bool) {
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
            c if ascii && legacy_surrogates && py::is_marker(c) => {
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
    let raw = n.to_string();
    if !raw.contains(['.', 'e', 'E']) {
        raw
    } else {
        float_repr(n.as_f64().or_else(|| raw.parse().ok()).unwrap_or(f64::NAN))
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

#[cfg(test)]
mod numeric_width_tests {
    #[test]
    fn integer_width_does_not_change_python_canonical_float_or_integer_representations() {
        for (raw,expected) in [("-9223372036854775808","-9223372036854775808"),("18446744073709551615","18446744073709551615"),("340282366920938463463374607431768211579","340282366920938463463374607431768211579"),("1.0","1.0"),("1e-5","1e-05"),("1e20","1e+20")] {
            let value=serde_json::from_str(raw).unwrap();
            assert_eq!(super::dumps(&value,false),expected,"{raw}");
        }
    }
}
