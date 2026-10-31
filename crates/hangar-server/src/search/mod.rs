// crates/hangar-server/src/search/mod.rs
//! Índice SQLite (FTS5) das conversas Claude de todas as contas, porte de
//! `backend/app/transcript_index.py`. Um escritor só (o produtor ou a passada disparada pela busca,
//! sob a mesma trava); cada leitura abre conexão própria e o WAL a deixa ler durante a escrita.
//! Também guarda por arquivo o que o Arquivo precisa (cabeçalho, conta, se tem usuário, última
//! mensagem), para listar sem reabrir os transcripts.

mod db;
pub(crate) mod hits;
pub mod http;
mod ingest;
mod watch;
#[cfg(test)]
mod tests;

pub use watch::Producer;
pub(crate) use ingest::{FileState, absorb_line, cut, internal_prefixes};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension};
use unicode_normalization::UnicodeNormalization;

pub const FILE_NAME: &str = "busca-rust.sqlite3";
const PER_FILE: usize = 3;
// Mensagens casadas mais novas consideradas antes do teto por arquivo: termo comum não ordena tudo.
const CANDIDATES: usize = 2000;

/// Uma conta Claude a varrer: `config_dir` como o catálogo o escreve (`None` = a do servidor),
/// `dir` a pasta da conta (dá o `account`) e `projects` a pasta das conversas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub config_dir: Option<String>,
    pub dir: PathBuf,
    pub projects: PathBuf,
}

/// Erro = catálogo ilegível agora; a passada não tira nada do índice por causa dele.
pub type AccountSource = Arc<dyn Fn() -> Result<Vec<Account>, String> + Send + Sync>;
/// Linha do diário: código e detalhe. Nunca texto de conversa.
pub type Reporter = Arc<dyn Fn(&'static str, String) + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// A 1ª passada espera a subida pagar sondas de CLI e a coleta de custos.
    pub first_pass: Duration,
    /// Calma depois do último evento de arquivo antes da passada.
    pub debounce: Duration,
    /// Sem observador, busca com índice mais velho que isto dispara uma passada em segundo plano.
    pub stale: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { first_pass: Duration::from_secs(90), debounce: Duration::from_millis(500), stale: Duration::from_secs(5) }
    }
}

#[derive(Debug)]
pub enum IndexError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// Catálogo de contas ilegível: a passada não roda, o índice fica como estava.
    Catalog(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Catalog(code) => write!(f, "catálogo de contas: {code}"),
        }
    }
}

impl std::error::Error for IndexError {}
impl From<rusqlite::Error> for IndexError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<std::io::Error> for IndexError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

/// Um trecho achado: o `(path, project, session_id, cwd, text, role, event_id, ts)` do Python mais
/// agente e conta.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: String,
    pub project: String,
    pub session_id: String,
    pub cwd: Option<String>,
    pub text: String,
    pub role: String,
    pub event_id: String,
    pub ts: Option<f64>,
    pub provider: String,
    pub config_dir: Option<String>,
    pub account: String,
}

/// O que o Arquivo lê de um transcript sem abri-lo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    pub has_user: bool,
    pub last_text: String,
    pub account: String,
    pub config_dir: Option<String>,
    pub provider: String,
}

/// Um transcript indexado como o Arquivo o lê, com o tamanho e o relógio da leitura: arquivo que
/// mudou depois dela é lido do disco.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveRow {
    pub size: i64,
    pub mtime_ns: i64,
    pub preview: String,
    pub cwd: Option<String>,
    pub has_user: bool,
    pub internal: bool,
    pub last_text: String,
}

impl ArchiveRow {
    /// O arquivo está como o índice o leu (tamanho e relógio do stat de agora).
    pub fn fresh(&self, md: &std::fs::Metadata) -> bool {
        let stat = ingest::Stat::of(md);
        (self.size, self.mtime_ns) == (stat.size, stat.mtime_ns)
    }
}

#[derive(Clone)]
pub struct Index {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    accounts: AccountSource,
    report: Reporter,
    timing: Timing,
    writer: Mutex<Option<Connection>>,
    ready: AtomicBool,
    last_pass: Mutex<Option<Instant>>,
    watch_ok: AtomicBool,
    started: AtomicBool,
    /// Acorda o produtor depois de uma passada que ele não fez (refazer o observador).
    wake: Mutex<Option<std::sync::mpsc::Sender<watch::Wake>>>,
    /// Pastas que o produtor vigia agora; `None` = produtor parado ou observador fora.
    watched: Mutex<Option<watch::Targets>>,
}

impl Index {
    /// Não toca no disco: o banco abre com o produtor ou com a primeira passada.
    pub fn new(dir: &Path, accounts: AccountSource, report: Reporter, timing: Timing) -> Self {
        Self { inner: Arc::new(Inner {
            path: dir.join(FILE_NAME), accounts, report, timing,
            writer: Mutex::new(None), ready: AtomicBool::new(false), last_pass: Mutex::new(None),
            watch_ok: AtomicBool::new(false), started: AtomicBool::new(false), wake: Mutex::new(None),
            watched: Mutex::new(None),
        }) }
    }

    /// O do servidor: contas do catálogo e a do processo, banco ao lado do de custos, diário do Rust.
    pub fn server(service: crate::accounts::AccountService) -> Self {
        let report: Reporter = Arc::new(|code, detail| {
            crate::diag::record_blocking(crate::diag::diary_dir().as_deref(),
                &[serde_json::json!({"evento": code, "nivel": "aviso", "detalhe": detail})], "busca");
        });
        Self::for_accounts(service, &crate::costs::index::default_dir(), report, Timing::default())
    }

    /// Contas do catálogo do serviço e a do processo; banco em `dir`.
    pub fn for_accounts(service: crate::accounts::AccountService, dir: &Path, report: Reporter, timing: Timing) -> Self {
        let server_dir = service.env.claude_base.clone();
        let accounts: AccountSource = Arc::new(move || {
            let rows = service.claude_catalog().map_err(|e| e.code.to_string())?;
            let dirs: Vec<String> = rows.as_array().into_iter().flatten()
                .filter_map(|r| r["path"].as_str().filter(|p| !p.is_empty()).map(str::to_owned)).collect();
            Ok(server_accounts(&dirs, &server_dir))
        });
        Self::new(dir, accounts, report, timing)
    }

    /// As contas que a passada varreria agora (a busca sem índice varre as mesmas).
    pub(crate) fn accounts(&self) -> Result<Vec<Account>, String> { (self.inner.accounts)() }

    /// Linha no diário do índice: código e detalhe, nunca texto de conversa.
    pub(crate) fn report(&self, code: &'static str, detail: String) { (self.inner.report)(code, detail) }

    pub fn path(&self) -> &Path { &self.inner.path }

    /// A 1ª passada completa já terminou (`meta.built`); antes disso a busca não responde pelo índice.
    pub fn ready(&self) -> bool { self.inner.ready.load(Ordering::Acquire) }

    /// Uma passada completa: contas relidas, arquivo novo entra, mudado é lido do offset, sumido sai.
    pub fn update(&self) -> Result<(), IndexError> {
        let mut writer = self.inner.writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.pass(&mut writer)
    }

    fn pass(&self, writer: &mut Option<Connection>) -> Result<(), IndexError> {
        if writer.is_none() {
            *writer = Some(self.open()?);
        }
        let conn = writer.as_mut().unwrap();
        let started = Instant::now();
        let accounts = (self.inner.accounts)().map_err(IndexError::Catalog)?;
        ingest::pass(conn, &accounts, &*self.inner.report)?;
        if !self.ready() {
            conn.execute("INSERT OR REPLACE INTO meta(k, v) VALUES ('built', ?)", [now_text()])?;
            self.inner.ready.store(true, Ordering::Release);
        }
        *self.inner.last_pass.lock().unwrap() = Some(Instant::now());
        let spent = started.elapsed();
        // Passada normal leva milissegundos: só a lenta merece linha no journal.
        if spent > Duration::from_secs(1) {
            tracing::info!(secs = spent.as_secs_f64(), "índice da busca atualizado");
        } else {
            tracing::debug!(secs = spent.as_secs_f64(), "índice da busca atualizado");
        }
        Ok(())
    }

    /// Abre (ou reconstrói) o banco e lê se ele já estava pronto.
    fn open(&self) -> Result<Connection, IndexError> {
        let conn = db::open(&self.inner.path)?;
        let built: Option<String> = conn.query_row("SELECT v FROM meta WHERE k='built'", [], |r| r.get(0)).optional()?;
        self.inner.ready.store(built.is_some(), Ordering::Release);
        Ok(conn)
    }

    /// Abre o banco agora (subida do produtor): índice já pronto em disco responde antes da 1ª passada.
    pub fn open_now(&self) -> Result<(), IndexError> {
        let mut writer = self.inner.writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if writer.is_none() {
            *writer = Some(self.open()?);
        }
        Ok(())
    }

    /// Nunca bloqueia quem busca: sem observador de pé, índice velho ganha uma passada numa thread.
    fn update_if_stale(&self) {
        let inner = &self.inner;
        if inner.watch_ok.load(Ordering::Acquire) {
            return;
        }
        let fresh = inner.last_pass.lock().unwrap().is_some_and(|t| t.elapsed() < inner.timing.stale);
        if fresh {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("busca-indice-busca".into()).spawn(move || {
            let mut writer = match me.inner.writer.try_lock() {
                Ok(writer) => writer,
                // Pânico de uma passada antiga não pode travar as próximas.
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            if let Err(e) = me.pass(&mut writer) {
                tracing::warn!(error = %e, "passada do índice na busca falhou");
            }
            drop(writer);
            me.wake_producer(watch::Wake::Rewatch);
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "busca: passada em segundo plano não iniciou");
        }
    }

    fn wake_producer(&self, wake: watch::Wake) {
        if let Some(tx) = self.inner.wake.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            let _ = tx.send(wake);
        }
    }

    /// Conta nova ou removida do catálogo não escreve nas pastas vigiadas: quem lê confere as pastas
    /// que o catálogo pede agora e, mudou, acorda o produtor (a resposta desta leitura não espera).
    fn notice_catalog(&self) {
        let watched = self.inner.watched.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let Some(watched) = watched else { return };
        if watch::targets(self).is_some_and(|now| now != watched) {
            self.wake_producer(watch::Wake::Changed);
        }
    }

    fn reader(&self) -> rusqlite::Result<Connection> {
        let conn = Connection::open(&self.inner.path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        Ok(conn)
    }

    /// Trechos dos termos, arquivo mais novo primeiro, até 3 por arquivo, conversa interna fora.
    /// `None`: índice ainda não pronto ou nenhum termo (quem chama procura de outro jeito).
    pub fn search(&self, terms: &[String], limit: usize) -> Result<Option<Vec<Row>>, IndexError> {
        if terms.is_empty() || !self.ready() {
            return Ok(None);
        }
        self.update_if_stale();
        self.notice_catalog();
        let (long, short): (Vec<&String>, Vec<&String>) = terms.iter().partition(|t| t.chars().count() >= 3);
        let short: Vec<String> = short.into_iter().map(|t| fold(t)).collect();
        let matches = |text: &str| short.is_empty() || { let f = fold(text); short.iter().all(|t| f.contains(t.as_str())) };
        let conn = self.reader()?;
        let mut batch: Vec<Candidate> = Vec::new();
        if long.is_empty() {
            // Só termos curtos: o FTS não forma trigrama; percorre do mais novo, dobrando aqui.
            let mut stmt = conn.prepare(&format!(
                "SELECT {CANDIDATE_COLUMNS} FROM msg m JOIN files f ON f.id = m.file_id \
                 WHERE COALESCE(f.internal, 0) = 0 ORDER BY m.id DESC"))?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let c = Candidate::from_row(row)?;
                if matches(&c.row.text) {
                    batch.push(c);
                    if batch.len() >= CANDIDATES { break; }
                }
            }
        } else {
            let query = long.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" AND ");
            // O lote sai do FTS em rowid decrescente e para no LIMIT, como no Python.
            let mut stmt = conn.prepare(&format!(
                "SELECT {CANDIDATE_COLUMNS} FROM msg_fts JOIN msg m ON m.id = msg_fts.rowid JOIN files f ON f.id = m.file_id \
                 WHERE msg_fts MATCH ?1 AND COALESCE(f.internal, 0) = 0 ORDER BY msg_fts.rowid DESC LIMIT ?2"))?;
            let rows = stmt.query_map(rusqlite::params![query, CANDIDATES as i64], Candidate::from_row)?;
            for c in rows {
                let c = c?;
                if matches(&c.row.text) { batch.push(c); }
            }
        }
        Ok(Some(cap(batch, limit)))
    }

    /// path → (prévia, cwd) dos arquivos com cabeçalho completo; erro dá vazio (o Arquivo lê do disco).
    pub fn heads(&self) -> HashMap<String, (String, Option<String>)> {
        let read = || -> rusqlite::Result<HashMap<String, (String, Option<String>)>> {
            let conn = self.reader()?;
            let mut stmt = conn.prepare("SELECT path, preview, cwd FROM files WHERE head_done=1")?;
            let heads = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect();
            heads
        };
        if !self.inner.path.exists() {
            return HashMap::new();
        }
        self.notice_catalog();
        read().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "busca: cabeçalhos do índice ilegíveis");
            HashMap::new()
        })
    }

    /// Fatos de um transcript indexado; `None` = fora do índice (o Arquivo lê do disco).
    pub fn file_facts(&self, path: &str) -> Result<Option<FileFacts>, IndexError> {
        if !self.inner.path.exists() {
            return Ok(None);
        }
        let conn = self.reader()?;
        Ok(conn.query_row(
            "SELECT has_user, last_text, account, config_dir, provider FROM files WHERE path=?", [path],
            |r| Ok(FileFacts { has_user: r.get::<_, i64>(0)? != 0, last_text: r.get(1)?,
                account: r.get::<_, Option<String>>(2)?.unwrap_or_default(), config_dir: r.get(3)?, provider: r.get(4)? }),
        ).optional()?)
    }

    /// Todos os transcripts indexados numa leitura só (o Arquivo lista milhares); erro dá vazio e o
    /// Arquivo lê do disco, como o `_heads` do Python.
    pub fn archive_rows(&self) -> HashMap<String, ArchiveRow> {
        let read = || -> rusqlite::Result<HashMap<String, ArchiveRow>> {
            let conn = self.reader()?;
            let mut stmt = conn.prepare(
                "SELECT path, size, mtime_ns, preview, cwd, has_user, COALESCE(internal, 0), last_text FROM files")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, ArchiveRow { size: r.get(1)?, mtime_ns: r.get(2)?, preview: r.get(3)?,
                cwd: r.get(4)?, has_user: r.get::<_, i64>(5)? != 0, internal: r.get::<_, i64>(6)? != 0, last_text: r.get(7)? })))?.collect();
            rows
        };
        if !self.inner.path.exists() {
            return HashMap::new();
        }
        self.notice_catalog();
        read().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "arquivo: índice ilegível, lendo do disco");
            HashMap::new()
        })
    }

    /// Liga o produtor uma vez por processo (só a subida real chama).
    pub fn start(&self) -> Option<watch::Producer> {
        if self.inner.started.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(watch::spawn(self.clone()))
    }
}

/// As contas do servidor: as do catálogo e a do processo por último, sem repetir pelo caminho
/// resolvido, como `archive._contas`. Conta ainda sem `projects` fica: a pasta nasce com a 1ª conversa.
pub fn server_accounts(config_dirs: &[String], server_dir: &Path) -> Vec<Account> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let dirs = config_dirs.iter().map(|d| (Some(d.clone()), PathBuf::from(d))).chain([(None, server_dir.to_owned())]);
    for (config_dir, dir) in dirs {
        let projects = dir.join("projects");
        let key = crate::list::links::resolve_lenient(&projects);
        if seen.contains(&key) { continue; }
        seen.push(key);
        out.push(Account { config_dir, dir, projects });
    }
    out
}

const CANDIDATE_COLUMNS: &str = "m.id, m.file_id, f.mtime_ns, f.path, f.project, f.session_id, f.cwd, m.text, \
    m.role, m.event_id, m.ts, f.provider, f.config_dir, f.account";

struct Candidate { id: i64, file_id: i64, mtime_ns: i64, row: Row }

impl Candidate {
    fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Self { id: r.get(0)?, file_id: r.get(1)?, mtime_ns: r.get(2)?, row: Row {
            path: r.get(3)?, project: r.get(4)?, session_id: r.get(5)?, cwd: r.get(6)?, text: r.get(7)?,
            role: r.get(8)?, event_id: r.get(9)?, ts: r.get(10)?, provider: r.get(11)?, config_dir: r.get(12)?,
            account: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
        } })
    }
}

/// O `ROW_NUMBER() OVER (PARTITION BY file_id ORDER BY id)` e a ordem final do Python, sobre o lote.
fn cap(mut batch: Vec<Candidate>, limit: usize) -> Vec<Row> {
    batch.sort_by_key(|c| c.id);
    let mut seen: HashMap<i64, usize> = HashMap::new();
    batch.retain(|c| { let n = seen.entry(c.file_id).or_default(); *n += 1; *n <= PER_FILE });
    batch.sort_by(|a, b| b.mtime_ns.cmp(&a.mtime_ns).then(a.id.cmp(&b.id)));
    batch.into_iter().take(limit).map(|c| c.row).collect()
}

/// Minúsculas e sem marca combinante: o que o trigram com `remove_diacritics` ignora.
pub fn fold(text: &str) -> String {
    text.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

fn now_text() -> String {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()).to_string()
}
