//! Ponte privada `list.*` (Python → Rust): com o Rust de pé, a descoberta, o cache de resolução do
//! transcript e o retrato da lista são dele; o Python pergunta por aqui (`list_bridge.py`), na porta
//! privada, com o mesmo segredo da ponte de Git/arquivos. Falha volta com código, nunca lista vazia.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use hangar_api::session::SessionRow;
use serde::Deserialize;
use serde_json::{Value, json};

use super::classify::{CaptureSource, Classifier, Effect, Facts, MuxCapture};
use super::context::{self, ContextCache, ReadingInputs};
use super::discover::{self, Resolver};
use super::discover_other::{self, Dirs};
use super::capped;
use super::facts::{self as list_facts, FactsClient, ListFacts};
use super::facts_files::{self, HookStates};
use super::hub::HeadlessSource;
use super::mux::{Mux, Pane};
use super::plan::PlanTracker;
use super::procs::{self, ChildrenMap, ProcessView};
use super::reply::ReplyCache;
use crate::routes::AppState;

/// Descoberta reaproveitada por quem pede dentro deste prazo (`_LIST_TTL` do `api.py`).
pub const DISCOVER_TTL: Duration = Duration::from_secs(1);
/// Retrato decorado servido sem produzir de novo (`list_sessions` do `api.py`).
pub const SNAPSHOT_TTL: Duration = Duration::from_secs(2);
/// Sessão fora da lista por este tempo perde os caches por nome.
const FORGET_AFTER: Duration = Duration::from_secs(10);
const PYTHON_APPS_TTL: Duration = Duration::from_secs(10);
/// Rodada da lista mais velha que isto não serve à limpeza das páginas: são vários tiques sem rodada.
const LIVE_FRESH: Duration = Duration::from_secs(15);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIG_DIRS_TTL: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 64 * 1024;
/// `git status` simultâneos da lista (`_git_pool`).
const GIT_SLOTS: usize = 4;

/// Falha da ponte: o código vai ao Python, que levanta (`mux_unavailable` vira `MuxIndisponivel`).
/// `detail` é código ou frase fixa, nunca saída de processo nem texto de conversa.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListError { pub code: &'static str, pub detail: &'static str }

fn fail(code: &'static str, detail: &'static str) -> ListError { ListError { code, detail } }

/// Entradas do sistema: multiplexador, processos e as pastas que o Python resolveu
/// (`HANGAR_LIST_DIRS`). Sem pastas, a ponte recusa em vez de adivinhar onde cada provedor grava.
pub struct ListEnv {
    pub mux: Mux,
    pub capture_program: OsString,
    pub procs: Arc<dyn ProcessView>,
    pub dirs: Option<Dirs>,
}

impl ListEnv {
    pub fn from_env() -> Self {
        let raw = std::env::var("HANGAR_LIST_DIRS").unwrap_or_default();
        let dirs = parse_dirs(&raw);
        if dirs.is_none() {
            // Cada pergunta da ponte vai recusar com `list_dirs_missing`; a causa fica dita uma vez.
            tracing::error!(code = "list_dirs_missing", present = !raw.is_empty(), "pastas da lista ausentes ou inválidas");
        }
        Self { mux: Mux::default(), capture_program: "tmux".into(), procs: Arc::new(procs::SystemProcs::default()), dirs }
    }
}

/// `{"home", "claude", "codex_home", "pi_sessions", "omp_config", "omp_agent", "kimi_home"}`;
/// faltou um campo, nenhum vale.
pub fn parse_dirs(raw: &str) -> Option<Dirs> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let p = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(PathBuf::from);
    Some(Dirs { home: p("home")?, claude: p("claude")?, codex_home: p("codex_home")?, pi_sessions: p("pi_sessions")?,
        omp_config: p("omp_config")?, omp_agent: p("omp_agent")?, kimi_home: p("kimi_home")? })
}

/// O que quem produz sabe e o Python não: quantas listas do dono estão abertas no Rust (o hub).
#[derive(Clone, Default)]
pub struct ProduceFacts {
    pub owner_clients: u32,
    /// Rodada em sombra: nada do que ela produz sai daqui, nem o rebaixamento de `awaiting`.
    pub shadow: bool,
}

/// Lista decorada e os fatos do Python da mesma rodada (navegador, terminais de atalho, escondidas
/// do dono), que o hub entrega junto.
#[derive(Clone)]
pub struct Produced { pub rows: Arc<Vec<SessionRow>>, pub facts: Arc<ListFacts>,
    /// `false`: o Python não respondeu nesta rodada e `facts` é o último bom.
    pub facts_ok: bool }

struct Discovery { at: Instant, wall: f64, epoch: u64,
    /// Mapa de processos relido nesta descoberta, não o do cache de 3 s.
    fresh: bool, rows: Arc<Vec<SessionRow>>, agent_pids: Arc<HashMap<String, u32>>,
    panes: Arc<Vec<Pane>>, children: Arc<ChildrenMap> }

struct Snapshot { at: Instant, epoch: u64, produced: Produced,
    /// Entradas da produção, para a rodada parcial; `None` quando nenhuma lista do dono está aberta.
    round: Option<Arc<Round>> }

/// O que a classificação e a decoração de uma rodada usaram além das linhas.
struct RoundInputs { agent_pids: Arc<HashMap<String, u32>>, targets: Arc<BTreeMap<String, String>>,
    headless: Option<BTreeMap<String, Value>>, facts: Arc<ListFacts>,
    /// Problema do runtime por sessão: o do lançador (Python) sem terminal, o do ator do Rust com terminal.
    problems: BTreeMap<String, String> }

/// Uma produção inteira guardada: as linhas Claude como a descoberta e os fatos as deram, antes de
/// classificar, e as entradas. A rodada parcial reclassifica uma delas sem descobrir de novo.
struct Round { inputs: Arc<RoundInputs>, pre: Vec<SessionRow> }

/// Resultado da rodada acordada por arquivo.
pub enum Partial {
    /// Linhas reclassificadas e a lista inteira com elas.
    Done(Produced),
    /// Nenhuma linha conhecida mudou.
    Unchanged,
    /// Sem produção inteira guardada (ou ela venceu): só o tique serve.
    NeedsFull,
}

type Op<T> = Box<dyn FnOnce(&mut T) + Send>;

/// Valor que a produção segura durante I/O longa (captura de até 5 s, rabo de transcript) e que
/// criar, fechar e renomear sessão mudam sem esperar: a mudança entra numa fila curta e quem segura
/// o valor a aplica antes de soltar; com o valor livre, quem pediu aplica na hora.
struct Guarded<T> { value: Mutex<T>, ops: Mutex<Vec<Op<T>>> }

impl<T: Default> Default for Guarded<T> {
    fn default() -> Self { Self { value: Mutex::default(), ops: Mutex::default() } }
}

impl<T> Guarded<T> {
    /// Para quem lê e grava o valor; produções juntas esperam uma à outra aqui.
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut v = lock(&self.value);
        self.drain(&mut v);
        let out = f(&mut v);
        // O que chegou durante a I/O não espera a próxima rodada.
        self.drain(&mut v);
        out
    }

    fn drain(&self, v: &mut T) {
        let ops = std::mem::take(&mut *lock(&self.ops));
        for op in ops { op(v) }
    }

    /// Nunca espera a produção. A ordem dos pedidos é mantida.
    fn apply(&self, op: impl FnOnce(&mut T) + Send + 'static) {
        lock(&self.ops).push(Box::new(op));
        let held = match self.value.try_lock() {
            Ok(v) => Some(v),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        if let Some(mut v) = held { self.drain(&mut v); }
    }
}

/// O que a decoração das linhas Claude e de todas guarda entre rodadas.
#[derive(Default)]
struct Decor { context: ContextCache, replies: ReplyCache, limits: super::limit::LimitWatch }

/// Marcadores junto da classificação: duas produções seguidas não releem tudo do zero.
#[derive(Default)]
struct Classify { classifier: Classifier, hooks: HookStates }

/// Caches que atravessam rodadas, por nome de sessão, cada um com a própria trava.
#[derive(Default)]
struct Caches {
    resolver: Guarded<Resolver>,
    decor: Guarded<Decor>,
    /// Trava própria: a rota de plano não espera a decoração inteira da rodada.
    plans: Guarded<PlanTracker>,
    classify: Guarded<Classify>,
    /// Último resumo de Git por pasta: a lista não espera o `git status` (`_git_ultimo`).
    git: Mutex<HashMap<String, (Value, Value)>>,
    /// Quando cada pasta teve o Git relido e com que atividade e versão de refs.
    git_gate: Mutex<HashMap<String, GitGate>>,
    config_dirs: Mutex<Option<(Instant, Arc<Vec<PathBuf>>)>>,
    /// Avisos de limite que a rodada viu, à espera do vigia da troca de conta.
    limits: Mutex<Vec<crate::runtime::gateway::LimitHit>>,
}

struct GitGate { at: Instant, activity: u64, fingerprint: Option<Vec<u128>> }

/// Pasta cujo Git a rodada pede, com a atividade que decidiu.
struct GitDue { dir: String, activity: u64 }

/// Edição de arquivo por fora de uma sessão não toca no índice nem nas refs: só este teto a vê.
const GIT_IDLE_REFRESH: Duration = Duration::from_secs(30);

/// Relê o Git quando refs/índice mudaram, quando uma sessão da pasta teve atividade (o agente
/// editando) ou pelo teto. Sem isso cada repositório parado rodava dois `git` a cada 10 s.
fn git_due(prev: Option<&GitGate>, activity: u64, fingerprint: &Option<Vec<u128>>, now: Instant) -> bool {
    prev.is_none_or(|g| g.activity != activity || &g.fingerprint != fingerprint
        || now.saturating_duration_since(g.at) >= GIT_IDLE_REFRESH)
}

fn git_fingerprint(dir: &str) -> Option<Vec<u128>> {
    hangar_workspace::git::fingerprint(&hangar_workspace::real(Path::new(dir)))
}

/// Pastas com Git a reler nesta rodada; a que saiu da lista sai do portão.
fn git_due_dirs(caches: &Caches, rows: &[SessionRow], now: Instant) -> Vec<GitDue> {
    let mut activity: HashMap<&str, f64> = HashMap::new();
    for row in rows {
        let dir = git_dir(row);
        if dir.is_empty() { continue; }
        let a = activity.entry(dir).or_insert(0.0);
        *a = a.max(row.last_activity.unwrap_or(0.0));
    }
    let mut gate = lock(&caches.git_gate);
    gate.retain(|d, _| activity.contains_key(d.as_str()));
    activity.into_iter().filter_map(|(dir, a)| {
        let activity = a.to_bits();
        git_due(gate.get(dir), activity, &git_fingerprint(dir), now).then(|| GitDue { dir: dir.to_owned(), activity })
    }).collect()
}

pub struct ListBridge {
    env: Arc<ListEnv>,
    facts: FactsClient,
    shadow_facts: FactsClient,
    caches: Arc<Caches>,
    /// Retrato do runtime das sessões sem terminal; sem ele, a linha diz `list_runtime_absent`.
    runtime: std::sync::OnceLock<Arc<dyn HeadlessSource>>,
    /// Convites: a marca "compartilhada" sai da loja do Rust; sem ela, nenhuma linha é marcada.
    shares: std::sync::OnceLock<Arc<crate::share::ShareState>>,
    /// Listas do dono abertas no hub: o retrato pedido de fora manda a mesma contagem, senão a
    /// pergunta ao Python alternaria de chave e a presença do app mudaria a cada pedido.
    owner_clients: AtomicU32,
    /// Última rodada em que cada nome estava na lista: sumido há `FORGET_AFTER`, os caches dele saem.
    seen: Mutex<HashMap<String, Instant>>,
    /// Aparelhos do dono atendidos pelo Python (Connect), da última resposta boa dos fatos, e quando chegou.
    python_apps: Mutex<Option<(u32, Instant)>>,
    /// Trava assíncrona = um por vez: quem chega durante a varredura espera e reaproveita o resultado.
    discovery: tokio::sync::Mutex<Option<Discovery>>,
    snapshot: tokio::sync::Mutex<Option<Snapshot>>,
    git_running: Arc<Mutex<std::collections::HashSet<String>>>,
    git_slots: Arc<tokio::sync::Semaphore>,
    epoch: AtomicU64,
    /// Fatos do estado empurrados pelo Python (`state.facts`), lidos pelo `Monitor`.
    pub state_facts: Arc<crate::state::facts::FactsStore>,
    /// Registros nativos rebaixados pela lista, lidos também pelo `Monitor`.
    pub demoted: Arc<crate::state::demote::Demoted>,
    /// Último estado de cada `Monitor` vivo: a lista o lê em vez de capturar o pane.
    pub published: Arc<crate::state::published::Published>,
    plugin_runtime: std::sync::OnceLock<Arc<crate::runtime::gateway::RuntimeRegistry>>,
    plugin_projection: Mutex<HashMap<String, (crate::interactions::InteractionBinding, hangar_api::state::StateEvent)>>,
    /// O `problema` de cada linha só sai depois de valer pelo prazo (`problem_gate`); o tique da lista reconfere.
    problems: Mutex<HashMap<String, crate::problem_gate::ProblemGate>>,
    shown: Arc<crate::problem_gate::Shown>,
    problem_hold_ms: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    test_plugins: Mutex<Option<BTreeMap<String, Result<crate::state::facts::PluginReading, String>>>>,
    /// Transcripts da última rodada do dono, para a limpeza das páginas; `None` = rodada incerta.
    live_jsonl: Mutex<Option<(Instant, HashSet<String>)>>,
}

/// Tarefa bloqueante que entrou em pânico: o hook já registrou onde; aqui fica qual operação.
fn joined(e: tokio::task::JoinError, detail: &'static str) -> ListError {
    tracing::error!(code = "list_task_failed", detail, panic = e.is_panic(), "ponte da lista: tarefa interrompida");
    fail("list_task_failed", detail)
}

fn wall_now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

impl ListBridge {
    pub fn new(env: ListEnv, facts: FactsClient) -> Self {
        let (caches, demoted) = (Arc::<Caches>::default(), Arc::<crate::state::demote::Demoted>::default());
        caches.classify.with(|c| c.hooks.set_demoted(demoted.clone()));
        Self { env: Arc::new(env), shadow_facts: facts.sibling(), facts, caches, runtime: std::sync::OnceLock::new(),
            shares: std::sync::OnceLock::new(), owner_clients: AtomicU32::new(0), seen: Mutex::default(), python_apps: Mutex::default(),
            discovery: tokio::sync::Mutex::new(None),
            snapshot: tokio::sync::Mutex::new(None), git_running: Arc::default(),
            git_slots: Arc::new(tokio::sync::Semaphore::new(GIT_SLOTS)), epoch: AtomicU64::new(0),
            state_facts: Arc::default(), demoted, published: Arc::default(), live_jsonl: Mutex::default(),
            plugin_runtime: std::sync::OnceLock::new(), plugin_projection: Mutex::default(), problems: Mutex::default(), shown: Arc::default(),
            problem_hold_ms: std::sync::atomic::AtomicU64::new(if cfg!(test) { 0 } else { crate::problem_gate::HOLD.as_millis() as u64 }),
            #[cfg(test)] test_plugins: Mutex::default(),
        }
    }

    /// Rebaixamentos da rodada valem no cache Rust da lista e do Monitor, e vão ao mapa de marcadores do
    /// Python, que o push e as rotas dele ainda leem e que ele grava no arquivo de estado.
    fn demote(&self, effects: Vec<Effect>) {
        let sids: Vec<String> = effects.into_iter().map(|Effect::DemoteAwaiting { sid, ts }| {
            self.demoted.demote(&sid, ts);
            sid
        }).collect();
        if !sids.is_empty() { self.facts.demote(sids); }
    }

    /// Uma vez, na montagem do estado com `HANGAR_SHARES_FILE`.
    pub fn set_shares(&self, share: Arc<crate::share::ShareState>) {
        let _ = self.shares.set(share);
    }

    /// Uma vez, na subida do servidor com o runtime de pé.
    pub fn set_runtime(&self, runtime: Arc<dyn HeadlessSource>) {
        if self.runtime.set(runtime).is_err() {
            tracing::warn!(code = "list_runtime_twice", "lista: retrato do runtime já ligado; o segundo fica de fora");
            self.facts.diag.report("rust.list_failed", "", "list_runtime_twice", "retrato do runtime ligado duas vezes; vale o primeiro");
        }
    }

    pub fn set_plugin_runtime(&self, runtime: Arc<crate::runtime::gateway::RuntimeRegistry>) {
        self.caches.classify.apply({ let runtime = runtime.clone(); move |c| c.classifier.set_plugin_runtime(runtime) });
        if self.plugin_runtime.set(runtime).is_err() {
            self.facts.diag.report("rust.list_failed", "", "list_plugin_runtime_twice", "fonte do plugin já ligada");
        }
    }

    pub fn set_permissions(&self, permissions: crate::state::permission::PermissionStore) {
        self.caches.classify.apply(move |c| c.classifier.set_permissions(permissions));
    }

    pub fn owner_clients(&self) -> u32 { self.owner_clients.load(Ordering::Relaxed) }
    /// Aparelhos do dono que entraram pelo Python (Connect). Vence em 10 s, como o `_apps_remotos` do Python:
    /// fatos que pararam de chegar não seguram permissão num app que ninguém vê.
    pub fn python_apps(&self) -> u32 {
        lock(&self.python_apps).filter(|(_, at)| at.elapsed() <= PYTHON_APPS_TTL).map_or(0, |(count, _)| count)
    }

    async fn plugin_facts(&self, rows: &[SessionRow]) -> BTreeMap<String, Result<crate::state::facts::PluginReading, String>> {
        #[cfg(test)]
        if let Some(readings) = lock(&self.test_plugins).clone() { return readings; }
        // Sem terminal o cartão é o do canal nativo (retrato do runtime); o do plugin o motor recusaria.
        let jobs = rows.iter().filter(|row| row.provider == "claude" && !row.headless).map(|row| async {
            let read = match self.plugin_runtime.get() {
                // Sessão que ainda não foi aberta no Rust não tem pergunta segurada a projetar.
                Some(runtime) if !runtime.has_entry(None, &row.name).await => None,
                Some(runtime) => Some(crate::state::facts::plugin_reading(runtime, &row.name).await),
                None => Some(Err("list_plugin_source_absent".into())),
            };
            read.map(|read| (row.name.clone(), read))
        });
        futures_util::future::join_all(jobs).await.into_iter().flatten().collect()
    }

    fn project_plugins(&self, rows: &mut [SessionRow], plugins: &BTreeMap<String, Result<crate::state::facts::PluginReading, String>>) {
        let mut previous = lock(&self.plugin_projection);
        for row in rows.iter_mut().filter(|row| row.provider == "claude") {
            let Some(reading) = plugins.get(&row.name) else { previous.remove(&row.name); continue };
            let reading = match reading {
                Ok(reading) => reading,
                Err(code) => { previous.remove(&row.name); set_unless_screen(row, code.clone()); continue },
            };
            if previous.get(&row.name).is_some_and(|(binding, _)| binding != &reading.binding) { previous.remove(&row.name); }
            let mut state = hangar_api::state::StateEvent { state: row.state.clone(), label: row.label.clone(), question: row.question.clone(),
                options: row.options.clone(), interaction_id: row.interaction_id.clone(), ..Default::default() };
            let projection = reading.facts.as_ref().map_err(Clone::clone).and_then(|local| {
                crate::state::facts::project_plugin_state(&mut state, &local.questions)?;
                if let Some(code) = row.problema.as_deref().filter(|code| *code == "list_capture_failed" || code.starts_with("askq_")) {
                    return Err(code.to_owned());
                }
                Ok(())
            });
            if let Err(code) = projection {
                if let Some((binding, trusted)) = previous.get(&row.name).filter(|(binding, _)| binding == &reading.binding) {
                    let _ = binding;
                    crate::state::facts::preserve_cards(&mut state, trusted);
                }
                set_unless_screen(row, code);
            }
            if reading.facts.is_ok() { previous.insert(row.name.clone(), (reading.binding.clone(), state.clone())); }
            row.state = state.state;
            // A travada é medida sobre "trabalhando"; a pergunta segurada pelo plugin a desfaz.
            if row.state != "working" { row.stalled = false; }
            row.label = state.label;
            row.question = state.question.or_else(|| state.codex_question.as_ref()
                .and_then(|question| question["questions"][0]["question"].as_str().map(str::to_owned)));
            row.options = state.options;
            row.interaction_id = state.interaction_id;
            // A pergunta que o plugin segura vai inteira, com o ID do formulário: o card a responde pelo `/answer`.
            if let Some(form) = state.codex_question { row.ask_form = Some(Value::Object(form)); }
            if let Ok(local) = &reading.facts { row.pending_questions = row.pending_questions.max(local.questions.len().min(u32::MAX as usize) as u32); }
        }
    }

    /// Contagem do hub; o retrato pedido de fora vai com ela.
    pub fn set_owner_clients(&self, n: u32) { self.owner_clients.store(n, Ordering::Relaxed); }

    fn dirs(&self) -> Result<Dirs, ListError> {
        self.env.dirs.clone().ok_or(fail("list_dirs_missing", "pastas da lista ausentes"))
    }

    /// Pastas que acordam a lista do dono (`facts_files::state_dirs` de cada conta). Lê disco.
    pub fn watch_dirs(&self) -> Result<Vec<PathBuf>, ListError> {
        let dirs = self.dirs()?;
        Ok(facts_files::state_dirs(&self.caches.config_dirs(&dirs)))
    }

    pub fn env(&self) -> &Arc<ListEnv> { &self.env }

    /// Pastas das contas e as da lista, para o `Monitor` de estado. Lê disco (as pastas das contas
    /// ficam guardadas por 30 s): chamar fora do runtime.
    pub fn state_dirs_blocking(&self) -> Result<(Arc<Vec<PathBuf>>, Dirs), ListError> {
        let dirs = self.dirs()?;
        Ok((self.caches.config_dirs(&dirs), dirs))
    }

    pub fn invalidate_context(&self, name: &str) {
        let name = name.to_owned();
        self.caches.decor.apply(move |decor| decor.context.forget(&name));
        self.invalidate();
    }

    /// Pane do agente da sessão (`agent_pane::resolve`) pela descoberta compartilhada da lista.
    pub async fn agent_target(&self, name: &str) -> Result<Option<String>, ListError> {
        let (_, _, panes, children) = self.discovery(None).await?;
        let mine: Vec<Pane> = panes.iter().filter(|p| p.session == name).cloned().collect();
        let env = self.env.clone();
        tokio::task::spawn_blocking(move || crate::state::agent_pane::resolve(&mine, &children, &|pid| env.procs.argv(pid).join(" ")))
            .await.map_err(|e| joined(e, "pane do agente interrompido"))
    }

    /// Mudança de membro ou de modo: a próxima pergunta não serve a lista de antes.
    pub fn invalidate(&self) { self.epoch.fetch_add(1, Ordering::SeqCst); }

    /// O rastreador de planos da lista, para as rotas de plano: o eleito é o mesmo da linha.
    /// Lê disco: chamar fora do executor assíncrono.
    pub fn with_plans<R>(&self, f: impl FnOnce(&mut PlanTracker) -> R) -> R { self.caches.plans.with(f) }

    /// Depois de gravar pin ou mover plano: a pasta é redescoberta, sem esperar a rodada em curso.
    pub fn forget_plans(&self, root: PathBuf) { self.caches.plans.apply(move |t| t.forget_root(&root)) }

    /// A linha da sessão pelo retrato e, faltando o nome (sessão criada agora, o retrato tem até 2 s),
    /// por uma rodada nova. Sem os fatos do Python as linhas de transferência e `orq` faltam: aí a
    /// ausência não prova nada e volta `list_facts_unknown`.
    pub async fn find_row(&self, name: &str) -> Result<Option<SessionRow>, ListError> {
        let found = |rows: &[SessionRow]| rows.iter().find(|r| r.name == name).cloned();
        if let Some(row) = found(&self.snapshot().await?.rows) {
            return Ok(Some(row));
        }
        let produced = self.refresh().await?;
        match found(&produced.rows) {
            Some(row) => Ok(Some(row)),
            None if produced.facts.unknown => Err(fail("list_facts_unknown", "fatos da lista indisponíveis")),
            None => Ok(None),
        }
    }

    /// `registry.list()`. `newer_than` (época em s) é a sessão criada há menos de 1 s: só vale uma
    /// descoberta que começou depois disso, com o mapa de processos relido.
    pub async fn discover(&self, newer_than: Option<f64>) -> Result<Arc<Vec<SessionRow>>, ListError> {
        Ok(self.discovery(newer_than).await?.0)
    }

    /// Contexto local da sessão, sem pedir os fatos de acesso/estado ao Python.
    pub async fn discovered_session(&self, name: &str) -> Result<Option<(SessionRow, Option<u32>)>, ListError> {
        let (rows, pids, _, _) = self.discovery(None).await?;
        Ok(rows.iter().find(|row| row.name == name).cloned().map(|row| (row, pids.get(name).copied())))
    }

    async fn discovery(&self, newer_than: Option<f64>)
        -> Result<(Arc<Vec<SessionRow>>, Arc<HashMap<String, u32>>, Arc<Vec<Pane>>, Arc<ChildrenMap>), ListError> {
        let mut slot = self.discovery.lock().await;
        let epoch = self.epoch.load(Ordering::SeqCst);
        if let Some(d) = slot.as_ref().filter(|d| d.epoch == epoch && match newer_than {
            Some(t) => d.fresh && d.wall > t,
            None => d.at.elapsed() < DISCOVER_TTL,
        }) {
            return Ok((d.rows.clone(), d.agent_pids.clone(), d.panes.clone(), d.children.clone()));
        }
        let dirs = self.dirs()?;
        let (at, wall) = (Instant::now(), wall_now());
        let (panes, dropped) = self.env.mux.list_panes_checked().await.map_err(|e| fail("mux_unavailable", e.code))?;
        if let Some(p) = dropped {
            self.facts.diag.report("rust.list_discovery", &p.key, p.code, p.reason);
        }
        let panes = Arc::new(panes);
        let (env, caches, p) = (self.env.clone(), self.caches.clone(), panes.clone());
        let max_age = if newer_than.is_some() { Duration::ZERO } else { procs::CHILDREN_TTL };
        let (rows, agent_pids, children, problems) = tokio::task::spawn_blocking(move || {
            let children = env.procs.children(max_age).map_err(|_| fail("list_procs_unreadable", "mapa de processos ilegível"))?;
            let (rows, pids, problems) = caches.resolver.with(|r| run_discovery(&p, &*env.procs, &children, r, &dirs));
            Ok::<_, ListError>((Arc::new(rows), Arc::new(pids), children, problems))
        }).await.map_err(|e| joined(e, "descoberta interrompida"))??;
        for p in problems {
            self.facts.diag.report("rust.list_discovery", &p.key, p.code, p.reason);
        }
        self.flush_notes();
        *slot = Some(Discovery { at, wall, epoch, fresh: newer_than.is_some(), rows: rows.clone(), agent_pids: agent_pids.clone(),
            panes: panes.clone(), children: children.clone() });
        Ok((rows, agent_pids, panes, children))
    }

    /// Lista decorada para quem pergunta fora do tique do hub (`GET` do dono, vigia de travada,
    /// lista do convidado): o retrato de até 2 s (o do tique, com o hub de pé), senão produz na hora.
    pub async fn snapshot(&self) -> Result<Produced, ListError> {
        self.produce_kept(false).await
    }

    /// O tique do hub: produz sempre e deixa o retrato para quem pedir depois.
    pub async fn refresh(&self) -> Result<Produced, ListError> {
        self.produce_kept(true).await
    }

    async fn produce_kept(&self, fresh: bool) -> Result<Produced, ListError> {
        let mut slot = self.snapshot.lock().await;
        let epoch = self.epoch.load(Ordering::SeqCst);
        if let Some(s) = slot.as_ref().filter(|s| !fresh && s.epoch == epoch && s.at.elapsed() < SNAPSHOT_TTL) {
            return Ok(s.produced.clone());
        }
        let at = Instant::now();
        let input = ProduceFacts { owner_clients: self.owner_clients.load(Ordering::Relaxed), shadow: false };
        let done = self.produce_round(&input).await;
        // A rodada parcial só parte da última produção que deu certo: sobre uma anterior, publicaria
        // lista velha por cima do erro.
        let failed = done.as_ref().map_or(true, |(p, _)| p.facts.unknown);
        if let (true, Some(s)) = (failed, slot.as_mut()) { s.round = None; }
        let (produced, round) = done?;
        // Sem fatos ainda não é retrato: guardado, o Python que acabou de responder esperaria 2 s.
        if !produced.facts.unknown {
            *slot = Some(Snapshot { at, epoch, produced: produced.clone(), round });
        }
        Ok(produced)
    }

    /// Rodada acordada por arquivo: relê os marcadores e o registro nativo e reclassifica só as
    /// sessões cujo arquivo mudou (e as de `asked`, com a pergunta aberta escrita), sobre a última
    /// produção inteira, sem descoberta nem pergunta ao Python. O retrato passa a levar o resultado.
    pub async fn reclassify(&self, asked: Vec<String>) -> Result<Partial, ListError> {
        let mut slot = self.snapshot.lock().await;
        let epoch = self.epoch.load(Ordering::SeqCst);
        let Some((snap, round)) = slot.as_mut().filter(|s| s.epoch == epoch).and_then(|s| Some((s.produced.clone(), s.round.clone()?))) else {
            return Ok(Partial::NeedsFull);
        };
        let dirs = self.dirs()?;
        let (env, caches, handle) = (self.env.clone(), self.caches.clone(), tokio::runtime::Handle::current());
        let published = self.published.clone();
        let done = tokio::task::spawn_blocking(move || {
            let config_dirs = caches.config_dirs(&dirs);
            let alive = |pid: i64| pid_alive(&*env.procs, pid);
            let io = MuxCapture::new(env.capture_program.clone(), CAPTURE_TIMEOUT, round.inputs.targets.clone());
            let inp = &round.inputs;
            let classified = caches.classify.with(|Classify { classifier, hooks }| {
                let mut sids = hooks.refresh(&config_dirs);
                sids.extend(asked);
                let mut rows: Vec<SessionRow> = round.pre.iter()
                    .filter(|r| r.jsonl.as_deref().and_then(|j| Path::new(j).file_stem()).and_then(|s| s.to_str())
                        .is_some_and(|sid| sids.iter().any(|s| s == sid)))
                    .cloned().collect();
                if rows.is_empty() {
                    return None;
                }
                let facts = Facts { hooks, alive: &alive, config_dirs: &config_dirs, headless: inp.headless.as_ref(),
                    problems: &inp.problems, stall_seconds: inp.facts.stall_seconds, monitors: &published };
                let effects = handle.block_on(classifier.classify_some(&mut rows, &facts, &io));
                Some((rows, effects))
            });
            let (mut rows, effects) = classified?;
            decorate(&env, &caches, &dirs, &config_dirs, inp, &mut rows, io.wall(), io.mono(), false);
            Some((rows, effects))
        }).await.map_err(|e| joined(e, "rodada parcial interrompida"))?;
        self.flush_notes();
        let Some((mut changed, effects)) = done else { return Ok(Partial::Unchanged) };
        let plugins = self.plugin_facts(&changed).await;
        self.project_plugins(&mut changed, &plugins);
        public_problems(&mut changed);
        list_facts::mark_stale(&mut changed, &snap.facts, snap.facts_ok);
        self.gate_problems(&mut changed, false);
        self.demote(effects);
        let mut rows = (*snap.rows).clone();
        for new in changed {
            if let Some(row) = rows.iter_mut().find(|r| r.name == new.name) { *row = new; }
        }
        let produced = Produced { rows: Arc::new(rows), facts: snap.facts, facts_ok: snap.facts_ok };
        if let Some(s) = slot.as_mut() { s.produced = produced.clone(); }
        Ok(Partial::Done(produced))
    }

    /// A produção da lista: descoberta + fatos do Python + classificação + contexto, resposta,
    /// plano, loop e Git. É A função que o hub chama a cada tique; quem quer retrato pede
    /// `snapshot`, que segura a produção em um por vez.
    ///
    /// Linhas Codex, Pi, omp e Kimi levam o estado dos fatos; as de transferência em curso e as
    /// `orq` saem como o Python as deu, sem classificação nem decoração, no fim da lista.
    pub async fn produce(&self, input: &ProduceFacts) -> Result<Produced, ListError> {
        Ok(self.produce_round(input).await?.0)
    }

    async fn produce_round(&self, input: &ProduceFacts) -> Result<(Produced, Option<Arc<Round>>), ListError> {
        let dirs = self.dirs()?;
        let (rows, agent_pids, panes, children) = self.discovery(None).await?;
        let client = if input.shadow { &self.shadow_facts } else { &self.facts };
        // Um ator lento não soma o prazo dele ao dos fatos.
        let runtime = async { match self.runtime.get() { Some(r) => Some(r.snapshots().await), None => None } };
        let pane_pids = pi_pane_pids(&rows, &panes);
        let (fetched, runtime, plugins) = tokio::join!(client.fetch(&rows, input.owner_clients, &pane_pids, input.shadow), runtime, self.plugin_facts(&rows));
        if fetched.ok && !input.shadow { *lock(&self.python_apps) = Some((fetched.facts.owner_apps, Instant::now())); }
        let (mut rows, mut aside) = list_facts::apply((*rows).clone(), &fetched.facts, fetched.ok);
        if let Some(share) = self.shares.get() {
            let active = share.store.active_sessions();
            list_facts::mark_shared(&mut rows, &active);
            list_facts::mark_shared(&mut aside, &active);
            super::links::mark_pair_levels(rows.iter_mut().chain(aside.iter_mut()), share);
        }
        let mut targets = pane_targets(&panes, &agent_pids, &children);
        if let Some(registry) = self.plugin_runtime.get() {
            for row in rows.iter_mut().filter(|row| row.provider == "claude") {
                if let Some(Ok(reading)) = plugins.get(&row.name) {
                    row.lifecycle_id = super::links::session_life(Some(&reading.binding.key), None);
                    match registry.terminal_target(&row.name).await {
                        Ok(Some(target)) if target.key == reading.binding.key && target.generation == reading.binding.generation
                            && target.binding.conversation == reading.binding.conversation => {
                            row.jsonl = Some(target.transcript.to_string_lossy().into_owned());
                            targets.insert(row.name.clone(), target.binding.pane);
                        }
                        Err(error) => row.problema = Some(error.code),
                        Ok(Some(_)) => row.problema = Some("erro_interacao_desatualizada".into()),
                        _ => {},
                    }
                }
            }
        }
        let terminal = match self.plugin_runtime.get() { Some(registry) => registry.terminal_problems().await, None => BTreeMap::new() };
        let problems = claude_problems(&rows, &fetched.facts.problems, &terminal);
        let (env, caches) = (self.env.clone(), self.caches.clone());
        let inputs = Arc::new(RoundInputs { targets: Arc::new(targets),
            headless: runtime.map(|by_key| headless_by_name(&rows, by_key)), facts: fetched.facts.clone(), agent_pids, problems });
        // Só o hub acorda por arquivo: sem lista do dono aberta, ninguém usaria a cópia.
        let keep = input.owner_clients > 0 && !input.shadow;
        let handle = tokio::runtime::Handle::current();
        // Teto dos caches por sessão acompanha as linhas vivas: acima dele cada tique relia do zero.
        capped::set_live(rows.len() + aside.len());
        // Classificação e decoração leem arquivo (marcador, transcript, plano) e esperam captura:
        // fora da thread do runtime, que atende todas as conexões.
        let (inp, published) = (inputs.clone(), self.published.clone());
        let (rows, effects, git_dirs, pre) = tokio::task::spawn_blocking(move || {
            let config_dirs = caches.config_dirs(&dirs);
            let alive = |pid: i64| pid_alive(&*env.procs, pid);
            let io = MuxCapture::new(env.capture_program.clone(), CAPTURE_TIMEOUT, inp.targets.clone());
            let pre: Vec<SessionRow> = if keep { rows.iter().filter(|r| r.provider == "claude").cloned().collect() } else { Vec::new() };
            let effects = caches.classify.with(|Classify { classifier, hooks }| {
                hooks.refresh(&config_dirs);
                let facts = Facts { hooks, alive: &alive, config_dirs: &config_dirs,
                    headless: inp.headless.as_ref(), problems: &inp.problems, stall_seconds: inp.facts.stall_seconds,
                    monitors: &published };
                handle.block_on(classifier.classify(&mut rows, &facts, &io))
            });
            let git_dirs = decorate(&env, &caches, &dirs, &config_dirs, &inp, &mut rows, io.wall(), io.mono(), true);
            (rows, effects, git_dirs, pre)
        }).await.map_err(|e| joined(e, "produção interrompida"))?;
        self.flush_notes();
        self.refresh_git(git_dirs);
        if !input.shadow { self.demote(effects); }
        let mut rows = rows;
        self.project_plugins(&mut rows, &plugins);
        public_problems(&mut rows);
        rows.extend(aside);
        list_facts::mark_stale(&mut rows, &fetched.facts, fetched.ok);
        if !input.shadow { self.gate_problems(&mut rows, true); }
        if !input.shadow {
            self.prune_gone(&rows, Instant::now());
            self.record_live(&rows, fetched.fresh);
        }
        let round = keep.then(|| Arc::new(Round { inputs, pre }));
        Ok((Produced { rows: Arc::new(rows), facts: fetched.facts, facts_ok: fetched.ok }, round))
    }

    /// Git em segundo plano, um por pasta: um repositório lento não atrasa o card de ninguém. Só
    /// as pastas que `git_due` escolheu; `git::summary` ainda guarda o resultado por 3 s.
    fn refresh_git(&self, dirs: Vec<GitDue>) {
        for GitDue { dir, activity } in dirs {
            if !lock(&self.git_running).insert(dir.clone()) {
                continue;
            }
            let (caches, running, slots) = (self.caches.clone(), self.git_running.clone(), self.git_slots.clone());
            let diag = self.facts.diag.clone();
            tokio::spawn(async move {
                let Ok(_permit) = slots.acquire_owned().await else {
                    lock(&running).remove(&dir);
                    return;
                };
                let key = dir.clone();
                let done = tokio::task::spawn_blocking(move || {
                    let summary = hangar_workspace::git::summary(Some(&dir), false);
                    let diff = hangar_workspace::git::summary(Some(&dir), true);
                    // Sem `.git` o nulo é "não é repositório"; com ele, a consulta falhou.
                    let failed = (summary.is_null() || diff.is_null()) && Path::new(&dir).join(".git").exists();
                    let mut git = lock(&caches.git);
                    let before = git.remove(&dir).unwrap_or_default();
                    // Consulta que falhou fica com o último número bom, que segue o melhor valor.
                    let keep = |new: Value, old: Value| if new.is_null() { old } else { new };
                    git.insert(dir.clone(), (keep(summary, before.0), keep(diff, before.1)));
                    drop(git);
                    // Versão lida depois do `git status`, que pode regravar o índice. Consulta que falhou não
                    // grava a versão: a próxima rodada tenta de novo, sem esperar o teto.
                    if !failed {
                        lock(&caches.git_gate).insert(dir.clone(), GitGate { at: Instant::now(), activity, fingerprint: git_fingerprint(&dir) });
                    }
                    failed
                }).await;
                // A pasta no diário pelo nome, sem o caminho inteiro.
                let repo = Path::new(&key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match done {
                    Ok(false) => {}
                    Ok(true) => diag.report("rust.list_git_stale", &repo, "list_git_stale", "git da pasta falhou; fica o último número"),
                    Err(_) => diag.report("rust.list_git_stale", &repo, "list_git_failed", "resumo de Git interrompido; fica o último número"),
                }
                // Mesmo depois de um pânico: senão a pasta nunca mais teria Git.
                lock(&running).remove(&key);
            });
        }
    }

    pub async fn resolve(&self, name: &str, cwd: &str, pid: Option<i64>) -> Result<discover::Transcript, ListError> {
        let dirs = self.dirs()?;
        let (panes, dropped) = self.env.mux.list_panes_checked().await.map_err(|e| fail("mux_unavailable", e.code))?;
        if let Some(p) = dropped {
            self.facts.diag.report("rust.list_discovery", &p.key, p.code, p.reason);
        }
        let (env, caches, name, cwd) = (self.env.clone(), self.caches.clone(), name.to_owned(), cwd.to_owned());
        let out = tokio::task::spawn_blocking(move || {
            let children = env.procs.children(procs::CHILDREN_TTL).map_err(|_| fail("list_procs_unreadable", "mapa de processos ilegível"))?;
            Ok(caches.resolver.with(|r| discover::resolve_one(&panes, &*env.procs, &children, &dirs.claude.join("projects"), r, &name, &cwd, pid)))
        }).await.map_err(|e| joined(e, "resolução interrompida"))?;
        self.flush_notes();
        out
    }

    /// Falhas vistas pela leitura síncrona da rodada, ao diário.
    fn flush_notes(&self) {
        for n in list_facts::take_notes() {
            self.facts.diag.report(n.event, &n.session, &n.code, n.reason);
        }
        let hits = std::mem::take(&mut *self.caches.limits.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for hit in hits {
            tracing::info!(session = %hit.name, window = %hit.limit.window, "lista: aviso de limite de uso no fim do transcript");
            self.facts.diag.report("rust.list_limit_seen", &hit.name, &format!("limit_seen:{}", hit.limit.window), "a lista viu o aviso de limite de uso e o entregou ao vigia da troca de conta");
            match self.plugin_runtime.get() {
                Some(runtime) => { runtime.usage_limit(hit); }
                None => self.facts.diag.report("rust.list_limit_seen", &hit.name, "limit_no_runtime", "a lista viu o aviso de limite sem o runtime para entregá-lo"),
            }
        }
    }

    /// Sessão fora da lista há `FORGET_AFTER` sai dos caches por nome, como o `_forget` do fechamento:
    /// quem morreu sem o Python fechar (tmux morto por fora) não ocupa o cache até o teto. O prazo
    /// cobre a linha que some numa rodada só (pane ilegível) sem perder a resolução semeada.
    fn prune_gone(&self, rows: &[SessionRow], now: Instant) {
        let mut seen = lock(&self.seen);
        for row in rows {
            seen.insert(row.name.clone(), now);
        }
        let expired: Vec<String> = seen.iter().filter(|(_, at)| now.duration_since(**at) >= FORGET_AFTER).map(|(n, _)| n.clone()).collect();
        for name in expired {
            seen.remove(&name);
            self.forget(&name);
        }
    }

    /// Fatos que falharam ou sessão que publica página (Claude, Codex) sem transcript: não dá
    /// para dizer quem morreu. Pi, omp e Kimi não publicam e não travam a limpeza.
    fn record_live(&self, rows: &[SessionRow], facts_ok: bool) {
        let unknown = rows.iter().any(|r| r.jsonl.is_none() && matches!(r.provider.as_str(), "claude" | "codex"));
        let live = (facts_ok && !unknown).then(|| (Instant::now(), rows.iter().filter_map(|r| r.jsonl.clone()).collect()));
        *lock(&self.live_jsonl) = live;
    }

    /// Transcripts vivos pela última rodada do dono. Rodada velha também é `None`: com a lista
    /// fechada, sessão criada depois dela pareceria morta.
    pub fn live_jsonl(&self) -> Option<HashSet<String>> {
        lock(&self.live_jsonl).as_ref().filter(|(at, _)| at.elapsed() < LIVE_FRESH).map(|(_, set)| set.clone())
    }

    /// Não espera a rodada em curso: entra na fila e vale antes da próxima leitura do cache.
    pub fn seed(&self, name: &str, jsonl: &str) {
        let (name, jsonl) = (name.to_owned(), jsonl.to_owned());
        self.caches.resolver.apply(move |r| r.seed(&name, &jsonl));
    }

    /// `_forget`: nome reusado por outra sessão não herda nada da morta. Não espera, como `seed`.
    pub fn forget(&self, name: &str) {
        lock(&self.plugin_projection).remove(name);
        let c = &self.caches;
        let n = name.to_owned();
        c.resolver.apply({ let n = n.clone(); move |r| r.forget(&n) });
        c.decor.apply({ let n = n.clone(); move |d| { d.context.forget(&n); d.replies.forget(&n); } });
        c.classify.apply(move |k| k.classifier.forget(&n));
    }

    /// Não espera, como `seed`.
    pub fn rename(&self, old: &str, new: &str) {
        let mut projected = lock(&self.plugin_projection);
        projected.remove(old); projected.remove(new);
        drop(projected);
        let c = &self.caches;
        let (o, n) = (old.to_owned(), new.to_owned());
        c.resolver.apply({ let (o, n) = (o.clone(), n.clone()); move |r| r.rename(&o, &n) });
        c.decor.apply({ let (o, n) = (o.clone(), n.clone()); move |d| { d.context.forget(&o); d.replies.rename(&o, &n); } });
        c.classify.apply(move |k| k.classifier.rename(&o, &n));
    }
}

impl Caches {
    /// Pastas de conta (`list_config_dirs` + a base do backend): `CP_CLAUDE_CONFIG_DIRS` ou
    /// `~/.claude*`. Sobrar pasta só custa um `read_dir` vazio; faltar some com o marcador.
    fn config_dirs(&self, dirs: &Dirs) -> Arc<Vec<PathBuf>> {
        if let Some((at, v)) = &*lock(&self.config_dirs) && at.elapsed() < CONFIG_DIRS_TTL {
            return v.clone();
        }
        let mut found: Vec<PathBuf> = match std::env::var("CP_CLAUDE_CONFIG_DIRS").ok().filter(|s| !s.trim().is_empty()) {
            Some(raw) => raw.split(',').map(str::trim).filter(|s| !s.is_empty())
                .map(|item| expand_home(&labelled_path(item), &dirs.home)).collect(),
            None => match std::fs::read_dir(&dirs.home) {
                Ok(entries) => entries.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(".claude") && e.path().is_dir())
                    .map(|e| e.path()).collect(),
                Err(e) => {
                    // Sem as outras contas os marcadores delas somem: avisa e não guarda, tenta de novo no tique.
                    if crate::warn_limit::allow(None, "list_config_dirs_unreadable") {
                        tracing::warn!(code = "list_config_dirs_unreadable", kind = ?e.kind(), "pastas de conta ilegíveis");
                    }
                    return Arc::new(vec![dirs.claude.clone()]);
                }
            },
        };
        found.push(dirs.claude.clone());
        let mut seen = std::collections::HashSet::new();
        found.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
        let v = Arc::new(found);
        *lock(&self.config_dirs) = Some((Instant::now(), v.clone()));
        v
    }
}

pub(crate) fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_owned(),
        None => PathBuf::from(path),
    }
}

/// `rótulo:caminho` ou só o caminho; letra única seguida de barra é drive do Windows.
fn labelled_path(item: &str) -> String {
    match item.split_once(':') {
        Some((label, path)) if !(cfg!(windows) && label.len() == 1 && path.starts_with(['\\', '/'])) => path.trim().to_owned(),
        _ => item.to_owned(),
    }
}

/// (código, chave, motivo) de um arquivo ou processo que a descoberta não leu: vai ao diário.
/// Linhas descobertas, o pid do agente de cada uma (contexto de abertura e alvo da captura) e o que
/// a descoberta não conseguiu ler.
fn run_discovery(panes: &[Pane], procs: &dyn ProcessView, children: &ChildrenMap, resolver: &mut Resolver, dirs: &Dirs)
    -> (Vec<SessionRow>, HashMap<String, u32>, Vec<discover::DiscoveryProblem>) {
    let found = discover_other::discover_rows(panes, procs, children, resolver, dirs);
    (found.rows, found.agent_pids, found.problems)
}

/// Retrato do runtime (por chave) no nome de cada linha Claude sem terminal e de cada linha Codex: a
/// linha leva a chave na vida (`k:<chave>`). Linha sem chave fica fora, como sessão parada. Codex com
/// terminal só entra quando o Rust tem o ator dela: quem publica é o dono, não a presença do pane.
/// Falha ao identificar a pergunta de uma linha Claude (sidecar, plugin, vínculo do Rust): a tela recebe um
/// código só, com frase; o código interno vai ao diário da lista.
pub(crate) const INTERACTION_UNAVAILABLE: &str = "interacao_indisponivel";
impl ListBridge {
    /// Linha com problema que ainda não valeu pelo prazo sai sem ele. `complete`: rodada com todas as
    /// linhas; o portão de quem sumiu da lista vai embora.
    fn gate_problems(&self, rows: &mut [SessionRow], complete: bool) {
        use crate::problem_gate::Gate;
        let hold = Duration::from_millis(self.problem_hold_ms.load(std::sync::atomic::Ordering::Relaxed));
        let now = tokio::time::Instant::now().into_std();
        let mut gates = lock(&self.problems);
        if complete {
            gates.retain(|name, _| rows.iter().any(|row| &row.name == name));
            self.shown.retain(|name| rows.iter().any(|row| row.name == name));
        }
        for row in rows.iter_mut() {
            // Diagnóstico sai já, sem prazo: o código fica no diário da lista, nunca na tela nem no `Shown`.
            if let Some(code) = row.problema.as_deref().filter(|code| crate::problem_gate::diagnostic(code, &row.state)) {
                list_facts::note("rust.problem_hidden", &row.name, code.to_owned(), "aviso de diagnóstico fora da tela");
                row.problema = None;
            }
            let gate = gates.entry(row.name.clone()).or_default();
            if matches!(gate.admit(row.problema.as_deref(), now, hold), Gate::Hold(_)) { row.problema = None; }
            self.shown.set(&row.name, row.problema.as_deref());
        }
    }

    /// O que a lista mostra de cada sessão, para o hub da conversa nascer igual.
    pub fn shown_problems(&self) -> Arc<crate::problem_gate::Shown> { self.shown.clone() }

    /// Prazo do portão de problemas da lista; os testes dos produtores o zeram.
    pub fn set_problem_hold(&self, hold: Duration) { self.problem_hold_ms.store(hold.as_millis() as u64, std::sync::atomic::Ordering::Relaxed); }
}

/// O erro do plugin entra na linha só no lugar de nada ou de diagnóstico: a mensagem presa (código de tela)
/// não pode ser tapada por um código que o portão depois esconde.
fn set_unless_screen(row: &mut SessionRow, code: String) {
    if row.problema.as_deref().is_none_or(crate::problem_gate::is_diagnostic) { row.problema = Some(code); }
    else { list_facts::note("rust.problem_hidden", &row.name, code, "erro do plugin atrás do aviso de tela"); }
}

/// Causa interna de não identificar a pergunta aberta: a tela recebe `interacao_indisponivel` (lista e conversa).
pub(crate) fn interaction_unidentified(code: &str) -> bool {
    code.starts_with("askq_") || code.starts_with("erro_interacao_") || (code.starts_with("plugin_") && code.ends_with("_invalid"))
        || matches!(code, "runtime_binding" | "list_plugin_source_absent" | "terminal_target_missing" | "terminal_capture_target_changed" | "plugin_store_unavailable")
}

fn public_problems(rows: &mut [SessionRow]) {
    for row in rows.iter_mut().filter(|row| row.provider == "claude") {
        let Some(code) = row.problema.as_deref() else { continue };
        if interaction_unidentified(code) {
            list_facts::note("rust.list_interaction_unavailable", &row.name, code.to_owned(), "lista: pergunta da sessão sem identidade");
            row.problema = Some(INTERACTION_UNAVAILABLE.into());
        }
    }
}

/// Problema do runtime das linhas Claude: sem terminal, o que o lançador (Python) registrou; com terminal,
/// o do ator do Rust, nunca o eco dele que passou pelo Python. Os demais provedores seguem os fatos.
fn claude_problems(rows: &[SessionRow], python: &BTreeMap<String, String>, terminal: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut problems = python.clone();
    for row in rows.iter().filter(|row| row.provider == "claude" && !row.headless) {
        match terminal.get(&row.name) {
            Some(code) => { problems.insert(row.name.clone(), code.clone()); }
            None => { problems.remove(&row.name); }
        }
    }
    problems
}

fn headless_by_name(rows: &[SessionRow], by_key: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let mut by_key = by_key;
    rows.iter().filter(|r| r.provider == "codex" || (r.headless && r.provider == "claude")).filter_map(|r| {
        let key = r.lifecycle_id.as_deref()?.strip_prefix("k:")?;
        Some((r.name.clone(), by_key.remove(key)?))
    }).collect()
}

/// Pid do pane das linhas Pi e omp: o sidecar do catálogo, de onde sai a conta, mora no
/// `CLAUDE_CONFIG_DIR` dele.
fn pi_pane_pids(rows: &[SessionRow], panes: &[Pane]) -> BTreeMap<String, u32> {
    rows.iter().filter(|r| r.provider == "pi" || r.provider == "omp").filter_map(|r| {
        let pane = panes.iter().filter(|p| p.session == r.name).max_by_key(|p| p.active)?;
        Some((r.name.clone(), pane.pid?))
    }).collect()
}

/// Alvo da captura de cada sessão: o pane do agente; sem como saber, `=<sessão>:` (o ativo).
fn pane_targets(panes: &[Pane], agent_pids: &HashMap<String, u32>, children: &ChildrenMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pane in panes {
        let mine = panes.iter().filter(|p| p.session == pane.session).count() == 1
            || agent_pids.get(&pane.session).is_some_and(|a| pane.pid.is_some_and(|p| descends(i64::from(*a), i64::from(p), children)));
        if let (true, Some(t)) = (mine, pane.target()) {
            out.insert(pane.session.clone(), t);
        }
    }
    out
}

fn descends(pid: i64, root: i64, children: &ChildrenMap) -> bool {
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(p) = stack.pop() {
        if p == pid {
            return true;
        }
        if seen.insert(p) {
            stack.extend(children.get(&p).into_iter().flatten());
        }
    }
    false
}

pub(crate) fn pid_alive(procs: &dyn ProcessView, pid: i64) -> bool { pid > 0 && procs.start_time(pid).is_some() }

fn git_dir(row: &SessionRow) -> &str { row.git_cwd.as_deref().or(row.cwd.as_deref()).unwrap_or("") }

fn apply_git(row: &mut SessionRow, summary: &Value, diff: &Value) {
    let n = |v: &Value, k: &str| v[k].as_u64().and_then(|x| u32::try_from(x).ok());
    if !summary.is_null() {
        (row.git_dirty, row.git_ahead, row.git_behind) = (n(summary, "dirty"), n(summary, "ahead"), n(summary, "behind"));
    }
    if !diff.is_null() {
        (row.git_added, row.git_removed) = (n(diff, "added"), n(diff, "removed"));
    }
}

/// Contexto, resposta, plano, loop e Git das linhas classificadas; devolve as pastas com Git.
/// `whole`: a rodada inteira, que também tira do cache de Git as pastas sem sessão (a parcial só
/// tem as linhas que reclassificou).
#[allow(clippy::too_many_arguments)]
fn decorate(env: &ListEnv, caches: &Caches, dirs: &Dirs, config_dirs: &[PathBuf], inp: &RoundInputs,
            rows: &mut [SessionRow], wall: f64, mono: f64, whole: bool) -> Vec<GitDue> {
    caches.decor.with(|d| {
        for row in rows.iter_mut().filter(|r| r.provider == "claude") {
            let pid = inp.agent_pids.get(&row.name).map(|p| i64::from(*p));
            decorate_context(&mut d.context, row, pid, &*env.procs, dirs, config_dirs, wall, mono);
        }
        for (name, error) in d.replies.decorate(rows, |_| None) {
            if crate::warn_limit::allow(Some(&name), "list_reply_unreadable") {
                tracing::warn!(session = %name, kind = ?error.kind(), "lista: última resposta ilegível");
            }
            list_facts::note("rust.list_reply_unreadable", &name, format!("{:?}", error.kind()), "última resposta ilegível");
        }
        // O fim do transcript já gravado: o aviso de limite do Claude com terminal vai ao vigia daqui.
        for row in rows.iter() {
            let started = inp.agent_pids.get(&row.name).and_then(|pid| env.procs.start_time(i64::from(*pid)));
            if let Some(limit) = d.limits.seen(row, started, wall) {
                let account = super::context::config_dir_of(row.conta.as_deref()).unwrap_or_else(|| dirs.claude.clone());
                caches.limits.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(crate::runtime::gateway::LimitHit {
                    name: row.name.clone(), provider: "claude".into(), account: Some(account.to_string_lossy().into_owned()), limit });
            }
        }
    });
    caches.plans.with(|plans| rows.iter_mut().for_each(|row| plans.decorate(row, wall, mono)));
    for row in rows.iter_mut() {
        if let Some(p) = super::links::fill_loop(row, dirs) {
            list_facts::note("rust.list_discovery", &p.key, p.code.to_owned(), p.reason);
        }
    }
    let mut git = lock(&caches.git);
    for row in rows.iter_mut() {
        let (summary, diff) = git.get(git_dir(row)).cloned().unwrap_or_default();
        apply_git(row, &summary, &diff);
    }
    if !whole {
        return Vec::new();
    }
    // Pasta sem sessão sai: o cache não cresce com cada repositório que já passou pela lista.
    let live: HashSet<&str> = rows.iter().map(git_dir).filter(|d| !d.is_empty()).collect();
    git.retain(|d, _| live.contains(d.as_str()));
    drop(git);
    git_due_dirs(caches, rows, Instant::now())
}

/// `_claude_reading` com o cache de 20 s por (nome, transcript).
#[allow(clippy::too_many_arguments)]
fn decorate_context(cache: &mut ContextCache, row: &mut SessionRow, pid: Option<i64>, procs: &dyn ProcessView,
                    dirs: &Dirs, config_dirs: &[PathBuf], wall: f64, mono: f64) {
    let Some(jsonl) = row.jsonl.clone() else { return };
    let (ctx, model) = if cache.stale(&row.name, &jsonl, mono) {
        let version = context::source_version(&jsonl);
        let stem = Path::new(&jsonl).file_stem().and_then(|s| s.to_str()).map(str::to_owned);
        let chosen = facts_files::published_status(stem.as_deref(), config_dirs, wall).and_then(|p| p.model);
        let (opened, declared) = if row.headless {
            let meta = headless_meta(&dirs.home.join(".hangar/claude-headless").join(format!("{}.json", row.name)), &row.name);
            (meta["model"].as_str().map(str::to_owned), context::declared_window_value(&meta["context_window"]))
        } else if let Some(pid) = pid {
            let declared = procs.env_var(pid, "CLAUDE_CODE_MAX_CONTEXT_TOKENS").ok().flatten()
                .and_then(|v| context::declared_window(&v.to_string_lossy()));
            (context::opened_model(&procs.argv(pid)), declared)
        } else {
            (None, None)
        };
        let account_dir = context::config_dir_of(row.conta.as_deref()).unwrap_or_else(|| dirs.claude.clone());
        let scan = cache.scan(&row.name, &jsonl, &version);
        let reading = cache.reading(&ReadingInputs { jsonl: Path::new(&jsonl), account_dir: &account_dir,
            chosen: chosen.as_deref(), opened: opened.as_deref(), declared, engine: row.engine.is_some() }, &scan,
            row.headless, std::time::Instant::now());
        cache.store_with(&row.name, &jsonl, mono, reading, version, Some(scan))
    } else {
        cache.cached(&row.name, &jsonl)
    };
    (row.context, row.model) = (ctx, model);
}

/// Sidecar da sessão sem terminal: ilegível ou torto avisa, senão o modelo e a janela caem calados
/// nos da conta.
fn headless_meta(path: &Path, session: &str) -> Value {
    let code = match std::fs::read(path) {
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Ok(v) => return v,
            Err(e) => format!("list_headless_meta_invalid:{:?}", e.classify()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Value::Null,
        Err(e) => format!("list_headless_meta_unreadable:{:?}", e.kind()),
    };
    list_facts::note("rust.list_file_rejected", session, code, "sidecar da sessão sem terminal ilegível");
    Value::Null
}

#[derive(Deserialize)]
#[serde(tag = "op", content = "args")]
enum Operation {
    #[serde(rename = "list.discover")]
    Discover { #[serde(default)] newer_than: Option<f64> },
    #[serde(rename = "list.snapshot")]
    Snapshot {},
    #[serde(rename = "list.invalidate")]
    Invalidate {},
    #[serde(rename = "list.resolve")]
    Resolve { name: String, cwd: String, #[serde(default)] pid: Option<i64> },
    #[serde(rename = "list.seed")]
    Seed { name: String, jsonl: String },
    #[serde(rename = "list.forget")]
    Forget { name: String },
    #[serde(rename = "state.facts")]
    StateFacts { name: String, facts: crate::state::facts::StateFacts },
    /// Painel de terminal real aberto na sessão: o 409 de quem conta linha do pane.
    #[serde(rename = "term.active")]
    TermActive { name: String },
}

async fn execute(bridge: &Arc<ListBridge>, op: Operation) -> Result<Value, ListError> {
    let rows = |r: &[SessionRow]| serde_json::to_value(r).map_err(|_| fail("list_task_failed", "linha sem serializar"));
    let cache = |f: Box<dyn FnOnce(&ListBridge) + Send>| {
        let bridge = bridge.clone();
        async move {
            tokio::task::spawn_blocking(move || f(&bridge)).await
                .map(|()| Value::Null).map_err(|e| joined(e, "cache interrompido"))
        }
    };
    match op {
        Operation::Discover { newer_than } => rows(&bridge.discover(newer_than).await?),
        Operation::Snapshot {} => {
            let produced = bridge.snapshot().await?;
            // Sem nenhuma resposta boa do Python, acesso, escondidas e transferências são
            // desconhecidos, não vazios: o convidado veria o que não é dele.
            if produced.facts.unknown {
                return Err(fail("list_facts_unknown", "fatos da lista ainda sem resposta"));
            }
            rows(&produced.rows)
        }
        Operation::Invalidate {} => { bridge.invalidate(); Ok(Value::Null) }
        Operation::Resolve { name, cwd, pid } => {
            let t = bridge.resolve(&name, &cwd, pid).await?;
            Ok(json!({"jsonl": t.jsonl, "tracked": t.tracked}))
        }
        Operation::Seed { name, jsonl } => cache(Box::new(move |b| b.seed(&name, &jsonl))).await,
        Operation::Forget { name } => cache(Box::new(move |b| b.forget(&name))).await,
        // Respondido pelo `private`, que tem os painéis.
        Operation::TermActive { .. } => Err(fail("list_bridge_invalid_request", "term.active fora do private")),
        Operation::StateFacts { name, facts } => {
            use crate::state::facts::Push;
            Ok(match bridge.state_facts.push(&name, facts, Instant::now()) {
                Push::Accepted { gap } => json!({"accepted": true, "watched": true, "gap": gap}),
                Push::Dropped => json!({"accepted": false, "watched": true, "gap": false}),
                Push::Unwatched => json!({"accepted": false, "watched": false, "gap": false}),
            })
        }
    }
}

fn term_active(st: &AppState, name: &str) -> Result<Value, ListError> {
    Ok(json!({"active": st.term.is_active(name)}))
}

fn reply(value: Value) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

pub async fn private(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let refused = |code: &'static str| {
        if crate::warn_limit::allow(None, code) {
            tracing::warn!(code, "ponte da lista recusou o pedido");
        }
        StatusCode::BAD_REQUEST.into_response()
    };
    let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY)).await else {
        return refused("list_bridge_body");
    };
    // Operação desconhecida aqui costuma ser Python e Rust de versões diferentes.
    let Ok(op) = serde_json::from_slice::<Operation>(&bytes) else {
        return refused("list_bridge_invalid_request");
    };
    let result = match op {
        Operation::TermActive { name } => term_active(&st, &name),
        op => execute(&st.list, op).await,
    };
    match result {
        Ok(result) => reply(json!({"ok": true, "result": result})),
        Err(e) => {
            if crate::warn_limit::allow(None, e.code) {
                tracing::warn!(code = e.code, detail = e.detail, "ponte da lista falhou");
            }
            reply(json!({"ok": false, "error": {"code": e.code, "detail": e.detail}}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_round_rebuilds_rows_and_keeps_only_same_life_projection_on_error() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::{PluginFacts, PluginReading};
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs { home: tmp.path().into(), claude: tmp.path().join(".claude"), codex_home: tmp.path().join(".codex"),
            pi_sessions: tmp.path().join("pi"), omp_config: tmp.path().join("omp"), omp_agent: tmp.path().join("omp/agent"), kimi_home: tmp.path().join("kimi") };
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: Some(dirs) }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let row = || serde_json::from_value::<SessionRow>(json!({"name":"s","provider":"claude","headless":true,"lifecycle_id":"k"})).unwrap();
        // A descoberta semeada vale por `DISCOVER_TTL`; no Windows a recusa do endereço dos fatos leva ~2 s
        // por rodada e a seguinte redescobriria pelo multiplexador real. Cada rodada parte da mesma linha.
        let seed = || async { *bridge.discovery.lock().await = Some(Discovery { at: Instant::now(), wall: wall_now(), epoch: 0, fresh: true,
            rows: Arc::new(vec![row()]), agent_pids: Arc::default(), panes: Arc::default(), children: Arc::default() }); };
        seed().await;
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        let local = PluginFacts { plugin_state: None, waiter_open: true, heartbeat_age_ms: Some(0), question: None,
            questions: vec![json!({"id":"perm:a","interaction_id":"a","questions":[],"tool":"Bash","resumo":"cmd"})], suggestion: "".into() };
        let reading = |life: InteractionBinding, facts| BTreeMap::from([("s".into(), Ok(PluginReading { binding: life, facts }))]);
        *lock(&bridge.test_plugins) = Some(reading(binding.clone(), Ok(local.clone())));
        let first = bridge.produce_round(&ProduceFacts::default()).await.unwrap().0;
        assert_eq!(first.rows[0].interaction_id.as_deref(), Some("a"));
        let mut ambiguous = local;
        ambiguous.questions.push(json!({"id":"perm:b","interaction_id":"b","questions":[],"tool":"Bash","resumo":"cmd"}));
        *lock(&bridge.test_plugins) = Some(reading(binding.clone(), Ok(ambiguous)));
        seed().await;
        let rebuilt = bridge.produce_round(&ProduceFacts::default()).await.unwrap().0;
        // Decisão P21: duas permissões seguradas saem uma de cada vez, sem problema na linha.
        assert_eq!(rebuilt.rows[0].interaction_id.as_deref(), Some("a"));
        assert_ne!(rebuilt.rows[0].problema.as_deref(), Some(INTERACTION_UNAVAILABLE), "nunca o problema no lugar do cartão");
        *lock(&bridge.test_plugins) = Some(reading(InteractionBinding { generation: 2, ..binding }, Err("plugin_store_unavailable".into())));
        seed().await;
        let next = bridge.produce_round(&ProduceFacts::default()).await.unwrap().0;
        assert!(next.rows[0].interaction_id.is_none(), "rodada nova não herda cartão da geração anterior");
    }

    /// Com a fonte do plugin ligada, a vida da linha Claude segue `k:<chave>`: sem o prefixo o retrato
    /// do runtime não acha a linha sem terminal e ela aparece parada.
    #[tokio::test]
    async fn plugin_reading_keeps_the_k_life_and_the_runtime_snapshot_of_a_headless_claude() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::{PluginFacts, PluginReading};
        struct Snapshot;
        impl HeadlessSource for Snapshot {
            fn snapshots(&self) -> futures_util::future::BoxFuture<'_, BTreeMap<String, Value>> {
                Box::pin(async { BTreeMap::from([("k".to_owned(),
                    json!({"key":"k","generation":1,"error":null,"view":{"alive":true,"headless":true,"public_state":{"state":"working","status_line":"opus"}}}))]) })
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs { home: tmp.path().into(), claude: tmp.path().join(".claude"), codex_home: tmp.path().join(".codex"),
            pi_sessions: tmp.path().join("pi"), omp_config: tmp.path().join("omp"), omp_agent: tmp.path().join("omp/agent"), kimi_home: tmp.path().join("kimi") };
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: Some(dirs) }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        bridge.set_runtime(Arc::new(Snapshot));
        bridge.set_plugin_runtime(Arc::new(crate::runtime::gateway::RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "test".into(), "instance".into())));
        let row = serde_json::from_value::<SessionRow>(json!({"name":"s","provider":"claude","headless":true,"lifecycle_id":"k:k"})).unwrap();
        *bridge.discovery.lock().await = Some(Discovery { at: Instant::now(), wall: wall_now(), epoch: 0, fresh: true,
            rows: Arc::new(vec![row]), agent_pids: Arc::default(), panes: Arc::default(), children: Arc::default() });
        let local = PluginFacts { plugin_state: None, waiter_open: true, heartbeat_age_ms: Some(0), question: None, questions: vec![], suggestion: "".into() };
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        *lock(&bridge.test_plugins) = Some(BTreeMap::from([("s".into(), Ok(PluginReading { binding, facts: Ok(local) }))]));
        let produced = bridge.produce_round(&ProduceFacts::default()).await.unwrap().0;
        assert_eq!(produced.rows[0].lifecycle_id.as_deref(), Some("k:k"));
        assert_eq!(produced.rows[0].status_line.as_deref(), Some("opus"), "o retrato do runtime chega à linha");
    }

    /// A permissão que o plugin segura num Claude com terminal sai do `plugin::Store` do Rust, sem fato do
    /// Python (o desta ponte nem responde): o pane não desenha cartão e a lista ainda mostra a pergunta.
    #[tokio::test]
    async fn held_permission_of_a_terminal_claude_comes_from_the_rust_store() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::{PluginFacts, PluginReading};
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs { home: tmp.path().into(), claude: tmp.path().join(".claude"), codex_home: tmp.path().join(".codex"),
            pi_sessions: tmp.path().join("pi"), omp_config: tmp.path().join("omp"), omp_agent: tmp.path().join("omp/agent"), kimi_home: tmp.path().join("kimi") };
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: Some(dirs) }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let row = serde_json::from_value::<SessionRow>(json!({"name":"t","provider":"claude","headless":false,"lifecycle_id":"k"})).unwrap();
        *bridge.discovery.lock().await = Some(Discovery { at: Instant::now(), wall: wall_now(), epoch: 0, fresh: true,
            rows: Arc::new(vec![row]), agent_pids: Arc::default(), panes: Arc::default(), children: Arc::default() });
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        let local = PluginFacts { plugin_state: None, waiter_open: true, heartbeat_age_ms: Some(0), question: None,
            questions: vec![json!({"id":"perm:a","interaction_id":"shown-a","questions":[],"tool":"Bash","resumo":"ls"})], suggestion: "".into() };
        *lock(&bridge.test_plugins) = Some(BTreeMap::from([("t".into(), Ok(PluginReading { binding, facts: Ok(local) }))]));
        let produced = bridge.produce_round(&ProduceFacts::default()).await.unwrap().0;
        let row = &produced.rows[0];
        assert_eq!((row.state.as_str(), row.question.as_deref(), row.interaction_id.as_deref()),
            ("awaiting_input", Some("Bash: ls"), Some("shown-a")));
        assert_eq!(row.options.as_deref(), Some(&["Yes".to_owned(), "No".to_owned()][..]));
    }

    /// Parity M14: permissão segurada pelo plugin troca "trabalhando" por "aguardando" e desfaz a travada,
    /// que só vale para "trabalhando" (sem isso sai o aviso e o push de travada).
    #[test]
    fn held_permission_is_awaiting_not_stalled() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::{PluginFacts, PluginReading};
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let mut rows = vec![serde_json::from_value::<SessionRow>(json!({"name":"s","provider":"claude","state":"working","stalled":true})).unwrap()];
        let local = PluginFacts { plugin_state: None, waiter_open: true, heartbeat_age_ms: Some(0), question: None,
            questions: vec![json!({"id":"perm:a","interaction_id":"a","questions":[],"tool":"Bash","resumo":"cmd"})], suggestion: "".into() };
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        bridge.project_plugins(&mut rows, &BTreeMap::from([("s".into(), Ok(PluginReading { binding, facts: Ok(local) }))]));
        assert_eq!((rows[0].state.as_str(), rows[0].stalled), ("awaiting_input", false));
    }

    /// O problema da linha só sai depois de valer pelo prazo, reconferido a cada rodada; resolvido some na
    /// hora, e o estado da linha não espera.
    #[tokio::test(start_paused = true)]
    async fn a_row_problem_shows_only_after_it_held() {
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        bridge.set_problem_hold(crate::problem_gate::HOLD);
        let round = |problema: Option<&str>| {
            let mut rows = vec![serde_json::from_value::<SessionRow>(json!({"name":"s","provider":"claude","state":"dead","problema":problema})).unwrap()];
            bridge.gate_problems(&mut rows, true);
            (rows[0].state.clone(), rows[0].problema.clone())
        };
        assert_eq!(round(Some("headless_processo_caiu")), ("dead".into(), None), "o estado sai já, o problema espera");
        tokio::time::advance(std::time::Duration::from_millis(1500)).await;
        assert_eq!(round(Some("headless_processo_caiu")).1, None);
        tokio::time::advance(std::time::Duration::from_millis(1500)).await;
        assert_eq!(round(Some("headless_processo_caiu")).1.as_deref(), Some("headless_processo_caiu"), "duas rodadas e o prazo: sai");
        assert_eq!(round(None).1, None, "resolvido some na hora");
    }

    /// Diagnóstico sai da linha na hora e não passa à conversa (`Shown`); a pergunta sem identidade só fica na
    /// tela com a sessão esperando a pessoa (P1). O código vai ao diário da lista.
    #[tokio::test]
    async fn a_diagnostic_row_problem_never_shows() {
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        bridge.set_problem_hold(std::time::Duration::ZERO);
        let round = |state: &str, problema: &str| {
            let mut rows = vec![serde_json::from_value::<SessionRow>(json!({"name":"diag-row","provider":"claude","state":state,"problema":problema})).unwrap()];
            bridge.gate_problems(&mut rows, true);
            (rows[0].state.clone(), rows[0].problema.clone())
        };
        assert_eq!(round("idle", "list_facts_unavailable"), ("idle".into(), None), "fatos velhos: fora da tela");
        assert_eq!(bridge.shown.get("diag-row"), None, "a conversa não herda diagnóstico");
        assert!(list_facts::notes_for("diag-row").iter().any(|code| code == "list_facts_unavailable"), "o diário recebe o código");
        assert_eq!(round("working", INTERACTION_UNAVAILABLE).1, None, "P1: fora da espera, diário");
        assert_eq!(round("awaiting_input", INTERACTION_UNAVAILABLE).1.as_deref(), Some(INTERACTION_UNAVAILABLE), "P1: esperando a pessoa, tela");
        assert_eq!(round("idle", "headless_processo_caiu").1.as_deref(), Some("headless_processo_caiu"), "de tela segue saindo");
    }

    /// Mensagem presa e fatos do plugin indisponíveis juntos: a linha leva a mensagem presa, que o portão mostra;
    /// o erro do plugin, que é diagnóstico ou pergunta sem identidade, não a tapa.
    #[test]
    fn a_stuck_message_wins_over_unavailable_plugin_facts() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::PluginReading;
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let stuck = || vec![serde_json::from_value::<SessionRow>(json!({"name":"stuck-wins","provider":"claude","state":"working","problema":"terminal_input_stalled"})).unwrap()];
        let mut rows = stuck();
        bridge.project_plugins(&mut rows, &BTreeMap::from([("stuck-wins".into(), Err("plugin_store_unavailable".into()))]));
        assert_eq!(rows[0].problema.as_deref(), Some("terminal_input_stalled"), "leitura do plugin que falhou");
        let mut rows = stuck();
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        bridge.project_plugins(&mut rows, &BTreeMap::from([("stuck-wins".into(), Ok(PluginReading { binding, facts: Err("runtime_binding".into()) }))]));
        assert_eq!(rows[0].problema.as_deref(), Some("terminal_input_stalled"), "fatos do plugin que não vieram");
        // O erro que ficou fora da linha vai ao diário.
        let notes = list_facts::notes_for("stuck-wins");
        for code in ["plugin_store_unavailable", "runtime_binding"] { assert!(notes.iter().any(|noted| noted == code), "{code} no diário: {notes:?}"); }
    }

    /// Decisão M11: a pergunta que o plugin segura chega ao card com o formulário e o ID dele, para o `/answer`.
    #[test]
    fn held_ask_question_reaches_the_row_as_a_form() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::{PluginFacts, PluginReading};
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "missing-test-mux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let mut rows = vec![serde_json::from_value::<SessionRow>(json!({"name":"s","provider":"claude","state":"working"})).unwrap()];
        let form = json!({"id":"ask:t1","interaction_id":"form-a","questions":[{"header":"H","question":"Qual?","multiSelect":false,"options":[{"label":"A","description":""}]}]});
        let local = PluginFacts { plugin_state: None, waiter_open: true, heartbeat_age_ms: Some(0), question: None, questions: vec![form], suggestion: "".into() };
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        bridge.project_plugins(&mut rows, &BTreeMap::from([("s".into(), Ok(PluginReading { binding, facts: Ok(local) }))]));
        let row = &rows[0];
        assert_eq!(row.state, "awaiting_input");
        let form = row.ask_form.as_ref().expect("formulário na linha");
        assert_eq!((form["interaction_id"].as_str(), form["request_id"].as_str()), (Some("form-a"), Some("ask:t1")));
        assert_eq!(form["questions"][0]["question"], "Qual?");
    }

    /// Parity M12: o código interno de identidade vai ao diário; a linha leva o código com frase.
    #[test]
    fn internal_interaction_codes_become_the_public_one() {
        let mut rows = vec![serde_json::from_value::<SessionRow>(json!({"name":"m12-row","provider":"claude","problema":"askq_pane_mismatch"})).unwrap()];
        public_problems(&mut rows);
        assert_eq!(rows[0].problema.as_deref(), Some(INTERACTION_UNAVAILABLE));
        assert!(list_facts::notes_for("m12-row").iter().any(|code| code == "askq_pane_mismatch"));
    }

    /// Parity A5: o problema do runtime das linhas Claude chega à classificação. Sem terminal, o do
    /// lançador (Python); com terminal, o do ator do Rust, e nunca o eco velho do Python.
    #[test]
    fn claude_runtime_problems_reach_the_rows() {
        let row = |name: &str, provider: &str, headless: bool| serde_json::from_value::<SessionRow>(json!({"name":name,"provider":provider,"headless":headless})).unwrap();
        let rows = [row("hl", "claude", true), row("term", "claude", false), row("calm", "claude", false), row("pi", "pi", false)];
        let python = BTreeMap::from([("hl".into(), "headless_sem_login".into()), ("calm".into(), "runtime_falhou".into()), ("pi".into(), "x".into())]);
        let terminal = BTreeMap::from([("term".into(), "terminal_input_composer_busy".into())]);
        let problems = claude_problems(&rows, &python, &terminal);
        assert_eq!(problems.get("hl").map(String::as_str), Some("headless_sem_login"));
        assert_eq!(problems.get("term").map(String::as_str), Some("terminal_input_composer_busy"));
        assert_eq!(problems.get("calm"), None, "com terminal o Python não decide");
        assert_eq!(problems.get("pi").map(String::as_str), Some("x"));
    }

    #[test]
    fn git_reruns_on_ref_change_activity_or_idle_ceiling_only() {
        let now = Instant::now();
        let fp = Some(vec![1u128, 2]);
        let gate = GitGate { at: now, activity: 7, fingerprint: fp.clone() };
        assert!(git_due(None, 7, &fp, now), "pasta nova");
        assert!(!git_due(Some(&gate), 7, &fp, now + Duration::from_secs(29)), "parada e sem mudança");
        assert!(git_due(Some(&gate), 8, &fp, now), "sessão da pasta escreveu");
        assert!(git_due(Some(&gate), 7, &Some(vec![1, 3]), now), "commit, push ou fetch");
        assert!(git_due(Some(&gate), 7, &fp, now + GIT_IDLE_REFRESH), "edição por fora aparece no teto");
    }

    #[test]
    fn runtime_view_reaches_codex_headless_rows() {
        let row = |name: &str, provider: &str, headless: bool| -> SessionRow { serde_json::from_value(serde_json::json!({
            "name": name, "provider": provider, "headless": headless, "lifecycle_id": format!("k:{name}")})).unwrap() };
        let rows = [row("cl", "claude", true), row("cx", "codex", true), row("tui", "codex", false), row("ct", "claude", false), row("py", "codex", false)];
        // `tui` tem ator no Rust (chave no retrato): ele publica. `py` segue com o Python; Claude com terminal tem o `Monitor`.
        let by_key = ["cl", "cx", "tui", "ct"].map(|k| (k.to_owned(), Value::Null)).into_iter().collect();
        assert_eq!(headless_by_name(&rows, by_key).into_keys().collect::<Vec<_>>(), ["cl", "cx", "tui"]);
    }

    #[test]
    fn guarded_change_never_waits_for_the_holder_and_lands_before_the_next_read() {
        let g: Arc<Guarded<Vec<&'static str>>> = Arc::default();
        let (held, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
        let holder = {
            let g = g.clone();
            std::thread::spawn(move || g.with(|v| {
                v.push("rodada");
                held.0.send(()).unwrap();
                release.1.recv().unwrap();
            }))
        };
        held.1.recv().unwrap();
        let start = Instant::now();
        g.apply(|v| v.push("seed"));
        assert!(start.elapsed() < Duration::from_millis(100), "esperou a rodada");
        release.0.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(g.with(|v| v.clone()), ["rodada", "seed"]);
        g.apply(|v| v.push("livre"));
        assert_eq!(*lock(&g.value), ["rodada", "seed", "livre"], "valor livre: aplicado na hora");
    }

    #[test]
    fn session_gone_for_a_while_leaves_the_caches() {
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "tmux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let row = |n: &str| serde_json::from_value::<SessionRow>(json!({"name": n})).unwrap();
        let t0 = Instant::now();
        bridge.prune_gone(&[row("a"), row("b")], t0);
        bridge.seed("a", "/x/a.jsonl");
        bridge.seed("b", "/x/b.jsonl");
        let cached = || bridge.caches.resolver.with(|r| r.cached().keys().cloned().collect::<Vec<_>>());
        bridge.prune_gone(&[row("b")], t0 + Duration::from_secs(5));
        assert_eq!(cached(), ["a", "b"], "uma rodada fora não esquece");
        bridge.prune_gone(&[row("b")], t0 + FORGET_AFTER + Duration::from_secs(1));
        assert_eq!(cached(), ["b"]);
    }

    #[test]
    fn live_jsonl_is_certain_only_when_every_page_publisher_has_a_transcript() {
        let bridge = ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "tmux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into()));
        let row = |name: &str, provider: &str, jsonl: Option<&str>|
            serde_json::from_value::<SessionRow>(json!({"name": name, "provider": provider, "jsonl": jsonl})).unwrap();
        assert!(bridge.live_jsonl().is_none(), "lista nunca aberta");
        bridge.record_live(&[row("a", "claude", Some("/t/a.jsonl")), row("b", "claude", None)], true);
        assert!(bridge.live_jsonl().is_none(), "Claude sem transcript");
        bridge.record_live(&[row("c", "codex", None)], true);
        assert!(bridge.live_jsonl().is_none(), "Codex sem transcript");
        bridge.record_live(&[row("a", "claude", Some("/t/a.jsonl")), row("p", "pi", None), row("k", "kimi", None)], true);
        assert_eq!(bridge.live_jsonl(), Some(HashSet::from(["/t/a.jsonl".to_owned()])), "Pi e Kimi não publicam página");
        bridge.record_live(&[row("a", "claude", Some("/t/a.jsonl"))], false);
        assert!(bridge.live_jsonl().is_none(), "fatos falharam");
        bridge.record_live(&[row("a", "claude", Some("/t/a.jsonl"))], true);
        if let Some(old) = Instant::now().checked_sub(LIVE_FRESH + Duration::from_secs(1)) {
            lock(&bridge.live_jsonl).as_mut().unwrap().0 = old;
            assert!(bridge.live_jsonl().is_none(), "rodada velha não vale");
        }
    }

    #[tokio::test]
    async fn state_facts_op_reaches_the_store_and_drops_old_sequences() {
        let bridge = Arc::new(ListBridge::new(ListEnv { mux: Mux::default(), capture_program: "tmux".into(),
            procs: Arc::new(procs::SystemProcs::default()), dirs: None }, FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into())));
        let op = |seq: u64| serde_json::from_value::<Operation>(json!({"op": "state.facts", "args": {"name": "s1", "facts": {
            "seq": seq, "plugin_state": {"state": "idle", "reason": null, "age_ms": 0}, "waiter_open": true,
            "heartbeat_age_ms": 0, "question": null, "suggestion": "", "body_columns": null, "band_anchor": null,
            "in_transfer_ms": 0, "transfer_active": false, "permission_op": false}}})).unwrap();
        assert_eq!(execute(&bridge, op(1)).await.unwrap()["watched"], false, "sem Monitor não guarda nada");
        assert!(bridge.state_facts.get("s1").is_none());
        bridge.state_facts.watch("s1");
        assert_eq!(execute(&bridge, op(2)).await.unwrap(), json!({"accepted": true, "watched": true, "gap": false}));
        assert_eq!(execute(&bridge, op(1)).await.unwrap(), json!({"accepted": false, "watched": true, "gap": false}));
        assert_eq!(execute(&bridge, op(4)).await.unwrap()["gap"], true);
        assert_eq!(bridge.state_facts.get("s1").unwrap().facts.seq, 4);
        assert!(serde_json::from_value::<Operation>(json!({"op": "state.facts", "args": {"name": "s1", "facts": {"seq": 3}}})).is_err(),
            "fato incompleto é recusado, nunca vazio");
    }

    #[test]
    fn unreadable_headless_sidecar_reaches_the_diary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hl-bridge-test.json");
        assert_eq!(headless_meta(&path, "hl-bridge-test"), Value::Null, "ausente é normal");
        assert!(list_facts::notes_for("hl-bridge-test").is_empty());
        std::fs::write(&path, "{").unwrap();
        assert_eq!(headless_meta(&path, "hl-bridge-test"), Value::Null);
        assert_eq!(list_facts::notes_for("hl-bridge-test"), ["list_headless_meta_invalid:Eof"]);
    }
}
