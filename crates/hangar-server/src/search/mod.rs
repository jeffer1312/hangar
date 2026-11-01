// crates/hangar-server/src/search/mod.rs
//! Índice SQLite (FTS5) das conversas Claude de todas as contas, porte de
//! `backend/app/transcript_index.py`, mais as de Codex, Pi, omp e Kimi (`agents`). Um escritor só (o produtor ou a passada disparada pela busca,
//! sob a mesma trava); cada leitura abre conexão própria e o WAL a deixa ler durante a escrita.
//! Também guarda por arquivo o que o Arquivo precisa (cabeçalho, conta, se tem usuário, última
//! mensagem), para listar sem reabrir os transcripts.

mod agents;
pub mod ask;
mod db;
pub(crate) mod hits;
pub mod http;
mod ingest;
mod watch;
#[cfg(test)]
mod tests;

pub use watch::Producer;
pub(crate) use ingest::{FileState, absorb_line, cut, internal_prefixes};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension};

use crate::accounts::environment::AccountEnvironment;
use unicode_normalization::UnicodeNormalization;

pub const FILE_NAME: &str = "busca-rust.sqlite3";
const PER_FILE: usize = 3;
// Mensagens casadas mais novas consideradas antes do teto por arquivo: termo comum não ordena tudo.
const CANDIDATES: usize = 2000;
// Só termos curtos varrem `msg` linha a linha: termo raro não percorre o índice inteiro.
const SHORT_SCAN_ROWS: usize = if cfg!(test) { 50 } else { 200_000 };

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
    /// 1ª espera para abrir de novo o banco que não abriu; dobra até dez vezes ela.
    pub reopen: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { first_pass: Duration::from_secs(90), debounce: Duration::from_millis(500), stale: Duration::from_secs(5),
            reopen: Duration::from_secs(30) }
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
    /// A conta Codex dona do rollout; só no Codex.
    pub codex_account: Option<String>,
    /// 1ª mensagem do usuário do arquivo; vazia quando não há.
    pub preview: String,
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
    /// De onde saem as conversas de Codex, Pi, omp e Kimi; vazio = só Claude.
    agents: std::sync::OnceLock<AccountEnvironment>,
    /// Falhas já no diário, por (código, chave): só a entrada no estado de falha vai para lá.
    reported: Mutex<HashSet<(&'static str, String)>>,
    /// Bancos descartados por corrupção desde a última passada boa.
    rebuilds: std::sync::atomic::AtomicU32,
    /// O produtor está sendo solto: a passada para no próximo arquivo.
    stopping: AtomicBool,
    /// Relógio da conferência do catálogo; os testes trocam.
    clock: std::sync::OnceLock<Clock>,
    /// Última conferência do catálogo pelos leitores.
    last_notice: Mutex<Option<Instant>>,
}

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Busca, Arquivo e Perguntar leem em rajada: o catálogo de contas é conferido no máximo uma vez nisto.
const NOTICE_EVERY: Duration = Duration::from_secs(2);

const MAX_REBUILDS: u32 = 3;

impl Index {
    /// Não toca no disco: o banco abre com o produtor ou com a primeira passada.
    pub fn new(dir: &Path, accounts: AccountSource, report: Reporter, timing: Timing) -> Self {
        Self { inner: Arc::new(Inner {
            path: dir.join(FILE_NAME), accounts, report, timing,
            writer: Mutex::new(None), ready: AtomicBool::new(false), last_pass: Mutex::new(None),
            watch_ok: AtomicBool::new(false), started: AtomicBool::new(false), wake: Mutex::new(None),
            watched: Mutex::new(None), agents: std::sync::OnceLock::new(), reported: Mutex::new(HashSet::new()),
            rebuilds: std::sync::atomic::AtomicU32::new(0), stopping: AtomicBool::new(false),
            clock: std::sync::OnceLock::new(), last_notice: Mutex::new(None),
        }) }
    }

    #[cfg(test)]
    fn with_clock(self, clock: Clock) -> Self {
        let _ = self.inner.clock.set(clock);
        self
    }

    /// Liga as conversas de Codex, Pi, omp e Kimi do ambiente ao índice (uma vez, antes do produtor).
    pub fn with_agents(self, env: AccountEnvironment) -> Self {
        let _ = self.inner.agents.set(env);
        self
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
        let env = service.env.clone();
        let accounts: AccountSource = Arc::new(move || {
            let rows = service.claude_catalog().map_err(|e| e.code.to_string())?;
            let dirs: Vec<String> = rows.as_array().into_iter().flatten()
                .filter_map(|r| r["path"].as_str().filter(|p| !p.is_empty()).map(str::to_owned)).collect();
            Ok(server_accounts(&dirs, &server_dir))
        });
        Self::new(dir, accounts, report, timing).with_agents(env)
    }

    /// As contas que a passada varreria agora (a busca sem índice varre as mesmas).
    pub(crate) fn accounts(&self) -> Result<Vec<Account>, String> { (self.inner.accounts)() }

    /// De onde saem as conversas dos outros agentes; `None` = índice só do Claude.
    pub(crate) fn agents(&self) -> Option<&AccountEnvironment> { self.inner.agents.get() }

    /// Linha no diário do índice: código e detalhe, nunca texto de conversa.
    pub(crate) fn report(&self, code: &'static str, detail: String) { (self.inner.report)(code, detail) }

    /// `report` só na entrada em falha de `key`: a mesma falha a cada passada ou busca não se repete.
    pub(crate) fn report_once(&self, code: &'static str, key: &str, detail: impl FnOnce() -> String) {
        let new = self.inner.reported.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert((code, key.to_owned()));
        if new { self.logged(code, detail()); }
    }

    /// `key` voltou: a próxima falha dela volta ao diário.
    pub(crate) fn recovered(&self, code: &'static str, key: &str) {
        self.inner.reported.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&(code, key.to_owned()));
    }

    /// Todas as falhas de agora sob `code`, como (chave, detalhe): as novas vão ao diário e as que
    /// sumiram saem do registro, para voltarem ao diário se caírem de novo.
    pub(crate) fn failing(&self, code: &'static str, now: Vec<(String, String)>) {
        let fresh: Vec<String> = {
            let mut reported = self.inner.reported.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            reported.retain(|(c, k)| *c != code || now.iter().any(|(key, _)| key == k));
            now.into_iter().filter(|(key, _)| reported.insert((code, key.clone()))).map(|(_, detail)| detail).collect()
        };
        for detail in fresh { self.logged(code, detail); }
    }

    fn logged(&self, code: &'static str, detail: String) {
        tracing::warn!(code, detail = %detail, "busca: falha registrada no diário");
        self.report(code, detail);
    }

    pub fn path(&self) -> &Path { &self.inner.path }

    pub(super) fn stopping(&self) -> bool { self.inner.stopping.load(Ordering::Acquire) }

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
        if let Err(e) = ingest::pass(conn, &accounts, self.agents(), self) {
            // Conexão sobre arquivo corrompido não se recupera: solta, apaga, e a próxima passada refaz.
            if matches!(&e, IndexError::Sqlite(s) if db::corrupt(s)) {
                *writer = None;
                // O banco some agora: até a passada refazê-lo, a busca vai à varredura.
                self.inner.ready.store(false, Ordering::Release);
                tracing::warn!(error = %e, "índice da busca corrompido durante a passada, reconstruindo");
                match db::discard(&self.inner.path) {
                    Ok(()) => self.rebuild_after_corruption(),
                    Err(gone) => self.report_once("busca.indice_nao_apagado", "", || gone.to_string()),
                }
            }
            self.report_once("busca.ingestao_falhou", "", || e.to_string());
            return Err(e);
        }
        // Passada interrompida não está completa: nada de `built` nem de relógio da última passada.
        if self.stopping() {
            return Ok(());
        }
        self.recovered("busca.ingestao_falhou", "");
        self.inner.rebuilds.store(0, Ordering::Release);
        self.recovered("busca.indice_corrompido_seguido", "");
        if !self.ready() {
            conn.execute("INSERT OR REPLACE INTO meta(k, v) VALUES ('built', ?)", [now_text()])?;
            self.inner.ready.store(true, Ordering::Release);
        }
        *self.inner.last_pass.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =Some(Instant::now());
        let spent = started.elapsed();
        if slow_pass(spent) {
            tracing::info!(secs = spent.as_secs_f64(), "índice da busca atualizado");
        } else {
            tracing::debug!(secs = spent.as_secs_f64(), "índice da busca atualizado");
        }
        Ok(())
    }

    /// Sem escrita nova nada chamaria a passada que refaz o banco descartado: o produtor é acordado. Disco
    /// que corrompe todo banco novo faria disso um laço; depois de 3 seguidas, só a passada por evento.
    fn rebuild_after_corruption(&self) {
        let n = self.inner.rebuilds.fetch_add(1, Ordering::AcqRel) + 1;
        if n <= MAX_REBUILDS {
            self.wake_producer(watch::Wake::Changed);
        } else {
            self.report_once("busca.indice_corrompido_seguido", "", || format!("{n} reconstruções seguidas do índice corrompido; sem nova tentativa automática"));
        }
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
        let fresh = inner.last_pass.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some_and(|t| t.elapsed() < inner.timing.stale);
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
        let now = self.inner.clock.get().map_or_else(Instant::now, |clock| clock());
        {
            let mut last = self.inner.last_notice.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.is_some_and(|t| now.saturating_duration_since(t) < NOTICE_EVERY) {
                return;
            }
            *last = Some(now);
        }
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

    /// Trechos dos termos na ordem de `cap` (D28), até 3 por arquivo, conversa interna fora.
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
        let folded: Vec<String> = terms.iter().map(|t| fold(t)).collect();
        let conn = self.reader()?;
        let mut batch: Vec<Candidate> = Vec::new();
        if long.is_empty() {
            // Só termos curtos: o FTS não forma trigrama; percorre do mais novo, dobrando aqui.
            let mut stmt = conn.prepare(&format!(
                "SELECT {CANDIDATE_COLUMNS} FROM msg m JOIN files f ON f.id = m.file_id \
                 WHERE COALESCE(f.internal, 0) = 0 ORDER BY m.id DESC"))?;
            let mut rows = stmt.query([])?;
            let mut examined = 0;
            while let Some(row) = rows.next()? {
                examined += 1;
                if examined > SHORT_SCAN_ROWS { break; }
                let c = Candidate::from_row(row)?;
                if matches(&c.row.text) {
                    batch.push(c);
                    if batch.len() >= CANDIDATES { break; }
                }
            }
        } else {
            let query = long.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" AND ");
            // Cada lote sai do FTS em rowid decrescente e para no LIMIT, como no Python.
            let lot = |table: &str| -> Result<Vec<Candidate>, IndexError> {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {CANDIDATE_COLUMNS} FROM {table} JOIN msg m ON m.id = {table}.rowid JOIN files f ON f.id = m.file_id \
                     WHERE {table} MATCH ?1 AND COALESCE(f.internal, 0) = 0 ORDER BY {table}.rowid DESC LIMIT ?2"))?;
                let rows = stmt.query_map(rusqlite::params![query, CANDIDATES as i64], Candidate::from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            };
            // Palavra inteira com lote próprio (D28): pedaço comum ("estoura") não tira do lote a
            // conversa velha com "tour" inteiro. Termo só de pontuação não forma palavra: fica na trigram.
            if long.iter().all(|t| t.chars().any(char::is_alphanumeric)) {
                batch.extend(lot("msg_words")?.into_iter().filter(|c| matches(&c.row.text) && hits::whole(&c.row.text, &folded)));
            }
            let seen: HashSet<i64> = batch.iter().map(|c| c.id).collect();
            batch.extend(lot("msg_fts")?.into_iter().filter(|c| !seen.contains(&c.id) && matches(&c.row.text)));
        }
        Ok(Some(cap(batch, limit, &folded)))
    }

    /// Transcripts não internos do índice: as conversas que a busca considera.
    pub fn conversations(&self) -> Result<usize, IndexError> {
        let n: i64 = self.reader()?.query_row("SELECT COUNT(*) FROM files WHERE COALESCE(internal, 0) = 0", [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Os transcripts que o índice já marcou como internos; sem índice ou com erro, nenhum (o erro vai ao log).
    pub(crate) fn internal_paths(&self) -> HashSet<String> {
        if !self.inner.path.exists() {
            return HashSet::new();
        }
        let read = || -> rusqlite::Result<HashSet<String>> {
            let conn = self.reader()?;
            let mut stmt = conn.prepare("SELECT path FROM files WHERE internal = 1")?;
            let paths = stmt.query_map([], |r| r.get(0))?.collect();
            paths
        };
        read().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "busca: conversas internas do índice ilegíveis");
            HashSet::new()
        })
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
    m.role, m.event_id, m.ts, f.provider, f.config_dir, f.account, f.codex_account, f.preview";

struct Candidate { id: i64, file_id: i64, mtime_ns: i64, row: Row }

impl Candidate {
    fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Self { id: r.get(0)?, file_id: r.get(1)?, mtime_ns: r.get(2)?, row: Row {
            path: r.get(3)?, project: r.get(4)?, session_id: r.get(5)?, cwd: r.get(6)?, text: r.get(7)?,
            role: r.get(8)?, event_id: r.get(9)?, ts: r.get(10)?, provider: r.get(11)?, config_dir: r.get(12)?,
            account: r.get::<_, Option<String>>(13)?.unwrap_or_default(), codex_account: r.get(14)?, preview: r.get(15)?,
        } })
    }
}

/// Até 3 por arquivo, os de palavra inteira antes e depois a ordem do arquivo; arquivos com palavra
/// inteira primeiro, cada grupo do mais novo para o mais velho (D28).
fn cap(batch: Vec<Candidate>, limit: usize, folded: &[String]) -> Vec<Row> {
    let mut batch: Vec<(bool, Candidate)> = batch.into_iter().map(|c| (hits::whole(&c.row.text, folded), c)).collect();
    batch.sort_by_key(|(whole, c)| (!*whole, c.id));
    let mut seen: HashMap<i64, usize> = HashMap::new();
    batch.retain(|(_, c)| { let n = seen.entry(c.file_id).or_default(); *n += 1; *n <= PER_FILE });
    let led: HashSet<i64> = batch.iter().filter(|(whole, _)| *whole).map(|(_, c)| c.file_id).collect();
    batch.sort_by(|(wa, a), (wb, b)| led.contains(&b.file_id).cmp(&led.contains(&a.file_id))
        .then(b.mtime_ns.cmp(&a.mtime_ns)).then(a.file_id.cmp(&b.file_id)).then(wb.cmp(wa)).then(a.id.cmp(&b.id)));
    batch.into_iter().take(limit).map(|(_, c)| c.row).collect()
}

/// Minúsculas e sem marca combinante: o que o trigram com `remove_diacritics` ignora.
pub fn fold(text: &str) -> String {
    text.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

/// Passada normal leva milissegundos: só a lenta merece linha INFO no journal.
fn slow_pass(spent: Duration) -> bool { spent > Duration::from_secs(1) }

fn now_text() -> String {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()).to_string()
}
