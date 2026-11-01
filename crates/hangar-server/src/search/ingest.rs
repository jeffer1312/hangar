// crates/hangar-server/src/search/ingest.rs
//! Uma passada do índice (`Index._update`/`_ingest` do Python): varre as contas, tira o que sumiu e lê
//! cada transcript mudado a partir do offset, só linhas completas, um arquivo por transação. As
//! mensagens passam pelo mesmo filtro de reescrita e pelo mesmo parser do histórico, então o id de
//! cada trecho existe no contexto.

use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use super::{Account, IndexError};
use crate::accounts::environment::AccountEnvironment;
use crate::session_open::AgentProvider;
use crate::speech::history::transfer_history as transfer;
use crate::transcript::{RewriteFilter, parse_claude_object, py, pyjson};

// Mesmo teto do `rg -M` da busca antiga: linha maior é saída de ferramenta ou contexto injetado.
pub(super) const MAX_LINE_BYTES: usize = 40_000;
// Acima do teto da busca a linha ainda dá cabeçalho e última resposta (prompt com imagem colada);
// acima deste, nem isso, e ela é pulada sem ir à memória.
pub(super) const HUGE_LINE_BYTES: usize = 32 * 1024 * 1024;
const HEAD_LINES: i64 = 60;
const INTERNAL_LINES: i64 = 30;
const PREFIX_CHARS: usize = 60;
// O pedido do bastão só existe no Python (`bastao._REESCRITA_PEDIDO`).
const HANDOFF_PREFIX: &str = "Você recebeu abaixo um resumo automático de uma sessão de tr";

static SID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z").unwrap()
});

/// Os 60 primeiros caracteres de cada pedido interno do Hangar: conversa que começa assim fica fora.
pub(crate) fn internal_prefixes() -> Vec<String> {
    [super::ask::SYSTEM, crate::loops::refine::SYSTEM, HANDOFF_PREFIX].iter()
        .map(|p| p.chars().take(PREFIX_CHARS).collect()).collect()
}

/// Inode com sinal de 63 bits: o file ID do Windows passa do INTEGER do SQLite.
pub(super) fn masked_ino(raw: u64) -> i64 {
    (raw & 0x7FFF_FFFF_FFFF_FFFF) as i64
}

/// `ino` ausente: o sistema só o dá abrindo o arquivo (Windows), e ele é pedido quando preciso.
pub(super) struct Stat { pub size: i64, pub mtime_ns: i64, pub ino: Option<i64> }

impl Stat {
    pub(super) fn of(md: &Metadata) -> Self {
        #[cfg(unix)]
        let (ino, mtime_ns) = {
            use std::os::unix::fs::MetadataExt;
            (Some(masked_ino(md.ino())), md.mtime().saturating_mul(1_000_000_000).saturating_add(md.mtime_nsec()))
        };
        #[cfg(not(unix))]
        let (ino, mtime_ns) = {
            let modified = md.modified().unwrap_or(std::time::UNIX_EPOCH);
            let nanos = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos().min(i64::MAX as u128) as i64);
            (None, nanos)
        };
        Self { size: md.len() as i64, mtime_ns, ino }
    }
}

/// O arquivo está como o índice o leu: (size, mtime_ns, ino) gravados contra o stat de agora.
/// Sem `ino` no stat (Windows), tamanho e relógio iguais bastam: pedir o file ID abriria o arquivo.
pub(super) fn unchanged(known: (i64, i64, i64), stat: &Stat) -> bool {
    (known.0, known.1) == (stat.size, stat.mtime_ns) && stat.ino.is_none_or(|ino| ino == known.2)
}

fn file_id(path: &Path) -> i64 {
    masked_ino(windows_file_index(path))
}

/// O file ID do NTFS: sem ele, transcript trocado no lugar e maior que o lido seria continuado do
/// offset antigo. shortcut: arquivo que não abre dá 0, e a passada seguinte o relê inteiro.
#[cfg(windows)]
fn windows_file_index(path: &Path) -> u64 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let Ok(file) = std::fs::File::open(path) else { return 0 };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: o handle é do `file`, aberto e vivo até o fim da função; `info` é uma struct nossa,
    // inicializada, que a chamada só preenche.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 { return 0; }
    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
}

#[cfg(not(windows))]
fn windows_file_index(_path: &Path) -> u64 { 0 }

/// Um transcript a indexar. `cwd` vem da descoberta nos outros agentes (cabeçalho próprio de cada um);
/// no Claude sai das linhas.
pub(super) struct Found {
    pub project: String, pub sid: String, pub stat: Stat, pub provider: &'static str,
    pub config_dir: Option<String>, pub account: String, pub codex_account: Option<String>, pub cwd: Option<String>,
    /// Rollout Codex vindo de uma transferência do Claude: o registro dela e a casa da conta dona.
    pub transfer: Option<(Value, PathBuf)>,
}

struct Known {
    id: i64, size: i64, mtime_ns: i64, ino: i64, offset: i64, config_dir: Option<String>,
    head_len: i64, head_hash: Option<String>, provider: String, cwd: Option<String>,
}

/// Bytes do começo do arquivo guardados como hash: `cp` por cima mantém o inode e não encolhe.
const HEAD_BYTES: i64 = 4096;

/// Filhos diretos `<uuid>.jsonl` de cada pasta de projeto; a cópia `.from-<conta>` e `subagents/`
/// ficam fora. Devolve também o que não deu para ler agora (pasta ou arquivo, com o erro): o que o
/// índice já tem lá dentro fica como está.
pub(super) fn conversations(accounts: &[Account]) -> (BTreeMap<String, Found>, Vec<(PathBuf, String)>) {
    let mut out = BTreeMap::new();
    let mut unreadable = Vec::new();
    let mut failed = |path: PathBuf, e: &std::io::Error| {
        if e.kind() != std::io::ErrorKind::NotFound {
            unreadable.push((path, e.to_string()));
        }
    };
    for account in accounts {
        let projects = match std::fs::read_dir(&account.projects) {
            Ok(projects) => projects,
            Err(e) => { failed(account.projects.clone(), &e); continue; }
        };
        for project in projects {
            let project = match project {
                Ok(project) => project,
                Err(e) => { failed(account.projects.clone(), &e); continue; }
            };
            let dir = project.path();
            if !dir.is_dir() { continue; }
            let files = match std::fs::read_dir(&dir) {
                Ok(files) => files,
                Err(e) => { failed(dir, &e); continue; }
            };
            for file in files {
                let file = match file {
                    Ok(file) => file,
                    Err(e) => { failed(dir.clone(), &e); continue; }
                };
                let path: PathBuf = file.path();
                let (Some(stem), Some("jsonl")) = (path.file_stem().and_then(|s| s.to_str()), path.extension().and_then(|s| s.to_str())) else { continue };
                if !SID.is_match(stem) { continue; }
                let md = match std::fs::metadata(&path) {
                    Ok(md) => md,
                    Err(e) => { failed(path, &e); continue; }
                };
                if !md.is_file() { continue; }
                out.entry(path.to_string_lossy().into_owned()).or_insert_with(|| Found {
                    project: project.file_name().to_string_lossy().into_owned(), sid: stem.to_owned(), stat: Stat::of(&md),
                    provider: "claude", config_dir: account.config_dir.clone(), account: account_key(account), codex_account: None, cwd: None,
                    transfer: None,
                });
            }
        }
    }
    (out, unreadable)
}

fn head_hash(file: &mut File, len: i64) -> std::io::Result<String> {
    use md5::{Digest, Md5};
    let mut buf = vec![0; len as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut buf)?;
    Ok(format!("{:x}", Md5::digest(&buf)))
}

pub(super) fn account_key(account: &Account) -> String {
    format!("claude:{}", crate::list::links::resolve_lenient(&account.dir).to_string_lossy())
}

pub(super) type Report<'a> = &'a dyn Fn(&'static str, String);

/// Os transcripts de todas as contas e agentes, com o que não pôde ser lido agora; pasta e agente
/// ilegíveis vão ao diário quando passam a falhar (a passada e a varredura veem o mesmo conjunto).
/// `known` dá o cwd guardado do transcript de agente que não mudou desde a leitura do índice.
pub(super) fn discover(accounts: &[Account], agents: Option<&AccountEnvironment>, index: &super::Index, known: crate::archive_list::KnownCwd)
    -> (BTreeMap<String, Found>, Vec<(PathBuf, String)>, Vec<crate::archive_list::Unread>) {
    let (mut current, unreadable) = conversations(accounts);
    index.failing("busca.pasta_ilegivel", unreadable.iter()
        .map(|(path, e)| (path.to_string_lossy().into_owned(), format!("{}: {e}", path.display()))).collect());
    let (others, failed) = agents.map(|env| super::agents::discover(env, known)).unwrap_or_default();
    index.failing("busca.agente_ilegivel", failed.iter().map(|u| (u.key(), u.error.clone())).collect());
    current.extend(others);
    (current, unreadable, failed)
}

/// Erro do banco num arquivo não para os outros, mas volta no fim: índice que não gravou não fica pronto.
pub(super) fn pass(conn: &mut Connection, accounts: &[Account], agents: Option<&AccountEnvironment>, index: &super::Index) -> Result<(), IndexError> {
    let report = |code, detail| index.report(code, detail);
    let pi_account = |provider: &str| agents.and_then(|env| crate::provider_accounts::pi_account(env, Some(provider))).unwrap_or_default();
    let known: BTreeMap<String, Known> = conn.prepare("SELECT path, id, size, mtime_ns, ino, offset, config_dir, head_len, head_hash, provider, cwd FROM files")?
        .query_map([], |r| Ok((r.get(0)?, Known { id: r.get(1)?, size: r.get(2)?, mtime_ns: r.get(3)?, ino: r.get(4)?,
            offset: r.get(5)?, config_dir: r.get(6)?, head_len: r.get(7)?, head_hash: r.get(8)?, provider: r.get(9)?, cwd: r.get(10)? })))?
        .collect::<Result<_, _>>()?;
    // Transcript de agente parado não é reaberto: o cwd e a fase da transferência ficam como o índice leu.
    let stored = |path: &Path| -> Option<Option<String>> {
        let k = known.get(&*path.to_string_lossy()).filter(|k| k.provider != "claude")?;
        let md = std::fs::metadata(path).ok()?;
        unchanged((k.size, k.mtime_ns, k.ino), &Stat::of(&md)).then(|| k.cwd.clone())
    };
    let (current, unreadable, failed) = discover(accounts, agents, index, &stored);
    let listed: Vec<Option<&str>> = accounts.iter().map(|a| a.config_dir.as_deref()).collect();
    let tx = conn.transaction()?;
    for (path, k) in &known {
        // Agente, pasta ou rollout que não pôde ser varrido agora mantém o que o índice tem lá.
        if current.contains_key(path) || unreadable.iter().any(|(d, _)| Path::new(path).starts_with(d)) || failed.iter().any(|u| u.covers(&k.provider, path)) {
            continue;
        }
        // Conta em migração sai do catálogo por um tempo: as conversas dela ficam.
        let account = k.config_dir.as_deref();
        if !listed.contains(&account) && account.is_some_and(|d| crate::accounts::storage::pending(Path::new(d))) {
            continue;
        }
        tx.execute("DELETE FROM msg WHERE file_id=?", [k.id])?;
        tx.execute("DELETE FROM files WHERE id=?", [k.id])?;
    }
    tx.commit()?;
    let prefixes = internal_prefixes();
    // Mais velho primeiro: o id da mensagem acompanha a recência e a busca pega as mais novas pelo id.
    let mut order: Vec<(&String, &Found)> = current.iter().collect();
    order.sort_by(|a, b| a.1.stat.mtime_ns.cmp(&b.1.stat.mtime_ns).then(a.0.cmp(b.0)));
    let mut failure = None;
    let (mut illegible, mut illegible_files) = (0u64, 0usize);
    for (path, found) in order {
        // Cada arquivo já entrou inteiro na sua transação: parar aqui não deixa nada pela metade.
        if index.stopping() {
            break;
        }
        let old = known.get(path);
        if let Some(k) = old {
            if unchanged((k.size, k.mtime_ns, k.ino), &found.stat) {
                if k.config_dir != found.config_dir {
                    conn.execute("UPDATE files SET config_dir=?, account=? WHERE id=?", params![found.config_dir, found.account, k.id])?;
                }
                continue;
            }
        }
        // Um arquivo = uma transação: linhas e offset entram juntos ou nenhum entra; sobra parcial com
        // offset velho viraria mensagem repetida na passada seguinte.
        let ino = found.stat.ino.unwrap_or_else(|| file_id(Path::new(path)));
        let tx = conn.transaction()?;
        match ingest(&tx, path, found, ino, old, &prefixes, &pi_account, &report) {
            Ok((skipped, huge)) => {
                tx.commit()?;
                index.recovered("busca.indice_transcript_ilegivel", path);
                if skipped > 0 { illegible += u64::from(skipped); illegible_files += 1; }
                // Leitura sem linha enorme libera o registro: a próxima que aparecer volta ao diário.
                if huge > 0 { index.report_once("busca.linha_enorme", path, || format!("{path}: linha de {huge} bytes pulada")); }
                else { index.recovered("busca.linha_enorme", path); }
            }
            // Sumiu depois de achado: a passada seguinte o tira.
            Err(IndexError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            // O que o índice tem do arquivo fica; a próxima passada tenta de novo.
            Err(IndexError::Io(e)) => index.report_once("busca.indice_transcript_ilegivel", path, || format!("{path}: {e}")),
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "busca: ingestão falhou; fica para a próxima passada");
                failure.get_or_insert(e);
            }
        }
    }
    // Parser que regrediu indexaria sessões como vazias e a busca diria "nada".
    if illegible > 0 {
        report("busca.linhas_ilegiveis", format!("{illegible} linhas ilegíveis puladas em {illegible_files} arquivos"));
    }
    failure.map_or(Ok(()), Err)
}

/// O que o arquivo já tem no índice e o que a leitura acrescenta.
#[derive(Default)]
pub(crate) struct FileState {
    pub offset: i64,
    pub lines: i64,
    pub cwd: Option<String>,
    pub preview: String,
    pub head_done: bool,
    pub internal: Option<bool>,
    pub has_user: bool,
    pub last_text: String,
    pub rewrite: RewriteFilter,
    pub illegible: u32,
    /// Provedor do último modelo escolhido na leitura (Pi e omp): dá a conta da conversa.
    pub model: Option<String>,
}

/// Mensagem a indexar: (event_id, role, ts, texto).
pub(crate) type Message = (String, &'static str, Option<f64>, String);

/// Uma linha completa (com o `\n`). Devolve as mensagens a indexar.
pub(crate) fn absorb_line(st: &mut FileState, raw: &[u8], prefixes: &[String]) -> Vec<Message> {
    st.offset += raw.len() as i64;
    st.lines += 1;
    // O Python lê em modo texto: "\r\n" chega como "\n".
    let mut line = String::from_utf8_lossy(raw);
    if line.ends_with("\r\n") {
        let fixed = line.to_mut();
        fixed.truncate(fixed.len() - 2);
        fixed.push('\n');
    }
    let value = pyjson::loads_lossless(&line);
    let Some(Value::Object(obj)) = &value else {
        if value.is_none() {
            st.illegible += 1;
        }
        return Vec::new();
    };
    // O filtro vê toda linha, como no histórico: pular uma deixaria o estado dele diferente.
    let kept = st.rewrite.keep(obj);
    if obj.get("type").and_then(Value::as_str) == Some("user") {
        st.has_user = true;
    }
    let cwd = obj.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty());
    absorb_events(st, parse_claude_object(obj), cwd, raw.len(), kept, prefixes)
}

/// Uma linha completa de Codex, Pi, omp ou Kimi, pelo parser do histórico do Arquivo (os ids são os
/// dele). O cwd já vem da descoberta.
pub(crate) fn absorb_agent_line(st: &mut FileState, raw: &[u8], provider: AgentProvider, prefixes: &[String]) -> Vec<Message> {
    st.offset += raw.len() as i64;
    st.lines += 1;
    // O histórico parte o arquivo no `\n`: a linha chega ao parser sem ele.
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);
    if matches!(provider, AgentProvider::Pi | AgentProvider::Omp) && line.windows(14).any(|w| w == b"\"model_change\"") {
        if let Some(model) = super::agents::model_provider(line) { st.model = Some(model); }
    }
    let events = crate::archive_history::agent_line(provider, line);
    if events.iter().any(|e| e.kind == ChatKind::UserMsg) {
        st.has_user = true;
    }
    absorb_events(st, events, None, raw.len(), true, prefixes)
}

fn absorb_events(st: &mut FileState, events: Vec<ChatEvent>, cwd: Option<&str>, size: usize, kept: bool, prefixes: &[String]) -> Vec<Message> {
    let msgs: Vec<_> = events.into_iter()
        .filter(|e| matches!(e.kind, ChatKind::UserMsg | ChatKind::AssistantMsg) && e.text.as_deref().is_some_and(|t| !t.is_empty()))
        .collect();
    let first_user = msgs.iter().find(|e| e.kind == ChatKind::UserMsg).and_then(|e| e.text.as_deref());
    if !st.head_done {
        if st.cwd.is_none() {
            st.cwd = cwd.map(str::to_owned);
        }
        if st.preview.is_empty() {
            if let Some(text) = first_user { st.preview = cut(&crate::list::reply::plain_text(text)); }
        }
        st.head_done = (st.cwd.is_some() && !st.preview.is_empty()) || st.lines >= HEAD_LINES;
    }
    if st.internal.is_none() {
        if let Some(text) = first_user {
            let t = py::lstrip(text);
            st.internal = Some(prefixes.iter().any(|p| t.starts_with(p.as_str())));
        } else if st.lines >= INTERNAL_LINES {
            st.internal = Some(false);
        }
    }
    if let Some(text) = msgs.last().and_then(|e| e.text.as_deref()) {
        st.last_text = cut(&crate::list::reply::plain_text(text));
    }
    // Linha grande só alimenta cabeçalho e fatos do Arquivo.
    if size > MAX_LINE_BYTES || !kept {
        return Vec::new();
    }
    msgs.into_iter().map(|e| {
        let role = if e.kind == ChatKind::UserMsg { "user" } else { "assistant" };
        (e.id, role, e.ts, e.text.unwrap_or_default())
    }).collect()
}

/// `archive._cortar`: corte na palavra e com reticências.
pub(crate) fn cut(text: &str) -> String {
    const LIMIT: usize = 120;
    if text.chars().count() <= LIMIT { return text.to_owned(); }
    let mut short: String = text.chars().take(LIMIT).collect();
    if let Some(space) = short.rfind(' ') {
        if short[..space].chars().count() > LIMIT / 2 { short.truncate(space); }
    }
    format!("{}…", short.trim_end_matches([' ', ',', ';', ':', '.']))
}

/// O que `next_line` leu: uma linha inteira em `raw`, o fim sem `\n` (linha ainda sendo escrita) ou
/// uma linha maior que o teto, pulada com o tamanho dela.
pub(super) enum Line { Full, Partial, Skipped(u64) }

/// A próxima linha até `cap` bytes; a maior é descartada até o `\n` sem guardar o resto.
pub(super) fn next_line(rd: &mut impl BufRead, raw: &mut Vec<u8>, cap: usize) -> std::io::Result<Line> {
    raw.clear();
    let read = rd.by_ref().take(cap as u64 + 1).read_until(b'\n', raw)?;
    if raw.ends_with(b"\n") { return Ok(Line::Full); }
    if read <= cap { return Ok(Line::Partial); }
    raw.clear();
    let mut skipped = read as u64;
    loop {
        let buf = rd.fill_buf()?;
        if buf.is_empty() { return Ok(Line::Partial); }
        let (used, done) = match buf.iter().position(|b| *b == b'\n') { Some(i) => (i + 1, true), None => (buf.len(), false) };
        rd.consume(used);
        skipped += used as u64;
        if done { return Ok(Line::Skipped(skipped)); }
    }
}

/// Devolve quantas linhas ilegíveis a leitura pulou (as enormes entre elas) e o tamanho da maior enorme.
#[allow(clippy::too_many_arguments)]
fn ingest(tx: &Transaction, path: &str, found: &Found, ino: i64, old: Option<&Known>, prefixes: &[String],
          pi_account: &dyn Fn(&str) -> String, report: Report) -> Result<(u32, u64), IndexError> {
    let stat = &found.stat;
    let agent = AgentProvider::parse(found.provider).filter(|p| *p != AgentProvider::Claude);
    let mut file = File::open(path)?;
    let fid = match old {
        None => {
            tx.execute("INSERT INTO files(path, project, session_id, ino, provider, config_dir, account, codex_account) VALUES (?,?,?,?,?,?,?,?)",
                params![path, found.project, found.sid, ino, found.provider, found.config_dir, found.account, found.codex_account])?;
            tx.last_insert_rowid()
        }
        Some(k) => {
            // Começo diferente com o mesmo inode e sem encolher: regravado do zero (`cp` por cima).
            let rewritten = k.ino != ino || stat.size < k.offset
                || (k.head_len > 0 && Some(head_hash(&mut file, k.head_len)?) != k.head_hash);
            if rewritten {
                tx.execute("DELETE FROM msg WHERE file_id=?", [k.id])?;
                tx.execute("UPDATE files SET offset=0, lines=0, cwd=NULL, preview='', head_done=0, internal=NULL, \
                            has_user=0, last_text='', rewrite=NULL, head_len=0, head_hash=NULL, ino=? WHERE id=?", params![ino, k.id])?;
            }
            k.id
        }
    };
    let (mut st, saved_rewrite, saved_account) = tx.query_row(
        "SELECT offset, lines, cwd, preview, head_done, internal, has_user, last_text, rewrite, account FROM files WHERE id=?", [fid],
        |r| Ok((FileState {
            offset: r.get(0)?, lines: r.get(1)?, cwd: r.get(2)?, preview: r.get(3)?, head_done: r.get::<_, i64>(4)? != 0,
            internal: r.get::<_, Option<i64>>(5)?.map(|v| v != 0), has_user: r.get::<_, i64>(6)? != 0, last_text: r.get(7)?,
            rewrite: RewriteFilter::default(), illegible: 0, model: None,
        }, r.get::<_, Option<String>>(8)?, r.get::<_, Option<String>>(9)?)))?;
    if agent.is_some() {
        st.cwd = found.cwd.clone();
    }
    if let Some(saved) = saved_rewrite {
        match RewriteFilter::from_state(&saved) {
            Some(filter) => st.rewrite = filter,
            // Sem o estado, linha regravada pelo `--resume` que chegar agora pode entrar no índice.
            None => report("busca.filtro_ilegivel", format!("{}/{}", found.project, found.sid)),
        }
    }
    // Rollout transferido: só os turnos depois da fronteira, sem os importados, com os ids do composto.
    let transferred = match &found.transfer {
        None => None,
        Some((record, home)) => {
            transfer::verify_boundary(record, Path::new(path), home)?;
            let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "transferência sem fronteira legível");
            let boundary = &record["boundary"];
            let start = boundary["min_offset"].as_i64().ok_or_else(invalid)?;
            let imported: std::collections::HashSet<&str> = boundary["imported_item_ids"].as_array().ok_or_else(invalid)?
                .iter().filter_map(Value::as_str).collect();
            st.offset = st.offset.max(start);
            Some((record["id"].as_str().ok_or_else(invalid)?, imported))
        }
    };
    let mut rd = BufReader::new(file);
    rd.seek(SeekFrom::Start(st.offset as u64))?;
    let mut insert = tx.prepare_cached("INSERT INTO msg(file_id, event_id, role, ts, text) VALUES (?,?,?,?,?)")?;
    let mut exists = tx.prepare_cached("SELECT 1 FROM msg WHERE file_id=? AND event_id=?")?;
    let mut raw = Vec::new();
    let mut huge = 0u64;
    loop {
        match next_line(&mut rd, &mut raw, HUGE_LINE_BYTES)? {
            Line::Full => {}
            Line::Partial => break,   // linha ainda sendo escrita: entra na próxima passada
            Line::Skipped(len) => {
                st.offset += len as i64;
                st.lines += 1;
                st.illegible += 1;
                huge = huge.max(len);
                continue;
            }
        }
        if let Some((_, imported)) = &transferred {
            let row: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
            if [&row["id"], &row["payload"]["id"]].into_iter().filter_map(Value::as_str).any(|id| imported.contains(id)) {
                st.offset += raw.len() as i64;
                st.lines += 1;
                continue;
            }
        }
        let before = st.illegible;
        let mut msgs = match agent {
            Some(provider) => absorb_agent_line(&mut st, &raw, provider, prefixes),
            None => absorb_line(&mut st, &raw, prefixes),
        };
        if let Some((id, _)) = &transferred {
            for msg in &mut msgs { msg.0 = format!("transfer:{id}:codex:{}", msg.0); }
        }
        if before == 0 && st.illegible == 1 {
            // Parser que regrediu indexaria a sessão como vazia e a busca diria "nada".
            tracing::warn!(path, byte = st.offset - raw.len() as i64, "busca: linha ilegível; pulando");
        }
        for (id, role, ts, text) in msgs {
            // Cada reenvio de prompt barrado repete o "held:"; o histórico leva um só.
            if (id.starts_with("held:") || id.starts_with("delivery:")) && exists.query_row(params![fid, id], |_| Ok(())).optional()?.is_some() {
                continue;
            }
            insert.execute(params![fid, id, role, ts, text])?;
        }
    }
    let head_len = st.offset.min(HEAD_BYTES);
    let head = if head_len > 0 { Some(head_hash(rd.get_mut(), head_len)?) } else { None };
    // Pi e omp: a conta é a da credencial do último modelo; leitura sem troca de modelo mantém a gravada.
    let account = match (agent, &st.model) {
        (Some(AgentProvider::Pi | AgentProvider::Omp), Some(model)) => pi_account(model),
        (Some(AgentProvider::Pi | AgentProvider::Omp), None) => saved_account.unwrap_or_default(),
        _ => found.account.clone(),
    };
    // size/mtime da stat de antes da leitura: se o arquivo cresceu no meio, a próxima passada relê.
    tx.execute("UPDATE files SET size=?, mtime_ns=?, offset=?, lines=?, cwd=?, preview=?, head_done=?, internal=?, \
                has_user=?, last_text=?, rewrite=?, config_dir=?, account=?, head_len=?, head_hash=?, codex_account=? WHERE id=?",
        params![stat.size, stat.mtime_ns, st.offset, st.lines, st.cwd, st.preview, st.head_done as i64,
            st.internal.map(i64::from), st.has_user as i64, st.last_text, st.rewrite.to_state(),
            found.config_dir, account, head_len, head, found.codex_account, fid])?;
    Ok((st.illegible, huge))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_mtime_are_the_stat_from_before_the_read() {
        // IDX-35: o arquivo que cresce entre o stat e a leitura guarda o tamanho de antes; a passada
        // seguinte vê a diferença em vez de dar o arquivo por lido.
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("acct/projects");
        let file = projects.join("-home-u-proj/11111111-1111-1111-1111-111111111111.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let line = |uuid: &str, text: &str| format!("{}\n", serde_json::json!({"type": "user", "uuid": uuid, "cwd": "/home/u/proj",
            "message": {"role": "user", "content": text}}));
        std::fs::write(&file, line("u1", "primeira")).unwrap();
        let (found, _) = conversations(&[Account { config_dir: None, dir: dir.path().join("acct"), projects }]);
        let (path, found) = found.iter().next().unwrap();
        std::io::Write::write_all(&mut std::fs::OpenOptions::new().append(true).open(&file).unwrap(), line("u2", "segunda").as_bytes()).unwrap();
        let mut conn = super::super::db::open(&dir.path().join("idx.sqlite3")).unwrap();
        let tx = conn.transaction().unwrap();
        ingest(&tx, path, found, found.stat.ino.unwrap_or(0), None, &internal_prefixes(), &|_| String::new(), &|_, _| {}).unwrap();
        tx.commit().unwrap();
        let (size, offset, msgs): (i64, i64, i64) = conn.query_row("SELECT size, offset, (SELECT COUNT(*) FROM msg) FROM files", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        let now = std::fs::metadata(&file).unwrap();
        assert_eq!((size, offset, msgs), (found.stat.size, now.len() as i64, 2));
        let known: (i64, i64, i64) = conn.query_row("SELECT size, mtime_ns, ino FROM files", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert!(!unchanged(known, &Stat::of(&now)), "a passada seguinte relê o arquivo");
    }

    #[test]
    fn a_huge_line_is_skipped_without_being_kept() {
        // Linha acima do teto: o resto é descartado sem guardar e o offset passa dela; sem o `\n` final
        // ela ainda está sendo escrita e espera a próxima passada.
        let mut rd = std::io::Cursor::new(b"0123456789abcdef\n{\"ok\":1}\n0123456789".to_vec());
        let mut raw = Vec::new();
        assert!(matches!(next_line(&mut rd, &mut raw, 4).unwrap(), Line::Skipped(17)));
        assert!(raw.capacity() <= 64, "o resto da linha não foi guardado");
        assert!(matches!(next_line(&mut rd, &mut raw, 4).unwrap(), Line::Skipped(9)));
        assert!(matches!(next_line(&mut rd, &mut raw, 40).unwrap(), Line::Partial));
        let mut rd = std::io::Cursor::new(b"{\"ok\":1}\n".to_vec());
        assert!(matches!(next_line(&mut rd, &mut raw, 40).unwrap(), Line::Full));
        assert_eq!(raw, b"{\"ok\":1}\n");
        assert!(matches!(next_line(&mut rd, &mut raw, 40).unwrap(), Line::Partial));
    }

    #[test]
    fn a_huge_line_without_a_break_advances_the_offset_and_the_next_line_enters() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("acct/projects");
        let file = projects.join("-home-u-proj/11111111-1111-1111-1111-111111111111.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let huge = format!("{}\n", serde_json::json!({"type": "user", "uuid": "u0", "cwd": "/home/u/proj",
            "message": {"role": "user", "content": "x".repeat(HUGE_LINE_BYTES + 10)}}));
        let small = format!("{}\n", serde_json::json!({"type": "user", "uuid": "u1", "cwd": "/home/u/proj",
            "message": {"role": "user", "content": "agulha"}}));
        std::fs::write(&file, format!("{huge}{small}")).unwrap();
        let (found, _) = conversations(&[Account { config_dir: None, dir: dir.path().join("acct"), projects }]);
        let (path, found) = found.iter().next().unwrap();
        let mut conn = super::super::db::open(&dir.path().join("idx.sqlite3")).unwrap();
        let tx = conn.transaction().unwrap();
        ingest(&tx, path, found, found.stat.ino.unwrap_or(0), None, &internal_prefixes(), &|_| String::new(), &|_, _| {}).unwrap();
        tx.commit().unwrap();
        let (offset, lines, ids): (i64, i64, String) = conn.query_row(
            "SELECT offset, lines, (SELECT group_concat(event_id) FROM msg) FROM files", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((offset, lines, ids.as_str()), ((huge.len() + small.len()) as i64, 2, "u1"));
    }
}
