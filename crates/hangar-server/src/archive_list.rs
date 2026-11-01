//! O Arquivo: as pastas (`GET /api/archive`), as conversas de uma pasta (`/api/archive/{project}`), as
//! mais novas de tudo (`/api/archive/recent`) e as de um cwd (`/api/archive-por-cwd`). Port de
//! `archive.list_folders`/`list_conversations`/`list_recent` e `archive_providers.conversas`.
//! Cabeçalho, se tem usuário, se é conversa interna e última mensagem vêm do índice da busca; arquivo
//! fora dele, ou mudado depois da última passada, é lido do disco como o Python lê.
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, Path as RoutePath, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::{Value, json};

use crate::accounts::Provider as AccountProvider;
use crate::accounts::catalog::Account;
use crate::routes::{AppState, cors, gate, pass, route_failed};
use crate::search::ArchiveRow;
use crate::session_open::archive::{self, PROJECT, UUID, Where};
use crate::session_open::{AgentProvider, OpenError};
use crate::session_write::json_response;

const FOLDER_CAP: usize = 100;
const RECENT_CAP: i64 = 40;
const BY_CWD_CAP: i64 = 12;
const PROVIDERS: [&str; 4] = ["pi", "omp", "kimi", "codex"];
// `_RESERVA_JANELA_S`: a reserva pré-aquecida vive no máximo 10 min.
const RESERVE_WINDOW: f64 = 15.0 * 60.0;
const TAIL_BYTES: u64 = 64 * 1024;
const HEAD_LINES: usize = 60;

/// As quatro rotas, para o `migration_status`.
pub fn matches(method: &Method, path: &str) -> bool {
    *method == Method::GET && (matches!(path, "/api/archive" | "/api/archive-por-cwd")
        || path.strip_prefix("/api/archive/").is_some_and(|rest| !rest.is_empty() && !rest.contains('/')))
}

/// Um transcript Claude: `(mtime, arquivo, pasta, conta, rótulo)` do `_contas`/`_folder_files`.
struct Claude { mtime: f64, md: std::fs::Metadata, path: PathBuf, project: String, config_dir: Option<String>, label: String, account: usize }

/// Uma conversa de Pi, omp, Kimi ou Codex (`archive_providers.Conversa`), com a conta Codex dona.
pub(crate) struct Other {
    pub(crate) provider: &'static str, pub(crate) cwd: Option<String>, pub(crate) sid: String, pub(crate) path: PathBuf,
    pub(crate) mtime: f64, pub(crate) codex: Option<Account>,
}

/// O cwd que o índice já leu de um transcript que não mudou depois da leitura; `None` = ler do disco.
pub(crate) type KnownCwd<'a> = &'a dyn Fn(&Path) -> Option<Option<String>>;

/// O que o Arquivo sabe de um transcript Claude.
struct Facts { preview: String, cwd: Option<String>, listed: bool, last: Option<String> }

/// `os.stat().st_mtime` do Python: `sec + nsec * 1e-9`.
fn py_mtime(md: &std::fs::Metadata) -> f64 {
    let d = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).unwrap_or_default();
    d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9
}

fn real(path: &Path) -> PathBuf { std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()) }

/// `_projeto_de`: a pasta do Claude para o cwd de outro agente.
pub(crate) fn project_of(cwd: Option<&str>) -> Option<String> {
    let project = hangar_workspace::worktrees::sanitize_cwd(cwd.filter(|c| !c.is_empty())?);
    PROJECT.is_match(&project).then_some(project)
}

fn account_error(e: crate::accounts::catalog::AccountError) -> OpenError { OpenError::new(e.status, e.code, e.message).with_params(e.params) }

/// O pedido, com o que a máquina sabe agora.
struct Ctx<'a> {
    st: &'a AppState,
    /// (caminho, rótulo) das contas Claude do catálogo, na ordem dele.
    catalog: Vec<(String, String)>,
    index: HashMap<String, ArchiveRow>,
    live: HashSet<PathBuf>,
    closed: HashMap<String, String>,
    now: f64,
}

impl<'a> Ctx<'a> {
    fn new(st: &'a AppState, live: HashSet<PathBuf>) -> Result<Self, OpenError> {
        let catalog = st.accounts.claude_catalog().map_err(account_error)?.as_array().into_iter().flatten()
            .filter_map(|row| Some((row["path"].as_str()?.to_owned(), row["label"].as_str().unwrap_or("").to_owned()))).collect();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        Ok(Self { st, catalog, index: st.search.archive_rows(), live, closed: closed_names(&st.accounts.env.claude_base), now })
    }

    fn valid_config_dir(&self, dir: &str) -> bool { self.catalog.iter().any(|(path, _)| path == dir) }

    /// `_contas`: (conta, rótulo, `projects`); a do servidor por último, se ainda não veio.
    fn accounts(&self, only: Option<&str>) -> Vec<(Option<String>, String, PathBuf)> {
        if let Some(dir) = only {
            let label = self.catalog.iter().find(|(path, _)| path == dir).map(|(_, l)| l.clone()).unwrap_or_default();
            return vec![(Some(dir.to_owned()), label, Path::new(dir).join("projects"))];
        }
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (dir, label) in &self.catalog {
            let projects = Path::new(dir).join("projects");
            if seen.insert(crate::list::links::resolve_lenient(&projects)) { out.push((Some(dir.clone()), label.clone(), projects)); }
        }
        let own = self.st.accounts.env.claude_base.join("projects");
        if seen.insert(crate::list::links::resolve_lenient(&own)) { out.push((None, String::new(), own)); }
        out
    }

    /// Os transcripts `<uuid>.jsonl` das contas; a conversa que mudou de pasta fica só no caminho
    /// mais novo (F9). Pasta ilegível é pulada com linha no diário, stat que falha tira o arquivo.
    fn claude_files(&self, only: Option<&str>) -> Vec<Claude> {
        let mut out: Vec<Claude> = Vec::new();
        let mut newest: HashMap<String, usize> = HashMap::new();
        let read_dir = |dir: &Path| std::fs::read_dir(dir).inspect_err(|e| if e.kind() != std::io::ErrorKind::NotFound {
            self.st.search.report_once("arquivo.pasta_ilegivel", &dir.to_string_lossy(), || format!("{}: {e}", dir.display()));
        }).into_iter().flatten().flatten();
        for (i, (config_dir, label, base)) in self.accounts(only).into_iter().enumerate() {
            for project in read_dir(&base) {
                let dir = project.path();
                if !dir.is_dir() { continue; }
                for file in read_dir(&dir) {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if !hangar_workspace::worktrees::is_conversation_file(&name) { continue; }
                    let path = file.path();
                    let Ok(md) = std::fs::metadata(&path) else { continue };
                    let sid = name.trim_end_matches(".jsonl").to_owned();
                    let entry = Claude { mtime: py_mtime(&md), md, path, project: project.file_name().to_string_lossy().into_owned(),
                        config_dir: config_dir.clone(), label: label.clone(), account: i };
                    match newest.get(&sid) {
                        Some(&at) if out[at].mtime >= entry.mtime => {}
                        Some(&at) => out[at] = entry,
                        None => { newest.insert(sid, out.len()); out.push(entry); }
                    }
                }
            }
        }
        out
    }

    /// A linha do índice, se o arquivo não mudou depois da leitura dela.
    fn row(&self, f: &Claude) -> Option<&ArchiveRow> { self.index.get(&*f.path.to_string_lossy()).filter(|r| r.fresh(&f.md)) }

    /// A linha do índice de um transcript de outro agente, se ele não mudou depois da leitura dela.
    fn agent_row(&self, path: &Path) -> Option<&ArchiveRow> {
        let row = self.index.get(&*path.to_string_lossy())?;
        std::fs::metadata(path).ok().filter(|md| row.fresh(md)).map(|_| row)
    }

    /// Prévia ou arquivo parado há mais de 15 min basta (`_tem_usuario`): há conversa com prévia e sem
    /// linha `user`.
    fn settled(&self, f: &Claude, preview: &str) -> bool { !preview.is_empty() || self.now - f.mtime > RESERVE_WINDOW }

    /// Entra no Arquivo. A conversa interna sai só quando o índice já a marcou (F8): fora dele,
    /// descobrir custaria ler o começo de cada transcript, o que o Python não fazia.
    fn row_listed(&self, f: &Claude, row: &ArchiveRow) -> bool { (row.has_user || self.settled(f, &row.preview)) && !row.internal }

    /// Se entra no Arquivo sem ler o cabeçalho (lista de pastas): o índice, ou o `_tem_usuario` sem prévia.
    fn listed(&self, f: &Claude) -> bool {
        match self.row(f) { Some(row) => self.row_listed(f, row), None => self.settled(f, "") || has_user_line(&f.path) }
    }

    /// O cwd do índice; fora dele, o do cabeçalho.
    fn cwd(&self, f: &Claude) -> Option<String> { self.row(f).map_or_else(|| read_head(&f.path).cwd, |row| row.cwd.clone()) }

    /// Prévia, cwd, se entra no Arquivo e a última mensagem do índice; arquivo fora dele ou mudado é
    /// lido do disco (`_head_info`).
    fn facts(&self, f: &Claude) -> Facts {
        if let Some(row) = self.row(f) {
            return Facts { preview: row.preview.clone(), cwd: row.cwd.clone(), listed: self.row_listed(f, row), last: Some(row.last_text.clone()) };
        }
        let head = read_head(&f.path);
        let listed = self.settled(f, &head.preview) || head.has_user || has_user_line(&f.path);
        Facts { listed, preview: head.preview, cwd: head.cwd, last: None }
    }

    fn name(&self, path: &Path) -> Value { self.closed.get(&*real(path).to_string_lossy()).map_or(Value::Null, |n| json!(n)) }

    fn claude_entry(&self, f: &Claude, facts: Facts) -> Value {
        let ultima = facts.last.unwrap_or_else(|| tail_text(&f.path, AgentProvider::Claude));
        json!({"project": f.project, "cwd": facts.cwd, "session_id": f.path.file_stem().map(|s| s.to_string_lossy()), "mtime": f.mtime,
            "preview": facts.preview, "ultima": ultima, "live": self.live.contains(&real(&f.path)), "config_dir": f.config_dir,
            "conta": f.label, "provider": "claude", "codex_home": null, "codex_account": null, "name": self.name(&f.path)})
    }

    fn other_entry(&self, c: &Other, project: &str) -> Value {
        json!({"project": project, "cwd": c.cwd, "session_id": c.sid, "mtime": c.mtime, "preview": "",
            "ultima": self.other_tail(c), "live": self.live.contains(&real(&c.path)), "config_dir": null, "conta": "",
            "provider": c.provider, "codex_home": c.codex.as_ref().map(|a| a.home.to_string_lossy().into_owned()),
            "codex_account": c.codex.as_ref().map(|a| a.id.clone()), "name": self.name(&c.path)})
    }

    /// `_tail_info` de outro agente; o Codex transferido sai do histórico composto (ARQ-21).
    fn other_tail(&self, c: &Other) -> String {
        let provider = AgentProvider::parse(c.provider).unwrap_or(AgentProvider::Pi);
        if let Some(account) = &c.codex {
            let home = &self.st.accounts.env.home;
            let key = crate::locate::session_key(&c.path.to_string_lossy());
            let composed = crate::speech::history::transfer_history::archive_record(home, &account.home, &key)
                .and_then(|record| record.map(|r| crate::speech::history::transfer_history::archived_history(&r, &c.path, &account.home)).transpose());
            match composed {
                Ok(Some(events)) => return last_message(events.iter().rev()).unwrap_or_default(),
                Ok(None) => {}
                Err(e) => {
                    // Fonte inválida nunca vira o fim do rollout cru.
                    tracing::warn!(error = %e, path = %c.path.display(), "arquivo: histórico da transferência indisponível");
                    return String::new();
                }
            }
        }
        match self.agent_row(&c.path) {
            Some(row) => row.last_text.clone(),
            None => tail_text(&c.path, provider),
        }
    }

    /// `archive_providers.conversas`: o que falha sai da lista com linha no diário, sem derrubar o resto.
    fn others(&self) -> Vec<Other> {
        let env = &self.st.accounts.env;
        let known = |path: &Path| self.agent_row(path).map(|row| row.cwd.clone());
        let (mut out, mut unread) = agent_conversations(env, &known);
        match codex_conversations(env, &known) {
            Ok((codex, bad)) => { out.extend(codex); unread.extend(bad); }
            Err(e) => unread.push(Unread::new("codex", None, e)),
        }
        self.st.search.failing("arquivo.conversa_ilegivel", unread.into_iter().map(|u| (u.key(), u.error)).collect());
        out
    }

    /// `list_folders`: as pastas com conversa, de todas as contas somadas; os outros agentes entram na
    /// pasta do Claude do cwd deles.
    fn folders(&self) -> Vec<Value> {
        struct Folder { project: String, cwd: Option<String>, count: usize, mtime: f64 }
        let mut files = self.claude_files(None);
        files.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        // Por (conta, pasta): quantas entram e o cwd da mais nova; depois somadas na ordem das contas.
        let mut groups: Vec<(usize, String, usize, f64, Option<String>)> = Vec::new();
        let mut at: HashMap<(usize, String), usize> = HashMap::new();
        for f in &files {
            if !self.listed(f) { continue; }
            match at.get(&(f.account, f.project.clone())) {
                Some(&i) => groups[i].2 += 1,
                None => {
                    at.insert((f.account, f.project.clone()), groups.len());
                    groups.push((f.account, f.project.clone(), 1, f.mtime, self.cwd(f)));
                }
            }
        }
        groups.sort_by_key(|g| g.0);
        let mut agg: Vec<Folder> = Vec::new();
        let mut merge = |project: String, cwd: Option<String>, count: usize, mtime: f64| {
            match agg.iter_mut().find(|f| f.project == project) {
                None => agg.push(Folder { project, cwd, count, mtime }),
                Some(f) => {
                    f.count += count;
                    if mtime > f.mtime { f.mtime = mtime; f.cwd = cwd.or(f.cwd.take()); }
                }
            }
        };
        for (_, project, count, mtime, cwd) in groups { merge(project, cwd, count, mtime); }
        for c in self.others() {
            if let Some(project) = project_of(c.cwd.as_deref()) { merge(project, c.cwd, 1, c.mtime); }
        }
        agg.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        agg.into_iter().map(|f| json!({"project": f.project, "cwd": f.cwd, "count": f.count, "mtime": f.mtime})).collect()
    }

    /// `list_conversations`: as conversas de uma pasta, de todas as contas (ou da pedida), mais novas
    /// primeiro, com o teto sobre o total.
    fn conversations(&self, project: &str, cap: usize, config_dir: Option<&str>, codex_account: Option<&str>, provider: Option<&str>)
        -> Result<Vec<Value>, OpenError> {
        if !PROJECT.is_match(project) { return Err(OpenError::new(400, "erro_path_invalido", "invalid path")); }
        if let Some(id) = codex_account {
            crate::accounts::AccountService::new(self.st.accounts.env.clone()).resolve(AccountProvider::Codex, id).map_err(account_error)?;
        }
        let found = self.accounts(config_dir).iter().any(|(_, _, base)| base.join(project).is_dir());
        let mut others: Vec<Other> = if config_dir.is_some() || provider == Some("claude") { Vec::new() } else {
            self.others().into_iter().filter(|c| project_of(c.cwd.as_deref()).as_deref() == Some(project)
                && provider.is_none_or(|p| c.provider == p)).collect()
        };
        others.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        if !found && others.is_empty() { return Err(OpenError::new(404, "erro_projeto_nao_encontrado", "project not found")); }
        let mut files: Vec<Claude> = if provider.is_some_and(|p| p != "claude") { Vec::new() } else {
            self.claude_files(config_dir).into_iter().filter(|f| f.project == project).collect()
        };
        files.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        let mut out: Vec<(f64, Value)> = Vec::new();
        for f in &files {
            if out.len() >= cap { break; }
            let facts = self.facts(f);
            if facts.listed { out.push((f.mtime, self.claude_entry(f, facts))); }
        }
        let others = others.into_iter().filter(|c| codex_account.is_none() || c.provider != "codex"
            || c.codex.as_ref().is_some_and(|a| Some(a.id.as_str()) == codex_account));
        out.extend(others.take(cap).map(|c| (c.mtime, self.other_entry(&c, project))));
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        Ok(out.into_iter().take(cap).map(|(_, v)| v).collect())
    }

    /// `list_recent`: as mais novas de tudo. A reserva sem usuário e a conversa interna não ocupam
    /// vaga (F7, F8): o filtro vem antes do teto.
    fn recent(&self, cap: usize) -> Vec<Value> {
        enum Item { Claude(Claude), Other(Other, String) }
        let mut items: Vec<(f64, Item)> = self.claude_files(None).into_iter().map(|f| (f.mtime, Item::Claude(f))).collect();
        items.extend(self.others().into_iter().filter_map(|c| { let p = project_of(c.cwd.as_deref())?; Some((c.mtime, Item::Other(c, p))) }));
        items.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out = Vec::new();
        for (_, item) in &items {
            if out.len() >= cap { break; }
            match item {
                Item::Claude(f) => {
                    let facts = self.facts(f);
                    if facts.listed { out.push(self.claude_entry(f, facts)); }
                }
                Item::Other(c, project) => out.push(self.other_entry(c, project)),
            }
        }
        out
    }
}

fn mtime_of(path: &Path) -> f64 { std::fs::metadata(path).map_or(0.0, |md| py_mtime(&md)) }

/// O que não pôde ser lido agora: o agente, onde (`None` = o agente inteiro) e o erro com o lugar.
/// O índice da busca mantém o que já tem lá; o Arquivo lista o resto e registra no diário.
pub(crate) struct Unread { pub(crate) provider: &'static str, pub(crate) scope: Option<PathBuf>, pub(crate) error: String }

impl Unread {
    pub(crate) fn new(provider: &'static str, scope: Option<&Path>, error: impl std::fmt::Display) -> Self {
        let error = match scope { Some(p) => format!("{provider} {}: {error}", p.display()), None => format!("{provider}: {error}") };
        Self { provider, scope: scope.map(Path::to_owned), error }
    }

    /// Erro de leitura; a ausência não é falha (agente não instalado, pasta que nasce depois).
    fn io(provider: &'static str, scope: Option<&Path>, at: &Path, e: &std::io::Error, unread: &mut Vec<Unread>) {
        if e.kind() != std::io::ErrorKind::NotFound { unread.push(Self::new(provider, scope, format!("{}: {e}", at.display()))); }
    }

    pub(crate) fn key(&self) -> String {
        format!("{}:{}", self.provider, self.scope.as_deref().map_or(String::new(), |p| p.to_string_lossy().into_owned()))
    }

    /// O transcript `path` do agente `provider` está no que não pôde ser lido.
    pub(crate) fn covers(&self, provider: &str, path: &str) -> bool {
        self.provider == provider && self.scope.as_deref().is_none_or(|s| Path::new(path).starts_with(s))
    }
}

/// As conversas de Pi, omp e Kimi (o índice da busca lê as mesmas) e o que não pôde ser lido.
pub(crate) fn agent_conversations(env: &crate::accounts::environment::AccountEnvironment, known: KnownCwd) -> (Vec<Other>, Vec<Unread>) {
    let w = Where { home: &env.home, env: &env.base, server_projects: Path::new(""), config_dirs: &[], live: &[] };
    let mut unread = Vec::new();
    let mut out = pi_conversations(&archive::agent_root(&w, AgentProvider::Pi), "pi", known, &mut unread);
    out.extend(pi_conversations(&archive::agent_root(&w, AgentProvider::Omp), "omp", known, &mut unread));
    out.extend(kimi_conversations(&crate::session_open::agents::kimi_home(&env.home, &env.base), &mut unread));
    (out, unread)
}

/// `_codex_conversas`: rollouts de cada conta só na conta dona, sem transferência em andamento.
/// Erro = catálogo de contas ilegível; rollout ambíguo ou com transferência ilegível sai sozinho.
pub(crate) fn codex_conversations(env: &crate::accounts::environment::AccountEnvironment, known: KnownCwd) -> Result<(Vec<Other>, Vec<Unread>), String> {
    let service = crate::accounts::AccountService::new(env.clone());
    let accounts = service.snapshot(AccountProvider::Codex).map_err(|e| e.code.to_string())?;
    let (mut out, mut unread) = (Vec::new(), Vec::new());
    for account in &accounts {
        for path in codex_rollouts(&account.home, &mut unread) {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let sid: String = stem.chars().skip(stem.chars().count().saturating_sub(36)).collect();
            if !UUID.is_match(&sid) { continue; }
            let owner = match archive::codex_owner(&accounts, &path) {
                Ok(owner) => owner,
                Err(_) => { unread.push(Unread::new("codex", Some(&path), "codex_account_ambiguous_rollout")); continue }
            };
            let Some(owner) = owner.filter(|o| o.id == account.id) else { continue };
            let phase = match crate::list::local_transfers::thread_phase(&env.home, &owner.home.to_string_lossy(), &sid) {
                Ok(phase) => phase,
                Err(e) => { unread.push(Unread::new("codex", Some(&path), e)); continue }
            };
            if phase.is_some_and(|p| p != "complete") { continue; }
            let cwd = known(&path).unwrap_or_else(|| archive::header(&path, 5,
                |o| (o["type"] == "session_meta").then(|| o["payload"]["cwd"].as_str().filter(|c| !c.is_empty()).map(str::to_owned)).flatten()));
            out.push(Other { provider: "codex", cwd, sid, mtime: mtime_of(&path), path, codex: Some(owner.clone()) });
        }
    }
    Ok((out, unread))
}

/// `_pi_conversas`: o transcript mora direto no slug do cwd; subagente (`<stem>/…`) fica fora.
fn pi_conversations(root: &Path, provider: &'static str, known: KnownCwd, unread: &mut Vec<Unread>) -> Vec<Other> {
    let mut out = Vec::new();
    let dirs = match std::fs::read_dir(root) {
        Ok(dirs) => dirs,
        Err(e) => { Unread::io(provider, Some(root), root, &e, unread); return out }
    };
    for dir in dirs.flatten().map(|d| d.path()).filter(|d| d.is_dir()) {
        let files = match std::fs::read_dir(&dir) {
            Ok(files) => files,
            Err(e) => { Unread::io(provider, Some(&dir), &dir, &e, unread); continue }
        };
        for path in files.flatten().map(|f| f.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let sid = stem.split_once('_').map_or(stem.as_str(), |(_, s)| s).to_owned();
            if !UUID.is_match(&sid) { continue; }
            let cwd = known(&path).unwrap_or_else(|| archive::header(&path, 5,
                |o| (o["type"] == "session").then(|| o["cwd"].as_str().filter(|c| !c.is_empty()).map(str::to_owned)).flatten()));
            out.push(Other { provider, cwd, sid, mtime: mtime_of(&path), path, codex: None });
        }
    }
    out
}

/// `_kimi_conversas`: o índice de trás para frente, a entrada mais recente por sessão, só com o wire.
fn kimi_conversations(kimi_home: &Path, unread: &mut Vec<Unread>) -> Vec<Other> {
    let index = kimi_home.join("session_index.jsonl");
    let raw = match std::fs::read(&index) {
        Ok(raw) => raw,
        // Os wires moram fora do índice: sem ele, nenhuma conversa do Kimi pode sair do índice da busca.
        Err(e) => { Unread::io("kimi", None, &index, &e, unread); return Vec::new() }
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for o in raw.split(|b| *b == b'\n').rev().filter_map(|line| serde_json::from_slice::<Value>(line).ok()) {
        let (Some(sid), Some(dir)) = (o["sessionId"].as_str().filter(|s| !s.is_empty()), o["sessionDir"].as_str().filter(|s| !s.is_empty())) else { continue };
        if !seen.insert(sid.to_owned()) { continue; }
        let wire = Path::new(dir).join("agents").join("main").join("wire.jsonl");
        if !wire.is_file() { continue; }
        out.push(Other { provider: "kimi", cwd: o["workDir"].as_str().map(str::to_owned), sid: sid.to_owned(), mtime: mtime_of(&wire), path: wire, codex: None });
    }
    out
}

/// `_codex_rollouts`: `rollout-*.jsonl` em qualquer profundidade de `sessions` e `archived_sessions`,
/// sem repetir pelo caminho real.
fn codex_rollouts(home: &Path, unread: &mut Vec<Unread>) -> Vec<PathBuf> {
    fn walk(dir: &Path, seen: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>, unread: &mut Vec<Unread>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => return Unread::io("codex", Some(dir), dir, &e, unread),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() { walk(&path, seen, out, unread); continue; }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) { continue; }
            if let Ok(canonical) = path.canonicalize() && seen.insert(canonical) { out.push(path); }
        }
    }
    let (mut seen, mut out) = (HashSet::new(), Vec::new());
    for root in [home.join("sessions"), home.join("archived_sessions")] { walk(&root, &mut seen, &mut out, unread); }
    out
}

/// `closed_names.names()`: caminho real do transcript → nome da sessão fechada; ilegível é vazio.
pub(crate) fn closed_names(claude_base: &Path) -> HashMap<String, String> {
    let path = claude_base.join("closed-sessions.json");
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => { tracing::warn!(error = %e, "arquivo: nomes de sessão fechada ilegíveis"); return HashMap::new() }
    };
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&raw) else {
        tracing::warn!("arquivo: nomes de sessão fechada ilegíveis");
        return HashMap::new();
    };
    map.into_iter().filter_map(|(k, v)| Some((k, v.get("name")?.as_str()?.to_owned()))).collect()
}

/// `_head_info`: as primeiras linhas pelo mesmo leitor do índice, até achar prévia e cwd.
fn read_head(path: &Path) -> crate::search::FileState {
    let mut st = crate::search::FileState::default();
    let Ok(file) = std::fs::File::open(path) else { return st };
    let prefixes = crate::search::internal_prefixes();
    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::new();
    for _ in 0..HEAD_LINES {
        line.clear();
        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) { break; }
        crate::search::absorb_line(&mut st, &line, &prefixes);
        if st.head_done { break; }
    }
    st
}

/// A varredura do `_tem_usuario`: alguma linha `type: "user"`; arquivo ilegível conta como conversa.
fn has_user_line(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        tracing::warn!(path = %path.display(), "arquivo: transcript ilegível");
        return true;
    };
    std::io::BufReader::new(file).split(b'\n').map_while(Result::ok).any(|raw| {
        raw.windows(6).any(|w| w == b"\"user\"")
            && matches!(crate::transcript::decode_line(&raw), Some(Value::Object(o)) if o.get("type").and_then(Value::as_str) == Some("user"))
    })
}

/// A última mensagem com texto, sem marcação e cortada.
fn last_message<'e>(events: impl Iterator<Item = &'e ChatEvent>) -> Option<String> {
    events.into_iter().find(|e| matches!(e.kind, ChatKind::UserMsg | ChatKind::AssistantMsg) && e.text.as_deref().is_some_and(|t| !t.is_empty()))
        .map(|e| crate::search::cut(&crate::list::reply::plain_text(e.text.as_deref().unwrap_or(""))))
}

/// `_tail_info`: a última mensagem lida do fim (64 KB, depois 1 MB).
fn tail_text(path: &Path, provider: AgentProvider) -> String {
    for span in [TAIL_BYTES, TAIL_BYTES * 16] {
        let read = || -> std::io::Result<(Vec<u8>, bool)> {
            let mut file = std::fs::File::open(path)?;
            let size = file.metadata()?.len();
            let start = size.saturating_sub(span);
            file.seek(SeekFrom::Start(start))?;
            let mut raw = Vec::new();
            file.read_to_end(&mut raw)?;
            Ok((raw, start == 0))
        };
        let (raw, whole) = match read() {
            Ok(read) => read,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "arquivo: não consegui ler o fim do transcript");
                return String::new();
            }
        };
        // O começo da janela pode cair no meio de uma linha.
        let lines: Vec<&[u8]> = raw.split(|b| *b == b'\n').skip(usize::from(!whole)).collect();
        for line in lines.iter().rev() {
            let events = if provider == AgentProvider::Claude {
                match crate::transcript::decode_line(line) { Some(Value::Object(o)) => crate::transcript::parse_claude_object(&o), _ => continue }
            } else {
                crate::archive_history::agent_line(provider, line)
            };
            if let Some(text) = last_message(events.iter().rev()) { return text; }
        }
        if whole { break; }
    }
    String::new()
}

/// `int` do FastAPI na query; ausente é `default`.
fn int_param(query: Option<&str>, name: &str, default: i64) -> Result<i64, Value> {
    let Some(raw) = crate::auth::query_param(query, name) else { return Ok(default) };
    let t = raw.trim();
    crate::search_context::PY_INT.is_match(t).then(|| t.replace('_', "").parse::<f64>().ok()).flatten().map(|n| n as i64)
        .ok_or_else(|| json!({"type": "int_parsing", "loc": ["query", name],
            "msg": "Input should be a valid integer, unable to parse string as an integer", "input": raw}))
}

fn unprocessable(errors: Vec<Value>) -> Response { json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": errors})) }

type Work = Box<dyn FnOnce(&Ctx) -> Result<Value, OpenError> + Send>;

/// O que as quatro rotas fazem em volta da leitura: só o dono, as sessões vivas da lista, o trabalho
/// fora do laço e o erro com o código.
async fn serve(st: Arc<AppState>, peer: SocketAddr, req: Request, work: impl FnOnce(Option<&str>) -> Result<Work, Response>) -> Response {
    let (forward, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || req.method() != Method::GET { return pass(&st, req, &forward).await; }
    let work = match work(req.uri().query()) { Ok(work) => work, Err(response) => return response };
    let live: HashSet<PathBuf> = match st.list.snapshot().await {
        Ok(produced) => produced.rows.iter().filter_map(|r| r.jsonl.as_deref()).map(|j| real(Path::new(j))).collect(),
        Err(e) => return crate::archive_history::reply(OpenError::new(503, "erro_lista_indisponivel", "a lista de sessões está indisponível")
            .with_params(json!({"detalhe": e.code}))),
    };
    let worker = st.clone();
    let done = tokio::task::spawn_blocking(move || Ctx::new(&worker, live).and_then(|ctx| work(&ctx))).await;
    let mut response = match done {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(error)) => crate::archive_history::reply(error),
        Err(_) => route_failed(&st, req.headers(), "rust.archive_list_failed", "archive", "archive_list_panic", "a leitura do Arquivo caiu no servidor"),
    };
    cors(req.headers(), response.headers_mut());
    response
}

pub async fn folders(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    serve(st, peer, req, |_| Ok(Box::new(|ctx: &Ctx| Ok(Value::Array(ctx.folders()))))).await
}

pub async fn recent(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    serve(st, peer, req, |query| {
        let cap = int_param(query, "cap", RECENT_CAP).map_err(|e| unprocessable(vec![e]))?.clamp(1, FOLDER_CAP as i64) as usize;
        Ok(Box::new(move |ctx: &Ctx| Ok(Value::Array(ctx.recent(cap)))))
    }).await
}

pub async fn folder(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, RoutePath(project): RoutePath<String>, req: Request) -> Response {
    serve(st, peer, req, |query| {
        let codex_account = crate::auth::query_param(query, "codex_account");
        Ok(Box::new(move |ctx: &Ctx| ctx.conversations(&project, FOLDER_CAP, None, codex_account.as_deref(), None).map(Value::Array)))
    }).await
}

pub async fn by_cwd(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    serve(st, peer, req, |query| {
        let cwd = crate::auth::query_param(query, "cwd");
        let mut errors = Vec::new();
        if cwd.is_none() { errors.push(json!({"type": "missing", "loc": ["query", "cwd"], "msg": "Field required", "input": null})); }
        let cap = int_param(query, "cap", BY_CWD_CAP).map_err(|e| errors.push(e)).unwrap_or(0);
        if !errors.is_empty() { return Err(unprocessable(errors)); }
        let (cwd, config_dir) = (cwd.unwrap_or_default(), crate::auth::query_param(query, "config_dir"));
        let provider = crate::auth::query_param(query, "provider").unwrap_or_else(|| "claude".into());
        let codex_account = crate::auth::query_param(query, "codex_account");
        Ok(Box::new(move |ctx: &Ctx| {
            if config_dir.as_deref().is_some_and(|d| !ctx.valid_config_dir(d)) {
                return Err(OpenError::new(400, "erro_config_dir_invalido", "config_dir invalido"));
            }
            if provider != "claude" && !PROVIDERS.contains(&provider.as_str()) {
                return Err(OpenError::new(400, "erro_provider_invalido", "provider invalido"));
            }
            // shortcut: `cap` negativo vira 0 (o Python fatiava pelo fim), ninguém o manda.
            let cap = cap.max(0) as usize;
            let claude = provider == "claude";
            let found = ctx.conversations(&hangar_workspace::worktrees::sanitize_cwd(&cwd), cap,
                config_dir.as_deref().filter(|_| claude), codex_account.as_deref().filter(|_| provider == "codex"), Some(&provider));
            match found {
                Ok(list) => Ok(Value::Array(list.into_iter().filter(|e| e["provider"] == provider.as_str()).take(cap).collect())),
                Err(e) if matches!(e.code, Some("erro_path_invalido" | "erro_projeto_nao_encontrado")) => Ok(json!([])),
                Err(e) => Err(e),
            }
        }))
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_of_keeps_the_drive_root_and_drops_an_empty_cwd() {
        // ARQ-04 e ARQ-05 (`sanitize_cwd` + `_projeto_de`): a raiz do drive mantém a barra; cwd nulo
        // ou vazio não dá pasta.
        for (cwd, want) in [(Some("C:\\"), Some("C--")), (Some("C:\\x\\"), Some("C--x")), (Some("/"), Some("-")),
            (Some("/a/b/"), Some("-a-b")), (Some(""), None), (None, None)] {
            assert_eq!(project_of(cwd).as_deref(), want, "{cwd:?}");
        }
    }

    #[test]
    fn preview_text_drops_markdown_like_the_python() {
        // ARQ-18 (`test_previa_sai_sem_marcacao_de_markdown` e saídas do `_texto_simples`).
        for (text, want) in [
            ("**O backend responde certo** — veja `app.py`", "O backend responde certo — veja app.py"),
            ("## Titulo\n- item um\n- item dois", "Titulo item um item dois"),
            ("veja [o guia](http://x/y) agora", "veja o guia agora"),
            ("> citado\n> mais", "citado mais"),
            ("1. um\n2. dois", "um dois"),
            ("* a\n+ b", "a b"),
            ("_ênfase_ e ~~riscado~~", "ênfase e riscado"),
        ] {
            assert_eq!(crate::list::reply::plain_text(text), want, "{text:?}");
        }
    }

    #[test]
    fn preview_cut_like_the_python() {
        // ARQ-19 (`_cortar`): corte na palavra só com espaço depois do 60º caractere, pontuação final
        // aparada, reticências.
        let a = |n| "a".repeat(n);
        let b = |n| "b".repeat(n);
        for (text, want) in [
            (a(130), format!("{}…", a(120))),
            (format!("{} {}", a(61), b(70)), format!("{}…", a(61))),
            (format!("{} {}", a(60), b(100)), format!("{} {}…", a(60), b(59))),
            (format!("{} {}", a(50), b(100)), format!("{} {}…", a(50), b(69))),
            ("fim, ".repeat(30), format!("{}…", "fim, ".repeat(24).trim_end_matches([' ', ',']))),
            (format!("{}.", "z".repeat(119)), format!("{}.", "z".repeat(119))),
        ] {
            assert_eq!(crate::search::cut(&text), want, "{text:?}");
        }
    }

    #[test]
    fn mtime_of_a_missing_file_is_zero() {
        // PRV-06 (`_mtime`).
        assert_eq!(mtime_of(Path::new("/nao/existe/arquivo.jsonl")), 0.0);
    }

    #[test]
    fn last_message_of_a_file_shorter_than_the_window_keeps_the_first_line() {
        // `test_ultima_msg_com_arquivo_menor_que_o_span`: o começo do arquivo cai na janela e a 1ª
        // linha não é parcial.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("11111111-1111-1111-1111-111111111111.jsonl");
        std::fs::write(&path, format!("{}\n", json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": "unica msg"}}))).unwrap();
        assert_eq!(tail_text(&path, AgentProvider::Claude), "unica msg");
    }
}
