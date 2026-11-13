//! Novidades: o cartão do aviso da atualização e o histórico completo.
use crate::{frontmatter, is_note_file, platforms, valid_article_id};
use serde_json::{Value, json};
use std::io;
use std::path::Path;

/// `sistema` vazio = todos; nome fora de `linux`/`windows`/`macos` = arquivo ignorado, como o `onde`.
/// A mesma regra do `_for_system` do `scripts/montar-pacote.py`.
fn for_system(field: Option<&String>, os: &str) -> Option<bool> {
    let names: Vec<&str> = field.map(String::as_str).unwrap_or_default().split(|c: char| c == ',' || c.is_whitespace()).filter(|n| !n.is_empty()).collect();
    if names.iter().any(|n| !matches!(*n, "linux" | "windows" | "macos")) { return None; }
    Some(names.is_empty() || names.contains(&os))
}

/// O cartão do aviso da atualização (`GET /api/update/notes`, `update-news.json`) e a ordem do tipo
/// (novo, melhoria, correção). `os`: o sistema da máquina, nos nomes do `std::env::consts::OS`.
#[must_use]
pub fn news_item(id: &str, text: &str, os: &str) -> Option<(u8, Value)> {
    let (f, body) = frontmatter(text);
    let title = f.get("titulo").filter(|t| !t.is_empty())?;
    if !for_system(f.get("sistema"), os)? { return None; }
    let (rank, kind) = match f.get("tipo").map(String::as_str)? { "novo" => (0, "new"), "melhoria" => (1, "improvement"), "correcao" => (2, "fix"), _ => return None };
    let platforms = platforms(f.get("onde"))?;
    let opt = |k: &str| f.get(k).filter(|v| !v.is_empty()).cloned();
    // Correção não ganha animação: não há gesto novo a mostrar.
    let animation = if kind == "fix" { None } else { opt("animacao") };
    let mut item = json!({"id": id, "title": title, "kind": kind, "text": body, "platforms": platforms, "try_it": opt("experimente"), "animation": animation});
    // Ausente sai sem a chave: o JSON de quem não usa o campo continua o de antes.
    if let Some(anchor) = opt("ancora") { item["anchor"] = Value::String(anchor); }
    Some((rank, item))
}

/// O item do histórico: o de `news_item` mais `date`, `summary`, `more` e, quando o artigo
/// `root/docs/manual/<manual>.md` existe, `manual`. Sem data no nome, fica fora.
#[must_use]
pub fn history_item(root: &Path, id: &str, text: &str, os: &str) -> Option<(u8, Value)> {
    let date = date_from_id(id)?;
    let (rank, mut item) = news_item(id, text, os)?;
    let body = item["text"].as_str().unwrap_or_default().to_owned();
    item["date"] = date.into();
    item["summary"] = summary(&body).into();
    item["more"] = has_more(&body).into();
    let (fields, _) = frontmatter(text);
    let manual = root.join("docs").join("manual");
    if let Some(article) = fields.get("manual").filter(|m| valid_article_id(m) && manual.join(format!("{m}.md")).is_file()) {
        item["manual"] = article.as_str().into();
    }
    Some((rank, item))
}

/// O primeiro parágrafo do corpo: o texto até a primeira linha em branco.
#[must_use]
pub fn summary(body: &str) -> String {
    body.trim_start().lines().take_while(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n").trim().to_owned()
}

/// Há texto depois do primeiro parágrafo. Se o cartão abre é o cliente que decide, somando a
/// animação que ele conhece.
#[must_use]
pub fn has_more(body: &str) -> bool {
    body.trim_start().lines().skip_while(|l| !l.trim().is_empty()).any(|l| !l.trim().is_empty())
}

/// A data `AAAA-MM-DD` que abre o nome do arquivo; sem ela a novidade fica fora do histórico.
#[must_use]
pub fn date_from_id(id: &str) -> Option<&str> {
    let date = id.get(..10)?;
    if !matches!(id.as_bytes().get(10), None | Some(b'-')) { return None; }
    // O chrono aceitaria sinal e dígito a menos; aqui é exatamente AAAA-MM-DD.
    if !date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }) { return None; }
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date)
}

/// Todas as novidades de `root/docs/novidades` que valem para `os`: mais nova primeiro, depois
/// novo, melhoria e correção, depois o id.
///
/// # Errors
/// Pasta ausente (raiz errada) ou falha ao listar ou ler um arquivo.
pub fn history(root: &Path, os: &str) -> io::Result<Vec<Value>> {
    let mut news = Vec::new();
    for entry in std::fs::read_dir(root.join("docs").join("novidades"))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if !is_note_file(&name) { continue; }
        let id = name.trim_end_matches(".md");
        if date_from_id(id).is_none() { continue; }
        if let Some((rank, item)) = history_item(root, id, &std::fs::read_to_string(entry.path())?, os) {
            news.push((item["date"].as_str().unwrap_or_default().to_owned(), rank, id.to_owned(), item));
        }
    }
    news.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    Ok(news.into_iter().map(|n| n.3).collect())
}
