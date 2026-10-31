// crates/hangar-server/src/search/ingest.rs
//! Uma passada do índice (`Index._update`/`_ingest` do Python): varre as contas, tira o que sumiu e lê
//! cada transcript mudado a partir do offset, só linhas completas, um arquivo por transação. As
//! mensagens passam pelo mesmo filtro de reescrita e pelo mesmo parser do histórico, então o id de
//! cada trecho existe no contexto.

use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
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
// O pedido do bastão só existe no Python (`bastao._REESCRITA_PEDIDO`).
const HANDOFF_PREFIX: &str = "Você recebeu abaixo um resumo automático de uma sessão de tr";

static SID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z").unwrap()
});

/// Os 60 primeiros caracteres de cada pedido interno do Hangar: conversa que começa assim fica fora.
pub(super) fn internal_prefixes() -> Vec<String> {
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
    fn of(md: &Metadata) -> Self {
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

pub(super) struct Found<'a> { pub project: String, pub sid: String, pub stat: Stat, pub account: &'a Account }

struct Known {
    id: i64, size: i64, mtime_ns: i64, ino: i64, offset: i64, config_dir: Option<String>,
    head_len: i64, head_hash: Option<String>,
}

/// Bytes do começo do arquivo guardados como hash: `cp` por cima mantém o inode e não encolhe.
const HEAD_BYTES: i64 = 4096;

/// Filhos diretos `<uuid>.jsonl` de cada pasta de projeto; a cópia `.from-<conta>` e `subagents/`
/// ficam fora. Devolve também o que não deu para ler agora (pasta ou arquivo): o que o índice já
/// tem lá dentro fica como está.
pub(super) fn conversations(accounts: &[Account]) -> (BTreeMap<String, Found<'_>>, Vec<PathBuf>) {
    let mut out = BTreeMap::new();
    let mut unreadable = Vec::new();
    let mut failed = |path: PathBuf, e: &std::io::Error| {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(path = %path.display(), error = %e, "busca: pasta ilegível agora; índice dela mantido");
            unreadable.push(path);
        }
    };
    for account in accounts {
        let projects = match std::fs::read_dir(&account.projects) {
            Ok(projects) => projects,
            Err(e) => { failed(account.projects.clone(), &e); continue; }
        };
        for project in projects.flatten() {
            let dir = project.path();
            if !dir.is_dir() { continue; }
            let files = match std::fs::read_dir(&dir) {
                Ok(files) => files,
                Err(e) => { failed(dir, &e); continue; }
            };
            for file in files.flatten() {
                let path: PathBuf = file.path();
                let (Some(stem), Some("jsonl")) = (path.file_stem().and_then(|s| s.to_str()), path.extension().and_then(|s| s.to_str())) else { continue };
                if !SID.is_match(stem) { continue; }
                let md = match std::fs::metadata(&path) {
                    Ok(md) => md,
                    Err(e) => { failed(path, &e); continue; }
                };
                if !md.is_file() { continue; }
                out.entry(path.to_string_lossy().into_owned()).or_insert(Found {
                    project: project.file_name().to_string_lossy().into_owned(), sid: stem.to_owned(), stat: Stat::of(&md), account,
                });
            }
        }
    }
    (out, unreadable)
}

fn head_hash(file: &mut File, len: i64) -> std::io::Result<String> {
    use md5::{Digest, Md5};
    use std::io::Read;
    let mut buf = vec![0; len as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut buf)?;
    Ok(format!("{:x}", Md5::digest(&buf)))
}

pub(super) fn account_key(account: &Account) -> String {
    format!("claude:{}", crate::list::links::resolve_lenient(&account.dir).to_string_lossy())
}

pub(super) type Report<'a> = &'a dyn Fn(&'static str, String);

pub(super) fn pass(conn: &mut Connection, accounts: &[Account], report: Report) -> Result<(), IndexError> {
    let (current, unreadable) = conversations(accounts);
    let known: BTreeMap<String, Known> = conn.prepare("SELECT path, id, size, mtime_ns, ino, offset, config_dir, head_len, head_hash FROM files")?
        .query_map([], |r| Ok((r.get(0)?, Known { id: r.get(1)?, size: r.get(2)?, mtime_ns: r.get(3)?, ino: r.get(4)?,
            offset: r.get(5)?, config_dir: r.get(6)?, head_len: r.get(7)?, head_hash: r.get(8)? })))?
        .collect::<Result<_, _>>()?;
    let listed: Vec<Option<&str>> = accounts.iter().map(|a| a.config_dir.as_deref()).collect();
    let tx = conn.transaction()?;
    for (path, k) in &known {
        if current.contains_key(path) || unreadable.iter().any(|d| Path::new(path).starts_with(d)) {
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
    for (path, found) in order {
        let old = known.get(path);
        if let Some(k) = old {
            if unchanged((k.size, k.mtime_ns, k.ino), &found.stat) {
                if k.config_dir != found.account.config_dir {
                    conn.execute("UPDATE files SET config_dir=?, account=? WHERE id=?",
                        params![found.account.config_dir, account_key(found.account), k.id])?;
                }
                continue;
            }
        }
        // Um arquivo = uma transação: linhas e offset entram juntos ou nenhum entra; sobra parcial com
        // offset velho viraria mensagem repetida na passada seguinte.
        let ino = found.stat.ino.unwrap_or_else(|| file_id(Path::new(path)));
        let tx = conn.transaction()?;
        match ingest(&tx, path, found, ino, old, &prefixes, report) {
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

#[allow(clippy::too_many_arguments)]
fn ingest(tx: &Transaction, path: &str, found: &Found, ino: i64, old: Option<&Known>, prefixes: &[String], report: Report) -> Result<(), IndexError> {
    let stat = &found.stat;
    let mut file = File::open(path)?;
    let fid = match old {
        None => {
            tx.execute("INSERT INTO files(path, project, session_id, ino, provider, config_dir, account) VALUES (?,?,?,?,'claude',?,?)",
                params![path, found.project, found.sid, ino, found.account.config_dir, account_key(found.account)])?;
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
    let (mut st, saved_rewrite) = tx.query_row(
        "SELECT offset, lines, cwd, preview, head_done, internal, has_user, last_text, rewrite FROM files WHERE id=?", [fid],
        |r| Ok((FileState {
            offset: r.get(0)?, lines: r.get(1)?, cwd: r.get(2)?, preview: r.get(3)?, head_done: r.get::<_, i64>(4)? != 0,
            internal: r.get::<_, Option<i64>>(5)?.map(|v| v != 0), has_user: r.get::<_, i64>(6)? != 0, last_text: r.get(7)?,
            rewrite: RewriteFilter::default(), illegible: 0,
        }, r.get::<_, Option<String>>(8)?)))?;
    if let Some(saved) = saved_rewrite {
        match RewriteFilter::from_state(&saved) {
            Some(filter) => st.rewrite = filter,
            // Sem o estado, linha regravada pelo `--resume` que chegar agora pode entrar no índice.
            None => report("busca.filtro_ilegivel", format!("{}/{}", found.project, found.sid)),
        }
    }
    let mut rd = BufReader::new(file);
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
    let head_len = st.offset.min(HEAD_BYTES);
    let head = if head_len > 0 { Some(head_hash(rd.get_mut(), head_len)?) } else { None };
    // size/mtime da stat de antes da leitura: se o arquivo cresceu no meio, a próxima passada relê.
    tx.execute("UPDATE files SET size=?, mtime_ns=?, offset=?, lines=?, cwd=?, preview=?, head_done=?, internal=?, \
                has_user=?, last_text=?, rewrite=?, config_dir=?, account=?, head_len=?, head_hash=? WHERE id=?",
        params![stat.size, stat.mtime_ns, st.offset, st.lines, st.cwd, st.preview, st.head_done as i64,
            st.internal.map(i64::from), st.has_user as i64, st.last_text, st.rewrite.to_state(),
            found.account.config_dir, account_key(found.account), head_len, head, fid])?;
    Ok(())
}
