//! Uma rodada completa: inventário de todas as máquinas, um sonho por projeto (do mais recente ao
//! teto), gravação conferida em cada pasta de memória e a passada do usuário no fim.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use hangar_dream::budget::{self, Candidate, Limits, Plan};
use hangar_dream::config::Config;
use hangar_dream::memory;
use hangar_dream::project::sanitize;
use hangar_dream::render::{self, Header};
use hangar_dream::run::{self, Auth, Input};
use hangar_dream::state::{Mode, Month, ProjectState, RootEntry, State};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::inventory::{self, SessionRef};
use super::{load_config, load_state, peers, this_machine, Paths, RUNNING};
use crate::routes::AppState;

const RUNS_KEPT_DAYS: u64 = 30;
pub const USER_MEMORY_PATH: &str = "rules/memoria-do-usuario.md";
/// O arquivo global de antes e de depois da rodada: o Desfazer só volta se ninguém mexeu depois.
pub const USER_BEFORE: &str = "memoria-do-usuario.antes.md";
pub const USER_AFTER: &str = "memoria-do-usuario.depois.md";

pub(crate) struct Options {
    pub project: Option<String>,
    /// "Sonhar agora": não exige 3 sessões novas.
    pub forced: bool,
}

impl Options {
    pub fn nightly() -> Self {
        Self { project: None, forced: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaceChange {
    pub machine: String,
    pub root: String,
    /// Só nesta máquina: a pasta exata (principal ou de uma conta).
    pub dir: Option<String>,
    pub before: BTreeMap<String, String>,
    pub after: BTreeMap<String, String>,
    pub written: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub project: String,
    pub at: DateTime<Utc>,
    pub cost_usd: f64,
    pub places: Vec<PlaceChange>,
    /// Até onde cada sessão lida tinha sido lida antes (`None` = nunca): o Desfazer devolve, senão o
    /// próximo sonho não relê o que foi desfeito.
    #[serde(default)]
    pub cursors_before: BTreeMap<String, Option<usize>>,
    /// O último sonho do projeto antes desta rodada: o inventário só pega conversa mexida depois dele.
    #[serde(default)]
    pub last_dream_before: Option<DateTime<Utc>>,
}

/// Solta a marca de rodada em andamento mesmo se a tarefa cair no meio.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn begin() -> Result<(String, Running), String> {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.is_some() {
        return Err("dream_running".into());
    }
    let suffix: u16 = rand_u16();
    let run_id = format!("{}-{suffix:04x}", Utc::now().format("%Y%m%d-%H%M%S"));
    *running = Some(run_id.clone());
    Ok((run_id, Running))
}

fn rand_u16() -> u16 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    (nanos ^ std::process::id()) as u16
}

/// "Sonhar agora": marca a rodada e a roda em segundo plano.
pub(crate) fn start(st: &Arc<AppState>, opts: Options) -> Result<String, String> {
    let (run_id, guard) = begin()?;
    let st = st.clone();
    let id = run_id.clone();
    tokio::spawn(async move {
        let _guard = guard;
        if let Err(e) = execute(&st, &opts, &id).await {
            st.diag.record_line(json!({"evento": "dream.round_failed", "nivel": "aviso", "run_id": id, "codigo": super::code_of(&e)}));
        }
    });
    Ok(run_id)
}

pub(crate) async fn run(st: &Arc<AppState>, opts: Options) -> Result<String, String> {
    let (run_id, _guard) = begin()?;
    execute(st, &opts, &run_id).await?;
    Ok(run_id)
}

async fn execute(st: &Arc<AppState>, opts: &Options, run_id: &str) -> Result<(), String> {
    let me = this_machine(st);
    let config = load_config(st)?;
    if config.machine != me {
        return Err("dream_not_dreamer".into());
    }
    let paths = Paths::of(st);
    let mut state = load_state(st)?;
    let now = Utc::now();
    let pruned_failed = prune_runs(&paths.runs());
    if pruned_failed > 0 {
        st.diag.record_line(json!({"evento": "dream.prune_failed", "nivel": "aviso", "codigo": "dream_io", "pastas": pruned_failed}));
    }
    let scrub = |t: &str| hangar_dream::secrets::scrub(t).into_owned();

    // Janela fixa: um projeto que nunca sonhou não aparece no estado, e a data de outro projeto
    // esconderia as sessões dele. O que já foi lido sai por projeto, em `dream_one`.
    let since = now - Duration::days(budget::FIRST_DREAM_DAYS);
    let mut sessions = collect_sessions(st, &me, since).await?;
    resolve_roots(st, &mut state, &mut sessions);
    let groups = group(&state, sessions);
    let accounts = weekly(st).await;
    let account_dirs = claude_account_dirs(st);

    let mut dreamt = false;
    for (pid, list) in groups {
        if opts.project.as_ref().is_some_and(|p| p != &pid) {
            continue;
        }
        let ps = state.projects.entry(pid.clone()).or_default().clone();
        if !ps.enabled {
            continue;
        }
        let ctx = Ctx { st, config: &config, me: &me, run_id, now, accounts: &accounts, account_dirs: &account_dirs, scrub: &scrub, paths: &paths };
        match dream_one(&ctx, &mut state, &pid, &ps, list, opts.forced).await {
            Ok(done) => dreamt |= done,
            Err(e) => {
                st.diag.record_line(json!({"evento": "dream.project_failed", "nivel": "aviso", "projeto": pid, "codigo": super::code_of(&e)}));
                state.projects.entry(pid.clone()).or_default().last_error = Some(super::code_of(&e));
            }
        }
        state.save(&paths.state()).map_err(|e| format!("dream_state_unwritable: {e}"))?;
    }
    if config.user_memory && (dreamt || (opts.forced && opts.project.is_none())) {
        let ctx = Ctx { st, config: &config, me: &me, run_id, now, accounts: &accounts, account_dirs: &account_dirs, scrub: &scrub, paths: &paths };
        if let Err(e) = user_pass(&ctx, &mut state).await {
            st.diag.record_line(json!({"evento": "dream.user_failed", "nivel": "aviso", "codigo": super::code_of(&e)}));
        }
    }
    if !opts.forced {
        state.last_round_at = Some(now);
    }
    state.save(&paths.state()).map_err(|e| format!("dream_state_unwritable: {e}"))
}

struct Ctx<'a> {
    st: &'a Arc<AppState>,
    config: &'a Config,
    me: &'a str,
    run_id: &'a str,
    now: DateTime<Utc>,
    accounts: &'a [(PathBuf, f64)],
    account_dirs: &'a [PathBuf],
    scrub: &'a (dyn Fn(&str) -> String + Sync),
    paths: &'a Paths,
}

async fn collect_sessions(st: &AppState, me: &str, since: DateTime<Utc>) -> Result<Vec<(String, SessionRef)>, String> {
    let index = st.search.path().to_owned();
    let local = tokio::task::spawn_blocking(move || inventory::local_sessions(&index, Some(since)))
        .await
        .map_err(|e| format!("dream_index_unavailable: {e}"))??;
    let mut all: Vec<(String, SessionRef)> = local.into_iter().map(|s| (me.to_string(), s)).collect();
    for peer in st.peers.enabled_ids() {
        match peers::inventory(st, &peer, Some(since)).await {
            Ok(list) => all.extend(list.into_iter().map(|s| (peer.clone(), s))),
            Err(e) => st.diag.record_line(json!({"evento": "dream.peer_unreachable", "nivel": "aviso", "maquina": peer, "codigo": super::code_of(&e)})),
        }
    }
    Ok(all)
}

/// Raiz conhecida vai para o estado; pasta que sumiu usa o que o estado guardou.
fn resolve_roots(st: &AppState, state: &mut State, sessions: &mut Vec<(String, SessionRef)>) {
    let mut orphans = 0;
    for (machine, s) in sessions.iter_mut() {
        let key = format!("{machine}/{}", s.cwd);
        match (&s.root, &s.project) {
            (Some(root), Some(project)) => {
                state.roots.insert(key, RootEntry { root: root.clone(), project: project.clone() });
            }
            _ => match state.roots.get(&key) {
                Some(entry) => {
                    s.root = Some(entry.root.clone());
                    s.project = Some(entry.project.clone());
                }
                None => orphans += 1,
            },
        }
    }
    if orphans > 0 {
        st.diag.record_line(json!({"evento": "dream.session_orphan", "nivel": "info", "sessoes": orphans}));
    }
    sessions.retain(|(_, s)| s.root.is_some() && s.project.is_some());
}

/// Projeto sem git só junta máquinas depois de confirmado na tela (o `alias`).
pub(crate) fn project_key(state: &State, project: &str, machine: &str) -> String {
    let raw = if project.starts_with("dir:") { format!("{project}@{machine}") } else { project.to_string() };
    state.canonical(&raw).to_string()
}

fn group(state: &State, sessions: Vec<(String, SessionRef)>) -> BTreeMap<String, Vec<(String, SessionRef)>> {
    let mut out: BTreeMap<String, Vec<(String, SessionRef)>> = BTreeMap::new();
    for (machine, s) in sessions {
        let key = project_key(state, s.project.as_deref().unwrap_or_default(), &machine);
        out.entry(key).or_default().push((machine, s));
    }
    out
}

/// Conta Claude e o `pct` da semana; conta sem leitura boa da janela de 7 dias fica de fora.
pub(crate) async fn weekly(st: &AppState) -> Vec<(PathBuf, f64)> {
    let bridge = crate::accounts::http::bridge(st).ok();
    let Ok(rows) = st.accounts.quotas(bridge, false, false).await else { return Vec::new() };
    rows.as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["provedor"] == "claude" && r["estado"] == "lida")
        .filter_map(|r| {
            let dir = r["id"].as_str()?.strip_prefix("claude:")?;
            let pct = r["janelas"].as_array()?.iter().filter(|j| j["rotulo"] == "7d").filter_map(|j| j["pct"].as_f64()).reduce(f64::max)?;
            Some((PathBuf::from(dir), pct))
        })
        .collect()
}

fn claude_account_dirs(st: &AppState) -> Vec<PathBuf> {
    st.accounts
        .snapshot(crate::accounts::Provider::Claude)
        .map(|list| list.into_iter().map(|a| a.home).collect())
        .unwrap_or_default()
}

struct Piece {
    cursor_key: String,
    mtime: DateTime<Utc>,
    parts: Vec<String>,
    end: usize,
}

/// Sonha um projeto. `Ok(false)` = nada a fazer (menos de 3 sessões novas).
async fn dream_one(ctx: &Ctx<'_>, state: &mut State, pid: &str, ps: &ProjectState, list: Vec<(String, SessionRef)>, forced: bool) -> Result<bool, String> {
    let st = ctx.st;
    let mut pieces = Vec::new();
    let mut places: BTreeSet<(String, String)> = BTreeSet::new();
    for (machine, s) in &list {
        places.insert((machine.clone(), s.root.clone().unwrap_or_default()));
        let cursor_key = format!("{machine}/{}", s.path);
        // Já lida e sem mudança desde o último sonho do projeto: nem relê o arquivo.
        if ps.last_dream_at.is_some_and(|t| s.mtime <= t) && state.cursors.contains_key(&cursor_key) {
            continue;
        }
        let from = state.cursors.get(&cursor_key).copied().unwrap_or(0);
        let rendered = if machine == ctx.me {
            let Some(provider) = hangar_transcript::Provider::parse(&s.provider) else { continue };
            let session = Path::new(&s.path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // Lê o arquivo inteiro: sai do executor sem copiar o limpador, que é emprestado.
            tokio::task::block_in_place(|| render::render(provider, Path::new(&s.path), from, None, &Header { session: &session, machine: ctx.me }, ctx.scrub))
                .map(|r| (r.parts, r.end_event))
                .map_err(|e| e.to_string())
        } else {
            peers::transcript(st, machine, &s.path, from).await
        };
        match rendered {
            Ok((parts, end)) if parts.is_empty() => {
                if end > from {
                    state.cursors.insert(cursor_key, end);
                }
            }
            Ok((parts, end)) => pieces.push(Piece { cursor_key, mtime: s.mtime, parts, end }),
            Err(e) => st.diag.record_line(json!({"evento": "dream.transcript_failed", "nivel": "aviso", "maquina": machine, "codigo": super::code_of(&e)})),
        }
    }
    if pieces.is_empty() || (pieces.len() < 3 && !forced) {
        return Ok(false);
    }

    let limits = Limits { project_cap_usd: ctx.config.project_cap_usd, month_cap_usd: ctx.config.month_cap_usd };
    let month = state.month_at(ctx.now).clone();
    let (auth, cap) = match budget::decide(ps.mode, !ctx.config.api_key.is_empty(), &month, &limits, ctx.accounts) {
        Plan::Key { cap_usd } => (Auth::Key, cap_usd),
        Plan::Account { dir, cap_usd } => (Auth::Account(dir), cap_usd),
        Plan::Skip(_) => return Err("dream_no_budget".into()),
    };
    let candidates: Vec<Candidate> = pieces.iter().map(|p| Candidate { mtime: p.mtime, chars: p.parts.iter().map(String::len).sum() }).collect();
    let picked = budget::pick_sessions(&candidates, cap, ps.last_dream_at.is_none(), ctx.now);
    if picked.is_empty() {
        return Err("dream_project_cap".into());
    }

    // Memória de cada lugar onde o projeto existe.
    let mut snapshots: Vec<(PlaceChange, String)> = Vec::new();
    // Lugar sem leitura entra no registro como falha: sem isto os cursores andariam e aquela
    // máquina nunca receberia o que foi lido.
    let mut unread: Vec<PlaceChange> = Vec::new();
    for (machine, root) in &places {
        if machine == ctx.me {
            for dir in inventory::memory_dirs(&st.accounts.env.claude_base, ctx.account_dirs, Path::new(root)) {
                let snap = memory::read(&dir).map_err(|e| format!("dream_memory_unreadable: {e}"))?;
                snapshots.push((place(machine, root, Some(&dir), snap.files), snap.digest));
            }
        } else {
            match peers::memory_get(st, machine, root).await {
                Ok(snap) => snapshots.push((place(machine, root, None, snap.files), snap.digest)),
                Err(e) => {
                    st.diag.record_line(json!({"evento": "dream.peer_unreachable", "nivel": "aviso", "maquina": machine, "codigo": super::code_of(&e)}));
                    unread.push(unread_place(machine, root));
                }
            }
        }
    }
    let base_at = snapshots
        .iter()
        .position(|(p, _)| p.machine == ctx.me)
        .or_else(|| snapshots.iter().enumerate().max_by_key(|(_, (p, _))| p.before.len()).map(|(i, _)| i))
        .ok_or("dream_no_memory_place")?;
    let base = snapshots[base_at].0.before.clone();
    let base_digest = snapshots[base_at].1.clone();

    let mut inputs = Vec::new();
    for (n, i) in picked.iter().enumerate() {
        for (k, part) in pieces[*i].parts.iter().enumerate() {
            inputs.push(Input { name: format!("s{n}-p{k}.txt"), text: part.clone() });
        }
    }
    for (n, (p, digest)) in snapshots.iter().enumerate() {
        if *digest != base_digest && !p.before.is_empty() {
            let text: String = p.before.iter().map(|(name, content)| format!("=== FILE {name} ===\n{content}\n")).collect();
            let machine: String = p.machine.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            inputs.push(Input { name: format!("memory-{machine}-{n}.txt"), text });
        }
    }

    std::fs::create_dir_all(ctx.paths.work()).map_err(|e| format!("dream_io: {e}"))?;
    let workdir = tempfile::Builder::new().prefix("run-").tempdir_in(ctx.paths.work()).map_err(|e| format!("dream_io: {e}"))?;
    let model = super::cli::ClaudeModel { accounts: st.accounts.clone(), api_key: ctx.config.api_key.clone() };
    let fallback_accounts = ctx.accounts.to_vec();
    let fallback_limits = Limits { project_cap_usd: limits.project_cap_usd, month_cap_usd: None };
    let on_no_credit = move || match budget::decide(Mode::Account, false, &Month::default(), &fallback_limits, &fallback_accounts) {
        Plan::Account { dir, .. } => Some(Auth::Account(dir)),
        _ => None,
    };
    let payer = run::Payer::new(auth, &on_no_credit);
    let job = run::Job { workdir: workdir.path(), base: &base, inputs: &inputs, cap_usd: cap, scrub: ctx.scrub };
    let outcome = run::dream_project(&job, &payer, &model).await;
    let spent = payer.spent();
    {
        let month = state.month_at(ctx.now);
        month.key_usd += spent.key_usd;
        month.account_usd += spent.account_usd;
    }
    let outcome = outcome.map_err(|e| e.to_string())?;

    let mut changes = Vec::new();
    for (mut change, digest) in snapshots {
        // Só o código fica no registro: o erro do peer e o de IO podem trazer caminho ou texto de lá.
        let result: Result<(), String> = match &change.dir {
            Some(dir) => {
                let (dir, files, digest) = (PathBuf::from(dir), outcome.files.clone(), digest.clone());
                tokio::task::spawn_blocking(move || memory::write(&dir, &files, &digest))
                    .await
                    .map_err(|_| "dream_io".to_string())
                    .and_then(|r| r.map_err(|e| e.code().to_string()))
            }
            None => peers::memory_put(st, &change.machine, &change.root, &outcome.files, &digest, ctx.me).await.map_err(|e| super::code_of(&e)),
        };
        change.after = outcome.files.clone();
        change.written = result.is_ok();
        change.error = result.err();
        changes.push(change);
    }
    changes.extend(unread);
    let cost = spent.key_usd + spent.account_usd;
    let cursors_before = picked.iter().map(|i| (pieces[*i].cursor_key.clone(), state.cursors.get(&pieces[*i].cursor_key).copied())).collect();
    let record = RunRecord {
        run_id: ctx.run_id.into(),
        project: pid.into(),
        at: ctx.now,
        cost_usd: cost,
        places: changes,
        cursors_before,
        last_dream_before: ps.last_dream_at,
    };
    save_record(ctx.paths, &record)?;

    // Uma pasta sem gravar: o que foi lido volta na próxima noite, senão aquela máquina nunca o recebe.
    let failure = place_failure(&record.places);
    if failure.is_none() {
        for i in &picked {
            state.cursors.insert(pieces[*i].cursor_key.clone(), pieces[*i].end);
        }
    }
    let entry = state.projects.entry(pid.into()).or_default();
    // A data também fica: com ela a sessão de cursor antigo e mtime anterior seria pulada.
    if failure.is_none() {
        entry.last_dream_at = Some(ctx.now);
    }
    entry.last_cost_usd = cost;
    entry.last_run_id = Some(ctx.run_id.into());
    entry.last_error = failure;
    st.diag.record_line(json!({"evento": "dream.project_done", "nivel": "info", "projeto": pid, "custo_usd": cost,
        "notas": outcome.files.len(), "aposentadas": outcome.retired, "sessoes": picked.len()}));
    Ok(true)
}

/// `None` com todas as pastas gravadas; senão o primeiro código e as máquinas que falharam
/// (`dream_locked: vps`).
pub(super) fn place_failure(places: &[PlaceChange]) -> Option<String> {
    let failed: Vec<&PlaceChange> = places.iter().filter(|p| !p.written).collect();
    let code = failed.first()?.error.clone().unwrap_or_else(|| "dream_failed".into());
    let machines: BTreeSet<&str> = failed.iter().map(|p| p.machine.as_str()).collect();
    Some(format!("{code}: {}", machines.into_iter().collect::<Vec<_>>().join(", ")))
}

fn place(machine: &str, root: &str, dir: Option<&Path>, files: BTreeMap<String, String>) -> PlaceChange {
    PlaceChange {
        machine: machine.into(),
        root: root.into(),
        dir: dir.map(|d| d.to_string_lossy().into_owned()),
        before: files,
        after: BTreeMap::new(),
        written: false,
        error: None,
    }
}

pub(super) fn unread_place(machine: &str, root: &str) -> PlaceChange {
    PlaceChange { error: Some("dream_peer_failed".into()), ..place(machine, root, None, BTreeMap::new()) }
}

pub(crate) fn record_dir(paths: &Paths, run_id: &str) -> PathBuf {
    paths.runs().join(run_id)
}

pub(super) fn save_record(paths: &Paths, record: &RunRecord) -> Result<(), String> {
    let dir = record_dir(paths, &record.run_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("dream_io: {e}"))?;
    let bytes = serde_json::to_vec(record).map_err(|e| format!("dream_io: {e}"))?;
    hangar_core::fs::atomic_write(&dir.join(record_file(&record.project)), &bytes).map_err(|e| format!("dream_io: {e}"))
}

/// `git:a/b-c` e `git:a/b/c` dão o mesmo nome limpo: o hash do id separa os dois registros.
fn record_file(project: &str) -> String {
    let hash = sha1_smol::Sha1::from(project).digest().to_string();
    format!("{}-{}.json", sanitize(project), &hash[..12])
}

pub(crate) fn load_records(paths: &Paths, run_id: &str) -> Result<Vec<RunRecord>, String> {
    if !run::valid_input_name(run_id) {
        return Err("dream_run_not_found".into());
    }
    let entries = std::fs::read_dir(record_dir(paths, run_id)).map_err(|_| "dream_run_not_found".to_string())?;
    let mut out = Vec::new();
    for entry in entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")) {
        let bytes = std::fs::read(entry.path()).map_err(|e| format!("dream_io: {e}"))?;
        out.push(serde_json::from_slice(&bytes).map_err(|e| format!("dream_io: {e}"))?);
    }
    Ok(out)
}

/// A memória de antes fica 30 dias, para o Desfazer. Devolve quantas pastas velhas não saíram.
fn prune_runs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let limit = std::time::Duration::from_secs(RUNS_KEPT_DAYS * 86_400);
    let mut failed = 0;
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > limit);
        if old && std::fs::remove_dir_all(entry.path()).is_err() {
            failed += 1;
        }
    }
    failed
}

/// O texto do arquivo; só o que não existe vira vazio.
pub(crate) fn read_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) => Err("dream_io".into()),
    }
}

/// Grava o arquivo global; texto vazio apaga. Com `expect`, só grava se o atual ainda for ele.
pub(crate) fn write_user_memory(target: &Path, content: &str, expect: Option<&str>) -> Result<(), String> {
    if let Some(expect) = expect {
        if read_or_empty(target)? != expect {
            return Err("dream_memory_changed".into());
        }
    }
    let written = if content.is_empty() {
        std::fs::remove_file(target).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
    } else {
        target.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| hangar_core::fs::atomic_write(target, content.as_bytes()))
    };
    written.map_err(|_| "dream_io".into())
}

/// Notas `user`/`feedback` de todos os projetos desta máquina viram o arquivo global.
async fn user_pass(ctx: &Ctx<'_>, state: &mut State) -> Result<(), String> {
    let st = ctx.st;
    let base = st.accounts.env.claude_base.clone();
    let scan = base.clone();
    // Projeto desligado na tela não manda nota para o arquivo global.
    let read: &State = state;
    let disabled: HashSet<String> = read
        .roots
        .iter()
        .filter_map(|(key, entry)| {
            let machine = key.split_once('/')?.0;
            let off = machine == ctx.me && read.projects.get(&project_key(read, &entry.project, machine)).is_some_and(|p| !p.enabled);
            off.then(|| sanitize(&entry.root))
        })
        .collect();
    let notes = tokio::task::spawn_blocking(move || user_notes(&scan.join("projects"), &disabled)).await.map_err(|e| e.to_string())?;
    if notes.is_empty() {
        return Ok(());
    }
    let claude_md = read_or_empty(&base.join("CLAUDE.md"))?;
    let target = base.join(USER_MEMORY_PATH);
    let current = read_or_empty(&target)?;
    let limits = Limits { project_cap_usd: ctx.config.project_cap_usd, month_cap_usd: ctx.config.month_cap_usd };
    let month = state.month_at(ctx.now).clone();
    // Quem pôs todos os projetos na conta não quer a chave gasta na passada do usuário.
    let enabled: Vec<&ProjectState> = state.projects.values().filter(|p| p.enabled).collect();
    let mode = if !enabled.is_empty() && enabled.iter().all(|p| p.mode == Mode::Account) { Mode::Account } else { Mode::Key };
    let (auth, cap) = match budget::decide(mode, !ctx.config.api_key.is_empty(), &month, &limits, ctx.accounts) {
        Plan::Key { cap_usd } => (Auth::Key, cap_usd),
        Plan::Account { dir, cap_usd } => (Auth::Account(dir), cap_usd),
        Plan::Skip(_) => return Err("dream_no_budget".into()),
    };
    std::fs::create_dir_all(ctx.paths.work()).map_err(|e| format!("dream_io: {e}"))?;
    let workdir = tempfile::Builder::new().prefix("user-").tempdir_in(ctx.paths.work()).map_err(|e| format!("dream_io: {e}"))?;
    let model = super::cli::ClaudeModel { accounts: st.accounts.clone(), api_key: ctx.config.api_key.clone() };
    let none = || None;
    let payer = run::Payer::new(auth, &none);
    let job = run::UserJob { workdir: workdir.path(), notes: &notes, claude_md: &claude_md, current: &current, cap_usd: cap, scrub: ctx.scrub };
    let outcome = run::dream_user(&job, &payer, &model).await;
    let spent = payer.spent();
    {
        let month = state.month_at(ctx.now);
        month.key_usd += spent.key_usd;
        month.account_usd += spent.account_usd;
    }
    let outcome = outcome.map_err(|e| e.to_string())?;
    // A versão de antes fica no registro da rodada, como a memória dos projetos.
    let record = record_dir(ctx.paths, ctx.run_id);
    std::fs::create_dir_all(&record).map_err(|e| format!("dream_io: {e}"))?;
    hangar_core::fs::atomic_write(&record.join(USER_BEFORE), current.as_bytes()).map_err(|e| format!("dream_io: {e}"))?;
    hangar_core::fs::atomic_write(&record.join(USER_AFTER), outcome.content.as_bytes()).map_err(|e| format!("dream_io: {e}"))?;
    write_user_memory(&target, &outcome.content, None)?;
    // Sem conferência: esta máquina é a única que escreve o arquivo global.
    for peer in st.peers.enabled_ids() {
        if let Err(e) = peers::user_put(st, &peer, &outcome.content, None, ctx.me).await {
            st.diag.record_line(json!({"evento": "dream.peer_unreachable", "nivel": "aviso", "maquina": peer, "codigo": super::code_of(&e)}));
        }
    }
    state.suggestions = outcome.suggestions;
    Ok(())
}

/// `(pasta do projeto, conteúdo)` das notas cujo cabeçalho diz `type: user` ou `type: feedback`.
fn user_notes(projects: &Path, skip: &HashSet<String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(projects) else { return out };
    for project in entries.flatten() {
        if skip.contains(project.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        let Ok(notes) = std::fs::read_dir(project.path().join("memory")) else { continue };
        for note in notes.flatten() {
            let name = note.file_name().to_string_lossy().into_owned();
            if name == memory::INDEX || !memory::valid_name(&name) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(note.path()) else { continue };
            let head = content.split("---").nth(1).unwrap_or("");
            if head.contains("type: user") || head.contains("type: feedback") {
                out.push((project.file_name().to_string_lossy().into_owned(), content));
            }
        }
    }
    out
}
