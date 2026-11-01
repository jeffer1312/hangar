// crates/hangar-server/src/search/hits.rs
//! A busca por palavra (`search.search` do Python): o índice responde quando está pronto; antes disso,
//! ou com o índice em erro, uma varredura dos transcripts no próprio processo com os tetos do antigo
//! `rg`. Os dois caminhos usam o mesmo parser e o mesmo filtro de reescrita, então devolvem os
//! mesmos trechos com os mesmos ids do histórico.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::LazyLock;

use memchr::memmem;
use rayon::prelude::*;
use serde::Serialize;

use super::{Account, Index, Row, fold, ingest};
use crate::session_open::AgentProvider;
use crate::transcript::py;

pub(crate) const MAX_HITS: usize = 50;
const PER_FILE: usize = 3;
const MAX_QUERY_CHARS: usize = 200;
const SNIPPET_CHARS: usize = 240;
// Contexto injetado e saída de ferramenta casam muito mais que a conversa: a varredura de um arquivo
// para depois de tantas linhas com o termo, como o `rg -m` fazia.
const MAX_MATCHED_LINES: usize = 200;
const MAX_FILE_BYTES: i64 = 64 * 1024 * 1024;
// Arquivos lidos juntos; o lote seguinte só começa se o teto ainda não fechou.
const SCAN_BATCH: usize = 16;

// Metade dos núcleos, até 4: a passada do índice segue no seu ritmo enquanto a varredura responde.
static POOL: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).div_ceil(2).min(4);
    rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("busca-{i}")).build().ok()
});

// Trechos que o parser apaga ou monta (lembrete, colado, imagem, recado com o nome da sessão, colega,
// ANSI do Pi): o texto da mensagem tem o que a linha crua não tem contínuo, e só o parser da linha diz.
const REWRITTEN: [&[u8]; 7] = [b"<system-reminder", b"<pasted_content", b"[image #", b"<teammate-message",
    b"<cross-session-message", b"\"peer\"", b"\x1b"];

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
    // Posição achada no texto em minúsculas, levada ao caractere do original: minúscula que alarga
    // (`İ` vira dois) deixaria o índice além do fim.
    let origin: Vec<usize> = chars.iter().enumerate().flat_map(|(k, c)| std::iter::repeat_n(k, c.to_lowercase().count())).collect();
    let i = terms.first().and_then(|t| find(&joined.to_lowercase().chars().collect::<Vec<_>>(), t))
        .and_then(|at| origin.get(at).copied()).unwrap_or(0);
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

/// Os termos dobrados, prontos para o `memmem`.
pub(crate) struct Needles(Vec<memmem::Finder<'static>>);

impl Needles {
    pub(crate) fn new(folded: &[String]) -> Self {
        Self(folded.iter().map(|t| memmem::Finder::new(t.as_bytes()).into_owned()).collect())
    }
}

/// Pré-filtro da linha crua: `false` só quando nenhuma mensagem dela pode ter todos os termos. A linha
/// comum não passa pelo parser; a que ele reescreve passa, sozinha. `buf` é reaproveitado entre as linhas.
pub(crate) fn may_match(raw: &[u8], agent: Option<AgentProvider>, needles: &Needles, buf: &mut Vec<u8>) -> bool {
    // Linha acima do teto não vira mensagem (`absorb_events`).
    if raw.len() > ingest::MAX_LINE_BYTES { return false; }
    if fold_json(raw, buf) {
        if needles.0.iter().all(|n| n.find(buf).is_some()) { return true; }
        let rewritten = REWRITTEN.iter().any(|m| memmem::find(buf, m).is_some())
            // O Codex junta os blocos de texto sem separador.
            || memmem::find_iter(buf, b"_text\"").nth(1).is_some();
        if !rewritten { return false; }
    }
    // Estado novo: o filtro de reescrita só tira mensagens e o cabeçalho não muda o texto delas.
    let mut st = ingest::FileState::default();
    let msgs = match agent {
        Some(provider) => ingest::absorb_agent_line(&mut st, raw, provider, &[]),
        None => ingest::absorb_line(&mut st, raw, &[]),
    };
    msgs.iter().any(|m| { let text = fold(&m.3); needles.0.iter().all(|n| n.find(text.as_bytes()).is_some()) })
}

/// `fold` da linha inteira com os escapes do JSON resolvidos: o texto de cada string aparece nela
/// contínuo, como o parser o entrega. `false`: surrogate solto, que o parser troca por outro caractere.
fn fold_json(raw: &[u8], out: &mut Vec<u8>) -> bool {
    out.clear();
    let text = String::from_utf8_lossy(raw);
    let b = text.as_bytes();
    let hex = |at: usize| b.get(at..at + 4).filter(|h| h.iter().all(u8::is_ascii_hexdigit))
        .and_then(|h| u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
    let mut i = 0;
    while i < b.len() {
        // ASCII sem escape, a maior parte da linha: copiado em bloco.
        let plain = b[i..].iter().position(|&c| c == b'\\' || !c.is_ascii()).unwrap_or(b.len() - i);
        out.extend(b[i..i + plain].iter().map(u8::to_ascii_lowercase));
        i += plain;
        if i == b.len() { break; }
        let c = match (b[i], b.get(i + 1)) {
            (b'\\', Some(b'u')) => match hex(i + 2) {
                Some(hi @ 0xD800..=0xDBFF) => match (b.get(i + 6..i + 8), hex(i + 8)) {
                    (Some(br"\u"), Some(lo @ 0xDC00..=0xDFFF)) => {
                        let c = char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00));
                        i += 12;
                        c
                    }
                    _ => return false,
                },
                Some(0xDC00..=0xDFFF) => return false,
                Some(code) => { i += 6; char::from_u32(code) }
                None => { i += 1; Some('\\') }
            },
            (b'\\', Some(&e)) if e.is_ascii() => {
                i += 2;
                Some(match e { b'n' => '\n', b't' => '\t', b'r' => '\r', b'b' => '\u{8}', b'f' => '\u{c}', _ => e as char })
            }
            (b'\\', _) => { i += 1; Some('\\') }
            _ => {
                let c = text[i..].chars().next();
                i += c.map_or(1, char::len_utf8);
                c
            }
        };
        match c {
            Some(c) if c.is_ascii() => out.push(c.to_ascii_lowercase() as u8),
            Some(c) => unicode_normalization::char::decompose_canonical(c, |d| {
                if !unicode_normalization::char::is_combining_mark(d) {
                    for l in d.to_lowercase() { out.extend_from_slice(l.encode_utf8(&mut [0; 4]).as_bytes()); }
                }
            }),
            None => {}
        }
    }
    true
}

/// Trechos de `q` em todas as contas Claude e nos outros agentes, conversa mais nova primeiro, até `limit`. `live`:
/// realpath do transcript → nome da sessão viva. Erro: o catálogo de contas não pôde ser lido.
pub(crate) fn search(index: &Index, q: &str, live: &HashMap<String, String>, limit: usize) -> Result<Vec<Hit>, String> {
    let terms = terms(q);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let hits = match index.search(&terms, limit) {
        Ok(Some(rows)) => from_rows(rows, &terms, live),
        Ok(None) => scan(index, &index.accounts()?, &terms, live, limit),
        Err(e) => {
            index.report("busca.indice_falhou", e.to_string());
            scan(index, &index.accounts()?, &terms, live, limit)
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
    provider: &'a str, config_dir: Option<String>, account: String, codex_account: Option<String> }

fn hit(src: &Source, live: &HashMap<String, String>, terms: &[String], (event_id, role, ts, text): ingest::Message) -> Hit {
    let session_name = live.get(&crate::list::discover::real(src.path)).cloned();
    Hit {
        project: src.project.to_owned(), session_id: src.session_id.to_owned(), live: session_name.is_some(), session_name,
        cwd: src.cwd.clone(), line: snippet(&text, terms), mtime: src.mtime, role: Some(role.to_owned()),
        event_id: Some(event_id), ts, provider: src.provider.to_owned(), conta: src.account.clone(),
        config_dir: src.config_dir.clone(), codex_account: src.codex_account.clone(),
    }
}

fn from_rows(rows: Vec<Row>, terms: &[String], live: &HashMap<String, String>) -> Vec<Hit> {
    let mut mtimes: HashMap<String, Option<f64>> = HashMap::new();
    let mut hits = Vec::new();
    for row in rows {
        let Some(mtime) = *mtimes.entry(row.path.clone()).or_insert_with(|| mtime_of(&row.path)) else { continue };
        let src = Source { path: &row.path, project: &row.project, session_id: &row.session_id, cwd: row.cwd.clone(), mtime,
            provider: &row.provider, config_dir: row.config_dir.clone(), account: row.account.clone(), codex_account: row.codex_account.clone() };
        let role: &'static str = if row.role == "user" { "user" } else { "assistant" };
        hits.push(hit(&src, live, terms, (row.event_id, role, row.ts, row.text)));
    }
    by_mtime(&mut hits);
    hits
}

/// A varredura de antes do índice pronto: arquivo mais novo primeiro, até 3 por arquivo e `limit` no total.
fn scan(index: &Index, accounts: &[Account], terms: &[String], live: &HashMap<String, String>, limit: usize) -> Vec<Hit> {
    scan_with(index, accounts, terms, live, limit, true)
}

/// `fast`: pré-filtro da linha crua e lotes de arquivos em paralelo; sem ele, um arquivo por vez com o
/// parser em toda linha (a referência dos testes de equivalência).
pub(super) fn scan_with(index: &Index, accounts: &[Account], terms: &[String], live: &HashMap<String, String>, limit: usize, fast: bool)
    -> Vec<Hit> {
    let agents = index.agents();
    let (files, _, _) = ingest::discover(accounts, agents, index);
    let mut order: Vec<_> = files.iter().filter(|(path, f)| f.stat.size <= MAX_FILE_BYTES || {
        index.report_once("busca.arquivo_grande", path, || format!("{path}: {} bytes, fora da varredura", f.stat.size));
        false
    }).collect();
    order.sort_by(|a, b| b.1.stat.mtime_ns.cmp(&a.1.stat.mtime_ns).then(a.0.cmp(b.0)));
    let folded: Vec<String> = terms.iter().map(|t| fold(t)).collect();
    // O primeiro dos mais longos, como o `max(t, key=len)` que escolhia o termo do `rg`.
    let longest = (1..terms.len()).fold(0, |best, k| if terms[k].chars().count() > terms[best].chars().count() { k } else { best });
    let longest = &folded[longest];
    let prefixes = ingest::internal_prefixes();
    let needles = fast.then(|| Needles::new(&folded));
    let read = |&(path, found): &(&String, &ingest::Found)| {
        let agent = AgentProvider::parse(found.provider).filter(|p| *p != AgentProvider::Claude);
        (agent, scan_file(path, agent, found.cwd.clone(), &folded, longest, &prefixes, needles.as_ref()))
    };
    let mut hits = Vec::new();
    for batch in order.chunks(if fast { SCAN_BATCH } else { 1 }) {
        if hits.len() >= limit { break; }
        let read: Vec<_> = match POOL.as_ref().filter(|_| fast) {
            Some(pool) => pool.install(|| batch.par_iter().map(read).collect()),
            None => batch.iter().map(read).collect(),
        };
        for (&(path, found), (agent, scanned)) in batch.iter().zip(read) {
            if hits.len() >= limit { break; }
            let scanned = match scanned {
                Ok(Some(scanned)) => scanned,
                Ok(None) => continue,
                Err(e) => { index.report_once("busca.transcript_ilegivel", path, || format!("{path}: {e}")); continue }
            };
            let Some(mtime) = mtime_of(path) else { continue };
            // Pi e omp: a credencial do modelo lido até aqui, como a passada do índice faz.
            let account = match (agent, &scanned.model, agents) {
                (Some(AgentProvider::Pi | AgentProvider::Omp), Some(model), Some(env)) =>
                    crate::provider_accounts::pi_account(env, Some(model.as_str())).unwrap_or_default(),
                _ => found.account.clone(),
            };
            let src = Source { path, project: &found.project, session_id: &found.sid, cwd: scanned.cwd, mtime,
                provider: found.provider, config_dir: found.config_dir.clone(), account, codex_account: found.codex_account.clone() };
            let room = limit - hits.len();
            hits.extend(scanned.messages.into_iter().take(room).map(|m| hit(&src, live, terms, m)));
        }
    }
    by_mtime(&mut hits);
    hits
}

struct Scanned { messages: Vec<ingest::Message>, cwd: Option<String>, model: Option<String> }

/// Mensagens do arquivo com todos os termos; `None` = conversa interna do Hangar ou sem achado.
/// `agent`: outro agente, lido pelo parser dele, com o `cwd` da descoberta. `needles`: arquivo sem
/// nenhuma linha que passe no pré-filtro fica sem parse.
#[allow(clippy::too_many_arguments)]
fn scan_file(path: &str, agent: Option<AgentProvider>, cwd: Option<String>, terms: &[String], longest: &str, prefixes: &[String],
    needles: Option<&Needles>) -> std::io::Result<Option<Scanned>> {
    let mut rd = BufReader::new(File::open(path)?);
    if let Some(needles) = needles {
        let (mut raw, mut buf) = (Vec::new(), Vec::new());
        loop {
            raw.clear();
            if rd.read_until(b'\n', &mut raw)? == 0 { return Ok(None); }
            if may_match(&raw, agent, needles, &mut buf) { break; }
        }
        // O parser precisa de todas as linhas: o filtro de reescrita e o cabeçalho têm estado.
        rd.seek(SeekFrom::Start(0))?;
    }
    let mut st = ingest::FileState { cwd, ..Default::default() };
    let mut messages: Vec<ingest::Message> = Vec::new();
    let mut held = HashSet::new();
    let mut matched_lines = 0;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if rd.read_until(b'\n', &mut raw)? == 0 { break; }
        let open = matched_lines < MAX_MATCHED_LINES;
        if raw.len() <= ingest::MAX_LINE_BYTES && fold(&String::from_utf8_lossy(&raw)).contains(longest) {
            matched_lines += 1;
        }
        let msgs = match agent {
            Some(provider) => ingest::absorb_agent_line(&mut st, &raw, provider, prefixes),
            None => ingest::absorb_line(&mut st, &raw, prefixes),
        };
        for msg in msgs {
            // Cada reenvio de prompt barrado repete o `held:`; o histórico leva um só.
            if (msg.0.starts_with("held:") || msg.0.starts_with("delivery:")) && !held.insert(msg.0.clone()) { continue; }
            if open && messages.len() < PER_FILE {
                let text = fold(&msg.3);
                if terms.iter().all(|t| text.contains(t.as_str())) { messages.push(msg); }
            }
        }
        let collected = messages.len() >= PER_FILE || matched_lines >= MAX_MATCHED_LINES;
        // O cwd é o do cabeçalho, como o índice guarda: a leitura segue até ele fechar.
        if collected && (messages.is_empty() || (st.internal.is_some() && st.head_done)) { break; }
    }
    if messages.is_empty() || st.internal == Some(true) {
        return Ok(None);
    }
    Ok(Some(Scanned { messages, cwd: st.cwd, model: st.model }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

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

    #[test]
    fn snippet_of_text_that_grows_in_lowercase_keeps_the_term() {
        // `İ` vira dois caracteres em minúsculas: a posição achada passa do fim do texto original.
        let text = format!("{} meio {}", "İ".repeat(300), "x".repeat(10));
        assert_eq!(snippet(&text, &["meio".to_owned()]), "…meio xxxxxxxxxx");
    }

    /// As linhas que cada parser lê, com os dois textos nos campos de mensagem.
    fn shapes(t1: &str, t2: &str) -> Vec<(Option<AgentProvider>, Value)> {
        use serde_json::json;
        let blocks = |kind: &str| json!([{"type": kind, "text": t1}, {"type": kind, "text": t2}]);
        vec![
            (None, json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": t1}})),
            (None, json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": blocks("text")}})),
            (None, json!({"type": "assistant", "uuid": "a1", "message": {"role": "assistant", "content": blocks("text")}})),
            (None, json!({"type": "user", "uuid": "u1", "origin": {"kind": "peer", "body": t1, "name": t2}, "message": {"role": "user", "content": t1}})),
            (None, json!({"type": "queue-operation", "operation": "remove", "content": t1})),
            (None, json!({"type": "system", "content": format!("UserPromptSubmit operation blocked by hook: x Original prompt: {t1}")})),
            (None, json!({"type": "attachment", "uuid": "q1", "attachment": {"type": "queued_command", "prompt": blocks("text")}})),
            (Some(AgentProvider::Codex), json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": blocks("input_text")}})),
            (Some(AgentProvider::Codex), json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": blocks("output_text")}})),
            (Some(AgentProvider::Pi), json!({"type": "message", "id": "n1", "message": {"role": "user", "content": blocks("text")}})),
            (Some(AgentProvider::Kimi), json!({"type": "context.append_message", "message": {"role": "user", "origin": {"kind": "user"}, "id": "m1", "content": blocks("text")}})),
        ]
    }

    /// A regra da varredura numa linha: alguma mensagem dela tem todos os termos dobrados.
    fn accepted(agent: Option<AgentProvider>, raw: &[u8], folded: &[String]) -> bool {
        let prefixes = ingest::internal_prefixes();
        let mut st = ingest::FileState::default();
        let msgs = match agent {
            Some(provider) => ingest::absorb_agent_line(&mut st, raw, provider, &prefixes),
            None => ingest::absorb_line(&mut st, raw, &prefixes),
        };
        msgs.iter().any(|m| { let text = fold(&m.3); folded.iter().all(|t| text.contains(t.as_str())) })
    }

    fn may(raw: &[u8], q: &str) -> bool {
        let folded: Vec<String> = terms(q).iter().map(|t| fold(t)).collect();
        may_match(raw, None, &Needles::new(&folded), &mut Vec::new())
    }

    #[test]
    fn prefilter_reads_case_accents_and_json_escapes() {
        let line = |text: &str, ascii: bool| {
            let v = serde_json::json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": text}});
            format!("{}\n", if ascii { crate::transcript::pyjson::dumps(&v, false) } else { v.to_string() }).into_bytes()
        };
        for ascii in [false, true] {
            assert!(may(&line("Configuração do Login", ascii), "configuracao login"), "ascii={ascii}");
            assert!(may(&line("configuracao", ascii), "CONFIGURAÇÃO"), "ascii={ascii}");
            assert!(may(&line("diga \"olá\" a/b", ascii), "\"ola\" a/b"), "ascii={ascii}");
            assert!(may(&line("emoji 😀 fim", ascii), "😀"), "ascii={ascii}");
            assert!(!may(&line("Configuração", ascii), "login-claude-navegador"), "ascii={ascii}");
            assert!(!may(&line("só um dos termos: login", ascii), "login navegador"), "ascii={ascii}");
        }
        // `#` no lugar da barra do escape: o JSON cru com o `\uXXXX` que o `ensure_ascii` grava.
        let esc = |s: &str| s.replace('#', "\\").into_bytes();
        assert!(may(&esc(r##"{"message":{"content":"a#u00e7#u00e3o"}}"##), "ação"), "escape do JSON");
        assert!(may(&esc(r##"{"message":{"content":"#ud83d#ude00"}}"##), "😀"), "par de surrogates");
        let big = format!("{{\"message\":{{\"content\":\"agulha {}\"}}}}\n", "x".repeat(ingest::MAX_LINE_BYTES));
        assert!(!may(big.as_bytes(), "agulha"), "linha acima do teto não vira mensagem");
        // A deleção do `<system-reminder>` junta os pedaços: o termo só existe depois do parser.
        assert!(may(&line("log<system-reminder>x</system-reminder>in", false), "login"));
    }

    proptest::proptest! {
        #[test]
        fn prefilter_never_drops_a_line_the_rule_accepts(
            parts in proptest::collection::vec(proptest::sample::select(vec![
                "a", "B", "o", "ç", "Ç", "ã", "É", "\u{301}", "\u{212A}", "İ", "ß", "1", "-", "\"", "\\", "/", "_", "😀", " ", "\n",
                "<system-reminder>r</system-reminder>", "[Image #2] ", "<pasted_content id=\"1\">", "</pasted_content id=\"1\">",
                "\u{1b}[31m", "\u{1b}[0m", "<teammate-message teammate_id=\"t\">", "</teammate-message>", "<cross-session-message from-name=\"p\">",
            ]), 0..14),
            split in 0usize..14, start in 0usize..40, len in 1usize..12, ascii: bool,
        ) {
            let cut = split.min(parts.len());
            let (t1, t2) = (parts[..cut].concat(), parts[cut..].concat());
            let whole: Vec<char> = fold(&format!("{t1}{t2}")).chars().collect();
            let q: String = whole.iter().skip(start.min(whole.len())).take(len).collect();
            let folded: Vec<String> = terms(&q).iter().map(|t| fold(t)).collect();
            proptest::prop_assume!(!folded.is_empty());
            for (agent, v) in shapes(&t1, &t2) {
                let text = if ascii { crate::transcript::pyjson::dumps(&v, false) } else { v.to_string() };
                let raw = format!("{text}\n").into_bytes();
                if accepted(agent, &raw, &folded) {
                    proptest::prop_assert!(may_match(&raw, agent, &Needles::new(&folded), &mut Vec::new()), "linha {text} q={q:?}");
                }
            }
        }

        #[test]
        fn snippet_never_panics(text in "\\PC{0,400}", grown in 0usize..300, term in "\\PC{1,8}") {
            let text = format!("{}{text}", "İ".repeat(grown));
            let _ = snippet(&text, &[term.to_lowercase()]);
        }
    }
}
