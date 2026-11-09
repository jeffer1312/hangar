//! Coleta incremental com escopos nativos e uma varredura compartilhada por vez.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::{Duration, Instant};
use std::panic::{AssertUnwindSafe, catch_unwind};
use serde::{Deserialize, Serialize};
use indexmap::IndexMap;
use super::areas::AreaMap;
use super::index::{Index, IndexError, Progress};
use super::pricing::{Pricing, IGNORADOS, canonizar_provedor};
use super::rows::{UsageRow, UsoLinha};
use super::{claude, codex, simple};

const FRESHNESS: Duration = Duration::from_secs(30);
const FRESH_WAIT: Duration = Duration::from_secs(3);
// Falha que persiste (disco, escopos) não vira uma varredura por pedido.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Scopes {
    pub claude: Vec<ClaudeScope>, pub codex: Vec<CodexScope>, pub pi: Vec<PiScope>,
    pub kimi: Option<KimiScope>, pub repo: PathBuf,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ClaudeScope { pub root: PathBuf, pub account: String, pub label: String }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CodexScope { pub home: PathBuf, pub account: String, pub label: String }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PiScope { pub root: PathBuf, pub source: String }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct KimiScope { pub root: PathBuf, pub index: PathBuf }

#[derive(Debug)]
pub enum CollectError { NoScopes, Index(IndexError) }
impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScopes => f.write_str("escopos de custos indisponíveis"),
            Self::Index(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CollectError {}
impl From<IndexError> for CollectError { fn from(error: IndexError) -> Self { Self::Index(error) } }

pub trait ScopeSource: Send + Sync { fn fetch(&self) -> Result<Scopes, CollectError>; }

#[derive(Debug)]
pub enum Ready { Go, Warming { read: usize, total: usize } }

#[derive(Clone, Copy)]
enum Failure { Scopes, Disk, Reader, Database }
impl Failure {
    fn from_error(error: &CollectError) -> Self {
        match error {
            CollectError::NoScopes => Self::Scopes,
            CollectError::Index(IndexError::NoDisk) => Self::Disk,
            CollectError::Index(IndexError::ReaderPanic) => Self::Reader,
            CollectError::Index(IndexError::Sqlite(_)) => Self::Database,
        }
    }
    fn error(self) -> CollectError {
        match self {
            Self::Scopes => CollectError::NoScopes,
            Self::Disk => IndexError::NoDisk.into(),
            Self::Reader => IndexError::ReaderPanic.into(),
            Self::Database => IndexError::Sqlite(rusqlite::Error::InvalidQuery).into(),
        }
    }
}
struct ScanTicket { result: Mutex<Option<Result<(), Failure>>>, done: Condvar }
#[derive(Default)]
struct Scanner {
    attempted: bool,
    running: Option<Arc<ScanTicket>>,
    completed: Option<Arc<ScanTicket>>,
    last_success: Option<Instant>,
    failure: Option<Failure>,
    failed_at: Option<Instant>,
}
#[derive(Default)]
struct Metadata {
    last_good: Option<Scopes>,
    active: Option<Scopes>,
    labels: Vec<(String, String)>,
    kimi_projects: std::collections::HashMap<String, String>,
    generation: u64,
}

pub struct Collector {
    index: OnceLock<Index>, index_dir: PathBuf, index_init: Mutex<()>, pool: OnceLock<Arc<rayon::ThreadPool>>,
    pricing: Mutex<Pricing>, areas: AreaMap,
    source: Arc<dyn ScopeSource>, progress: Progress, metadata: Mutex<Metadata>,
    scanner: Mutex<Scanner>, completed_scans: AtomicU64,
    /// Causa da primeira pasta ilegível da última varredura (`costs_dir_<tipo>`), para o diário.
    unread_issue: Mutex<Option<String>>,
}
impl Collector {
    pub fn new(index_dir: PathBuf, pricing_dir: PathBuf, area_file: PathBuf, scopes: Arc<dyn ScopeSource>) -> Self {
        Self { index: OnceLock::new(), index_dir, index_init: Mutex::new(()), pool: OnceLock::new(),
            pricing: Mutex::new(Pricing::load(&pricing_dir)), areas: AreaMap::load(&area_file),
            source: scopes, progress: Progress::default(), metadata: Mutex::new(Metadata::default()),
            scanner: Mutex::new(Scanner::default()), completed_scans: AtomicU64::new(0),
            unread_issue: Mutex::new(None) }
    }

    /// Pasta que a última varredura não leu: as linhas dela ficaram como estavam.
    pub fn unread_issue(&self) -> Option<String> { self.unread_issue.lock().unwrap().clone() }

    pub fn schedule_warmup(self: &Arc<Self>, delay: Duration) {
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("custos-boot".into()).spawn(move || {
            std::thread::sleep(delay);
            if let Some(collector) = weak.upgrade() {
                let mut scanner = collector.scanner.lock().unwrap();
                if !scanner.attempted { collector.start_scan(&mut scanner); }
            }
        });
    }

    /// Workers síncronos podem esperar mesmo quando carregam o Handle do Tokio.
    pub fn prepare_blocking(self: &Arc<Self>, fresh: bool) -> Result<Ready, CollectError> {
        self.prepare_inner(fresh, true)
    }

    pub fn prepare(self: &Arc<Self>, fresh: bool) -> Result<Ready, CollectError> {
        self.prepare_inner(fresh, tokio::runtime::Handle::try_current().is_err())
    }

    fn prepare_inner(self: &Arc<Self>, fresh: bool, can_wait: bool) -> Result<Ready, CollectError> {
        let observed = self.completed_scans.load(Ordering::Acquire);
        self.pricing.lock().unwrap().reload_if_changed();
        let mut scanner = self.scanner.lock().unwrap();
        if !scanner.attempted {
            self.start_scan(&mut scanner);
            return Ok(self.warming());
        }
        if scanner.completed.is_none() { return Ok(self.warming()); }
        if !fresh {
            if let Some(failure) = scanner.failure {
                // Sem isto a falha ficava presa até um `fresco`: um pedido seguinte já pega a nova coleta.
                if scanner.running.is_none() && scanner.failed_at.is_none_or(|at| at.elapsed() >= RETRY_AFTER_FAILURE) {
                    self.start_scan(&mut scanner);
                }
                return Err(failure.error());
            }
            if scanner.last_success.is_some_and(|at| at.elapsed() >= FRESHNESS) && scanner.running.is_none() {
                self.start_scan(&mut scanner);
            }
            return Ok(Ready::Go);
        }
        // Uma conclusão durante a entrada já atende este pedido, sem repetir a coleta.
        let ticket = if self.completed_scans.load(Ordering::Acquire) != observed {
            scanner.completed.as_ref().unwrap().clone()
        } else if let Some(ticket) = &scanner.running {
            ticket.clone()
        } else { self.start_scan(&mut scanner) };
        drop(scanner);
        // A entrada explícita evita confundir o Handle de spawn_blocking com o executor.
        if !can_wait {
            return match *ticket.result.lock().unwrap() {
                Some(Ok(())) => Ok(Ready::Go), Some(Err(failure)) => Err(failure.error()), None => Ok(self.warming()),
            };
        }
        let result = ticket.result.lock().unwrap();
        let (result, _) = ticket.done.wait_timeout_while(result, FRESH_WAIT, |result| result.is_none()).unwrap();
        match *result {
            Some(Ok(())) => Ok(Ready::Go), Some(Err(failure)) => Err(failure.error()), None => Ok(self.warming()),
        }
    }

    fn warming(&self) -> Ready {
        let (read, total) = self.progress.total(); Ready::Warming { read, total }
    }

    fn start_scan(self: &Arc<Self>, scanner: &mut Scanner) -> Arc<ScanTicket> {
        let ticket = Arc::new(ScanTicket { result: Mutex::new(None), done: Condvar::new() });
        scanner.attempted = true;
        scanner.running = Some(ticket.clone());
        self.progress.reset();
        let collector = self.clone(); let running = ticket.clone();
        let spawned = std::thread::Builder::new().name("custos-scan".into()).spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| collector.scan()))
                .unwrap_or_else(|_| Err(IndexError::ReaderPanic.into())).map_err(|e| Failure::from_error(&e));
            let mut scanner = collector.scanner.lock().unwrap();
            scanner.running = None;
            scanner.completed = Some(running.clone());
            scanner.failure = result.err();
            scanner.failed_at = scanner.failure.map(|_| Instant::now());
            if result.is_ok() { scanner.last_success = Some(Instant::now()); }
            *running.result.lock().unwrap() = Some(result);
            collector.completed_scans.fetch_add(1, Ordering::Release);
            running.done.notify_all();
        });
        if spawned.is_err() {
            scanner.running = None; scanner.completed = Some(ticket.clone()); scanner.failure = Some(Failure::Disk);
            scanner.failed_at = Some(Instant::now());
            *ticket.result.lock().unwrap() = Some(Err(Failure::Disk));
            self.completed_scans.fetch_add(1, Ordering::Release);
            ticket.done.notify_all();
        }
        ticket
    }

    fn scan(&self) -> Result<(), CollectError> {
        // Varredura que falha antes da listagem não pode citar a pasta de uma varredura antiga.
        *self.unread_issue.lock().unwrap() = None;
        let scopes = match self.source.fetch() {
            Ok(scopes) => {
                let mut labels = IndexMap::new();
                for scope in &scopes.claude { labels.insert(scope.account.clone(), scope.label.clone()); }
                for scope in &scopes.codex { labels.insert(scope.account.clone(), scope.label.clone()); }
                let labels: Vec<_> = labels.into_iter().collect();
                let mut metadata = self.metadata.lock().unwrap();
                if metadata.labels != labels { metadata.generation += 1; }
                metadata.labels = labels; metadata.last_good = Some(scopes.clone());
                scopes
            },
            Err(_) => {
                tracing::warn!(code = "escopos_custos");
                self.metadata.lock().unwrap().last_good.clone().ok_or(CollectError::NoScopes)?
            },
        };
        let index = self.index()?;
        let mut unread_kinds = Vec::new();
        let (owners, codex_unread, codex_blocked) = rollout_listing(&scopes.codex, &mut unread_kinds);
        let mut active = scopes.clone();
        active.codex = owners.values().map(|(scope, _)| scope.clone()).chain(codex_blocked.iter().cloned()).collect();
        let pi_root = scopes.pi.iter().find(|s| s.source == "pi").map(|s| &s.root);
        active.pi.retain(|scope| scope.root.is_dir() && !(scope.source == "omp" && pi_root == Some(&scope.root)));
        active.kimi = scopes.kimi.filter(|scope| scope.root.is_dir());
        let mut claude_files = Vec::new(); let mut pi_files = Vec::new();
        for scope in &active.claude {
            let listing = list_dir(&scope.root, |n| n.ends_with(".jsonl"), &mut unread_kinds);
            self.progress.set(&claude_key(scope), 0, listing.files.len()); claude_files.push(listing);
        }
        for (scope, files) in owners.values() { self.progress.set(&scope.account, 0, files.len()); }
        for scope in &active.pi {
            let listing = list_dir(&scope.root, |n| n.ends_with(".jsonl"), &mut unread_kinds);
            self.progress.set(&pi_key(scope), 0, listing.files.len()); pi_files.push(listing);
        }
        let kimi_files = active.kimi.as_ref().map(|scope| {
            let listing = list_dir(&scope.root, |n| n == "wire.jsonl", &mut unread_kinds);
            self.progress.set(&kimi_key(scope), 0, listing.files.len()); listing
        });
        *self.unread_issue.lock().unwrap() = unread_kinds.first().map(|kind| format!("costs_dir_{}", kind_slug(*kind)));
        let redo = |entries: &_| self.areas.area_lines(entries);
        let signature = self.areas.signature(); let mut keys = Vec::new();
        for (scope, listing) in active.claude.iter().zip(claude_files) {
            let key = claude_key(scope);
            index.sync_keeping(&key, &listing.files, &listing.unread, &claude::new_fold(&scope.root), claude::VERSION, signature, &redo, &self.progress)?;
            keys.push(key);
        }
        // Conta sem pasta resolvível: sem varredura, linhas como estavam, e ativa no relatório.
        keys.extend(codex_blocked.iter().map(|scope| scope.account.clone()));
        for (scope, files) in owners.values() {
            index.sync_keeping(&scope.account, files, &codex_unread, &codex::new_fold, codex::VERSION, signature, &redo, &self.progress)?;
            keys.push(scope.account.clone());
        }
        for (scope, listing) in active.pi.iter().zip(pi_files) {
            let key = pi_key(scope);
            index.sync_keeping(&key, &listing.files, &listing.unread, &simple::new_pi_fold(&scope.root, &scope.source), simple::PI_VERSION, signature, &redo, &self.progress)?;
            keys.push(key);
        }
        if let (Some(scope), Some(listing)) = (&active.kimi, kimi_files) {
            let key = kimi_key(scope);
            index.sync_keeping(&key, &listing.files, &listing.unread, &simple::new_kimi_fold, simple::KIMI_VERSION, signature, &redo, &self.progress)?;
            keys.push(key);
        }
        index.forget_outside(&keys)?;
        let mut metadata = self.metadata.lock().unwrap();
        if metadata.active.as_ref() != Some(&active) { metadata.generation += 1; }
        metadata.active = Some(active);
        Ok(())
    }

    fn active_scopes(&self) -> Result<Scopes, CollectError> {
        let scanner = self.scanner.lock().unwrap();
        if let Some(failure) = scanner.failure { return Err(failure.error()); }
        drop(scanner);
        self.metadata.lock().unwrap().active.clone().ok_or(CollectError::NoScopes)
    }

    fn claude_rows(&self, scope: &ClaudeScope, since: Option<&str>, stamp: bool, pricing: &Pricing) -> Result<Vec<UsageRow>, CollectError> {
        let mut rows = self.index()?.read_costs(Some(&claude_key(scope)), since, None)?;
        rows.retain(|row| !IGNORADOS.contains(&crate::transcript::py::strip(&row.model)));
        for row in &mut rows {
            row.model = crate::transcript::py::strip(&row.model).to_owned();
            if row.project.is_empty() { row.project = "desconhecido".into(); }
            let provider = canonizar_provedor(&pricing.provider_for(&row.model).unwrap_or_default());
            row.provider = if provider.is_empty() || provider == "anthropic" { scope.account.clone() } else { provider };
            row.account_id = stamp.then(|| scope.account.clone());
        }
        Ok(rows)
    }

    pub fn read_costs(&self, since: Option<&str>) -> Result<Vec<UsageRow>, CollectError> {
        let active = self.active_scopes()?; let index = self.index()?; let mut output = Vec::new();
        {
            let pricing = self.pricing.lock().unwrap();
            for scope in &active.claude { output.extend(self.claude_rows(scope, since, false, &pricing)?); }
        }
        for scope in &active.codex {
            let mut rows = index.read_costs(Some(&scope.account), since, None)?;
            for row in &mut rows {
                row.account_id = Some(scope.account.clone());
                if row.provider == "openai" { row.provider = scope.account.clone(); }
            }
            output.extend(rows);
        }
        for scope in &active.pi { output.extend(index.read_costs(Some(&pi_key(scope)), since, None)?); }
        if let Some(scope) = &active.kimi {
            let projects = simple::kimi_projects(&scope.index);
            let mut rows = index.read_costs(Some(&kimi_key(scope)), since, None)?;
            for row in &mut rows {
                row.project = projects.get(&row.session_id).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "desconhecido".into());
            }
            output.extend(rows);
        }
        Ok(output)
    }

    pub fn read_usage(&self, since: Option<&str>) -> Result<(Vec<(UsoLinha, String)>, Vec<UsageRow>), CollectError> {
        let active = self.active_scopes()?; let index = self.index()?;
        let mut usage = Vec::new(); let mut tokens = Vec::new();
        {
            let pricing = self.pricing.lock().unwrap();
            for scope in &active.claude {
                tokens.extend(self.claude_rows(scope, since, true, &pricing)?);
                index.append_usage(&claude_key(scope), since, &mut usage, |row| (row, scope.account.clone()))?;
            }
        }
        for scope in &active.codex {
            index.append_usage(&scope.account, since, &mut usage, |row| (row, scope.account.clone()))?;
        }
        Ok((usage, tokens))
    }

    /// `read_usage` sem materializar o uso: `start` recebe os tokens antes da primeira linha, e
    /// as linhas chegam na mesma ordem, com a conta. A tarifa fica travada até o fim.
    pub fn fold_usage<B: Clone>(&self, since: Option<&str>, start: impl FnOnce(&[UsageRow], &Pricing) -> B,
                                visit: &mut dyn FnMut(&mut B, &UsoLinha, &str, &Pricing)) -> Result<B, CollectError> {
        let active = self.active_scopes()?; let index = self.index()?;
        let pricing = self.pricing.lock().unwrap();
        let mut tokens = Vec::new();
        for scope in &active.claude { tokens.extend(self.claude_rows(scope, since, true, &pricing)?); }
        let mut state = start(&tokens, &pricing);
        drop(tokens);
        let scopes = active.claude.iter().map(|s| (claude_key(s), &s.account))
            .chain(active.codex.iter().map(|s| (s.account.clone(), &s.account)));
        for (key, account) in scopes {
            state = index.fold_usage(&key, since, state, &mut |b, row| visit(b, &row, account, &pricing))?;
        }
        Ok(state)
    }

    pub fn index(&self) -> Result<&Index, CollectError> {
        if let Some(index) = self.index.get() { return Ok(index); }
        // Construir o estado das rotas não deve tocar o índice de produção nos testes.
        let _init = self.index_init.lock().unwrap();
        if self.index.get().is_none() {
            if self.pool.get().is_none() {
                let threads = std::thread::available_parallelism().map_or(1, usize::from).min(4);
                let pool = rayon::ThreadPoolBuilder::new().num_threads(threads)
                    .thread_name(|i| format!("custos-{i}")).build().map_err(|_| IndexError::NoDisk)?;
                let _ = self.pool.set(Arc::new(pool));
            }
            let index = Index::open(&self.index_dir)?.with_pool(self.pool.get().unwrap().clone());
            let _ = self.index.set(index);
        }
        Ok(self.index.get().unwrap())
    }
    pub fn label(&self, key: &str) -> Option<String> {
        self.metadata.lock().unwrap().labels.iter().find(|(name, _)| name == key).map(|(_, label)| label.clone())
    }
    pub fn labels_key(&self) -> Vec<(String, String)> {
        self.metadata.lock().unwrap().labels.clone()
    }
    pub fn data_version(&self) -> u64 {
        // Nunca segura metadados junto de Pricing: os relatórios podem já ter a tarifa travada.
        let mut metadata = self.metadata.lock().unwrap();
        let projects = metadata.active.as_ref().and_then(|s| s.kimi.as_ref()).map(|s| simple::kimi_projects(&s.index)).unwrap_or_default();
        if projects != metadata.kimi_projects { metadata.kimi_projects = projects; metadata.generation += 1; }
        metadata.generation + self.index.get().map_or(0, Index::generation)
    }
    pub fn pricing(&self) -> MutexGuard<'_, Pricing> { self.pricing.lock().unwrap() }
    pub fn areas(&self) -> &AreaMap { &self.areas }
    pub fn repo(&self) -> Option<PathBuf> { self.metadata.lock().unwrap().active.as_ref().map(|s| s.repo.clone()) }
}

fn claude_key(scope: &ClaudeScope) -> String { format!("claude:{}", scope.root.display()) }
fn pi_key(scope: &PiScope) -> String { format!("{}:{}", scope.source, scope.root.display()) }
fn kimi_key(scope: &KimiScope) -> String { format!("kimi:{}", scope.root.display()) }

/// Só para quem não apaga nada a partir da lista; a varredura usa `rollout_listing`.
pub fn rollout_owners(codex: &[CodexScope]) -> IndexMap<String, (CodexScope, Vec<PathBuf>)> {
    rollout_listing(codex, &mut Vec::new()).0
}

/// Donos dos rollouts e as pastas que não deu para ler, que nenhum dono pode apagar.
/// Pasta ou arquivo que não deu para ler: causa no log (uma linha por tipo e minuto) e na lista.
fn unreadable(error: &std::io::Error, kinds: &mut Vec<std::io::ErrorKind>) {
    if crate::warn_limit::allow(None, &format!("custos_pasta_ilegivel:{:?}", error.kind())) {
        tracing::warn!(code = "custos_pasta_ilegivel", kind = ?error.kind(), os = ?error.raw_os_error());
    }
    kinds.push(error.kind());
}

/// Donos dos rollouts, as pastas que não deu para ler (chave canônica, que nenhum dono pode
/// apagar) e as contas cuja pasta nem se resolve: essas seguem ativas sem varredura.
#[allow(clippy::type_complexity)]
fn rollout_listing(codex: &[CodexScope], kinds: &mut Vec<std::io::ErrorKind>)
    -> (IndexMap<String, (CodexScope, Vec<PathBuf>)>, Vec<PathBuf>, Vec<CodexScope>) {
    let (mut unread, mut blocked) = (Vec::new(), Vec::new());
    let homes: Vec<_> = codex.iter().map(|scope| match std::fs::canonicalize(&scope.home) {
        Ok(home) => Some(home),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => { unreadable(&error, kinds); blocked.push(scope.clone()); None },
    }).collect();
    let mut owners: IndexMap<String, (CodexScope, Vec<PathBuf>)> = IndexMap::new();
    for (scope, home) in codex.iter().zip(&homes) {
        let Some(home) = home else { continue };
        for name in ["sessions", "archived_sessions"] {
            let root = scope.home.join(name);
            let listing = list_dir(&root, |n| n.starts_with("rollout-") && n.ends_with(".jsonl"), kinds);
            // A chave do índice é canônica: a pasta mantida vira canônica pela raiz já resolvida,
            // mesmo quando ela própria não se resolve.
            unread.extend(listing.unread.iter().map(|dir| std::fs::canonicalize(dir)
                .unwrap_or_else(|_| home.join(name).join(dir.strip_prefix(&root).unwrap_or(Path::new(""))))));
            for path in listing.files {
                let path = match std::fs::canonicalize(&path) {
                    Ok(path) => path,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    // Arquivo que não deu para resolver não sumiu: a linha dele fica.
                    Err(error) => {
                        unreadable(&error, kinds);
                        unread.push(home.join(name).join(path.strip_prefix(&root).unwrap_or(&path)));
                        continue;
                    },
                };
                let mut matches = codex.iter().zip(&homes).filter(|(_, home)| home.as_ref().is_some_and(|home| {
                    path.starts_with(home.join("sessions")) || path.starts_with(home.join("archived_sessions"))
                }));
                let Some((owner, _)) = matches.next() else { continue };
                if matches.next().is_some() { continue; }
                let (_, files) = owners.entry(owner.account.clone()).or_insert_with(|| (owner.clone(), Vec::new()));
                if !files.contains(&path) { files.push(path); }
            }
        }
    }
    // Conta com pasta ilegível segue ativa mesmo sem arquivo listado: senão as linhas mantidas
    // dela sairiam do relatório como se a conta tivesse acabado.
    for (scope, home) in codex.iter().zip(&homes) {
        if home.as_ref().is_some_and(|home| unread.iter().any(|dir| dir.starts_with(home))) {
            owners.entry(scope.account.clone()).or_insert_with(|| (scope.clone(), Vec::new()));
        }
    }
    for (_, files) in owners.values_mut() { files.sort(); }
    (owners, unread, blocked)
}

/// Arquivos achados e as pastas que não deu para ler. Pasta ilegível não é pasta vazia: quem
/// sincroniza mantém as linhas que já tinha debaixo dela.
pub struct Listing { pub files: Vec<PathBuf>, pub unread: Vec<PathBuf> }

/// Só para quem não apaga nada a partir da lista (contagens, testes); a varredura usa `list_dir`.
pub fn list_files(root: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    list_dir(root, matches, &mut Vec::new()).files
}

/// `kinds` recebe a causa de cada pasta ilegível. Pasta que sumiu (`NotFound`) é pasta sem
/// arquivos: o que estava nela foi apagado mesmo.
pub fn list_dir(root: &Path, matches: impl Fn(&str) -> bool, kinds: &mut Vec<std::io::ErrorKind>) -> Listing {
    let (mut files, mut unread) = (Vec::new(), Vec::new());
    let mut failed = |path: PathBuf, error: std::io::Error, unread: &mut Vec<PathBuf>| {
        unreadable(&error, kinds);
        unread.push(path);
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => { failed(path, error, &mut unread); continue },
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                // A listagem parou no meio: o resto da pasta é desconhecido.
                Err(error) => { failed(path.clone(), error, &mut unread); break },
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(entry.path()),
                Ok(_) => if matches(&entry.file_name().to_string_lossy()) { files.push(entry.path()) },
                Err(error) => failed(entry.path(), error, &mut unread),
            }
        }
    }
    // O caminho textual acompanha a ordem usada na leitura do índice.
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Listing { files, unread }
}

/// `PermissionDenied` → `permission_denied`, para caber no código do diário (`[a-z0-9_]`).
fn kind_slug(kind: std::io::ErrorKind) -> String {
    let mut slug = String::new();
    for (i, c) in format!("{kind:?}").chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 { slug.push('_'); }
        if c.is_ascii_alphanumeric() { slug.push(c.to_ascii_lowercase()); }
    }
    slug
}
