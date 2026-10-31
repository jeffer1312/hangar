// crates/hangar-server/src/search/hits.rs
//! A busca por palavra (`search.search` do Python): o índice responde quando está pronto; antes disso,
//! ou com o índice em erro, uma varredura dos transcripts no próprio processo com os tetos do antigo
//! `rg`. Os dois caminhos usam o mesmo parser e o mesmo filtro de reescrita, então devolvem os
//! mesmos trechos com os mesmos ids do histórico.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

use serde::Serialize;
use serde_json::Value;

use super::{Account, Index, Row, fold, ingest};
use crate::transcript::py;

pub(crate) const MAX_HITS: usize = 50;
const PER_FILE: usize = 3;
const MAX_QUERY_CHARS: usize = 200;
const SNIPPET_CHARS: usize = 240;
// Contexto injetado e saída de ferramenta casam muito mais que a conversa: a varredura de um arquivo
// para depois de tantas linhas com o termo, como o `rg -m` fazia.
const MAX_MATCHED_LINES: usize = 200;
const MAX_FILE_BYTES: i64 = 64 * 1024 * 1024;

/// Um trecho achado: os 10 campos do Python mais agente e conta (N1).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    pub project: String,
    pub session_id: String,
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub line: String,
    pub mtime: f64,
    pub live: bool,
    pub role: Option<String>,
    pub event_id: Option<String>,
    pub ts: Option<f64>,
    pub provider: String,
    pub conta: String,
    pub config_dir: Option<String>,
    pub codex_account: Option<String>,
}

/// Palavras da busca: minúsculas, cortadas em 200 caracteres, sem repetir, na ordem.
pub(crate) fn terms(q: &str) -> Vec<String> {
    let lower: String = q.to_lowercase().chars().take(MAX_QUERY_CHARS).collect();
    let mut out: Vec<String> = Vec::new();
    for word in lower.split(py::is_space).filter(|w| !w.is_empty()) {
        if !out.iter().any(|o| o == word) { out.push(word.to_owned()); }
    }
    out
}

/// Janela legível em volta do 1º termo: sem quebras, sem cortar palavra, `…` no lado cortado.
pub(crate) fn snippet(text: &str, terms: &[String]) -> String {
    let joined = text.split(py::is_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() <= SNIPPET_CHARS { return joined; }
    // Como o Python: posição achada no texto em minúsculas, aplicada ao original.
    let i = terms.first().and_then(|t| find(&joined.to_lowercase().chars().collect::<Vec<_>>(), t)).unwrap_or(0);
    let mut start = i.saturating_sub(60);
    if start > 0 {
        if let Some(space) = (start..chars.len()).find(|&k| chars[k] == ' ').filter(|&s| s < i) { start = space + 1; }
    }
    let mut end = start + SNIPPET_CHARS;
    if end < chars.len() {
        if let Some(space) = (start..end).rev().find(|&k| chars[k] == ' ').filter(|&s| s > i) { end = space; }
    }
    let body: String = chars[start..end.min(chars.len())].iter().collect();
    format!("{}{body}{}", if start > 0 { "…" } else { "" }, if end < chars.len() { "…" } else { "" })
}

fn find(hay: &[char], needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() { return Some(0); }
    hay.windows(needle.len()).position(|w| w == needle.as_slice())
}

/// Trechos de `q` em todas as contas Claude, conversa mais nova primeiro, até `limit`. `live`:
/// realpath do transcript → nome da sessão viva. Erro: o catálogo de contas não pôde ser lido.
pub(crate) fn search(index: &Index, q: &str, live: &HashMap<String, String>, limit: usize) -> Result<Vec<Hit>, String> {
    let terms = terms(q);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let hits = match index.search(&terms, limit) {
        Ok(Some(rows)) => from_rows(rows, &terms, live),
        Ok(None) => scan(&index.accounts()?, &terms, live, limit),
        Err(e) => {
            index.report("busca.indice_falhou", e.to_string());
            scan(&index.accounts()?, &terms, live, limit)
        }
    };
    Ok(without_repeats(hits))
}

/// O mesmo trecho em duas pastas de projeto (sessão que mudou de pasta) fica uma vez, o mais novo.
fn without_repeats(hits: Vec<Hit>) -> Vec<Hit> {
    let mut seen = HashSet::new();
    hits.into_iter().filter(|h| seen.insert((h.session_id.clone(), h.event_id.clone()))).collect()
}

/// `os.path.getmtime`; `None` = o arquivo sumiu desde que foi visto.
fn mtime_of(path: &str) -> Option<f64> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9,
        Err(e) => -(e.duration().as_secs() as f64 + f64::from(e.duration().subsec_nanos()) * 1e-9),
    })
}

fn by_mtime(hits: &mut [Hit]) {
    hits.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
}

struct Source<'a> { path: &'a str, project: &'a str, session_id: &'a str, cwd: Option<String>, mtime: f64,
    config_dir: Option<String>, account: String }

fn hit(src: &Source, live: &HashMap<String, String>, terms: &[String], (event_id, role, ts, text): ingest::Message) -> Hit {
    let session_name = live.get(&crate::list::discover::real(src.path)).cloned();
    Hit {
        project: src.project.to_owned(), session_id: src.session_id.to_owned(), live: session_name.is_some(), session_name,
        cwd: src.cwd.clone(), line: snippet(&text, terms), mtime: src.mtime, role: Some(role.to_owned()),
        event_id: Some(event_id), ts, provider: "claude".into(), conta: src.account.clone(),
        config_dir: src.config_dir.clone(), codex_account: None,
    }
}

fn from_rows(rows: Vec<Row>, terms: &[String], live: &HashMap<String, String>) -> Vec<Hit> {
    let mut mtimes: HashMap<String, Option<f64>> = HashMap::new();
    let mut hits = Vec::new();
    for row in rows {
        let Some(mtime) = *mtimes.entry(row.path.clone()).or_insert_with(|| mtime_of(&row.path)) else { continue };
        let src = Source { path: &row.path, project: &row.project, session_id: &row.session_id, cwd: row.cwd.clone(), mtime,
            config_dir: row.config_dir.clone(), account: row.account.clone() };
        let role: &'static str = if row.role == "user" { "user" } else { "assistant" };
        hits.push(hit(&src, live, terms, (row.event_id, role, row.ts, row.text)));
    }
    by_mtime(&mut hits);
    hits
}

/// A varredura de antes do índice pronto: arquivo mais novo primeiro, até 3 por arquivo e `limit` no total.
fn scan(accounts: &[Account], terms: &[String], live: &HashMap<String, String>, limit: usize) -> Vec<Hit> {
    let (files, _) = ingest::conversations(accounts);
    let mut order: Vec<_> = files.iter().filter(|(_, f)| f.stat.size <= MAX_FILE_BYTES).collect();
    order.sort_by(|a, b| b.1.stat.mtime_ns.cmp(&a.1.stat.mtime_ns).then(a.0.cmp(b.0)));
    let folded: Vec<String> = terms.iter().map(|t| fold(t)).collect();
    // O primeiro dos mais longos, como o `max(t, key=len)` que escolhia o termo do `rg`.
    let longest = (1..terms.len()).fold(0, |best, k| if terms[k].chars().count() > terms[best].chars().count() { k } else { best });
    let longest = &folded[longest];
    let prefixes = ingest::internal_prefixes();
    let mut hits = Vec::new();
    for (path, found) in order {
        if hits.len() >= limit { break; }
        let scanned = match scan_file(path, &folded, longest, &prefixes) {
            Ok(Some(scanned)) => scanned,
            Ok(None) => continue,
            Err(e) => { tracing::debug!(path = %path, error = %e, "busca: transcript ilegível na varredura"); continue }
        };
        let Some(mtime) = mtime_of(path) else { continue };
        let src = Source { path, project: &found.project, session_id: &found.sid, cwd: scanned.cwd, mtime,
            config_dir: found.account.config_dir.clone(), account: ingest::account_key(found.account) };
        let room = limit - hits.len();
        hits.extend(scanned.messages.into_iter().take(room).map(|m| hit(&src, live, terms, m)));
    }
    by_mtime(&mut hits);
    hits
}

struct Scanned { messages: Vec<ingest::Message>, cwd: Option<String> }

/// Mensagens do arquivo com todos os termos; `None` = conversa interna do Hangar ou sem achado.
fn scan_file(path: &str, terms: &[String], longest: &str, prefixes: &[String]) -> std::io::Result<Option<Scanned>> {
    let mut rd = BufReader::new(File::open(path)?);
    let mut st = ingest::FileState::default();
    let mut messages: Vec<ingest::Message> = Vec::new();
    let mut held = HashSet::new();
    let mut matched_lines = 0;
    // cwd da 1ª linha casada; sem ele, o do cabeçalho (como o Python).
    let mut line_cwd: Option<String> = None;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if rd.read_until(b'\n', &mut raw)? == 0 { break; }
        let open = matched_lines < MAX_MATCHED_LINES;
        if raw.len() <= ingest::MAX_LINE_BYTES && fold(&String::from_utf8_lossy(&raw)).contains(longest) {
            matched_lines += 1;
        }
        for msg in ingest::absorb_line(&mut st, &raw, prefixes) {
            // Cada reenvio de prompt barrado repete o `held:`; o histórico leva um só.
            if (msg.0.starts_with("held:") || msg.0.starts_with("delivery:")) && !held.insert(msg.0.clone()) { continue; }
            if open && messages.len() < PER_FILE {
                let text = fold(&msg.3);
                if terms.iter().all(|t| text.contains(t.as_str())) {
                    if messages.is_empty() { line_cwd = cwd_of(&raw); }
                    messages.push(msg);
                }
            }
        }
        let collected = messages.len() >= PER_FILE || matched_lines >= MAX_MATCHED_LINES;
        if collected && (messages.is_empty() || (st.internal.is_some() && (line_cwd.is_some() || st.head_done))) { break; }
    }
    if messages.is_empty() || st.internal == Some(true) {
        return Ok(None);
    }
    Ok(Some(Scanned { messages, cwd: line_cwd.or(st.cwd) }))
}

fn cwd_of(raw: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    value.get("cwd")?.as_str().filter(|c| !c.is_empty()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(name: &str) -> serde_json::Map<String, Value> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/search/contract/search/{name}.json"));
        let value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        value["data"].as_object().unwrap().clone()
    }

    #[test]
    fn search_terms_like_the_python() {
        // SRCH-01 e SRCH-02: `termos()` da captura.
        for (q, want) in capture("termos") {
            let want: Vec<String> = serde_json::from_value(want).unwrap();
            assert_eq!(terms(&q), want, "q={q:?}");
        }
    }

    #[test]
    fn search_snippet_like_the_python() {
        // SRCH-14, 15 e 16: `_snippet(texto, ["meio"])` da captura.
        for (text, want) in capture("snippet") {
            assert_eq!(snippet(&text, &["meio".to_owned()]), want.as_str().unwrap(), "texto={text:?}");
        }
        assert_eq!(snippet(&"x ".repeat(200), &["ausente".to_owned()]).chars().next(), Some('x'), "termo não achado começa em 0");
    }
}
