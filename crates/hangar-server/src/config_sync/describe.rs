//! Descrições derivadas dos arquivos, sem catálogo paralelo à configuração.
use std::{io::Read, path::Path};

pub(super) fn short(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= 220 {
        return text;
    }
    let cut: String = text.chars().take(220).collect();
    let end = [cut.rfind(". "), cut.rfind("; ")]
        .into_iter()
        .flatten()
        .max();
    if let Some(end) = end.filter(|end| cut[..*end].chars().count() > 220 / 3) {
        cut[..end + 1].to_owned()
    } else {
        format!("{}…", cut.trim_end())
    }
}

fn front(head: &str) -> Option<(&str, &str)> {
    static FRONT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?s)\A---\s*\n(.*?)\n---\s*(?:\n|\z)").unwrap()
    });
    let c = FRONT.captures(head)?;
    Some((c.get(1)?.as_str(), &head[c.get(0)?.end()..]))
}

fn markdown(head: &str) -> String {
    let mut body = head;
    if let Some((meta, remaining)) = front(head) {
        body = remaining;
        let lines: Vec<_> = meta.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some(value) = line.strip_prefix("description:") else {
                continue;
            };
            let value = value.trim();
            let value = if ["", "|", ">", "|-", ">-", "|+", ">+"].contains(&value) {
                lines[i + 1..]
                    .iter()
                    .take_while(|l| l.chars().next().is_none_or(char::is_whitespace))
                    .map(|l| l.trim())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                value.to_owned()
            };
            let value = value.trim_matches(['\'', '"']);
            if !value.is_empty() {
                return value.to_owned();
            }
            break;
        }
    }
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

fn script(head: &str) -> String {
    let head = if head.starts_with("#!") {
        head.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        head
    };
    let trimmed = head.trim_start();
    let doc = trimmed.trim_start_matches(['r', 'u', 'b']);
    for quote in ["\"\"\"", "'''"] {
        if let Some(rest) = doc.strip_prefix(quote) {
            let end = [rest.find(quote), rest.find("\n\n")]
                .into_iter()
                .flatten()
                .min();
            if let Some(end) = end {
                let content = rest[..end].trim();
                if !content.is_empty() {
                    return content.to_owned();
                }
            }
        }
    }
    static COMMENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^\s*(?:#|//|--|;|rem\s|::)\s?").unwrap()
    });
    let mut found = Vec::new();
    for line in head.lines() {
        let trimmed = line.trim();
        if line.contains("coding") && line.trim_start().starts_with('#') {
            continue;
        }
        if !trimmed.starts_with("#!") && !trimmed.starts_with("# shellcheck") {
            if let Some(m) = COMMENT.find(line) {
                let text = line[m.end()..].trim();
                if !text.is_empty() {
                    found.push(text);
                    continue;
                }
            }
        }
        if !found.is_empty()
            || (!trimmed.is_empty()
                && !["set ", "@echo", "use "]
                    .iter()
                    .any(|p| trimmed.starts_with(p)))
        {
            break;
        }
    }
    found.join(" ")
}

pub fn describe(path: &Path) -> String {
    if path.is_dir() {
        return ["SKILL.md", "README.md"]
            .iter()
            .map(|n| path.join(n))
            .find(|p| p.is_file())
            .map(|p| describe(&p))
            .unwrap_or_default();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut raw = Vec::new();
    if file.take(8192).read_to_end(&mut raw).is_err() || raw.contains(&0) {
        return String::new();
    }
    let text = String::from_utf8_lossy(&raw);
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "md" | "markdown" => short(&markdown(&text)),
        "json" | "toml" | "yaml" | "yml" | "css" | "txt" => String::new(),
        _ => short(&script(&text)),
    }
}
