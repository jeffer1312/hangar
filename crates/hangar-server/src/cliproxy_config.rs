//! Chave de cliente usada na configuração do Jev, independente do endereço do proxy.
use crate::accounts::{environment::AccountEnvironment, quotas::other::py_strip};
use std::sync::LazyLock;

/// Conserva o leitor por linhas do RuntimeConfig; host, porta e YAML fora da lista não participam.
pub fn client_key(env: &AccountEnvironment) -> Option<String> {
    let text = match std::fs::read_to_string(env.home.join(".cli-proxy-api/config.yaml")) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(kind=?error.kind(), "Configuração de chave CLIProxy ilegível");
            }
            return None;
        }
    };
    line_keys(&text).into_iter().next()
}

fn line_keys(text: &str) -> Vec<String> {
    static HEAD: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^([\s\x1c-\x1f]*)api-keys[\s\x1c-\x1f]*:[\s\x1c-\x1f]*$").unwrap()
    });
    static ENTRY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r##"^([\s\x1c-\x1f]*)-[\s\x1c-\x1f]*(?:"([^"]*)"|'([^']*)'|([^#\s\x1c-\x1f]+))[\s\x1c-\x1f]*(#.*)?$"##).unwrap()
    });
    let lines: Vec<_> = text
        .split([
            '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
            '\u{2029}',
        ])
        .collect();
    let (mut root, mut access) = (None, None);
    let mut section = "";
    for (index, line) in lines.iter().enumerate() {
        if line
            .chars()
            .next()
            .is_some_and(|c| c != '#' && !c.is_whitespace() && !('\u{1c}'..='\u{1f}').contains(&c))
        {
            section = py_strip(line.split(':').next().unwrap_or(""));
        }
        let Some(head) = HEAD.captures(line) else {
            continue;
        };
        let indent = head[1].chars().count();
        if indent > 0 && section != "access" {
            continue;
        }
        let mut keys = Vec::new();
        for line in &lines[index + 1..] {
            let stripped = py_strip(line);
            if stripped.is_empty() || stripped.starts_with('#') {
                continue;
            }
            let Some(entry) = ENTRY.captures(line) else {
                break;
            };
            if entry[1].chars().count() < indent {
                break;
            }
            let key = (2..=4)
                .find_map(|i| entry.get(i))
                .map(|m| py_strip(m.as_str()))
                .unwrap_or("");
            if !key.is_empty() {
                keys.push(key.to_owned());
            }
        }
        if !keys.is_empty() {
            let target = if indent > 0 { &mut access } else { &mut root };
            if target.is_none() {
                *target = Some(keys);
            }
        }
    }
    access.or(root).unwrap_or_default()
}
