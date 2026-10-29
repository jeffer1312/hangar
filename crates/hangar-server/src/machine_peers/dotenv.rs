mod environment;
pub use environment::SettingsEnvironment;
mod read;
pub use read::{file_settings, resolve, settings};
// Edição de atribuições sem reformatar comentários, espaços ou valores multilinha alheios.
use std::collections::HashSet;

struct Binding<'a> {
    raw: &'a str,
    key: Option<&'a str>,
    value: Option<(usize, usize)>,
}
fn line_end(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && !matches!(bytes[at], b'\n' | b'\r') {
        at += 1;
    }
    if bytes.get(at) == Some(&b'\r') {
        at += 1;
    }
    if bytes.get(at) == Some(&b'\n') {
        at += 1;
    }
    at
}
fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}
fn horizontal(raw: &str, at: usize) -> usize {
    raw.get(at..)
        .and_then(|s| s.chars().next())
        .filter(|&c| whitespace(c) && !matches!(c, '\r' | '\n'))
        .map(char::len_utf8)
        .unwrap_or(0)
}
fn quoted_end(bytes: &[u8], start: usize) -> Option<usize> {
    let quote = bytes[start];
    let mut at = start + 1;
    while at < bytes.len() {
        if bytes[at] == quote {
            return Some(at + 1);
        }
        if bytes[at] == b'\\' && bytes.get(at + 1) == Some(&quote) {
            at += 2;
        } else {
            at += 1;
        }
    }
    None
}
fn binding(raw: &str) -> Binding<'_> {
    let bytes = raw.as_bytes();
    let mut at = 0;
    while at < bytes.len() && raw[at..].chars().next().is_some_and(whitespace) {
        at += raw[at..].chars().next().unwrap().len_utf8();
    }
    let fallback = |end| Binding {
        raw: &raw[..end],
        key: None,
        value: None,
    };
    if at == bytes.len() {
        return fallback(at);
    }
    if bytes[at] == b'#' {
        return fallback(line_end(bytes, at));
    }
    if raw[at..].starts_with("export") && horizontal(raw, at + 6) > 0 {
        at += 6;
        while horizontal(raw, at) > 0 {
            at += horizontal(raw, at);
        }
    }
    let key_start = at;
    let key = if bytes.get(at) == Some(&b'\'') {
        let Some(end) = bytes[at + 1..]
            .iter()
            .position(|byte| *byte == b'\'')
            .map(|n| at + n + 2)
        else {
            return fallback(line_end(bytes, at));
        };
        at = end;
        &raw[key_start + 1..end - 1]
    } else {
        while at < bytes.len()
            && !raw[at..].chars().next().is_some_and(whitespace)
            && !matches!(bytes[at], b'=' | b'#')
        {
            at += raw[at..].chars().next().unwrap().len_utf8();
        }
        &raw[key_start..at]
    };
    if key.is_empty() {
        return fallback(line_end(bytes, at));
    }
    while horizontal(raw, at) > 0 {
        at += horizontal(raw, at);
    }
    if bytes.get(at) != Some(&b'=') {
        if at == bytes.len() || matches!(bytes[at], b'#' | b'\n' | b'\r') {
            return Binding {
                raw: &raw[..line_end(bytes, at)],
                key: Some(key),
                value: None,
            };
        }
        return fallback(line_end(bytes, at));
    }
    at += 1;
    while horizontal(raw, at) > 0 {
        at += horizontal(raw, at);
    }
    let start = at;
    let end = if bytes.get(at).is_some_and(|b| matches!(b, b'\'' | b'"')) {
        let Some(end) = quoted_end(bytes, at) else {
            return fallback(line_end(bytes, at));
        };
        end
    } else {
        let mut end = at;
        while end < bytes.len() && !matches!(bytes[end], b'\n' | b'\r') {
            if horizontal(raw, end) > 0 {
                let mut look = end;
                while horizontal(raw, look) > 0 {
                    look += horizontal(raw, look);
                }
                if bytes.get(look) == Some(&b'#') {
                    break;
                }
            }
            end += raw[end..].chars().next().unwrap().len_utf8();
        }
        start + raw[start..end].trim_end_matches(whitespace).len()
    };
    let mut after = end;
    while horizontal(raw, after) > 0 {
        after += horizontal(raw, after);
    }
    if after < bytes.len() && !matches!(bytes[after], b'#' | b'\n' | b'\r') {
        return fallback(line_end(bytes, after));
    }
    Binding {
        raw: &raw[..line_end(bytes, after)],
        key: Some(key),
        value: Some((start, end)),
    }
}
pub fn replace(raw: &str, values: &[(&str, &str)]) -> String {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let newline = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut remaining = raw;
    let mut output = String::new();
    let mut seen = HashSet::new();
    while !remaining.is_empty() {
        let part = binding(remaining);
        let target = part
            .key
            .and_then(|key| values.iter().find(|(candidate, _)| *candidate == key));
        match target {
            Some((key, value)) if seen.insert(*key) => {
                if let Some((start, end)) = part.value {
                    output.push_str(&part.raw[..start]);
                    output.push_str(value);
                    output.push_str(&part.raw[end..]);
                } else {
                    output.push_str(key);
                    output.push('=');
                    output.push_str(value);
                    if part.raw.ends_with("\r\n") {
                        output.push_str("\r\n");
                    } else if part.raw.ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
            Some(_) => {}
            None => output.push_str(part.raw),
        }
        remaining = &remaining[part.raw.len()..];
    }
    for &(key, value) in values {
        if !seen.contains(key) {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push_str(newline);
            }
            output.push_str(key);
            output.push('=');
            output.push_str(value);
            output.push_str(newline);
        }
    }
    output
}

/// Leitura das chaves de canal; a escrita remove aspas delas, mas o arquivo inicial pode usá-las.
pub fn value(raw: &str, key: &str) -> Option<String> {
    resolve(raw, &Default::default()).remove(key).flatten()
}
