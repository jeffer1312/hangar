// crates/hangar-server/src/transcript/py.rs
//! Semântica do Python que o porte repete: `str.strip`, surrogate solto, `str()`/`repr()` e o relógio
//! do `datetime.fromisoformat`.

use std::borrow::Cow;
use std::fmt::Write;

use md5::{Digest, Md5};
use regex::Regex;
use serde_json::{Map, Value};

use super::pyjson;

/// Surrogate solto não cabe numa `String`: ele vira um caractere da área privada até a borda do
/// `ChatEvent`, onde vira U+FFFD como no `scrub_surrogates` (models.py:22). Até lá, os ids que o
/// Python calcula sobre o texto cru saem iguais.
// ponytail: um U+10F800..U+10FFFF de verdade no transcript seria lido como surrogate; trocar por um
// tipo de texto próprio se aparecer.
const MARK_BASE: u32 = 0x10F800;

pub(crate) fn is_marker(c: char) -> bool {
    c as u32 >= MARK_BASE
}

pub(crate) fn mark(surrogate: u32) -> char {
    char::from_u32(MARK_BASE + (surrogate - 0xD800)).expect("surrogate entre D800 e DFFF")
}

pub(crate) fn surrogate_of(c: char) -> u32 {
    c as u32 - MARK_BASE + 0xD800
}

/// Ponto de código como o Python o vê (o marcador volta para U+D800..U+DFFF): ordem do `sort_keys`.
pub(crate) fn py_code(c: char) -> u32 {
    if is_marker(c) { surrogate_of(c) } else { c as u32 }
}

fn has_marker(s: &str) -> bool {
    // Todo caractere acima de U+100000 começa com o byte F4: a busca rápida descarta quase tudo.
    s.as_bytes().contains(&0xF4) && s.chars().any(is_marker)
}

pub(crate) fn scrub_str(s: &mut String) {
    if has_marker(s) {
        *s = s.chars().map(|c| if is_marker(c) { '\u{FFFD}' } else { c }).collect();
    }
}

pub(crate) fn scrub_map(m: &mut Map<String, Value>) {
    if m.keys().any(|k| has_marker(k)) {
        // Chaves que colidem depois da troca: fica a última, na posição da primeira, como no dict.
        for (mut k, mut v) in std::mem::take(m) {
            scrub_str(&mut k);
            scrub_value(&mut v);
            m.insert(k, v);
        }
    } else {
        m.values_mut().for_each(scrub_value);
    }
}

pub(crate) fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => scrub_str(s),
        Value::Array(items) => items.iter_mut().for_each(scrub_value),
        Value::Object(m) => scrub_map(m),
        _ => {}
    }
}

/// `str.isspace()`: o White_Space do Unicode mais os separadores \x1c-\x1f.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `transcript._ts` (transcript.py:616): relógio ISO da entrada em segundos; sem fuso vale UTC.
pub fn ts_of_iso(raw: &str) -> Option<f64> {
    if raw.is_empty() {
        return None;
    }
    iso_timestamp(&raw.replace('Z', "+00:00"))
}

/// `datetime.fromisoformat(s).timestamp()`. Sem fuso lê como UTC: o Python lê como hora local no
/// RewriteFilter e no merged_history, mas os transcripts trazem sempre o fuso.
pub(crate) fn iso_timestamp(s: &str) -> Option<f64> {
    let (naive, offset) = fromisoformat(s)?;
    // Mesma conta do Python: microssegundos inteiros divididos por 10**6.
    Some((naive - offset.unwrap_or(0)) as f64 / 1e6)
}

/// (microssegundos desde a época lendo a hora como UTC, deslocamento do fuso em microssegundos).
/// Porte do `_pydatetime.fromisoformat` (o C aceita fração vazia, e este segue o C). Datas por
/// semana ISO (`2026-W40-5`) ficam de fora.
fn fromisoformat(s: &str) -> Option<(i64, Option<i64>)> {
    let cs: Vec<char> = s.chars().collect();
    // Data sozinha de 7 caracteres só existe na forma por semana.
    if cs.len() < 8 {
        return None;
    }
    let sep = if cs[4] == '-' {
        if cs[5] == 'W' {
            return None;
        }
        10
    } else {
        if cs[4] == 'W' {
            return None;
        }
        8
    };
    let (mut y, mut mo, mut d) = parse_date(cs.get(..sep)?)?;
    let (mut h, mi, sec, us, offset) = if cs.len() > sep {
        let t = &cs[sep + 1..];
        if t.is_empty() {
            return None;
        }
        parse_time(t)?
    } else {
        (0, 0, 0, 0, None)
    };
    if !valid_date(y, mo, d) {
        return None;
    }
    if h == 24 {
        if mi != 0 || sec != 0 || us != 0 {
            return None;
        }
        h = 0;
        d += 1;
        if d > days_in_month(y, mo) {
            d = 1;
            mo += 1;
            if mo > 12 {
                mo = 1;
                y += 1;
            }
        }
        if !valid_date(y, mo, d) {
            return None;
        }
    }
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    Some(((((days * 24 + h) * 60 + mi) * 60 + sec) * 1_000_000 + us, offset))
}

fn digits(cs: &[char]) -> Option<i64> {
    if cs.is_empty() {
        return None;
    }
    cs.iter().try_fold(0i64, |acc, c| c.to_digit(10).map(|x| acc * 10 + i64::from(x)))
}

fn parse_date(d: &[char]) -> Option<(i64, i64, i64)> {
    match d.len() {
        10 if d[4] == '-' && d[7] == '-' => Some((digits(&d[0..4])?, digits(&d[5..7])?, digits(&d[8..10])?)),
        8 if d[4] != '-' => Some((digits(&d[0..4])?, digits(&d[4..6])?, digits(&d[6..8])?)),
        _ => None,
    }
}

type Time = (i64, i64, i64, i64, Option<i64>);

fn parse_time(t: &[char]) -> Option<Time> {
    if t.len() < 2 {
        return None;
    }
    // Primeiro '-', senão '+', senão 'Z', como o `_parse_isoformat_time`.
    let tz = ['-', '+', 'Z'].iter().find_map(|m| t.iter().position(|c| c == m));
    let [h, mi, sec, us] = hh_mm_ss_ff(&t[..tz.unwrap_or(t.len())])?;
    let offset = match tz {
        None => None,
        Some(p) if p + 1 == t.len() && t[p] == 'Z' => Some(0),
        Some(p) => {
            let zone = &t[p + 1..];
            if matches!(zone.len(), 0 | 1 | 3) || t[p] == 'Z' {
                return None;
            }
            let [zh, zm, zs, zus] = hh_mm_ss_ff(zone)?;
            let micros = ((zh * 60 + zm) * 60 + zs) * 1_000_000 + zus;
            if micros >= 86_400_000_000 {
                return None;
            }
            Some(if t[p] == '-' { -micros } else { micros })
        }
    };
    Some((h, mi, sec, us, offset))
}

fn hh_mm_ss_ff(t: &[char]) -> Option<[i64; 4]> {
    let mut comps = [0i64; 4];
    let mut pos = 0;
    let mut has_sep = false;
    for comp in 0..3 {
        if t.len() - pos < 2 {
            return None;
        }
        comps[comp] = digits(&t[pos..pos + 2])?;
        pos += 2;
        let next = t.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        if next.is_none() || comp >= 2 {
            break;
        }
        if has_sep && next != Some(':') {
            return None;
        }
        pos += usize::from(has_sep);
    }
    if pos < t.len() {
        if !matches!(t[pos], '.' | ',') {
            return None;
        }
        let frac = &t[pos + 1..];
        if !frac.iter().all(char::is_ascii_digit) {
            return None;
        }
        let n = frac.len().min(6);
        let mut us = if n == 0 { 0 } else { digits(&frac[..n])? };
        for _ in n..6 {
            us *= 10;
        }
        comps[3] = us;
    }
    Some(comps)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `s.encode("utf-8", "replace")`: o surrogate solto vira "?".
pub(crate) fn utf8_replace(s: &str) -> Cow<'_, [u8]> {
    if !has_marker(s) {
        return Cow::Borrowed(s.as_bytes());
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(if is_marker(c) { '?' } else { c });
    }
    Cow::Owned(out.into_bytes())
}

pub(crate) fn md5_hex(s: &str) -> String {
    format!("{:x}", Md5::digest(&*utf8_replace(s)))
}

pub(crate) fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// Regex com o `\s` do Python, que também casa \x1c-\x1f.
pub(crate) fn py_re(pattern: &str) -> Regex {
    Regex::new(&pattern.replace(r"\s", r"[\s\x1c-\x1f]")).expect("regex do porte")
}

/// Verdade do Python (`bool(x)`); chave ausente é falso.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
    }
}

/// `isinstance(v, int)`, com `bool` dentro.
pub(crate) fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

/// `int(v)` de um int ou float: o float é truncado em direção a zero.
pub(crate) fn int_trunc(v: &Value) -> Option<i128> {
    int_of(v).or_else(|| match v {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i128),
        _ => None,
    })
}

/// `str(v)` de um valor vindo do `json.loads`.
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

pub(crate) fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => pyjson::number_repr(n),
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, x)| format!("{}: {}", repr_str(k), py_repr(x))).collect::<Vec<_>>().join(", ")
        ),
    }
}

// ponytail: `str.isprintable` aproximado (controle, espaço que não é ' ' e os invisíveis comuns);
// tabela de categorias do Unicode se um caso raro aparecer.
fn printable(c: char) -> bool {
    !(c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}'))
}

fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_marker(c) => {
                let _ = write!(out, "\\u{:04x}", surrogate_of(c));
            }
            c if !printable(c) => {
                let n = c as u32;
                let _ = if n < 0x100 {
                    write!(out, "\\x{n:02x}")
                } else if n < 0x10000 {
                    write!(out, "\\u{n:04x}")
                } else {
                    write!(out, "\\U{n:08x}")
                };
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `isinstance(v, (int, float))`, que no Python inclui `bool`.
pub(crate) fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// `str.splitlines()`: além de \n e \r, quebra em \v, \f, \x1c-\x1e, \x85, U+2028 e U+2029.
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, n)| n == '\n') {
                chars.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}
