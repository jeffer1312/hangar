//! Os documentos que o Hangar mostra na tela: as novidades de `docs/novidades/` (aviso da
//! atualização e histórico) e os artigos do manual em `docs/manual/`.
#![forbid(unsafe_code)]

mod manual;
mod news;

pub use manual::{manual, slug};
pub use news::{date_from_id, has_more, history, history_item, news_item, summary};

use std::collections::HashMap;

/// Espelho do `_frontmatter` de `backend/app/atualizacoes.py`: `chave: valor` entre `---`.
#[must_use]
pub fn frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().is_none_or(|l| l.trim() != "---") { return (HashMap::new(), text.to_owned()); }
    let Some(end) = lines.iter().skip(1).position(|l| *l == "---").map(|i| i + 1) else { return (HashMap::new(), text.to_owned()) };
    let fields = lines[1..end].iter()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    (fields, lines[end + 1..].join("\n").trim().to_owned())
}

/// `*.md` que não é o README da pasta.
#[must_use]
pub fn is_note_file(name: &str) -> bool { name.ends_with(".md") && !name.to_uppercase().starts_with("README") }

/// Id de artigo do manual: vira nome de arquivo, então só `[a-z0-9-]{1,64}`.
fn valid_article_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `onde` do frontmatter no nome da API; valor desconhecido = arquivo fora.
fn platforms(onde: Option<&String>) -> Option<&'static str> {
    match onde.map(String::as_str)? { "desktop" => Some("desktop"), "celular" => Some("mobile"), "todos" => Some("all"), _ => None }
}

#[cfg(test)]
mod tests;
