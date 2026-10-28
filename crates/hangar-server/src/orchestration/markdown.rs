//! O app altera a tabela escolhida e preserva a prosa do documento.
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
pub fn normalize(text: &str) -> String {
    let whitespace = |c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c);
    let plain = text
        .trim_matches(whitespace)
        .trim_matches('`')
        .replace("**", "");
    plain
        .trim_matches(whitespace)
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
pub fn clean(text: &str) -> String {
    let mut s = text.trim();
    if s.starts_with("**") && s.ends_with("**") && s.len() >= 4 {
        s = s[2..s.len() - 2].trim()
    }
    s = s.trim_matches('`').trim();
    if matches!(s, "-" | "—") {
        String::new()
    } else {
        s.into()
    }
}
pub fn validate(value: &str) -> Result<String, String> {
    if value.contains('|') || value.chars().any(|c| c < '\u{20}' || c == '\u{7f}') {
        Err("celula com '|', quebra de linha ou caractere de controle".into())
    } else {
        Ok(value.trim().into())
    }
}
fn lines(text: &str) -> Vec<String> {
    crate::transcript::py::splitlines(text)
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn cells(line: &str) -> Vec<&str> {
    let mut s = line.trim();
    if let Some(rest) = s.strip_prefix('|') {
        s = rest
    }
    if let Some(rest) = s.strip_suffix('|') {
        s = rest
    }
    s.split('|').collect()
}
fn separator(line: &str) -> bool {
    let c = cells(line);
    static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*:?-{2,}:?\s*$").unwrap());
    !c.is_empty() && c.iter().all(|c| SEPARATOR.is_match(c))
}
fn masked(text: &str) -> String {
    let mut rows = lines(text);
    let mut i = 0;
    while i < rows.len() {
        let marker = if rows[i].starts_with("```") {
            Some("```")
        } else if rows[i].starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(marker) = marker {
            if let Some(end) = (i + 1..rows.len()).find(|n| rows[*n].starts_with(marker)) {
                for row in &mut rows[i..=end] {
                    *row = row.chars().map(|_| ' ').collect()
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    rows.join("\n")
}

fn locate(text: &str, headers: &[String]) -> Option<(usize, usize)> {
    let target: Vec<_> = headers.iter().map(|h| normalize(h)).collect();
    let masked = masked(text);
    let rows = lines(&masked);
    for (i, line) in rows.iter().enumerate() {
        if !line.trim_start().starts_with('|')
            || cells(line).iter().map(|c| normalize(c)).collect::<Vec<_>>() != target
        {
            continue;
        }
        let mut end = i + 1;
        while end < rows.len() && rows[end].trim_start().starts_with('|') {
            end += 1
        }
        return Some((i, end));
    }
    None
}
pub fn table(text: &str, headers: &[String]) -> Vec<Value> {
    let Some((start, end)) = locate(text, headers) else {
        return Vec::new();
    };
    let rows = lines(text);
    rows[start + 1..end]
        .iter()
        .filter(|l| !separator(l))
        .map(|line| {
            let c: Vec<_> = cells(line).iter().map(|c| clean(c)).collect();
            let row: serde_json::Map<_, _> = headers
                .iter()
                .enumerate()
                .map(|(i, h)| (h.clone(), json!(c.get(i).map(String::as_str).unwrap_or(""))))
                .collect();
            Value::Object(row)
        })
        .collect()
}
fn rendered(headers: &[String], values: &Value) -> Result<String, String> {
    let values: Result<Vec<_>, _> = headers
        .iter()
        .map(|h| {
            validate(super::text(&values[h])).map(|s| if s.is_empty() { "-".into() } else { s })
        })
        .collect();
    Ok(format!("| {} |", values?.join(" | ")))
}
fn matches_key(line: &str, key: &[String]) -> bool {
    let actual = cells(line);
    actual.len() >= key.len()
        && actual
            .iter()
            .zip(key)
            .all(|(a, b)| normalize(a) == normalize(b))
}
pub fn replace_row(
    text: &str,
    headers: &[String],
    key: &[String],
    values: &Value,
    title: &str,
) -> Result<String, String> {
    let new = rendered(headers, values)?;
    let mut rows = lines(text);
    let Some((start, end)) = locate(text, headers) else {
        let block = format!(
            "## {title}\n\n| {} |\n|{}\n{new}\n",
            headers.join(" | "),
            "---|".repeat(headers.len())
        );
        let base = text.trim_end_matches('\n');
        return Ok(format!(
            "{}{block}",
            if base.is_empty() {
                String::new()
            } else {
                format!("{base}\n\n")
            }
        ));
    };
    if let Some(i) = (start + 1..end).find(|i| !separator(&rows[*i]) && matches_key(&rows[*i], key))
    {
        rows[i] = new
    } else {
        rows.insert(end, new)
    }
    Ok(rows.join("\n")
        + if text.ends_with('\n') || text.is_empty() {
            "\n"
        } else {
            ""
        })
}
pub fn remove_row(text: &str, headers: &[String], key: &[String]) -> String {
    let Some((start, end)) = locate(text, headers) else {
        return text.into();
    };
    let mut rows = lines(text);
    if let Some(i) = (start + 1..end).find(|i| !separator(&rows[*i]) && matches_key(&rows[*i], key))
    {
        rows.remove(i);
    }
    rows.join("\n") + if text.ends_with('\n') { "\n" } else { "" }
}
pub fn insert_column(text: &str, headers: &[String], name: &str, at: usize, value: &str) -> String {
    let Some((start, end)) = locate(text, headers) else {
        return text.into();
    };
    let mut rows = lines(text);
    let mut h = headers.to_vec();
    let at = at.min(h.len());
    h.insert(at, name.into());
    for i in start..end {
        if i == start {
            rows[i] = format!("| {} |", h.join(" | "))
        } else if separator(&rows[i]) {
            rows[i] = format!("|{}", "---|".repeat(h.len()))
        } else {
            let mut c: Vec<_> = cells(&rows[i]).iter().map(|s| clean(s)).collect();
            while c.len() < headers.len() {
                c.push(String::new())
            }
            c.insert(at, value.into());
            rows[i] = format!(
                "| {} |",
                c.iter()
                    .map(|s| if s.is_empty() { "-" } else { s.as_str() })
                    .collect::<Vec<_>>()
                    .join(" | ")
            )
        }
    }
    rows.join("\n") + if text.ends_with('\n') { "\n" } else { "" }
}
pub fn replace_section(text: &str, title: &str, body: &str) -> String {
    let mut rows = lines(text);
    let target = normalize(title);
    let start = rows.iter().position(|l| {
        l.strip_prefix("## ")
            .is_some_and(|h| normalize(h) == target)
    });
    let mut new = vec![format!("## {title}"), String::new()];
    new.extend(lines(body.trim_end_matches('\n')));
    new.push(String::new());
    let Some(start) = start else {
        let base = text.trim_end_matches('\n');
        return format!(
            "{}{}\n",
            if base.is_empty() {
                String::new()
            } else {
                format!("{base}\n\n")
            },
            new.join("\n").trim_end_matches('\n')
        );
    };
    let mut end = start + 1;
    while end < rows.len() && !rows[end].starts_with("## ") {
        end += 1
    }
    rows.splice(start..end, new);
    rows.join("\n").trim_end_matches('\n').to_owned() + "\n"
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_markdown_contract() {
        let cases: Value = serde_json::from_str(include_str!("fixtures/markdown.json")).unwrap();
        for c in cases["cases"].as_array().unwrap() {
            let h: Vec<String> = c["headers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_owned())
                .collect();
            let k: Vec<String> = c["key"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(
                serde_json::to_value(table(c["text"].as_str().unwrap(), &h)).unwrap(),
                c["table"]
            );
            assert_eq!(
                replace_row(
                    c["text"].as_str().unwrap(),
                    &h,
                    &k,
                    &c["values"],
                    "Quem é quem"
                )
                .unwrap(),
                c["replaced"]
            );
            assert_eq!(
                remove_row(c["text"].as_str().unwrap(), &h, &k),
                c["removed"]
            );
            assert_eq!(
                insert_column(c["text"].as_str().unwrap(), &h, "vez", 1, "-"),
                c["column"]
            );
            assert_eq!(
                replace_section(c["text"].as_str().unwrap(), "Gates", "- Novo"),
                c["section"]
            );
        }
    }
}
