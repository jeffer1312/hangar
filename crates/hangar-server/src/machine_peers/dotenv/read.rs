//! Leitura e interpolação do arquivo sem depender do ambiente global.
use regex::Regex;
use std::{collections::BTreeMap, sync::LazyLock};
struct Grammar {
    whitespace: Regex,
    horizontal: Regex,
    key: Regex,
    single: Regex,
    double: Regex,
    variable: Regex,
    comment: Regex,
}
static GRAMMAR: LazyLock<Grammar> = LazyLock::new(|| Grammar {
    whitespace: Regex::new(r"^[\s\x1c-\x1f]*").unwrap(),
    horizontal: Regex::new(r"^(?:[^\S\r\n]|[\x1c-\x1f])*").unwrap(),
    key: Regex::new(r"^(?:'([^']+)'|([^=#\s\x1c-\x1f]+))").unwrap(),
    single: Regex::new(r"^'((?:\\'|[^'])*)'").unwrap(),
    double: Regex::new(r#"^"((?:\\"|[^"])*)""#).unwrap(),
    variable: Regex::new(r"\$\{([^}:]*)(?::-([^}]*))?\}").unwrap(),
    comment: Regex::new(r"[\s\x1c-\x1f]+#.*").unwrap(),
});
fn consume(regex: &Regex, raw: &str, at: &mut usize) {
    if let Some(m) = regex.find(&raw[*at..]) {
        *at += m.end();
    }
}
fn end_line(raw: &str, mut at: usize) -> usize {
    while at < raw.len() && !matches!(raw.as_bytes()[at], b'\r' | b'\n') {
        at += 1;
    }
    if raw.as_bytes().get(at) == Some(&b'\r') {
        at += 1;
    }
    if raw.as_bytes().get(at) == Some(&b'\n') {
        at += 1;
    }
    at
}
fn escapes(raw: &str, double: bool) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
        {
            let decoded = match next {
                '\\' => Some('\\'),
                '\'' => Some('\''),
                '"' if double => Some('"'),
                'a' if double => Some('\x07'),
                'b' if double => Some('\x08'),
                'f' if double => Some('\x0c'),
                'n' if double => Some('\n'),
                'r' if double => Some('\r'),
                't' if double => Some('\t'),
                'v' if double => Some('\x0b'),
                _ => None,
            };
            if let Some(decoded) = decoded {
                chars.next();
                out.push(decoded);
                continue;
            }
        }
        out.push(c);
    }
    out
}
fn parse(raw: &str) -> (usize, Option<(String, Option<String>)>) {
    let g = &*GRAMMAR;
    let mut at = 0;
    consume(&g.whitespace, raw, &mut at);
    if at == raw.len() {
        return (at, None);
    }
    if raw[at..].starts_with('#') {
        return (end_line(raw, at), None);
    }
    if raw[at..].starts_with("export") {
        let next = at + 6;
        let mut after = next;
        consume(&g.horizontal, raw, &mut after);
        if after > next {
            at = after;
        }
    }
    let Some(key) = g.key.captures(&raw[at..]) else {
        return (end_line(raw, at), None);
    };
    let name = key
        .get(1)
        .or_else(|| key.get(2))
        .unwrap()
        .as_str()
        .to_owned();
    at += key.get(0).unwrap().end();
    consume(&g.horizontal, raw, &mut at);
    let value = if raw.as_bytes().get(at) == Some(&b'=') {
        at += 1;
        consume(&g.horizontal, raw, &mut at);
        let quote = raw.as_bytes().get(at).copied();
        if matches!(quote, Some(b'\'' | b'"')) {
            let pattern = if quote == Some(b'\'') {
                &g.single
            } else {
                &g.double
            };
            let Some(value) = pattern.captures(&raw[at..]) else {
                return (end_line(raw, at), None);
            };
            let decoded = escapes(value.get(1).unwrap().as_str(), quote == Some(b'"'));
            at += value.get(0).unwrap().end();
            Some(decoded)
        } else {
            let end = raw[at..]
                .find(['\r', '\n'])
                .map(|n| at + n)
                .unwrap_or(raw.len());
            let value = g
                .comment
                .replace(&raw[at..end], "")
                .trim_end_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
                .to_owned();
            at = end;
            Some(value)
        }
    } else {
        None
    };
    consume(&g.horizontal, raw, &mut at);
    if raw.as_bytes().get(at) == Some(&b'#') {
        at = end_line(raw, at);
    } else if at < raw.len() && !matches!(raw.as_bytes()[at], b'\r' | b'\n') {
        return (end_line(raw, at), None);
    } else {
        at = end_line(raw, at);
    }
    (at, Some((name, value)))
}
fn ordered(raw: &str, environment: &BTreeMap<String, String>) -> Vec<(String, Option<String>)> {
    let mut values: Vec<(String, Option<String>)> = Vec::new();
    let mut remaining = raw;
    while !remaining.is_empty() {
        let (used, binding) = parse(remaining);
        debug_assert!(used > 0);
        remaining = &remaining[used..];
        let Some((key, value)) = binding else {
            continue;
        };
        let value = value.map(|value| {
            GRAMMAR
                .variable
                .replace_all(&value, |captures: &regex::Captures<'_>| {
                    let key = &captures[1];
                    if let Some((_, value)) = values.iter().find(|(name, _)| name == key) {
                        value.clone().unwrap_or_default()
                    } else {
                        environment.get(key).cloned().unwrap_or_else(|| {
                            captures.get(2).map(|v| v.as_str()).unwrap_or("").into()
                        })
                    }
                })
                .into_owned()
        });
        if let Some((_, current)) = values.iter_mut().find(|(name, _)| *name == key) {
            *current = value;
        } else {
            values.push((key, value));
        }
    }
    values
}
/// Como dotenv_values: o arquivo anterior vence o ambiente dentro de `${...}`.
pub fn resolve(
    raw: &str,
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, Option<String>> {
    ordered(raw, environment).into_iter().collect()
}
/// Valores do arquivo como Settings os lê, antes de aplicar overrides do ambiente.
pub fn file_settings(
    raw: &str,
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, Option<String>> {
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    ordered(&raw, environment)
        .into_iter()
        .map(|(key, value)| (key.to_lowercase(), value))
        .collect()
}
/// Como Settings: sem distinção de caixa, ambiente vence o valor final do arquivo.
pub fn settings(
    raw: &str,
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, Option<String>> {
    let mut values = file_settings(raw, environment);
    values.extend(
        environment
            .iter()
            .map(|(key, value)| (key.to_lowercase(), Some(value.clone()))),
    );
    values
}
