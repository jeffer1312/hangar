use super::*;
use serde_json::json;
use std::path::Path;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn news(tipo: &str, extra: &str, body: &str) -> String {
    format!("---\ntitulo: T {tipo}\ntipo: {tipo}\nonde: todos\n{extra}---\n\n{body}\n")
}

fn article(categoria: &str, ordem: &str, body: &str) -> String {
    format!("---\ntitulo: Artigo {ordem}\ncategoria: {categoria}\nresumo: R\nicone: book\nordem: {ordem}\nonde: desktop\n---\n\n{body}\n")
}

/// Raiz com as duas pastas, ainda vazias.
fn root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("docs").join("novidades")).unwrap();
    std::fs::create_dir_all(tmp.path().join("docs").join("manual")).unwrap();
    tmp
}

#[test]
fn summary_is_the_first_paragraph() {
    assert_eq!(summary("Linha um\nlinha dois\n\nOutro."), "Linha um\nlinha dois");
    assert_eq!(summary("  Só um.  "), "Só um.");
}

#[test]
fn more_means_text_after_the_first_paragraph() {
    assert!(!has_more("Um parágrafo só."));
    assert!(!has_more("Um parágrafo só.\n\n  \n"));
    assert!(has_more("Um.\n\nDois."));
}

#[test]
fn date_comes_from_a_valid_prefix() {
    assert_eq!(date_from_id("2026-10-26-historico"), Some("2026-10-26"));
    assert_eq!(date_from_id("2026-10-26"), Some("2026-10-26"));
    assert_eq!(date_from_id("historico"), None);
    assert_eq!(date_from_id("2026-02-30-x"), None);
    assert_eq!(date_from_id("2026-10-261"), None);
    assert_eq!(date_from_id("+026-10-26-x"), None);
}

#[test]
fn slug_drops_accents_and_punctuation() {
    assert_eq!(slug("Começar: a sessão!"), "comecar-a-sessao");
    assert_eq!(slug("  --Já  está--  "), "ja-esta");
}

/// O cartão do aviso da atualização não ganha os campos do histórico.
#[test]
fn news_item_keeps_the_update_card() {
    let (rank, item) = news_item("2026-01-01-x", &news("melhoria", "animacao: r\nmanual: guia\n", "Um.\n\nDois."), "linux").unwrap();
    assert_eq!(rank, 1);
    assert_eq!(item, json!({"id": "2026-01-01-x", "title": "T melhoria", "kind": "improvement", "text": "Um.\n\nDois.",
        "platforms": "all", "try_it": null, "animation": "r"}));
}

#[test]
fn history_orders_and_fills_the_new_fields() {
    let tmp = root();
    let root = tmp.path();
    write(root, "docs/novidades/README.md", "formato");
    write(root, "docs/novidades/sem-data.md", &news("novo", "", "Fora."));
    write(root, "docs/novidades/2026-01-01-um.md", &news("novo", "", "Só um."));
    write(root, "docs/novidades/2026-01-02-conserto.md", &news("correcao", "animacao: x\nmanual: guia\n", "Primeiro.\n\nSegundo."));
    write(root, "docs/novidades/2026-01-02-nova.md", &news("novo", "animacao: roteiro\nancora: lista\nmanual: sumido\n", "Corpo."));
    write(root, "docs/novidades/2026-01-03-linux.md", &news("melhoria", "sistema: linux\n", "Só no Linux."));
    write(root, "docs/manual/guia.md", &article("comecar", "1", "Texto."));
    assert_eq!(history(root, "windows").unwrap(), vec![
        json!({"id": "2026-01-02-nova", "date": "2026-01-02", "title": "T novo", "kind": "new", "platforms": "all", "summary": "Corpo.",
               "text": "Corpo.", "more": false, "try_it": null, "animation": "roteiro", "anchor": "lista"}),
        json!({"id": "2026-01-02-conserto", "date": "2026-01-02", "title": "T correcao", "kind": "fix", "platforms": "all", "summary": "Primeiro.",
               "text": "Primeiro.\n\nSegundo.", "more": true, "try_it": null, "animation": null, "manual": "guia"}),
        json!({"id": "2026-01-01-um", "date": "2026-01-01", "title": "T novo", "kind": "new", "platforms": "all", "summary": "Só um.",
               "text": "Só um.", "more": false, "try_it": null, "animation": null}),
    ]);
    assert_eq!(history(root, "linux").unwrap()[0]["id"], "2026-01-03-linux");
}

/// Pasta ausente é raiz errada: erro, nunca lista vazia.
#[test]
fn missing_folders_are_errors() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(history(tmp.path(), "linux").is_err());
    assert!(manual(tmp.path(), "linux").is_err());
    std::fs::create_dir_all(tmp.path().join("docs").join("manual")).unwrap();
    assert!(manual(tmp.path(), "linux").is_err(), "sem docs/novidades");
    std::fs::create_dir_all(tmp.path().join("docs").join("novidades")).unwrap();
    assert_eq!(manual(tmp.path(), "linux").unwrap(), json!({"categories": []}));
    assert_eq!(history(tmp.path(), "linux").unwrap(), Vec::<serde_json::Value>::new());
}

#[test]
fn manual_keeps_fixed_tab_order_and_drops_bad_articles() {
    let tmp = root();
    let root = tmp.path();
    write(root, "docs/manual/README.md", &article("comecar", "0", "x"));
    write(root, "docs/manual/b.md", &article("problemas", "1", "x"));
    write(root, "docs/manual/z.md", &article("comecar", "1", "x"));
    write(root, "docs/manual/a.md", &article("comecar", "1", "x"));
    write(root, "docs/manual/primeiro.md", &article("comecar", "0", "x"));
    write(root, "docs/manual/outra.md", &article("desconhecida", "0", "x"));
    write(root, "docs/manual/sem-titulo.md", "---\ncategoria: sessoes\nonde: todos\n---\n\nx\n");
    write(root, "docs/manual/Maiuscula.md", &article("sessoes", "0", "x"));
    let ids: Vec<(String, Vec<String>)> = manual(root, "linux").unwrap()["categories"].as_array().unwrap().iter()
        .map(|c| (c["id"].as_str().unwrap().to_owned(), c["articles"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_owned()).collect()))
        .collect();
    assert_eq!(ids, vec![
        ("comecar".to_owned(), vec!["primeiro".to_owned(), "a".to_owned(), "z".to_owned()]),
        ("problemas".to_owned(), vec!["b".to_owned()]),
    ]);
}

#[test]
fn article_with_intro_sections_and_linked_news() {
    let tmp = root();
    let root = tmp.path();
    write(root, "docs/manual/guia.md", "---\ntitulo: Guia\ncategoria: sessoes\nresumo: R\nicone: book\nordem: 2\nonde: celular\nancora: lista\n---\n\nIntro.\n\n## Primeiro passo\n\nFaça.\n\n```md\n## Não conta\n```\n\n### Nem esta\n\n## Já está\n\nPronto.\n");
    write(root, "docs/manual/simples.md", &article("sessoes", "9", "Sem seção."));
    write(root, "docs/novidades/2026-01-01-velha.md", &news("novo", "manual: guia\n", "A."));
    write(root, "docs/novidades/2026-01-05-nova.md", &news("melhoria", "manual: guia\n", "B."));
    write(root, "docs/novidades/2026-01-06-outra.md", &news("novo", "", "C."));
    assert_eq!(manual(root, "linux").unwrap(), json!({"categories": [{"id": "sessoes", "articles": [
        {"id": "guia", "title": "Guia", "summary": "R", "icon": "book", "platforms": "mobile", "anchor": "lista", "intro": "Intro.",
         "sections": [
             {"id": "primeiro-passo", "title": "Primeiro passo", "markdown": "Faça.\n\n```md\n## Não conta\n```\n\n### Nem esta"},
             {"id": "ja-esta", "title": "Já está", "markdown": "Pronto."},
         ],
         "news": [{"id": "2026-01-05-nova", "date": "2026-01-05", "title": "T melhoria"}, {"id": "2026-01-01-velha", "date": "2026-01-01", "title": "T novo"}]},
        {"id": "simples", "title": "Artigo 9", "summary": "R", "icon": "book", "platforms": "desktop", "intro": "Sem seção.",
         "sections": [], "news": []},
    ]}]}));
}

#[test]
fn section_ids_are_unique_and_never_empty_even_with_bom_and_crlf() {
    let tmp = root();
    let root = tmp.path();
    let text = "\u{feff}---\r\ntitulo: Guia\r\ncategoria: problemas\r\nonde: todos\r\n---\r\n\r\n## Passo\r\nA.\r\n## Passo\r\nB.\r\n## ???\r\nC.\r\n## 🙂\r\nD.\r\n";
    write(root, "docs/manual/guia.md", text);
    let sections = &manual(root, "linux").unwrap()["categories"][0]["articles"][0]["sections"];
    let ids: Vec<&str> = sections.as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["passo", "passo-2", "secao", "secao-2"]);
    assert_eq!(sections[1]["markdown"], "B.");
}
