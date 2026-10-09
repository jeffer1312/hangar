//! Descoberta dos provedores fora do Claude com terminal: bilhetes de Pi, omp e Kimi
//! (`registry.py:739-888`) e as linhas dos sidecars Codex e Claude sem terminal (`:1438-1480`).
//! A conta das linhas Pi, omp e Kimi casa credenciais (`cotas.py`) e chega pelos fatos do Python.
use super::capped::{Capped, SESSION_CAP};
use super::discover::{DiscoveryProblem, Problems, Resolver, discover_panes};
use super::links;
use super::mux::Pane;
use super::procs::{ChildrenMap, ProcessView};
use hangar_api::session::SessionRow;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

/// Pastas que o Python tira do ambiente do backend. Em parâmetro, para o teste rodar sem tocar no
/// `HOME` do processo.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// `Path.home()`: sidecars em `~/.hangar` e a conta Claude padrão (`~/.claude`).
    pub home: PathBuf,
    /// `settings.projects_dir.parent`: vínculos (`.hangar-pair`, `-chain`, `-loop`) e o transcript
    /// sem terminal quando o sidecar não declara conta.
    pub claude: PathBuf,
    /// `codex_contas.default_home()`.
    pub codex_home: PathBuf,
    /// `PI_CODING_AGENT_SESSION_DIR` ou `~/.pi/agent/sessions`.
    pub pi_sessions: PathBuf,
    /// `~/<PI_CONFIG_DIR ou .omp>`, base dos perfis do omp.
    pub omp_config: PathBuf,
    /// Diretório do agente omp do próprio backend (sem perfil na sessão).
    pub omp_agent: PathBuf,
    /// `KIMI_CODE_HOME` ou `~/.kimi-code`.
    pub kimi_home: PathBuf,
}

impl Dirs {
    /// Conta Claude padrão das sessões que não declaram `CLAUDE_CONFIG_DIR`.
    pub fn default_claude(&self) -> PathBuf { self.home.join(".claude") }
}

/// Variável do ambiente do processo; vazia é ausência. Ilegível também cai na ausência, como o
/// `_env_var_of`, mas avisa: motor e conta sairiam do padrão nesta rodada.
fn env_os(procs: &dyn ProcessView, pid: i64, name: &str) -> Option<OsString> {
    match procs.env_var(pid, name) {
        Ok(v) => v.filter(|v| !v.is_empty()),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound && crate::warn_limit::allow(None, "list_environ_unreadable") {
                tracing::warn!(code = "list_environ_unreadable", pid, io_kind = ?error.kind(), "ambiente do processo ilegível");
            }
            None
        }
    }
}

pub(super) fn env(procs: &dyn ProcessView, pid: i64, name: &str) -> Option<String> {
    env_os(procs, pid, name).map(|v| v.to_string_lossy().into_owned())
}

/// Bytes do caminho intactos, como o `surrogateescape` do Python: o caminho tem de existir no disco.
pub(super) fn config_dir_of(procs: &dyn ProcessView, pid: i64) -> Option<PathBuf> {
    env_os(procs, pid, "CLAUDE_CONFIG_DIR").map(PathBuf::from)
}

/// Chave do bilhete, a mesma da extensão: no psmux o `%N` repete entre sessões e vale o
/// `PSMUX_SESSION`.
pub fn ticket_key(pane_id: &str, pid: Option<i64>, procs: &dyn ProcessView) -> String {
    if let Some(psmux) = pid.and_then(|p| env(procs, p, "PSMUX_SESSION")) {
        return psmux.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
    }
    pane_id.trim_start_matches('%').to_owned()
}

/// As linhas da rodada, o pid do agente de cada linha com terminal que tem agente reconhecido e
/// as falhas contornadas, para o diário (no máximo `PROBLEMS_CAP`).
#[derive(Debug, Default)]
pub struct Discovered {
    pub rows: Vec<SessionRow>,
    pub agent_pids: HashMap<String, u32>,
    pub problems: Vec<DiscoveryProblem>,
}

/// `registry.list()` sem as linhas `orq` e de transferência (vêm dos fatos): sessões do
/// multiplexador, guarda de colisão e as linhas dos sidecars Codex e Claude sem terminal.
pub fn discover_rows(panes: &[Pane], procs: &dyn ProcessView, children: &ChildrenMap, resolver: &mut Resolver, dirs: &Dirs) -> Discovered {
    let projects = dirs.claude.join("projects");
    let skip = |name: &str| has_codex_sidecar(name, dirs);
    let mut rows = Vec::new();
    let mut sids = HashMap::new();
    let mut agent_pids = HashMap::new();
    let sessions = discover_panes(panes, procs, children, &projects, resolver, &skip);
    // Os da resolução fora da lista (`resolve_one`) também saem nesta rodada.
    let problems = &mut std::mem::take(&mut resolver.problems);
    for s in sessions {
        if let Some(pid) = s.agent_pid.and_then(|p| u32::try_from(p).ok()) {
            agent_pids.insert(s.name.clone(), pid);
        }
        let mut row = links::blank_row(&s.name);
        row.cwd = Some(s.cwd.clone());
        let (jsonl, tracked) = match (s.provider, s.transcript) {
            (_, Some(t)) => (t.jsonl, t.tracked),
            // Pane Codex sem sidecar: a TUI ainda não abriu a thread, não há rollout.
            ("codex", None) => (None, false),
            (provider, None) => {
                let t = ticket_transcript(provider, &s.pane_id, s.pane_pid, &s.cwd, procs, dirs, problems);
                let tracked = t.is_some();
                (t, tracked)
            }
        };
        row.jsonl = jsonl;
        row.tracked = tracked;
        links::fill_pane_row(&mut row, s.provider, s.agent_pid.or(s.pane_pid), s.session_created, procs, dirs, problems);
        sids.insert(s.name, s.repl_sid);
        rows.push(row);
    }
    links::dedupe_collisions(&mut rows, &sids);
    // Nascimento do terminal de mesmo nome, escondidas incluídas, como o `terminal_births`.
    let mut births = HashMap::new();
    for pane in panes {
        if let Some(b) = pane.session_created {
            births.entry(pane.session.clone()).or_insert(b);
        }
    }
    rows.extend(codex_rows(dirs, &births, procs, problems));
    rows.extend(headless_rows(dirs, procs, problems));
    Discovered { rows, agent_pids, problems: std::mem::take(problems).into_vec() }
}

/// Transcript de um pane Pi, omp ou Kimi pelo bilhete (e, no Pi, pelo `CP_PI_SESSION`). `None` =
/// sem vínculo, e a linha sai `tracked=false`. Lido do processo do pane, como faz a extensão.
pub fn ticket_transcript(provider: &str, pane_id: &str, pane_pid: Option<i64>, cwd: &str, procs: &dyn ProcessView, dirs: &Dirs,
                         problems: &mut Problems) -> Option<String> {
    match provider {
        "pi" | "omp" => pi_transcript(pane_id, pane_pid, cwd, provider, procs, dirs, problems),
        "kimi" => kimi_transcript(pane_id, pane_pid, cwd, procs, dirs, problems),
        _ => None,
    }
}

enum TicketError { Missing, Invalid }

/// Bilhete lido e já decodificado; o resto (`Err`) cai no reserva, como o `except (OSError,
/// ValueError)` do Python. `Missing` separa o arquivo ausente do ilegível ou torto.
fn read_ticket(path: &Path) -> Result<Value, TicketError> {
    let raw = std::fs::read(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { TicketError::Missing } else { TicketError::Invalid })?;
    let text = std::str::from_utf8(&raw).map_err(|_| TicketError::Invalid)?;
    serde_json::from_str(text).map_err(|_| TicketError::Invalid)
}

/// O bilhete como objeto; ausente é o normal, ilegível ou torto vale ausente (o reserva do
/// Python) mas vai ao diário: a sessão sairia sem conversa calada.
fn ticket_object(path: &Path, problems: &mut Problems) -> Option<Map<String, Value>> {
    match read_ticket(path) {
        Ok(Value::Object(data)) => Some(data),
        Err(TicketError::Missing) => None,
        _ => {
            problems.note("list_ticket_invalid", &path.to_string_lossy(), "bilhete do Kimi ilegível ou torto; sessão sem transcript");
            None
        }
    }
}

/// `isinstance(ts, (int, float))`: `bool` também conta no Python.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// A lista roda a cada 1,5 s: um aviso por minuto por chave e código.
fn warn_limited(key: &str, code: &'static str, field: &str) {
    if crate::warn_limit::allow(Some(key), code) {
        tracing::warn!(code, key, field, "list: entrada recusada");
    }
}

/// Folga entre o `Date.now()` da extensão e o nascimento do processo pelo kernel.
const TICKET_SLACK: f64 = 2.0;

fn pi_transcript(pane_id: &str, pid: Option<i64>, cwd: &str, provider: &str, procs: &dyn ProcessView, dirs: &Dirs,
                 problems: &mut Problems) -> Option<String> {
    let base = pid.and_then(|p| config_dir_of(procs, p)).unwrap_or_else(|| dirs.default_claude());
    let ticket = base.join(".hangar-pi").join(format!("{}.json", ticket_key(pane_id, pid, procs)));
    let sid = pid.and_then(|p| env(procs, p, "CP_PI_SESSION"));
    let profile = || if provider == "omp" { pid.and_then(|p| env(procs, p, "OMP_PROFILE")) } else { None };
    // O Python: bilhete que não é objeto encerra; ilegível ou torto cai no `CP_PI_SESSION`.
    let read = read_ticket(&ticket);
    if !matches!(read, Ok(Value::Object(_)) | Err(TicketError::Missing)) {
        problems.note("list_ticket_invalid", &ticket.to_string_lossy(), "bilhete de Pi ou omp ilegível ou torto; transcript pelo CP_PI_SESSION");
    }
    if let Ok(data) = read {
        let Value::Object(data) = data else { return None };
        let mut file = data.get("file").and_then(Value::as_str).filter(|f| !f.is_empty()).map(str::to_owned);
        let born = pid.and_then(|p| procs.start_time(p));
        match (born, number(data.get("ts"))) {
            // Frescor que não dá para provar é recusa: o pane reusado abriria a conversa anterior.
            (None, _) => { warn_limited(pane_id, "list_pi_ticket_refused", "nascimento"); file = None }
            (_, None) => { warn_limited(pane_id, "list_pi_ticket_refused", "ts"); file = None }
            (Some(born), Some(ts)) if ts < born - TICKET_SLACK => file = None,
            _ => {
                if file.as_deref().is_some_and(is_pi_subagent) {
                    warn_limited(pane_id, "list_pi_ticket_subagent", "file");
                    file = file.as_deref().and_then(pi_root_transcript);
                }
            }
        }
        if let Some(f) = file {
            // O omp grava o principal em `sessions/-/<nome>`, não no caminho do `--session`.
            if provider == "omp" && !Path::new(&f).exists() {
                let name = Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let root = sessions_root(provider, profile().as_deref(), dirs);
                return Some(find_in_root_cached(&root, format!("name:{name}"), |n| n == name).unwrap_or(f));
            }
            return Some(f);
        }
    }
    let sid = sid?;
    Some(pi_transcript_of_id(cwd, &sid, provider, profile().as_deref(), dirs)).filter(|t| !t.is_empty())
}

fn sessions_root(provider: &str, profile: Option<&str>, dirs: &Dirs) -> PathBuf {
    if provider == "pi" {
        return dirs.pi_sessions.clone();
    }
    omp_agent_dir(profile, dirs).join("sessions")
}

static OMP_PROFILE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{0,63}$").unwrap());
static WINDOWS_RESERVED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(?:\.|$)").unwrap());

/// Forma aceita pelo `omp_dirs` do Python, sem nome reservado do Windows.
pub(crate) fn valid_omp_profile(p: &str) -> bool {
    p != "." && p != ".." && !p.ends_with('.') && OMP_PROFILE_RE.is_match(p) && !WINDOWS_RESERVED.is_match(p)
}

/// Perfil do omp daquela sessão (`omp_plugin_sync.resolve_omp_directories`). Perfil inválido cai no
/// diretório do backend, com aviso, como o `omp_dirs.agent_dir` de quem só lê.
fn omp_agent_dir(profile: Option<&str>, dirs: &Dirs) -> PathBuf {
    let Some(p) = profile.map(str::trim).filter(|p| !p.is_empty() && *p != "default") else { return dirs.omp_agent.clone() };
    if !valid_omp_profile(p) {
        warn_limited(p, "list_omp_profile_invalid", "OMP_PROFILE");
        return dirs.omp_agent.clone();
    }
    dirs.omp_config.join("profiles").join(p).join("agent")
}

/// Pasta das sessões Pi de um cwd: só separador vira `-` (`getDefaultSessionDirPath` do Pi).
fn pi_cwd_slug(cwd: &str) -> String {
    let resolved = absolute(cwd);
    let trimmed = resolved.strip_prefix(['/', '\\']).unwrap_or(&resolved);
    format!("--{}--", trimmed.replace(['/', '\\', ':'], "-"))
}

/// `os.path.abspath(os.path.expanduser(p))` sem o `~`: o cwd do pane já vem absoluto.
fn absolute(path: &str) -> String {
    let p = Path::new(path);
    let joined = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    hangar_workspace::worktrees::normpath(&joined).to_string_lossy().into_owned()
}

fn mtime(path: &Path) -> SystemTime {
    path.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// `pi_sessions.transcript_path`: o mais novo `*_<sid>.jsonl` da pasta do cwd; no omp, em qualquer
/// pasta da raiz.
fn pi_transcript_of_id(cwd: &str, sid: &str, provider: &str, profile: Option<&str>, dirs: &Dirs) -> String {
    let root = sessions_root(provider, profile, dirs);
    let suffix = format!("_{sid}.jsonl");
    let matches = |n: &str| n.ends_with(&suffix);
    let dir = root.join(pi_cwd_slug(cwd));
    if let Some(found) = newest(&dir, &matches) {
        return found;
    }
    if provider == "omp" { find_in_root_cached(&root, format!("suffix:{suffix}"), matches).unwrap_or_default() } else { String::new() }
}

fn newest(dir: &Path, matches: &dyn Fn(&str) -> bool) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten()
        .filter(|e| matches(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .max_by_key(|p| mtime(p))
        .map(|p| p.to_string_lossy().into_owned())
}

/// Achado de uma varredura de pastas por (raiz, nome): o positivo vale enquanto o arquivo existe
/// (o nome não muda de lugar); o negativo, `MISS_TTL`, para o arquivo que ainda vai nascer.
type Found = LazyLock<std::sync::Mutex<Capped<(PathBuf, String), (std::time::Instant, Option<String>)>>>;
const MISS_TTL: std::time::Duration = std::time::Duration::from_secs(5);

fn found_or_scan(cache: &Found, key: (PathBuf, String), scan: impl FnOnce() -> Option<String>) -> Option<String> {
    let lock = || cache.lock().unwrap_or_else(|e| e.into_inner());
    // O `stat` fora da trava: ela é de todas as sessões da rodada.
    let cached = lock().get(&key).cloned();
    match cached {
        Some((_, Some(path))) if Path::new(&path).exists() => return Some(path),
        Some((at, None)) if at.elapsed() < MISS_TTL => return None,
        _ => {}
    }
    let hit = scan();
    lock().insert(key, (std::time::Instant::now(), hit.clone()));
    hit
}

static OMP_FOUND: Found = LazyLock::new(|| std::sync::Mutex::new(Capped::new(SESSION_CAP)));
static HEADLESS_FOUND: Found = LazyLock::new(|| std::sync::Mutex::new(Capped::new(SESSION_CAP)));

/// `find_in_root` lembrado por (raiz, chave do que casa): sem ele cada sessão omp varria a raiz
/// inteira a cada rodada.
// ponytail: o positivo segura o primeiro achado; um arquivo mais novo de mesmo nome em outra
// pasta só é visto quando o guardado some.
fn find_in_root_cached(root: &Path, key: String, matches: impl Fn(&str) -> bool) -> Option<String> {
    found_or_scan(&OMP_FOUND, (root.to_path_buf(), key), || find_in_root(root, matches))
}

/// `localizar_na_raiz`: o arquivo mais novo que casa em qualquer pasta de cwd da raiz.
fn find_in_root(root: &Path, matches: impl Fn(&str) -> bool) -> Option<String> {
    let dirs = std::fs::read_dir(root).ok()?;
    dirs.flatten()
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|d| d.flatten())
        .filter(|e| matches(&e.file_name().to_string_lossy()) && e.path().is_file())
        .map(|e| e.path())
        .max_by_key(|p| mtime(p))
        .map(|p| p.to_string_lossy().into_owned())
}

static STEM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_[0-9a-fA-F-]{36}$").unwrap());
static RUN_DIR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^run-\d+$").unwrap());

/// Transcript de subagente: `session.jsonl`, uma pasta `run-N` ou a pasta com o nome da sessão.
pub fn is_pi_subagent(path: &str) -> bool {
    let p = Path::new(path);
    let parents: Vec<String> = p.parent().map(|d| d.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    p.file_name().is_some_and(|n| n == "session.jsonl")
        || parents.iter().any(|d| RUN_DIR_RE.is_match(d) || STEM_RE.is_match(d))
}

/// Do subagente para a conversa: a pasta que guarda os runs tem o nome do arquivo da sessão.
fn pi_root_transcript(path: &str) -> Option<String> {
    Path::new(path).ancestors().skip(1).take(4)
        .map(|anc| PathBuf::from(format!("{}.jsonl", anc.to_string_lossy())))
        .find(|cand| cand.is_file())
        .map(|cand| cand.to_string_lossy().into_owned())
}

fn kimi_transcript(pane_id: &str, pid: Option<i64>, cwd: &str, procs: &dyn ProcessView, dirs: &Dirs, problems: &mut Problems) -> Option<String> {
    let base = pid.and_then(|p| config_dir_of(procs, p)).unwrap_or_else(|| dirs.default_claude());
    let ticket = base.join(".hangar-kimi").join(format!("{}.json", ticket_key(pane_id, pid, procs)));
    let data = ticket_object(&ticket, problems)?;
    let sid = data.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let born = pid.and_then(|p| procs.start_time(p));
    match (born, number(data.get("ts"))) {
        (None, _) => { warn_limited(pane_id, "list_kimi_ticket_refused", "nascimento"); return None }
        (_, None) => { warn_limited(pane_id, "list_kimi_ticket_refused", "ts"); return None }
        (Some(born), Some(ts)) if ts < born - TICKET_SLACK => return None,
        _ => {}
    }
    let wd = data.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).unwrap_or(cwd);
    kimi_transcript_of_id(wd, sid, dirs, problems)
}

fn kimi_wire(session_dir: &Path) -> String {
    session_dir.join("agents").join("main").join("wire.jsonl").to_string_lossy().into_owned()
}

/// `kimi_sessions.transcript_path`: o índice primeiro; a pasta calculada cobre o índice atrasado.
/// `None`: índice com byte inválido, que no Python levanta e deixa a sessão sem transcript.
fn kimi_transcript_of_id(cwd: &str, sid: &str, dirs: &Dirs, problems: &mut Problems) -> Option<String> {
    let index = dirs.kimi_home.join("session_index.jsonl");
    let version = file_version(&index);
    let key = (index, sid.to_owned());
    let lock = || KIMI_WIRES.lock().unwrap_or_else(|e| e.into_inner());
    let cached = lock().get(&key).cloned();
    let hit = match &cached {
        Some(KimiIndex::Found(wire)) => return Some(wire.clone()),
        // O índice não mudou desde que não tinha a sessão: só a pasta calculada é conferida.
        Some(KimiIndex::Missing(seen)) if version.is_some() && *seen == version => Some(None),
        Some(KimiIndex::Invalid(seen)) if version.is_some() && *seen == version => None,
        _ => match std::fs::read(&key.0) {
            Ok(raw) => match std::str::from_utf8(&raw) {
                Ok(text) => Some(kimi_index_find(text, sid)),
                Err(_) => {
                    lock().insert(key.clone(), KimiIndex::Invalid(version));
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
            // Ilegível não é "fora do índice": não fica guardado e vai ao diário.
            Err(_) => {
                problems.note("list_kimi_index_unreadable", &key.0.to_string_lossy(), "session_index.jsonl do Kimi ilegível; transcript pela pasta calculada");
                let dir = dirs.kimi_home.join("sessions").join(kimi_workdir_key(cwd)).join(sid);
                return dir.is_dir().then(|| kimi_wire(&dir));
            }
        },
    };
    let Some(found) = hit else {
        // No Python o índice com byte inválido levanta e a sessão fica sem transcript.
        problems.note("list_kimi_index_invalid", &key.0.to_string_lossy(), "session_index.jsonl do Kimi com UTF-8 inválido; sessão sem transcript");
        return None;
    };
    if let Some(wire) = found {
        lock().insert(key, KimiIndex::Found(wire.clone()));
        return Some(wire);
    }
    if version.is_some() {
        lock().insert(key, KimiIndex::Missing(version));
    }
    let dir = dirs.kimi_home.join("sessions").join(kimi_workdir_key(cwd)).join(sid);
    dir.is_dir().then(|| kimi_wire(&dir))
}

fn kimi_index_find(text: &str, sid: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let Ok(Value::Object(o)) = serde_json::from_str::<Value>(line) else { return None };
        if o.get("sessionId").and_then(Value::as_str) != Some(sid) {
            return None;
        }
        o.get("sessionDir").and_then(Value::as_str).filter(|d| !d.is_empty()).map(|d| kimi_wire(Path::new(d)))
    })
}

/// (mtime em ns, tamanho): o índice do Kimi só cresce, e qualquer escrita muda os dois.
fn file_version(path: &Path) -> Option<(u128, u64)> {
    let meta = path.metadata().ok()?;
    let mtime = meta.modified().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos();
    Some((mtime, meta.len()))
}

/// O que o índice disse de uma sessão. Achado, o `sessionDir` não muda; ausente ou torto vale até o
/// índice mudar. Sem isso uma sessão fora do índice relia e analisava o índice inteiro por rodada.
#[derive(Clone)]
enum KimiIndex { Found(String), Missing(Option<(u128, u64)>), Invalid(Option<(u128, u64)>) }

static KIMI_WIRES: LazyLock<std::sync::Mutex<Capped<(PathBuf, String), KimiIndex>>> = LazyLock::new(Default::default);

static KIMI_SLUG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._-]+").unwrap());

/// `wd_<slug do nome>_<sha256 do caminho>[:12]`, porte do `slugifyWorkDirName` do Kimi.
pub fn kimi_workdir_key(cwd: &str) -> String {
    let resolved = absolute(cwd);
    let name = Path::new(&resolved).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug = KIMI_SLUG_RE.replace_all(&name.to_lowercase(), "-").trim_matches('-').chars().take(40).collect::<String>();
    let slug = slug.trim_matches('-');
    let slug = if matches!(slug, "" | "." | "..") { "workspace" } else { slug };
    let digest = ring::digest::digest(&ring::digest::SHA256, resolved.as_bytes());
    let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("wd_{slug}_{}", &hex[..12])
}

/// `names.sanitize_session_name`: acento vira a letra sem ele, o resto fora de `[A-Za-z0-9_-]` vira `-`.
pub fn sanitize_session_name(name: &str) -> String {
    // ponytail: só os acentos do português e vizinhos, sem NFKD completo (crate novo); outra letra
    // composta some em vez de virar a base. Nome criado pelo app já chega sanitizado.
    let ascii: String = name.chars().filter_map(fold_accent).collect();
    if name.chars().any(|c| c.is_alphabetic() && fold_accent(c).is_none()) {
        warn_limited(name, "list_session_name_unfolded", "name");
    }
    ascii.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect::<String>().trim_matches('-').to_owned()
}

/// Letra latina acentuada → a base ASCII (o NFKD + `encode("ascii", "ignore")`); outro não-ASCII some.
fn fold_accent(c: char) -> Option<char> {
    if c.is_ascii() {
        return Some(c);
    }
    const MAP: [(&str, char); 14] = [
        ("ÀÁÂÃÄÅ", 'A'), ("àáâãäå", 'a'), ("ÈÉÊË", 'E'), ("èéêë", 'e'), ("ÌÍÎÏ", 'I'), ("ìíîï", 'i'),
        ("ÒÓÔÕÖ", 'O'), ("òóôõö", 'o'), ("ÙÚÛÜ", 'U'), ("ùúûü", 'u'), ("Ç", 'C'), ("ç", 'c'), ("Ñ", 'N'), ("ñ", 'n'),
    ];
    MAP.iter().find(|(set, _)| set.contains(c)).map(|(_, b)| *b)
}

/// O pane de mesmo nome sai da lista: identidade e histórico do Codex vêm do sidecar.
pub fn has_codex_sidecar(name: &str, dirs: &Dirs) -> bool {
    dirs.home.join(".hangar").join("codex-sessions").join(format!("{}.json", sanitize_session_name(name))).exists()
}

type Sidecars = Arc<Vec<(String, Map<String, Value>)>>;

/// Uma leitura por versão da pasta: o Python grava sidecar por `rename`, então pasta igual é
/// sidecar igual. Os tortos ficam junto, para voltarem ao diário a cada rodada.
static SIDECARS: LazyLock<std::sync::Mutex<Capped<PathBuf, (SystemTime, Sidecars, Arc<Vec<String>>)>>> =
    LazyLock::new(|| std::sync::Mutex::new(Capped::new(16)));

/// Sidecars de uma pasta em ordem de nome, com o nome do arquivo; ilegível ou não-objeto fica de
/// fora, como no Python, mas vai ao diário: a sessão viva dele sumiria da lista calada.
fn sidecars(dir: &Path, kind: &'static str, problems: &mut Problems) -> Sidecars {
    let stamp = match dir.metadata().and_then(|m| m.modified()) {
        Ok(stamp) => stamp,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                problems.note("list_sidecar_dir_unreadable", &dir.to_string_lossy(), kind);
            }
            return Sidecars::default();
        }
    };
    let cached = SIDECARS.lock().unwrap_or_else(|e| e.into_inner()).get(dir)
        .filter(|(s, _, _)| *s == stamp && super::discover::settled(stamp)).map(|(_, v, bad)| (v.clone(), bad.clone()));
    let (found, bad) = match cached {
        Some(hit) => hit,
        None => match read_sidecars(dir) {
            Ok((found, bad)) => {
                let entry = (Arc::new(found), Arc::new(bad));
                SIDECARS.lock().unwrap_or_else(|e| e.into_inner()).insert(dir.to_path_buf(), (stamp, entry.0.clone(), entry.1.clone()));
                entry
            }
            // Fora do cache: a próxima rodada tenta de novo e avisa de novo.
            Err(_) => {
                problems.note("list_sidecar_dir_unreadable", &dir.to_string_lossy(), kind);
                return Sidecars::default();
            }
        },
    };
    for file in bad.iter() {
        problems.note("list_sidecar_unreadable", file, kind);
    }
    found
}

fn read_sidecars(dir: &Path) -> std::io::Result<(Vec<(String, Map<String, Value>)>, Vec<String>)> {
    let entries = std::fs::read_dir(dir)?;
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    let (mut found, mut bad) = (Vec::with_capacity(files.len()), Vec::new());
    for f in files {
        match read_ticket(&f) {
            Ok(Value::Object(m)) => found.push((f.file_name().unwrap_or_default().to_string_lossy().into_owned(), m)),
            // Apagado entre a listagem e a leitura: a sessão fechou.
            Err(TicketError::Missing) => {}
            _ => bad.push(f.to_string_lossy().into_owned()),
        }
    }
    Ok((found, bad))
}

fn text(meta: &Map<String, Value>, key: &str) -> Option<String> {
    meta.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// `cwd_atual`: renomeada com a sessão viva, a pasta gravada some e o processo (conferido pela
/// chave no cmdline) mostra a nova.
fn current_cwd(meta: &Map<String, Value>, procs: &dyn ProcessView) -> Option<String> {
    let cwd = text(meta, "cwd")?;
    let pid = meta.get("cano").and_then(|c| c.get("pid")).and_then(Value::as_i64);
    let key = text(meta, "key");
    let (Some(pid), Some(key)) = (pid, key) else { return Some(cwd) };
    if Path::new(&cwd).is_dir() {
        return Some(cwd);
    }
    let prefix: String = key.chars().take(16).collect();
    if !procs.argv(pid).join("\0").contains(&prefix) {
        return Some(cwd);
    }
    match procs.cwd(pid) {
        Some(live) if live.is_dir() => Some(live.to_string_lossy().into_owned()),
        _ => Some(cwd),
    }
}

fn expand_user(path: &str, dirs: &Dirs) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => dirs.home.clone(),
        // No Windows o `~` também vem com `\\`.
        Some(rest) if rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\')) => dirs.home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// Uma linha por sidecar Codex (`include_incomplete=True`). `births` é o nascimento do terminal de
/// mesmo nome; a vida de uma transferência em curso vem dos fatos.
pub fn codex_rows(dirs: &Dirs, births: &HashMap<String, u64>, procs: &dyn ProcessView, problems: &mut Problems) -> Vec<SessionRow> {
    let mut out = Vec::new();
    let found = sidecars(&dirs.home.join(".hangar").join("codex-sessions"), "sidecar Codex ilegível ou torto; sessão fora da lista", problems);
    for (file, meta) in found.iter() {
        let Some(name) = text(&meta, "name") else {
            problems.note("list_codex_sidecar_skipped", file, "sidecar Codex sem nome; sessão fora da lista");
            continue;
        };
        let home = text(&meta, "codex_home").map(|h| expand_user(&h, dirs)).unwrap_or_else(|| dirs.codex_home.clone());
        let codex_home = links::resolve_lenient(&home).to_string_lossy().into_owned();
        let cwd = current_cwd(&meta, procs);
        let jsonl = text(&meta, "rollout_path");
        let mut row = links::blank_row(&name);
        row.lifecycle_id = links::session_life(text(&meta, "key").as_deref(), births.get(&name).copied());
        row.cwd = cwd;
        row.jsonl = jsonl;
        row.provider = "codex".to_owned();
        row.conta = Some(format!("codex:{codex_home}"));
        row.codex_home = Some(codex_home);
        // Cru, como o `meta.get` do Python: vazio continua vazio.
        row.codex_service_tier = meta.get("service_tier").and_then(Value::as_str).map(str::to_owned);
        row.headless = meta.get("headless").is_some_and(truthy);
        links::fill_location(&mut row, "codex", dirs, problems);
        links::fill_links(&mut row, dirs, problems);
        out.push(row);
    }
    out
}

pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `transcript_path` do Claude sem terminal: `<conta>/projects/<cwd>/<sid>.jsonl`, ou onde o
/// `EnterWorktree` o levou (o sid não repete entre pastas). A busca nas pastas de `projects/` é
/// lembrada: sem isso, cada sessão ainda sem transcript varria todos os projetos por rodada.
fn headless_transcript(cwd: &str, sid: &str, config_dir: Option<&str>, dirs: &Dirs, problems: &mut Problems) -> String {
    let base = config_dir.map(|c| Path::new(c).join("projects")).unwrap_or_else(|| dirs.claude.join("projects"));
    let file = format!("{sid}.jsonl");
    let expected = base.join(hangar_workspace::worktrees::sanitize_cwd(cwd)).join(&file);
    if expected.exists() {
        return expected.to_string_lossy().into_owned();
    }
    let moved = found_or_scan(&HEADLESS_FOUND, (base.clone(), file.clone()), || {
        let entries = match std::fs::read_dir(&base) {
            Ok(entries) => entries,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    problems.note("list_projects_unreadable", &base.to_string_lossy(), "pasta projects ilegível; transcript da sessão sem terminal pelo caminho esperado");
                }
                return None;
            }
        };
        let mut moved: Vec<PathBuf> = entries.flatten().map(|d| d.path().join(&file)).filter(|p| p.exists()).collect();
        moved.sort();
        moved.into_iter().next().map(|p| p.to_string_lossy().into_owned())
    });
    moved.unwrap_or_else(|| expected.to_string_lossy().into_owned())
}

/// Uma linha por sessão Claude sem terminal: a identidade vem do sidecar.
pub fn headless_rows(dirs: &Dirs, procs: &dyn ProcessView, problems: &mut Problems) -> Vec<SessionRow> {
    let mut out = Vec::new();
    let found = sidecars(&dirs.home.join(".hangar").join("claude-headless"), "sidecar Claude sem terminal ilegível ou torto; sessão fora da lista", problems);
    for (file, meta) in found.iter() {
        // Sem nome ou sid o Python também filtra calado: é o sidecar ainda sendo escrito.
        let (Some(name), Some(sid)) = (text(meta, "name"), text(meta, "session_id")) else { continue };
        let Some(saved_cwd) = text(meta, "cwd") else {
            problems.note("list_headless_sidecar_skipped", file, "sidecar Claude sem terminal sem cwd; sessão fora da lista");
            continue;
        };
        let config_dir = text(&meta, "config_dir");
        let engine = text(&meta, "engine");
        let account = text(&meta, "engine_account");
        let mut row = links::blank_row(&name);
        row.lifecycle_id = links::session_life(text(&meta, "key").as_deref(), None);
        row.cwd = current_cwd(&meta, procs);
        row.jsonl = Some(headless_transcript(&saved_cwd, &sid, config_dir.as_deref(), dirs, problems));
        row.headless = true;
        row.conta = if account.is_some() {
            text(&meta, "engine_credential_id")
        } else if let Some(e) = &engine {
            Some(format!("chave:{e}"))
        } else {
            let cdir = config_dir.map(PathBuf::from).unwrap_or_else(|| dirs.default_claude());
            Some(format!("claude:{}", links::resolve_lenient(&cdir).to_string_lossy()))
        };
        row.engine = engine;
        row.engine_account = account;
        links::fill_location(&mut row, "claude", dirs, problems);
        links::fill_links(&mut row, dirs, problems);
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::facts_files::{self, HookStates};
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Árvore pane → `claude --session-id`, contando as leituras por pid (no `sysinfo` cada uma
    /// era um retrato de todos os processos).
    struct Tree { sids: Vec<String>, config: String, reads: AtomicUsize }

    impl ProcessView for Tree {
        fn children(&self, _: Duration) -> std::io::Result<Arc<ChildrenMap>> { unreachable!() }
        fn argv(&self, pid: i64) -> Vec<String> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            match pid {
                2000.. => vec!["claude".into(), "--session-id".into(), self.sids[(pid - 2000) as usize].clone()],
                _ => vec!["fish".into()],
            }
        }
        fn cwd(&self, _: i64) -> Option<PathBuf> { None }
        fn env_var(&self, _: i64, name: &str) -> std::io::Result<Option<OsString>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok((name == "CLAUDE_CONFIG_DIR").then(|| OsString::from(&self.config)))
        }
        fn start_time(&self, _: i64) -> Option<f64> { self.reads.fetch_add(1, Ordering::Relaxed); Some(1.0) }
        fn fds(&self, _: i64) -> Vec<PathBuf> { Vec::new() }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Custo de um tique da descoberta + decoração com 20 sessões Claude num repositório com 5
    /// worktrees, transcripts de 2 MB (5 crescem por tique), 220 marcadores, pergunta e statusline.
    /// `cargo test --release -p hangar-server --lib list::discover_other::tests::tick_cost -- --ignored --nocapture`
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn tick_cost() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let claude = home.join(".claude");
        let dirs = Dirs { home: home.clone(), claude: claude.clone(), codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi"), omp_config: home.join(".omp"), omp_agent: home.join(".omp/agent"),
            kimi_home: home.join(".kimi-code") };
        let repo = home.join("repo");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        for k in 0..5 {
            let wt = home.join(format!("repo-wt{k}"));
            write(&repo.join(format!(".git/worktrees/wt{k}/gitdir")), &format!("{}/.git\n", wt.display()));
            write(&wt.join(".git"), &format!("gitdir: {}/.git/worktrees/wt{k}\n", repo.display()));
        }
        let projects = claude.join("projects").join(hangar_workspace::worktrees::sanitize_cwd(repo.to_str().unwrap()));
        let sids: Vec<String> = (0..20).map(|i| format!("00000000-0000-0000-0000-0000000000{i:02}")).collect();
        let filler = format!(r#"{{"type":"assistant","cwd":"{}","message":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
            repo.display(), "x".repeat(1000));
        let tool = |k: usize| format!(r#"{{"type":"assistant","cwd":"{}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"cd {}/repo-wt{} && ls"}}}}]}}}}"#,
            repo.display(), home.display(), k % 5);
        for (i, sid) in sids.iter().enumerate() {
            let jsonl = projects.join(format!("{sid}.jsonl"));
            let body: String = (0..2000).map(|n| if n % 100 == 99 { tool(n) } else { filler.clone() } + "\n").collect();
            write(&jsonl, &body);
            write(&claude.join(format!(".hangar-state/{sid}.json")), r#"{"state":"working","ts":1.0}"#);
            write(&claude.join(format!("sessions/{}.json", 2000 + i)),
                  &format!(r#"{{"sessionId":"{sid}","pid":{},"status":"busy","updatedAt":1000}}"#, 2000 + i));
            write(&claude.join(format!(".hangar-askq/{sid}.json")), &format!(
                r#"{{"tool_input":{{"questions":[{{"question":"Q?","header":"h","options":[{{"label":"A"}}]}}]}},"transcript_path":{:?}}}"#,
                jsonl.to_str().unwrap()));
            write(&claude.join(format!(".hangar-status/{sid}.json")), r#"{"line":"🤖 Haiku","ts":1e12}"#);
            write(&claude.join(format!(".hangar-pair/s{i}.json")), r#"{"peers":["peer::x"],"gid":"g"}"#);
        }
        for i in 0..200 {
            write(&claude.join(format!(".hangar-state/old{i}.json")), r#"{"state":"idle","ts":1.0}"#);
        }
        let ext: Vec<Value> = (0..20).map(|i| serde_json::json!({"share_id": "a", "local_session": format!("s{i}"),
            "alias": "peer", "peer_owner": "o", "peer_session": "x", "peer_address": "h", "peer_token": "t", "created_at": 1})).collect();
        write(&claude.join(".hangar-pair/external_pairs.json"), &Value::Array(ext).to_string());
        let panes: Vec<Pane> = (0..20).map(|i| Pane { session: format!("s{i}"), active: true, pid: Some(1000 + i),
            cwd: repo.to_string_lossy().into_owned(), pane_id: format!("%{i}"), ..Pane::default() }).collect();
        let children: ChildrenMap = (0..20).map(|i| (1000 + i, vec![2000 + i])).collect();
        let procs = Tree { sids: sids.clone(), config: claude.to_string_lossy().into_owned(), reads: AtomicUsize::new(0) };
        let config_dirs = vec![claude.clone()];
        let mut resolver = Resolver::default();
        let mut hooks = HookStates::default();
        let mut tick = |n: usize| {
            for k in 0..5 {
                let jsonl = projects.join(format!("{}.jsonl", sids[(n * 5 + k) % 20]));
                let mut f = std::fs::OpenOptions::new().append(true).open(jsonl).unwrap();
                writeln!(f, "{filler}").unwrap();
            }
            let found = discover_rows(&panes, &procs, &children, &mut resolver, &dirs);
            let rows = found.rows;
            assert_eq!(found.agent_pids.len(), 20);
            hooks.refresh(&config_dirs);
            for row in &rows {
                let sid = row.jsonl.as_deref().and_then(|j| Path::new(j).file_stem()).map(|s| s.to_string_lossy().into_owned());
                std::hint::black_box((hooks.get_state(sid.as_deref(), |_| true),
                    facts_files::open_question(sid.as_deref(), &config_dirs),
                    facts_files::published_status(sid.as_deref(), &config_dirs, 1e12)));
            }
            assert_eq!(rows.len(), 20);
            assert!(rows.iter().all(|r| r.worktree), "a worktree vem do transcript");
        };
        // CPU da thread (ns) e pico de RSS (kB), zerado antes de cada fase.
        let cpu = || std::fs::read_to_string("/proc/thread-self/schedstat").unwrap().split_whitespace().next().unwrap().parse::<u64>().unwrap();
        let hwm = || std::fs::read_to_string("/proc/self/status").unwrap().lines()
            .find_map(|l| l.strip_prefix("VmHWM:")).unwrap().trim().trim_end_matches("kB").trim().parse::<u64>().unwrap();
        let reset = || std::fs::write("/proc/self/clear_refs", "5").unwrap();
        let mut phase = |label: &str, ticks: std::ops::Range<usize>| {
            reset();
            let (n, c, t) = (ticks.len(), cpu(), std::time::Instant::now());
            procs.reads.store(0, Ordering::Relaxed);
            for i in ticks { tick(i); }
            println!("{label}: {:.0} µs/tique, CPU {:.0} µs/tique, pico RSS {} kB, {:.0} leituras de processo/tique",
                t.elapsed().as_micros() as f64 / n as f64, (cpu() - c) as f64 / 1000.0 / n as f64, hwm(),
                procs.reads.load(Ordering::Relaxed) as f64 / n as f64);
        };
        phase("tique frio", 0..1);
        phase("tiques seguintes", 1..51);
        // Passado o teto de 10 s, cada transcript que cresceu é relido.
        std::thread::sleep(std::time::Duration::from_millis(10_500));
        phase("tique depois de 10 s", 51..52);
    }

    fn dirs_at(home: &Path) -> Dirs {
        Dirs { home: home.to_path_buf(), claude: home.join(".claude"), codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi"), omp_config: home.join(".omp"), omp_agent: home.join(".omp/agent"),
            kimi_home: home.join(".kimi-code") }
    }

    #[test]
    fn broken_sidecar_reaches_the_diary_every_round() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        let codex = tmp.path().join(".hangar/codex-sessions");
        write(&codex.join("ok.json"), r#"{"name":"ok","cwd":"/w"}"#);
        write(&codex.join("torto.json"), r#"{"name":"#);
        write(&tmp.path().join(".hangar/claude-headless/lista.json"), "[]");
        let procs = RealTree { sids: Vec::new(), config: String::new() };
        let round = |resolver: &mut Resolver| discover_rows(&[], &procs, &ChildrenMap::new(), resolver, &dirs);
        let found = round(&mut Resolver::default());
        assert_eq!(found.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["ok"]);
        let mut seen: Vec<(&str, String)> = found.problems.iter().map(|p| (p.code, p.key.clone())).collect();
        seen.sort();
        let hangar = tmp.path().join(".hangar");
        assert_eq!(seen, [("list_sidecar_unreadable", hangar.join("claude-headless").join("lista.json").to_string_lossy().into_owned()),
            ("list_sidecar_unreadable", hangar.join("codex-sessions").join("torto.json").to_string_lossy().into_owned())]);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_sidecar_dir_is_reported_and_not_kept() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let codex = tmp.path().join(".hangar/codex-sessions");
        write(&codex.join("ok.json"), r#"{"name":"ok","cwd":"/w"}"#);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        std::fs::File::open(&codex).unwrap().set_modified(old).unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o300)).unwrap();
        let mut problems = Problems::default();
        let found = sidecars(&codex, "codex", &mut problems);
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(found.is_empty());
        assert_eq!(problems.into_vec().iter().map(|p| p.code).collect::<Vec<_>>(), ["list_sidecar_dir_unreadable"]);
        assert_eq!(sidecars(&codex, "codex", &mut Problems::default()).len(), 1, "a falha não ficou guardada como pasta vazia");
    }

    #[test]
    fn broken_ticket_reaches_the_diary() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        write(&dirs.default_claude().join(".hangar-kimi/7.json"), "{");
        write(&dirs.default_claude().join(".hangar-pi/8.json"), "[]");
        let procs = RealTree { sids: Vec::new(), config: String::new() };
        let mut problems = Problems::default();
        assert_eq!(ticket_transcript("kimi", "%7", None, "/w", &procs, &dirs, &mut problems), None);
        assert_eq!(ticket_transcript("pi", "%8", None, "/w", &procs, &dirs, &mut problems), None);
        assert_eq!(ticket_transcript("kimi", "%9", None, "/w", &procs, &dirs, &mut problems), None, "ausente é calado");
        assert_eq!(problems.into_vec().iter().map(|p| p.code).collect::<Vec<_>>(), ["list_ticket_invalid", "list_ticket_invalid"]);
    }

    #[test]
    fn kimi_index_with_invalid_utf8_is_reported_and_missing_is_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        let index = dirs.kimi_home.join("session_index.jsonl");
        write(&index, "");
        std::fs::write(&index, b"{\"sessionId\":\"\xff\"}\n").unwrap();
        let mut problems = Problems::default();
        assert_eq!(kimi_transcript_of_id("/w", "s1", &dirs, &mut problems), None);
        assert_eq!(problems.into_vec().iter().map(|p| p.code).collect::<Vec<_>>(), ["list_kimi_index_invalid"]);
        // Fora do índice: guardado até o índice mudar; quando cresce com a sessão, ela aparece.
        std::fs::write(&index, "{\"sessionId\":\"x\",\"sessionDir\":\"/k/x\"}\n").unwrap();
        let mut problems = Problems::default();
        assert_eq!(kimi_transcript_of_id("/w", "s2", &dirs, &mut problems), None);
        assert!(matches!(KIMI_WIRES.lock().unwrap().get(&(index.clone(), "s2".to_owned())), Some(KimiIndex::Missing(Some(_)))));
        let mut f = std::fs::OpenOptions::new().append(true).open(&index).unwrap();
        writeln!(f, "{{\"sessionId\":\"s2\",\"sessionDir\":\"/k/s2\"}}").unwrap();
        assert_eq!(kimi_transcript_of_id("/w", "s2", &dirs, &mut problems).as_deref(),
            Some(Path::new("/k/s2").join("agents").join("main").join("wire.jsonl").to_str().unwrap()));
        assert!(problems.into_vec().is_empty());
    }

    /// Árvore com sid opcional por sessão: sem sid a resolução cai no marcador por pid.
    struct RealTree { sids: Vec<Option<String>>, config: String }

    impl ProcessView for RealTree {
        fn children(&self, _: Duration) -> std::io::Result<Arc<ChildrenMap>> { unreachable!() }
        fn argv(&self, pid: i64) -> Vec<String> {
            match pid {
                2000.. => match &self.sids[(pid - 2000) as usize] {
                    Some(sid) => vec!["claude".into(), "--session-id".into(), sid.clone()],
                    None => vec!["claude".into()],
                },
                _ => vec!["fish".into()],
            }
        }
        fn cwd(&self, _: i64) -> Option<PathBuf> { None }
        fn env_var(&self, _: i64, name: &str) -> std::io::Result<Option<OsString>> {
            Ok((name == "CLAUDE_CONFIG_DIR").then(|| OsString::from(&self.config)))
        }
        fn start_time(&self, _: i64) -> Option<f64> { Some(1.0) }
        fn fds(&self, _: i64) -> Vec<PathBuf> { Vec::new() }
    }

    /// Descoberta de 20 sessões sobre as pastas REAIS desta máquina (só leitura): os 20 projetos
    /// com mais transcripts, metade com `--session-id` (o mais novo do projeto), metade sem (cai
    /// nos marcadores), e os sidecars reais de `~/.hangar`.
    /// `cargo test --release -p hangar-server --lib list::discover_other::tests::tick_cost_real -- --ignored --nocapture`
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn tick_cost_real() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let claude = home.join(".claude");
        let dirs = Dirs { home: home.clone(), claude: claude.clone(), codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi/agent/sessions"), omp_config: home.join(".omp"),
            omp_agent: home.join(".omp/agent"), kimi_home: home.join(".kimi-code") };
        let mut projects: Vec<(usize, PathBuf)> = std::fs::read_dir(claude.join("projects")).unwrap().flatten()
            .map(|d| (std::fs::read_dir(d.path()).map(|r| r.count()).unwrap_or(0), d.path())).collect();
        projects.sort_by(|a, b| b.0.cmp(&a.0));
        let mut sessions: Vec<(String, Option<String>)> = Vec::new();
        for (_, dir) in &projects {
            let Some(newest) = newest(dir, &|n| n.ends_with(".jsonl")) else { continue };
            // O cwd de verdade sai do transcript: o nome da pasta é o sanitizado.
            let head = std::fs::read(&newest).unwrap_or_default();
            let Some(cwd) = head.split(|b| *b == b'\n').filter_map(|l| serde_json::from_slice::<Value>(l).ok())
                .find_map(|v| v.get("cwd").and_then(Value::as_str).map(str::to_owned)) else { continue };
            if sessions.iter().any(|(c, _)| *c == cwd) { continue; }
            let sid = Path::new(&newest).file_stem().unwrap().to_string_lossy().into_owned();
            let with_sid = sessions.len() % 2 == 0;
            sessions.push((cwd, with_sid.then_some(sid)));
            if sessions.len() == 20 { break; }
        }
        let panes: Vec<Pane> = sessions.iter().enumerate().map(|(i, (cwd, _))| Pane { session: format!("s{i}"),
            active: true, pid: Some(1000 + i as u32), cwd: cwd.clone(), pane_id: format!("%{i}"), ..Pane::default() }).collect();
        let children: ChildrenMap = (0..sessions.len() as i64).map(|i| (1000 + i, vec![2000 + i])).collect();
        let procs = RealTree { sids: sessions.iter().map(|(_, s)| s.clone()).collect(), config: claude.to_string_lossy().into_owned() };
        let mut resolver = Resolver::default();
        let cpu = || std::fs::read_to_string("/proc/thread-self/schedstat").unwrap().split_whitespace().next().unwrap().parse::<u64>().unwrap();
        let mut phase = |label: &str, n: usize| {
            let (c, t) = (cpu(), std::time::Instant::now());
            for _ in 0..n { std::hint::black_box(discover_rows(&panes, &procs, &children, &mut resolver, &dirs)); }
            println!("{label} ({} sessões): {:.0} µs/tique, CPU {:.0} µs/tique", panes.len(),
                t.elapsed().as_micros() as f64 / n as f64, (cpu() - c) as f64 / 1000.0 / n as f64);
        };
        phase("pasta real, tique frio", 1);
        phase("pasta real, tiques seguintes", 50);
    }
}
