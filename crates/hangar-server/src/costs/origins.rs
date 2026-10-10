//! Descoberta das origens de skills com atualização assíncrona após a primeira consulta.

use indexmap::IndexMap;
use std::{fs, path::{Path, PathBuf}, sync::{Arc, Mutex, OnceLock}, time::{Duration, Instant, SystemTime}};

const CHECK_NANOS: u64 = 30_000_000_000;
/// Idade mínima da data de uma pasta para valer como "não mudou".
const RACY: Duration = Duration::from_secs(2);
type Clock = dyn Fn() -> u64 + Send + Sync;
/// Data de cada pasta na última varredura; `None` = recente demais para valer, olhar de novo.
type Watch = Vec<(PathBuf, Option<Option<SystemTime>>)>;

fn roots(home: &Path, repo: &Path) -> ([PathBuf; 3], [PathBuf; 3]) {
    ([home.join(".claude/skills"), repo.join("skills"), home.join(".agents/skills")],
     [home.join(".claude/plugins/cache"), home.join(".codex/plugins/cache"), home.join(".claude/plugins/marketplaces")])
}

fn children(path: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(path).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort(); entries
}

fn deep_skills(path: &Path, out: &mut Vec<PathBuf>) {
    for child in children(path) {
        if child.file_name().is_some_and(|n| n == "SKILL.md")
            && child.components().any(|c| c.as_os_str() == "skills") {
            out.push(child);
        } else if fs::symlink_metadata(&child).is_ok_and(|m| m.is_dir()) {
            deep_skills(&child, out);
        }
    }
}

fn origin(path: &Path, home: &Path, repo: &Path) -> String {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    // A raiz e a skill precisam usar o mesmo caminho depois de resolver aliases.
    let repo = fs::canonicalize(repo).unwrap_or_else(|_| repo.to_owned());
    if real.starts_with(repo.join("skills")) { return "@repo".into(); }
    let text = real.to_string_lossy().replace('\\', "/");
    let parts: Vec<_> = text.split('/').collect();
    let mut qualified = real.file_name().unwrap_or_default().to_string_lossy().into_owned();
    if parts.contains(&"skills") {
        if let Some(i) = parts.iter().position(|p| *p == "plugins") {
            let plugin = match parts.get(i + 1) {
                Some(&"cache") => parts.get(i + 3).copied(),
                Some(&"marketplaces") => parts.get(i + 2).map(|p| p.strip_suffix("-marketplace").unwrap_or(p)),
                _ => None,
            };
            if let Some(plugin) = plugin.filter(|s| !s.is_empty()) { qualified = format!("{plugin}:{qualified}"); }
        }
        if let Some((plugin, _)) = qualified.split_once(':') {
            if !plugin.is_empty() { return plugin.into(); }
        }
    }
    let standalone = home.join(".agents/skills");
    let standalone = fs::canonicalize(&standalone).unwrap_or(standalone);
    if real.starts_with(standalone) { "@avulsa" } else { "@pessoal" }.into()
}

pub fn scan(home: &Path, repo: &Path) -> IndexMap<String, String> {
    let (shallow, deep) = roots(home, repo);
    let mut found = IndexMap::new();
    for root in shallow {
        for path in children(&root).into_iter().filter(|p| p.join("SKILL.md").is_file()) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            found.entry(name).or_insert_with(|| origin(&path, home, repo));
        }
    }
    for root in deep {
        let mut paths = Vec::new(); deep_skills(&root, &mut paths); paths.sort();
        for md in paths {
            let path = md.parent().unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            found.entry(name).or_insert_with(|| origin(path, home, repo));
        }
    }
    found
}

fn watched_dirs(home: &Path, repo: &Path) -> Vec<PathBuf> {
    let (shallow, deep) = roots(home, repo);
    let mut out = Vec::new();
    for root in shallow {
        out.push(root.clone()); out.extend(children(&root).into_iter().filter(|p| p.is_dir()));
    }
    for root in deep {
        let mut level = vec![root];
        for depth in 0..=4 {
            out.extend(level.iter().cloned());
            if depth == 4 { break; }
            level = level.iter().flat_map(|p| children(p)).filter(|p|
                fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())).collect();
        }
    }
    out
}

fn mtime(path: &Path) -> Option<SystemTime> { fs::metadata(path).ok().and_then(|m| m.modified().ok()) }

#[derive(Default)]
struct State {
    initialized: bool,
    refreshing: bool,
    checked_at: u64,
    generation: u64,
    found: IndexMap<String, String>,
    watch: Watch,
}

struct Inner {
    home: PathBuf,
    repo: PathBuf,
    clock: Arc<Clock>,
    state: Mutex<State>,
    scanning: Mutex<()>,
}

#[derive(Clone)]
pub struct Origins { inner: Arc<Inner> }

impl Origins {
    pub fn new(home: PathBuf, repo: PathBuf) -> Self {
        // Recriar um cache não pode reutilizar o instante que chaveou um relatório anterior.
        static STARTED: OnceLock<Instant> = OnceLock::new();
        let started = *STARTED.get_or_init(Instant::now);
        Self::with_clock(home, repo, Arc::new(move || started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64))
    }

    /// O relógio monotônico em nanossegundos também identifica mudanças no mapa.
    pub fn with_clock(home: PathBuf, repo: PathBuf, clock: Arc<Clock>) -> Self {
        Self { inner: Arc::new(Inner { home, repo, clock, state: Mutex::new(State::default()), scanning: Mutex::new(()) }) }
    }

    pub fn recent(&self) -> (u64, IndexMap<String, String>) {
        if !self.inner.state.lock().unwrap().initialized {
            // Consultas simultâneas da primeira leitura compartilham a mesma varredura.
            let _scan = self.inner.scanning.lock().unwrap();
            if !self.inner.state.lock().unwrap().initialized { refresh(&self.inner, true); }
            let state = self.inner.state.lock().unwrap();
            return (state.generation, state.found.clone());
        }
        let mut state = self.inner.state.lock().unwrap();
        let snapshot = (state.generation, state.found.clone());
        let now = (self.inner.clock)();
        if !state.refreshing && now.saturating_sub(state.checked_at) > CHECK_NANOS {
            state.checked_at = now; state.refreshing = true;
            let inner = self.inner.clone();
            std::thread::spawn(move || {
                let _scan = inner.scanning.lock().unwrap();
                refresh(&inner, false);
                inner.state.lock().unwrap().refreshing = false;
            });
        }
        snapshot
    }
}

fn refresh(inner: &Inner, force: bool) {
    let checked_at = (inner.clock)();
    let watch = inner.state.lock().unwrap().watch.clone();
    if !force && watch.iter().all(|(p, seen)| *seen == Some(mtime(p))) { return; }
    // Ler os mtimes antes permite perceber mudanças ocorridas durante a varredura. Data recente demais
    // não prova nada: mudança no mesmo degrau do relógio do sistema de arquivos (até 16 ms no Windows, 2 s
    // no FAT) deixa a mesma data, e a pasta é olhada de novo na verificação seguinte.
    let started = SystemTime::now();
    let watch = watched_dirs(&inner.home, &inner.repo).into_iter().map(|p| {
        let seen = mtime(&p);
        let settled = seen.is_none_or(|t| started.duration_since(t).is_ok_and(|age| age >= RACY));
        (p, settled.then_some(seen))
    }).collect();
    let found = scan(&inner.home, &inner.repo);
    let mut state = inner.state.lock().unwrap();
    if !state.initialized || state.found != found {
        state.generation = (inner.clock)().max(state.generation.saturating_add(1)); state.found = found;
    }
    if force { state.checked_at = checked_at; }
    state.initialized = true; state.watch = watch;
}
