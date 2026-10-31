// crates/hangar-server/src/search/ingest.rs
//! Uma passada do índice (`Index._update`/`_ingest` do Python): varre as contas, tira o que sumiu e lê
//! cada transcript mudado a partir do offset, só linhas completas, um arquivo por transação. As
//! mensagens passam pelo mesmo filtro de reescrita e pelo mesmo parser do histórico, então o id de
//! cada trecho existe no contexto.

use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::LazyLock;

use hangar_api::chat::ChatKind;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use super::{Account, IndexError};
use crate::transcript::{RewriteFilter, parse_claude_object, py, pyjson};

// Mesmo teto do `rg -M` da busca antiga: linha maior é saída de ferramenta ou contexto injetado.
pub(super) const MAX_LINE_BYTES: usize = 40_000;
const HEAD_LINES: i64 = 60;
const INTERNAL_LINES: i64 = 30;
const PREFIX_CHARS: usize = 60;
// O pedido do bastão só existe no Python (`bastao._REESCRITA_PEDIDO`); o do Perguntar
// (`search._ASK_SYSTEM`) ainda não chegou ao Rust.
const ASK_PREFIX: &str = "Você responde onde um assunto apareceu nas conversas do usuá";
const HANDOFF_PREFIX: &str = "Você recebeu abaixo um resumo automático de uma sessão de tr";

static SID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z").unwrap()
});

/// Os 60 primeiros caracteres de cada pedido interno do Hangar: conversa que começa assim fica fora.
pub(super) fn internal_prefixes() -> Vec<String> {
    [ASK_PREFIX, crate::loops::refine::SYSTEM, HANDOFF_PREFIX].iter()
        .map(|p| p.chars().take(PREFIX_CHARS).collect()).collect()
}

/// Inode com sinal de 63 bits: o file ID do Windows passa do INTEGER do SQLite.
pub(super) fn masked_ino(raw: u64) -> i64 {
    (raw & 0x7FFF_FFFF_FFFF_FFFF) as i64
}

struct Stat { size: i64, mtime_ns: i64, ino: i64 }

impl Stat {
    fn of(md: &Metadata) -> Self {
        #[cfg(unix)]
        let (ino, mtime_ns) = {
            use std::os::unix::fs::MetadataExt;
            (masked_ino(md.ino()), md.mtime().saturating_mul(1_000_000_000).saturating_add(md.mtime_nsec()))
        };
        #[cfg(not(unix))]
        let (ino, mtime_ns) = {
            let modified = md.modified().unwrap_or(std::time::UNIX_EPOCH);
            let nanos = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos().min(i64::MAX as u128) as i64);
            (0, nanos)
        };
        Self { size: md.len() as i64, mtime_ns, ino }
    }
}

struct Found<'a> { project: String, sid: String, stat: Stat, account: &'a Account }

struct Known { id: i64, size: i64, mtime_ns: i64, ino: i64, offset: i64, config_dir: Option<String> }

/// Filhos diretos `<uuid>.jsonl` de cada pasta de projeto; a cópia `.from-<conta>` e `subagents/`
/// ficam fora. Conta ilegível não entra (os arquivos dela saem do índice).
fn conversations(accounts: &[Account]) -> BTreeMap<String, Found<'_>> {
    let mut out = BTreeMap::new();
    for account in accounts {
        let Ok(projects) = std::fs::read_dir(&account.projects) else { continue };
        for project in projects.flatten() {
            let dir = project.path();
            if !dir.is_dir() { continue; }
            let Ok(files) = std::fs::read_dir(&dir) else { continue };
            for file in files.flatten() {
                let path: PathBuf = file.path();
                let (Some(stem), Some("jsonl")) = (path.file_stem().and_then(|s| s.to_str()), path.extension().and_then(|s| s.to_str())) else { continue };
                if !SID.is_match(stem) { continue; }
                let Ok(md) = std::fs::metadata(&path) else { continue };
                if !md.is_file() { continue; }
                out.entry(path.to_string_lossy().into_owned()).or_insert(Found {
                    project: project.file_name().to_string_lossy().into_owned(), sid: stem.to_owned(), stat: Stat::of(&md), account,
                });
            }
        }
    }
    out
}

fn account_key(account: &Account) -> String {
    format!("claude:{}", crate::list::links::resolve_lenient(&account.dir).to_string_lossy())
}

pub(super) fn pass(conn: &mut Connection, accounts: &[Account]) -> Result<(), IndexError> {
    let current = conversations(accounts);
    let known: BTreeMap<String, Known> = conn.prepare("SELECT path, id, size, mtime_ns, ino, offset, config_dir FROM files")?
        .query_map([], |r| Ok((r.get(0)?, Known { id: r.get(1)?, size: r.get(2)?, mtime_ns: r.get(3)?, ino: r.get(4)?,
            offset: r.get(5)?, config_dir: r.get(6)? })))?
        .collect::<Result<_, _>>()?;
    let tx = conn.transaction()?;
    for (path, k) in &known {
        if !current.contains_key(path) {
            tx.execute("DELETE FROM msg WHERE file_id=?", [k.id])?;
            tx.execute("DELETE FROM files WHERE id=?", [k.id])?;
        }
    }
    tx.commit()?;
    let prefixes = internal_prefixes();
    // Mais velho primeiro: o id da mensagem acompanha a recência e a busca pega as mais novas pelo id.
    let mut order: Vec<(&String, &Found)> = current.iter().collect();
    order.sort_by(|a, b| a.1.stat.mtime_ns.cmp(&b.1.stat.mtime_ns).then(a.0.cmp(b.0)));
    for (path, found) in order {
        let old = known.get(path);
        if let Some(k) = old {
            if (k.size, k.mtime_ns, k.ino) == (found.stat.size, found.stat.mtime_ns, found.stat.ino) {
                if k.config_dir != found.account.config_dir {
                    conn.execute("UPDATE files SET config_dir=?, account=? WHERE id=?",
                        params![found.account.config_dir, account_key(found.account), k.id])?;
                }
                continue;
            }
        }
        // Um arquivo = uma transação: linhas e offset entram juntos ou nenhum entra; sobra parcial com
        // offset velho viraria mensagem repetida na passada seguinte.
        let tx = conn.transaction()?;
        match ingest(&tx, path, found, old, &prefixes) {
            Ok(()) => tx.commit()?,
            Err(IndexError::Io(e)) => tracing::debug!(path = %path, error = %e, "busca: transcript ilegível"),
            Err(e) => tracing::warn!(path = %path, error = %e, "busca: ingestão falhou; fica para a próxima passada"),
        }
    }
    Ok(())
}

/// O que o arquivo já tem no índice e o que a leitura acrescenta.
#[derive(Default)]
pub(super) struct FileState {
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
}

/// Mensagem a indexar: (event_id, role, ts, texto).
pub(super) type Message = (String, &'static str, Option<f64>, String);

/// Uma linha completa (com o `\n`). Devolve as mensagens a indexar.
pub(super) fn absorb_line(st: &mut FileState, raw: &[u8], prefixes: &[String]) -> Vec<Message> {
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
    let events = parse_claude_object(obj);
    let msgs: Vec<_> = events.into_iter()
        .filter(|e| matches!(e.kind, ChatKind::UserMsg | ChatKind::AssistantMsg) && e.text.as_deref().is_some_and(|t| !t.is_empty()))
        .collect();
    let first_user = msgs.iter().find(|e| e.kind == ChatKind::UserMsg).and_then(|e| e.text.as_deref());
    if !st.head_done {
        if st.cwd.is_none() {
            st.cwd = obj.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned);
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
    if raw.len() > MAX_LINE_BYTES || !kept {
        return Vec::new();
    }
    msgs.into_iter().map(|e| {
        let role = if e.kind == ChatKind::UserMsg { "user" } else { "assistant" };
        (e.id, role, e.ts, e.text.unwrap_or_default())
    }).collect()
}

/// `archive._cortar`: corte na palavra e com reticências.
pub(super) fn cut(text: &str) -> String {
    const LIMIT: usize = 120;
    if text.chars().count() <= LIMIT { return text.to_owned(); }
    let mut short: String = text.chars().take(LIMIT).collect();
    if let Some(space) = short.rfind(' ') {
        if short[..space].chars().count() > LIMIT / 2 { short.truncate(space); }
    }
    format!("{}…", short.trim_end_matches([' ', ',', ';', ':', '.']))
}

fn ingest(tx: &Transaction, path: &str, found: &Found, old: Option<&Known>, prefixes: &[String]) -> Result<(), IndexError> {
    let stat = &found.stat;
    let fid = match old {
        None => {
            tx.execute("INSERT INTO files(path, project, session_id, ino, provider, config_dir, account) VALUES (?,?,?,?,'claude',?,?)",
                params![path, found.project, found.sid, stat.ino, found.account.config_dir, account_key(found.account)])?;
            tx.last_insert_rowid()
        }
        Some(k) => {
            if k.ino != stat.ino || stat.size < k.offset {
                tx.execute("DELETE FROM msg WHERE file_id=?", [k.id])?;
                tx.execute("UPDATE files SET offset=0, lines=0, cwd=NULL, preview='', head_done=0, internal=NULL, \
                            has_user=0, last_text='', rewrite=NULL, ino=? WHERE id=?", params![stat.ino, k.id])?;
            }
            k.id
        }
    };
    let mut st = tx.query_row(
        "SELECT offset, lines, cwd, preview, head_done, internal, has_user, last_text, rewrite FROM files WHERE id=?", [fid],
        |r| Ok(FileState {
            offset: r.get(0)?, lines: r.get(1)?, cwd: r.get(2)?, preview: r.get(3)?, head_done: r.get::<_, i64>(4)? != 0,
            internal: r.get::<_, Option<i64>>(5)?.map(|v| v != 0), has_user: r.get::<_, i64>(6)? != 0, last_text: r.get(7)?,
            rewrite: r.get::<_, Option<String>>(8)?.and_then(|s| RewriteFilter::from_state(&s)).unwrap_or_default(),
            illegible: 0,
        }))?;
    let mut rd = BufReader::new(File::open(path)?);
    rd.seek(SeekFrom::Start(st.offset as u64))?;
    let mut insert = tx.prepare_cached("INSERT INTO msg(file_id, event_id, role, ts, text) VALUES (?,?,?,?,?)")?;
    let mut exists = tx.prepare_cached("SELECT 1 FROM msg WHERE file_id=? AND event_id=?")?;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if rd.read_until(b'\n', &mut raw)? == 0 || !raw.ends_with(b"\n") {
            break;   // linha ainda sendo escrita: entra na próxima passada
        }
        let before = st.illegible;
        let msgs = absorb_line(&mut st, &raw, prefixes);
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
    // size/mtime da stat de antes da leitura: se o arquivo cresceu no meio, a próxima passada relê.
    tx.execute("UPDATE files SET size=?, mtime_ns=?, offset=?, lines=?, cwd=?, preview=?, head_done=?, internal=?, \
                has_user=?, last_text=?, rewrite=?, config_dir=?, account=? WHERE id=?",
        params![stat.size, stat.mtime_ns, st.offset, st.lines, st.cwd, st.preview, st.head_done as i64,
            st.internal.map(i64::from), st.has_user as i64, st.last_text, st.rewrite.to_state(),
            found.account.config_dir, account_key(found.account), fid])?;
    Ok(())
}
