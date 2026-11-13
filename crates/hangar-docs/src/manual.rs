//! O manual: `docs/manual/<id>.md`, agrupados nas abas fixas, lido de uma vez.
use crate::{at, frontmatter, history, is_note_file, platforms, valid_article_id};
use serde_json::{Value, json};
use std::io;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

/// As abas na ordem da tela.
const CATEGORIES: [&str; 4] = ["comecar", "sessoes", "ferramentas", "problemas"];

struct Article {
    id: String,
    title: String,
    category: usize,
    summary: String,
    icon: Option<String>,
    platforms: &'static str,
    order: i64,
    anchor: Option<String>,
    body: String,
}

/// Sem título, com categoria ou `onde` desconhecidos, o artigo fica fora; o `Err` diz o motivo.
fn parse(id: &str, text: &str) -> Result<Article, &'static str> {
    // Editor do Windows salva com BOM, e com ele o `---` da primeira linha não casa.
    let (f, body) = frontmatter(text.strip_prefix('\u{feff}').unwrap_or(text));
    let opt = |k: &str| f.get(k).filter(|v| !v.is_empty()).cloned();
    Ok(Article {
        id: id.to_owned(),
        title: opt("titulo").ok_or("sem titulo")?,
        category: CATEGORIES.iter().position(|c| f.get("categoria").is_some_and(|v| v == c)).ok_or("categoria invalida")?,
        summary: opt("resumo").unwrap_or_default(),
        icon: opt("icone"),
        platforms: platforms(f.get("onde")).ok_or("onde invalido")?,
        // Sem `ordem` legível, vai para o fim da aba.
        order: f.get("ordem").and_then(|o| o.parse().ok()).unwrap_or(i64::MAX),
        anchor: opt("ancora"),
        body,
    })
}

/// O manual inteiro: as abas na ordem fixa (aba vazia não aparece), os artigos por `ordem` e id,
/// cada um com a introdução, as seções (`## `) e as novidades que apontam para ele (as que valem
/// para `os`, mais nova primeiro).
///
/// # Errors
/// `docs/manual` ou `docs/novidades` ausente (raiz errada) ou falha ao listar uma delas. Arquivo
/// ilegível fica fora, com aviso no log.
pub fn manual(root: &Path, os: &str) -> io::Result<Value> {
    let mut articles = Vec::new();
    let dir = root.join("docs").join("manual");
    for entry in std::fs::read_dir(&dir).map_err(at(&dir))? {
        let entry = entry.map_err(at(&dir))?;
        let path = entry.path();
        if !entry.file_type().map_err(at(&path))?.is_file() { continue; }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let id = name.trim_end_matches(".md");
        if !is_note_file(&name) || !valid_article_id(id) { continue; }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => { tracing::warn!(file = %path.display(), %error, "artigo do manual ilegivel; fica fora do manual"); continue; }
        };
        match parse(id, &text) {
            Ok(article) => articles.push(article),
            Err(reason) => tracing::warn!(file = %path.display(), reason, "artigo do manual fora do formato; fica fora do manual"),
        }
    }
    articles.sort_by(|a, b| (a.category, a.order, &a.id).cmp(&(b.category, b.order, &b.id)));
    let news = history(root, os)?;
    let categories: Vec<Value> = CATEGORIES.iter().enumerate().filter_map(|(index, category)| {
        let list: Vec<Value> = articles.iter().filter(|a| a.category == index).map(|a| article_json(a, &news)).collect();
        (!list.is_empty()).then(|| json!({"id": category, "articles": list}))
    }).collect();
    Ok(json!({"categories": categories}))
}

fn article_json(article: &Article, news: &[Value]) -> Value {
    let (intro, sections) = split_sections(&article.body);
    let news: Vec<Value> = news.iter().filter(|n| n["manual"] == article.id.as_str())
        .map(|n| json!({"id": n["id"], "date": n["date"], "title": n["title"], "platforms": n["platforms"]}))
        .collect();
    // Dois títulos iguais não podem dar o mesmo id: "Nesta página" rolaria sempre para o primeiro.
    let mut seen: Vec<String> = Vec::new();
    let sections: Vec<Value> = sections.into_iter().map(|(title, markdown)| {
        // Título só com símbolos ou em outro alfabeto dá slug vazio, e o "Nesta página" não acharia o bloco.
        let base = Some(slug(&title)).filter(|s| !s.is_empty()).unwrap_or_else(|| "secao".to_owned());
        let id = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| !seen.contains(id)).unwrap_or(base);
        seen.push(id.clone());
        json!({"id": id, "title": title, "markdown": markdown})
    }).collect();
    let mut item = json!({
        "id": article.id, "title": article.title, "summary": article.summary, "icon": article.icon, "platforms": article.platforms,
        "intro": intro, "sections": sections, "news": news,
    });
    if let Some(anchor) = &article.anchor { item["anchor"] = anchor.as_str().into(); }
    item
}

/// O texto antes do primeiro `## ` e cada seção (título, texto), com `## ` dentro de bloco de
/// código cercado contando como texto.
fn split_sections(body: &str) -> (String, Vec<(String, String)>) {
    let mut fence: Option<char> = None;
    let mut intro: Vec<&str> = Vec::new();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        let marker = ['`', '~'].into_iter().find(|c| trimmed.starts_with(&c.to_string().repeat(3)));
        let heading = match (fence, marker) {
            (None, Some(c)) => { fence = Some(c); None }
            (Some(open), Some(c)) if open == c => { fence = None; None }
            (Some(_), _) => None,
            (None, None) => line.strip_prefix("## ").map(str::trim).filter(|t| !t.is_empty()),
        };
        if let Some(title) = heading {
            sections.push((title.to_owned(), Vec::new()));
        } else if let Some((_, lines)) = sections.last_mut() {
            lines.push(line);
        } else {
            intro.push(line);
        }
    }
    let text = |lines: Vec<&str>| lines.join("\n").trim().to_owned();
    (text(intro), sections.into_iter().map(|(title, lines)| (title, text(lines))).collect())
}

/// Âncora de seção: minúsculas sem acento; o que não é letra ou número vira um `-` só.
#[must_use]
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() { out.push(c); } else if !out.ends_with('-') { out.push('-'); }
    }
    out.trim_matches('-').to_owned()
}
