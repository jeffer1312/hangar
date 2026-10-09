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
    headless: Option<BTreeMap<String, Value>>, facts: Arc<ListFacts> }

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
struct Decor { context: ContextCache, replies: ReplyCache, plans: PlanTracker }

/// Marcadores junto da classificação: duas produções seguidas não releem tudo do zero.
#[derive(Default)]
struct Classify { classifier: Classifier, hooks: HookStates }

/// Caches que atravessam rodadas, por nome de sessão, cada um com a própria trava.
#[derive(Default)]
struct Caches {
    resolver: Guarded<Resolver>,
    decor: Guarded<Decor>,
    classify: Guarded<Classify>,
    /// Último resumo de Git por pasta: a lista não espera o `git status` (`_git_ultimo`).
    git: Mutex<HashMap<String, (Value, Value)>>,
    config_dirs: Mutex<Option<(Instant, Arc<Vec<PathBuf>>)>>,
}

pub struct ListBridge {
    env: Arc<ListEnv>,
    facts: FactsClient,
    shadow_facts: FactsClient,
    caches: Arc<Caches>,
    /// Retrato do runtime das sessões sem terminal; sem ele, a linha diz `list_runtime_absent`.
    runtime: std::sync::OnceLock<Arc<dyn HeadlessSource>>,
    /// Listas do dono abertas no hub: o retrato pedido de fora manda a mesma contagem, senão a
    /// pergunta ao Python alternaria de chave e a presença do app mudaria a cada pedido.
    owner_clients: AtomicU32,
    /// Última rodada em que cada nome estava na lista: sumido há `FORGET_AFTER`, os caches dele saem.
    seen: Mutex<HashMap<String, Instant>>,
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
            owner_clients: AtomicU32::new(0), seen: Mutex::default(),
            discovery: tokio::sync::Mutex::new(None),
            snapshot: tokio::sync::Mutex::new(None), git_running: Arc::default(),
            git_slots: Arc::new(tokio::sync::Semaphore::new(GIT_SLOTS)), epoch: AtomicU64::new(0),
            state_facts: Arc::default(), demoted, published: Arc::default(), live_jsonl: Mutex::default() }
    }

    /// Rebaixamentos da rodada: valem já aqui (lista e `Monitor`), e os session ids vão ao Python.
    fn demote(&self, effects: Vec<Effect>) -> Vec<String> {
        effects.into_iter().map(|Effect::DemoteAwaiting { sid, ts }| {
            self.demoted.demote(&sid, ts);
            sid
        }).collect()
    }

    /// Uma vez, na subida do servidor com o runtime de pé.
    pub fn set_runtime(&self, runtime: Arc<dyn HeadlessSource>) {
        if self.runtime.set(runtime).is_err() {
            tracing::warn!(code = "list_runtime_twice", "lista: retrato do runtime já ligado; o segundo fica de fora");
            self.facts.diag.report("rust.list_failed", "", "list_runtime_twice", "retrato do runtime ligado duas vezes; vale o primeiro");
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

    /// `registry.list()`. `newer_than` (época em s) é a sessão criada há menos de 1 s: só vale uma
    /// descoberta que começou depois disso, com o mapa de processos relido.
    pub async fn discover(&self, newer_than: Option<f64>) -> Result<Arc<Vec<SessionRow>>, ListError> {
        Ok(self.discovery(newer_than).await?.0)
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
                    problems: &inp.facts.problems, stall_seconds: inp.facts.stall_seconds, held: &inp.facts.held,
                    monitors: &published };
                let effects = handle.block_on(classifier.classify_some(&mut rows, &facts, &io));
                Some((rows, effects))
            });
            let (mut rows, effects) = classified?;
            decorate(&env, &caches, &dirs, &config_dirs, inp, &mut rows, io.wall(), io.mono(), false);
            Some((rows, effects))
        }).await.map_err(|e| joined(e, "rodada parcial interrompida"))?;
        self.flush_notes();
        let Some((mut changed, effects)) = done else { return Ok(Partial::Unchanged) };
        list_facts::mark_stale(&mut changed, &snap.facts, snap.facts_ok);
        let demote = self.demote(effects);
        if !demote.is_empty() {
            self.facts.demote(demote);
        }
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
        let (fetched, runtime) = tokio::join!(client.fetch(&rows, input.owner_clients, &pane_pids, input.shadow), runtime);
        let (mut rows, aside) = list_facts::apply((*rows).clone(), &fetched.facts, fetched.ok);
        let (env, caches) = (self.env.clone(), self.caches.clone());
        let inputs = Arc::new(RoundInputs { targets: Arc::new(pane_targets(&panes, &agent_pids, &children)),
            headless: runtime.map(|by_key| headless_by_name(&rows, by_key)), facts: fetched.facts.clone(), agent_pids });
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
                    headless: inp.headless.as_ref(), problems: &inp.facts.problems, stall_seconds: inp.facts.stall_seconds,
                    held: &inp.facts.held, monitors: &published };
                handle.block_on(classifier.classify(&mut rows, &facts, &io))
            });
            let git_dirs = decorate(&env, &caches, &dirs, &config_dirs, &inp, &mut rows, io.wall(), io.mono(), true);
            (rows, effects, git_dirs, pre)
        }).await.map_err(|e| joined(e, "produção interrompida"))?;
        self.flush_notes();
        self.refresh_git(git_dirs);
        if !input.shadow {
            let demote = self.demote(effects);
            if !demote.is_empty() {
                self.facts.demote(demote);
            }
        }
        let mut rows = rows;
        rows.extend(aside);
        list_facts::mark_stale(&mut rows, &fetched.facts, fetched.ok);
        if !input.shadow {
            self.prune_gone(&rows, Instant::now());
            self.record_live(&rows, fetched.ok);
        }
        let round = keep.then(|| Arc::new(Round { inputs, pre }));
        Ok((Produced { rows: Arc::new(rows), facts: fetched.facts, facts_ok: fetched.ok }, round))
    }

    /// Git em segundo plano, um por pasta: um repositório lento não atrasa o card de ninguém. Não
    /// roda `git` a cada tique: `git::summary` guarda o resultado por pasta por 3 s.
    fn refresh_git(&self, dirs: Vec<String>) {
        for dir in dirs {
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
                    git.insert(dir, (keep(summary, before.0), keep(diff, before.1)));
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
        let c = &self.caches;
        let n = name.to_owned();
        c.resolver.apply({ let n = n.clone(); move |r| r.forget(&n) });
        c.decor.apply({ let n = n.clone(); move |d| { d.context.forget(&n); d.replies.forget(&n); } });
        c.classify.apply(move |k| k.classifier.forget(&n));
    }

    /// Não espera, como `seed`.
    pub fn rename(&self, old: &str, new: &str) {
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

fn expand_home(path: &str, home: &Path) -> PathBuf {
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

/// Retrato do runtime (por chave) no nome de cada linha Claude sem terminal: a linha leva a chave na
/// vida (`k:<chave>`). Linha sem chave fica fora, como sessão parada.
fn headless_by_name(rows: &[SessionRow], by_key: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let mut by_key = by_key;
    rows.iter().filter(|r| r.headless && (r.provider == "claude" || r.provider == "codex")).filter_map(|r| {
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
            rows: &mut [SessionRow], wall: f64, mono: f64, whole: bool) -> Vec<String> {
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
        for row in rows.iter_mut() {
            d.plans.decorate(row, wall, mono);
            if let Some(p) = super::links::fill_loop(row, dirs) {
                list_facts::note("rust.list_discovery", &p.key, p.code.to_owned(), p.reason);
            }
        }
    });
    let git_dirs: Vec<String> = rows.iter().map(|r| git_dir(r).to_owned()).filter(|d| !d.is_empty()).collect();
    let mut git = lock(&caches.git);
    for row in rows.iter_mut() {
        let (summary, diff) = git.get(git_dir(row)).cloned().unwrap_or_default();
        apply_git(row, &summary, &diff);
    }
    // Pasta sem sessão sai: o cache não cresce com cada repositório que já passou pela lista.
    if whole {
        git.retain(|d, _| git_dirs.contains(d));
    }
    git_dirs
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
        let reading = context::claude_reading(&ReadingInputs { jsonl: Path::new(&jsonl), account_dir: &account_dir,
            chosen: chosen.as_deref(), opened: opened.as_deref(), declared, engine: row.engine.is_some() });
        cache.store(&row.name, &jsonl, mono, reading, version)
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
    #[serde(rename = "list.rename")]
    Rename { old: String, new: String },
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
        Operation::Rename { old, new } => cache(Box::new(move |b| b.rename(&old, &new))).await,
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

    #[test]
    fn runtime_view_reaches_codex_headless_rows() {
        let row = |name: &str, provider: &str, headless: bool| -> SessionRow { serde_json::from_value(serde_json::json!({
            "name": name, "provider": provider, "headless": headless, "lifecycle_id": format!("k:{name}")})).unwrap() };
        let rows = [row("cl", "claude", true), row("cx", "codex", true), row("tui", "codex", false)];
        let by_key = ["cl", "cx", "tui"].map(|k| (k.to_owned(), Value::Null)).into_iter().collect();
        assert_eq!(headless_by_name(&rows, by_key).into_keys().collect::<Vec<_>>(), ["cl", "cx"]);
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
