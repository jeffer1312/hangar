//! Marcadores portáveis da configuração; caminhos e programas pertencem ao destino.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const MARKERS: [&str; 4] = ["HANGAR", "CLAUDE", "CODEX", "HOME"];
pub const HEAVY_DIRS: [&str; 5] = [".git", "node_modules", ".venv", "venv", "__pycache__"];
pub const PROGRAMS: [&str; 11] = [
    "python",
    "python3",
    "node",
    "bash",
    "sh",
    "pwsh",
    "powershell",
    "uv",
    "bun",
    "deno",
    "npx",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Roots {
    pub hangar: String,
    pub claude: String,
    pub codex: String,
    pub home: String,
}

impl Roots {
    pub fn value(&self, marker: &str) -> &str {
        match marker {
            "HANGAR" => &self.hangar,
            "CLAUDE" => &self.claude,
            "CODEX" => &self.codex,
            "HOME" => &self.home,
            _ => unreachable!(),
        }
    }
}

fn boundary(c: char) -> bool {
    c.is_whitespace() || "\\/'\"`;|&)".contains(c)
}
fn starts_path(prefix: &str) -> bool {
    prefix
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace() || "'\"`=:(;|&".contains(c))
        || prefix.ends_with("file://")
        || prefix.ends_with("file:///")
}
fn rest_end(c: char) -> bool {
    c.is_whitespace() || "'\"`;|&)⟦".contains(c)
}

pub fn canonicalize(text: &str, roots: &Roots) -> String {
    let mut result = text.to_owned();
    let versions = Path::new(&roots.home)
        .join(".hangar")
        .join("versions")
        .to_string_lossy()
        .into_owned();
    for form in [versions.clone(), versions.replace('\\', "/")] {
        let pattern = format!(r#"{}[\\/][^\\/\s'"`;|&)⟦]+"#, regex::escape(&form));
        let regex = regex::Regex::new(&pattern).expect("expressão escapada");
        result = regex
            .replace_all(&result, |capture: &regex::Captures| {
                let found = capture.get(0).unwrap();
                if starts_path(&result[..found.start()])
                    && result[found.end()..].chars().next().is_none_or(boundary)
                {
                    "⟦HANGAR⟧".to_string()
                } else {
                    found.as_str().to_string()
                }
            })
            .into_owned();
    }
    let mut pairs = Vec::new();
    for marker in MARKERS {
        for form in [
            roots.value(marker).to_owned(),
            roots.value(marker).replace('\\', "/"),
        ] {
            if !pairs.contains(&(form.clone(), marker)) {
                pairs.push((form, marker));
            }
        }
    }
    pairs.sort_by_key(|(form, _)| std::cmp::Reverse(form.chars().count()));
    for (form, marker) in pairs {
        if form.is_empty() {
            continue;
        }
        let mut out = String::new();
        let mut previous = 0;
        for (at, found) in result.match_indices(&form) {
            let end = at + found.len();
            if starts_path(&result[..at]) && result[end..].chars().next().is_none_or(boundary) {
                out.push_str(&result[previous..at]);
                out.push_str(&format!("⟦{marker}⟧"));
                previous = end;
            }
        }
        out.push_str(&result[previous..]);
        result = out;
    }
    result
}

pub fn resolve(text: &str, roots: &Roots) -> String {
    static MARKER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"⟦(HANGAR|CLAUDE|CODEX|HOME)⟧").unwrap());
    let mut out = String::new();
    let mut previous = 0;
    for capture in MARKER.captures_iter(text) {
        let found = capture.get(0).unwrap();
        if found.start() < previous {
            continue;
        }
        out.push_str(&text[previous..found.start()]);
        out.push_str(
            roots
                .value(&capture[1])
                .replace('\\', "/")
                .trim_end_matches('/'),
        );
        let rest = &text[found.end()..];
        let end = rest.find(rest_end).unwrap_or(rest.len());
        out.push_str(&rest[..end].replace('\\', "/"));
        previous = found.end() + end;
    }
    out.push_str(&text[previous..]);
    out
}

pub fn marked_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut ranges = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let Some((relative, quote)) = text[offset..]
            .char_indices()
            .find(|(_, c)| matches!(c, '\'' | '"'))
        else {
            break;
        };
        let start = offset + relative;
        let inside = start + quote.len_utf8();
        let end = text[inside..].find(['\'', '"']).map(|i| inside + i);
        if let Some(end) = end {
            if text[end..].starts_with(quote)
                && MARKERS
                    .iter()
                    .any(|m| text[inside..].starts_with(&format!("⟦{m}⟧")))
            {
                let value = text[inside..end].to_owned();
                if !found.contains(&value) {
                    found.push(value);
                }
                ranges.push((start, end + quote.len_utf8()));
                offset = end + quote.len_utf8();
                continue;
            }
        }
        offset = inside;
    }
    let mut rest = text.to_owned();
    for (start, end) in ranges.into_iter().rev() {
        rest.replace_range(start..end, " ");
    }
    static MARKER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"⟦(?:HANGAR|CLAUDE|CODEX|HOME)⟧").unwrap());
    for m in MARKER.find_iter(&rest) {
        let tail = &rest[m.end()..];
        let end = tail.find(rest_end).unwrap_or(tail.len());
        let value = rest[m.start()..m.end() + end].to_owned();
        if !found.contains(&value) {
            found.push(value);
        }
    }
    found
}

pub fn map_strings(value: &Value, transform: &impl Fn(&str) -> String) -> Value {
    match value {
        Value::String(s) => Value::String(transform(s)),
        Value::Array(values) => {
            Value::Array(values.iter().map(|v| map_strings(v, transform)).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(k, v)| (transform(k), map_strings(v, transform)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

pub fn fix_programs(
    command: &str,
    which: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&str) -> bool,
) -> (String, Vec<String>) {
    static TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"'[^'"]*'|"[^'"]*"|[^\s'"`;|&()]+"#).unwrap()
    });
    let mut first = true;
    let mut missing = Vec::new();
    let result = TOKEN
        .replace_all(command, |capture: &regex::Captures| {
            let m = capture.get(0).unwrap();
            if command[..m.start()]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace() && !";|&(".contains(c))
            {
                return m.as_str().to_owned();
            }
            let raw = m.as_str();
            let quote = raw.chars().next().filter(|c| matches!(c, '\'' | '"'));
            let token = if quote.is_some() {
                &raw[1..raw.len() - 1]
            } else {
                raw
            };
            let is_first = std::mem::replace(&mut first, false);
            let absolute = token.starts_with('/')
                || (token.as_bytes().get(0).is_some_and(u8::is_ascii_alphabetic)
                    && token.as_bytes().get(1) == Some(&b':')
                    && matches!(token.as_bytes().get(2), Some(b'/' | b'\\')));
            if !absolute {
                if is_first
                    && quote.is_none()
                    && ![
                        "if", "then", "else", "fi", "[", "[[", "test", "exec", "env", "cd", "for",
                        "while", "do", "done", "!",
                    ]
                    .contains(&token)
                    && !token.contains('=')
                    && !token.starts_with(['$', '~'])
                    && which(token).is_none()
                {
                    missing.push(token.to_owned());
                }
                return raw.to_owned();
            }
            if exists(token) {
                return raw.to_owned();
            }
            let normal = token.replace('\\', "/");
            let base = normal
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("");
            let base = base.strip_suffix(".exe").unwrap_or(base).to_lowercase();
            if !PROGRAMS.contains(&base.as_str()) && !is_first {
                return raw.to_owned();
            }
            let found = which(&base).or_else(|| match base.as_str() {
                "python" => which("python3"),
                "python3" => which("python"),
                _ => None,
            });
            let Some(found) = found else {
                missing.push(base);
                return raw.to_owned();
            };
            let found = found.replace('\\', "/");
            if let Some(quote) = quote {
                format!("{quote}{found}{quote}")
            } else if found.contains(' ') {
                format!("\"{found}\"")
            } else {
                found
            }
        })
        .into_owned();
    (result, missing)
}
