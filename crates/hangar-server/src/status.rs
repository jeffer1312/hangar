//! Tela Status: `GET /api/status` (retrato) e `GET /api/status/events` (evento `status`), só do dono.
//! As leituras e a lista de atenção são do crate `hangar-status`; aqui ficam as rotas, o hub e a coleta
//! das fontes que só o servidor tem (lista, registro, peers, presença, atualização).
//!
//! O hub segue o `ListHub`: um produtor por servidor, ligado só enquanto há tela aberta. Servidor e
//! máquina a cada 5 s, processos e sessões a cada 15 s, peers e atualização a cada 30 s; diário e
//! pastas pelos caches do crate. O amostrador do processo roda sempre (`spawn_sampler`). Bloco que
//! falha sai ausente e vai ao diário uma vez por episódio (`status.bloco_falhou`), nunca à tela.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use hangar_api::status::{
    ConnectState, ExitsBlock, HostBlock, LeftoversBlock, Machine, MachineState, OutsideBlock, ServerBlock, SessionsBlock,
    SlowBlock, StatusSnapshot, UpdateBlock, UpdateResult,
};
use hangar_status::leftovers::MuxSession;
use hangar_status::process::ProcessSampler;
use hangar_status::sessions::{Children, ProcessGroup, QueueWait, Root};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};

use crate::routes::{AppState, cors};
use crate::tail::{comment_frame, ping_frame, sse_frame};

/// Servidor e máquina; a base das outras cadências.
const FAST: Duration = Duration::from_secs(5);
/// Rodadas de `FAST` entre duas leituras de processos e sessões (15 s) e de peers e atualização (30 s).
const MEDIUM_EVERY: u64 = 3;
const SLOW_EVERY: u64 = 6;
const PING_EVERY: Duration = Duration::from_secs(10);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
/// Envio preso por 30 s fecha a conexão, como na lista.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// Quanto o `GET` espera o primeiro retrato quando o produtor estava parado (peers levam até 3 s).
const FIRST_WAIT: Duration = Duration::from_secs(20);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
/// Panes que a descoberta da lista acabou de ler servem; mais velhos, lê de novo.
const PANES_MAX_AGE: Duration = Duration::from_secs(5);
const OWNER_ONLY: (&str, &str) = ("status_owner_only", "só o dono vê o status do servidor");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }

fn stamp(ts: DateTime<Utc>) -> String { ts.to_rfc3339_opts(SecondsFormat::Secs, true) }

/// Leitura de um bloco: `Ok(None)` = sem fonte (bloco ausente, sem aviso); `Err` = a leitura falhou.
pub(crate) type Block<T> = Result<Option<T>, &'static str>;

/// Quais cadências vencem nesta rodada; servidor e máquina entram em todas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Due { pub medium: bool, pub slow: bool }

/// Uma rodada de coleta. Bloco `None` não foi lido agora: fica o último valor.
#[derive(Default)]
pub(crate) struct Reading {
    /// A `package_version` deste servidor, para a lista de atenção.
    pub package_version: Option<String>,
    pub server: Option<Block<ServerBlock>>,
    pub exits: Option<Block<ExitsBlock>>,
    pub update: Option<Block<UpdateBlock>>,
    pub machines: Option<Block<Vec<Machine>>>,
    pub sessions: Option<Block<SessionsBlock>>,
    pub slow: Option<Block<SlowBlock>>,
    pub host: Option<Block<HostBlock>>,
    pub outside: Option<Block<OutsideBlock>>,
    pub leftovers: Option<Block<LeftoversBlock>>,
}

/// Quem lê as fontes; o teste troca por um roteiro.
pub(crate) trait Collect: Send + Sync + 'static {
    fn collect(&self, due: Due) -> BoxFuture<'_, Reading>;
}

/// Aviso de bloco que entrou em falha: (bloco, código).
pub(crate) type Report = Arc<dyn Fn(&'static str, &'static str) + Send + Sync>;

#[derive(Default)]
struct Refs { subscribers: u32, running: bool }

/// O produtor único e o último retrato publicado (JSON pronto).
pub(crate) struct Hub {
    tx: watch::Sender<Option<Arc<str>>>,
    refs: Mutex<Refs>,
    /// Acorda o produtor quando a última tela fecha, sem esperar a cadência.
    idle: Notify,
    /// Blocos em falha agora: um aviso por episódio, não por rodada.
    failing: Mutex<HashSet<&'static str>>,
    reported: AtomicU64,
}

impl Default for Hub {
    fn default() -> Self {
        Self { tx: watch::channel(None).0, refs: Mutex::default(), idle: Notify::new(), failing: Mutex::default(), reported: AtomicU64::new(0) }
    }
}

/// Uma tela aberta (ou um `GET` esperando o retrato); solta a contagem ao cair.
pub(crate) struct Subscription { hub: Arc<Hub>, rx: watch::Receiver<Option<Arc<str>>> }

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut refs = lock(&self.hub.refs);
        refs.subscribers = refs.subscribers.saturating_sub(1);
        if refs.subscribers == 0 { self.hub.idle.notify_one(); }
    }
}

impl Subscription {
    /// O retrato atual, ou o primeiro que o produtor publicar.
    async fn first(&mut self) -> Option<Arc<str>> {
        self.rx.wait_for(Option::is_some).await.ok().and_then(|current| (*current).clone())
    }
}

impl Hub {
    pub(crate) fn subscribe(self: &Arc<Self>, make: impl FnOnce() -> Arc<dyn Collect>, report: Report) -> Subscription {
        let start = {
            let mut refs = lock(&self.refs);
            refs.subscribers += 1;
            !std::mem::replace(&mut refs.running, true)
        };
        if start { tokio::spawn(supervise(self.clone(), make(), report)); }
        Subscription { hub: self.clone(), rx: self.tx.subscribe() }
    }

    #[cfg(test)]
    fn running(&self) -> bool { lock(&self.refs).running }

    /// Leitura que veio troca o bloco; falha o tira e abre um episódio, que fecha na próxima boa.
    fn settle<T>(&self, slot: &mut Option<T>, read: Option<Block<T>>, block: &'static str, report: &Report) {
        match read {
            None => {}
            Some(Ok(value)) => {
                *slot = value;
                lock(&self.failing).remove(block);
            }
            Some(Err(code)) => {
                *slot = None;
                if lock(&self.failing).insert(block) {
                    self.reported.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(block, code, "tela Status: bloco sem leitura");
                    report(block, code);
                }
            }
        }
    }
}

/// Os blocos da última leitura de cada um.
#[derive(Default)]
struct Parts {
    package_version: Option<String>,
    server: Option<ServerBlock>,
    exits: Option<ExitsBlock>,
    update: Option<UpdateBlock>,
    machines: Option<Vec<Machine>>,
    sessions: Option<SessionsBlock>,
    slow: Option<SlowBlock>,
    host: Option<HostBlock>,
    outside: Option<OutsideBlock>,
    leftovers: Option<LeftoversBlock>,
}

impl Parts {
    fn absorb(&mut self, hub: &Hub, reading: Reading, report: &Report) {
        self.package_version = reading.package_version;
        hub.settle(&mut self.server, reading.server, "server", report);
        hub.settle(&mut self.exits, reading.exits, "exits", report);
        hub.settle(&mut self.update, reading.update, "update", report);
        hub.settle(&mut self.machines, reading.machines, "machines", report);
        hub.settle(&mut self.sessions, reading.sessions, "sessions", report);
        hub.settle(&mut self.slow, reading.slow, "slow", report);
        hub.settle(&mut self.host, reading.host, "host", report);
        hub.settle(&mut self.outside, reading.outside, "outside", report);
        hub.settle(&mut self.leftovers, reading.leftovers, "leftovers", report);
    }

    fn snapshot(&self, now: DateTime<Utc>) -> StatusSnapshot {
        let mut snapshot = StatusSnapshot {
            generated_at: stamp(now), server: self.server.clone(), exits: self.exits.clone(), update: self.update.clone(),
            machines: self.machines.clone(), sessions: self.sessions.clone(), slow: self.slow.clone(), host: self.host.clone(),
            outside: self.outside.clone(), leftovers: self.leftovers.clone(), attention: Vec::new(),
        };
        snapshot.attention = hangar_status::attention::attention(&snapshot, self.package_version.as_deref());
        snapshot
    }
}

/// Pânico na coleta não deixa a tela aberta sem retrato: o produtor recomeça.
async fn supervise(hub: Arc<Hub>, collect: Arc<dyn Collect>, report: Report) {
    loop {
        match tokio::spawn(produce(hub.clone(), collect.clone(), report.clone())).await {
            Err(error) if error.is_panic() => {
                report("hub", "status_producer_panic");
                tokio::time::sleep(FAST).await;
            }
            _ => return,
        }
    }
}

/// O laço único. Para quando a última tela fecha; quem abrir depois religa.
async fn produce(hub: Arc<Hub>, collect: Arc<dyn Collect>, report: Report) {
    let mut parts = Parts::default();
    let mut round: u64 = 0;
    loop {
        {
            let mut refs = lock(&hub.refs);
            if refs.subscribers == 0 {
                refs.running = false;
                // Quem abrir depois não recebe primeiro o retrato de quando esta fechou.
                hub.tx.send_replace(None);
                return;
            }
        }
        let due = Due { medium: round % MEDIUM_EVERY == 0, slow: round % SLOW_EVERY == 0 };
        let reading = collect.collect(due).await;
        parts.absorb(&hub, reading, &report);
        match serde_json::to_string(&parts.snapshot(Utc::now())) {
            Ok(json) => { hub.tx.send_replace(Some(json.into())); }
            Err(_) => report("snapshot", "status_unserializable"),
        }
        round = round.wrapping_add(1);
        tokio::select! {
            () = tokio::time::sleep(FAST) => {}
            () = hub.idle.notified() => {}
        }
    }
}

pub(crate) type Maker = Arc<dyn Fn(Arc<AppState>) -> Arc<dyn Collect> + Send + Sync>;

/// O que a tela Status guarda no servidor: o hub, o amostrador sempre ligado e os caches das leituras.
pub struct Status {
    hub: Arc<Hub>,
    sampler: Arc<Mutex<ProcessSampler>>,
    dirs: hangar_status::dirs::DirSizes,
    diary: hangar_status::diary::DiaryReader,
    /// Desde quando cada peer não responde; sai quando ele volta.
    down_since: Mutex<HashMap<String, DateTime<Utc>>>,
    /// O que o diário deixou de fora na última conta, para avisar só quando muda.
    diary_skipped: Mutex<hangar_status::diary::Skipped>,
    make: Maker,
}

impl Default for Status {
    fn default() -> Self { Self::with_collector(Arc::new(|st| Arc::new(Live::new(st)) as Arc<dyn Collect>)) }
}

impl Status {
    pub(crate) fn with_collector(make: Maker) -> Self {
        Self {
            hub: Arc::default(), sampler: Arc::new(Mutex::new(ProcessSampler::new())),
            dirs: hangar_status::dirs::DirSizes::new(), diary: hangar_status::diary::DiaryReader::machine(),
            down_since: Mutex::default(), diary_skipped: Mutex::default(), make,
        }
    }

    fn subscribe(&self, st: &Arc<AppState>) -> Subscription {
        let diag = st.diag.clone();
        let report: Report = Arc::new(move |block, code| {
            diag.report_episode("status.bloco_falhou", block, code, "bloco da tela Status sem leitura", json!({}), true);
        });
        let (make, st) = (self.make.clone(), st.clone());
        self.hub.subscribe(move || make(st), report)
    }
}

/// O amostrador do processo, ligado desde a subida: uma leitura agora e uma por minuto.
pub fn spawn_sampler(st: &AppState) -> tokio::task::JoinHandle<()> {
    let sampler = st.status.sampler.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(hangar_status::process::TICK);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            let sampler = sampler.clone();
            if tokio::task::spawn_blocking(move || lock(&sampler).tick()).await.ok() != Some(true) {
                tracing::debug!(code = "status_sample_missed", "leitura do processo do servidor não veio");
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Rotas

fn refused() -> Response { crate::session_write::detail(StatusCode::FORBIDDEN, OWNER_ONLY.0, OWNER_ONLY.1, json!({})) }

/// Só o dono. Convidado (convite, par ou login) leva 403 sem contar falha de senha; token errado, a
/// recusa contada. O pedido é lido antes da espera: `&Request` atravessando um `await` tira o `Send`.
fn admit(st: &AppState, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let guest = req.extensions().get::<crate::login_guests::gate::Login>().is_some()
        || matches!(crate::share::caller::caller(req), Some(crate::share::caller::Caller::Guest(_)));
    let owner = st.edge().owner_only(peer, req, OWNER_ONLY);
    async move {
        if guest { return Err(refused()); }
        owner.await
    }
}

/// `GET /api/status`: o retrato atual; com a tela fechada, liga o produtor até o primeiro.
pub async fn snapshot(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let mut response = match admit(&st, peer, &req).await {
        Err(response) => response,
        Ok(()) => {
            let mut subscription = st.status.subscribe(&st);
            match tokio::time::timeout(FIRST_WAIT, subscription.first()).await {
                Ok(Some(json)) => {
                    let mut response = Response::new(Body::from(json.to_string()));
                    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
                    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                    response
                }
                _ => crate::session_write::detail(StatusCode::SERVICE_UNAVAILABLE, "status_unavailable",
                    "o retrato do status não ficou pronto", json!({})),
            }
        }
    };
    cors(&headers, response.headers_mut());
    response
}

/// `GET /api/status/events`: `status` a cada retrato novo, `ping` a cada 10 s.
pub async fn events(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(mut response) = admit(&st, peer, &req).await {
        cors(&headers, response.headers_mut());
        return response;
    }
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    tokio::spawn(client_loop(st.status.subscribe(&st), tx));
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (Ok::<Bytes, Infallible>(b), rx)) });
    let mut response = Response::new(Body::from_stream(stream));
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(&headers, h);
    response
}

async fn push(out: &mpsc::Sender<Bytes>, frame: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, out.send(frame)).await, Ok(Ok(())))
}

async fn client_loop(mut subscription: Subscription, out: mpsc::Sender<Bytes>) {
    // O ping sai antes do retrato: a primeira rodada pode esperar os peers.
    if !push(&out, ping_frame()).await { return; }
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING_EVERY, PING_EVERY);
    let mut comment = tokio::time::interval_at(start + COMMENT_EVERY, COMMENT_EVERY);
    loop {
        let current = subscription.rx.borrow_and_update().clone();
        if let Some(json) = current && !push(&out, sse_frame("status", &json, None)).await { return; }
        tokio::select! {
            () = out.closed() => return,
            _ = ping.tick() => if !push(&out, ping_frame()).await { return },
            _ = comment.tick() => if !push(&out, comment_frame()).await { return },
            changed = subscription.rx.changed() => if changed.is_err() { return },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Coleta do servidor de verdade

/// As fontes do servidor, por vida do produtor: a assinatura interna da lista e o `System` dos
/// processos ficam enquanto a tela está aberta.
struct Live {
    st: Arc<AppState>,
    /// Mantém a lista do dono produzindo; as linhas saem dela sem recalcular.
    list: crate::list::hub::InternalSubscription,
    /// O mesmo `System` entre rodadas: a CPU de cada processo é a diferença desde a anterior.
    procs: Arc<Mutex<Option<sysinfo::System>>>,
    /// O Funnel só é relido na cadência lenta (é um processo do `tailscale`).
    funnel: Mutex<Option<bool>>,
    /// Restos do multiplexador vistos na rodada anterior; `None` antes da primeira ou com ele fora.
    tmux_seen: Mutex<Option<HashSet<String>>>,
}

impl Live {
    fn new(st: Arc<AppState>) -> Self {
        let list = st.hub.subscribe_internal(&st.list, &st.devices, &st.diag);
        Self { st, list, procs: Arc::default(), funnel: Mutex::new(None), tmux_seen: Mutex::new(None) }
    }

    fn server(&self) -> Block<ServerBlock> {
        let (version, commit) = match crate::update::running_package() {
            Some(package) => (package.version.clone(), Some(package.commit.clone()).filter(|c| !c.is_empty())),
            None => (env!("CARGO_PKG_VERSION").to_owned(), None),
        };
        Ok(lock(&self.st.status.sampler).block(version, commit))
    }

    /// Diário (saídas e lentidão) e máquina, numa ida só ao pool bloqueante.
    async fn diary_and_host(&self, now: DateTime<Utc>) -> (Block<ExitsBlock>, Block<SlowBlock>, Block<HostBlock>) {
        let st = self.st.clone();
        let read = tokio::task::spawn_blocking(move || {
            let status = &st.status;
            let diary = diary_readable(hangar_core::diag::diary_dir().as_deref(), hangar_core::paths::log_base().as_deref());
            let (exits, slow) = match diary {
                Err(code) => (Err(code), Err(code)),
                Ok(()) => (Ok(status.diary.exits(now)), Ok(status.diary.slow(now))),
            };
            let skipped = status.diary.skipped();
            let sizes = status.dirs.current();
            // Sem casa ou sem disco que a contenha a máquina não foi lida: é falha, não "sem fonte".
            let host = hangar_status::host::read_host(sizes.hangar, sizes.build_cache).map(Some).ok_or("status_host_unread");
            (exits, slow, skipped, host)
        }).await;
        let Ok((exits, slow, skipped, host)) = read else {
            return (Err("status_task_failed"), Err("status_task_failed"), Err("status_task_failed"));
        };
        let changed = {
            let mut last = lock(&self.st.status.diary_skipped);
            std::mem::replace(&mut *last, skipped) != skipped
        };
        if changed && (skipped.lines > 0 || skipped.files > 0) {
            self.st.diag.report_with("status.diario_ignorado", "", "status_diary_skipped",
                "linhas ou arquivos do diário fora das contas da tela Status", json!({"lines": skipped.lines, "files": skipped.files}));
        }
        (exits, slow, host)
    }

    async fn outside(&self, slow: bool) -> OutsideBlock {
        if slow {
            let funnel = match &self.st.share {
                // Sem convites não há Funnel do Hangar nesta máquina.
                None => None,
                Some(_) => match crate::share::tunnel::funnel_on(&crate::share::tunnel::Real).await {
                    Ok(on) => Some(on),
                    Err(fix) => {
                        if !crate::share::tunnel::confirmed_absent(&fix) {
                            tracing::debug!(code = "status_funnel_unread", "tela Status: Funnel sem resposta do tailscale");
                        }
                        None
                    }
                },
            };
            *lock(&self.funnel) = funnel;
        }
        // `running` do supervisor é a supervisão ligada pela pessoa; `enabled`, os dois filhos de pé.
        let connect = self.st.connect_supervisor.status();
        let keyring_locked = match self.st.list.keyring.current() {
            crate::keyring::KeyringState::Locked => Some(true),
            crate::keyring::KeyringState::Unlocked => Some(false),
            crate::keyring::KeyringState::Absent => None,
        };
        OutsideBlock { connect: Some(ConnectState { enabled: connect.running, running: connect.enabled }),
            funnel: *lock(&self.funnel), keyring_locked }
    }

    async fn update(&self) -> Block<UpdateBlock> {
        let Some(body) = crate::update::http::status_body(&self.st, false).await else { return Ok(None) };
        Ok(update_block(&body, crate::update::periodic::last_check()))
    }

    /// Linhas do dono: as da assinatura interna; antes da primeira produção dela, o retrato da lista.
    async fn rows(&self) -> Option<Arc<Vec<SessionRow>>> {
        if let Some(rows) = self.list.rows() { return Some(rows); }
        self.st.list.snapshot().await.ok().filter(|produced| !produced.facts.unknown).map(|produced| produced.rows)
    }

    async fn sessions_and_leftovers(&self, now: DateTime<Utc>) -> (Block<SessionsBlock>, Block<LeftoversBlock>) {
        const NO_LIST: &str = "status_list_unavailable";
        let Some(rows) = self.rows().await else { return (Err(NO_LIST), Err(NO_LIST)) };
        let env = self.st.list.env().clone();
        let panes = match env.mux.shared(PANES_MAX_AGE) {
            Some(panes) => Some(panes),
            None => match env.mux.list_panes().await {
                Ok(panes) => Some(Arc::new(panes)),
                Err(error) => {
                    tracing::warn!(code = error.code, "tela Status: panes do multiplexador sem leitura");
                    None
                }
            },
        };
        let mut agents = Vec::new();
        for row in rows.iter().filter(|row| !row.headless) {
            let agent = match self.st.list.discovered_session(&row.name).await {
                Ok(found) => Ok(found.and_then(|(_, pid)| pid)),
                Err(error) => {
                    tracing::warn!(session = %row.name, code = error.code, "tela Status: agente da sessão sem leitura");
                    Err(())
                }
            };
            agents.push((row.name.clone(), row.provider.clone(), agent));
        }
        let roots = terminal_roots(panes.as_deref().map(Vec::as_slice), &agents);
        let queue_files = match self.st.state.runtime.get() {
            Some(registry) => registry.queue_files().await,
            None => BTreeMap::new(),
        };
        let working: HashSet<String> = rows.iter().filter(|row| row.state == "working").map(|row| row.name.clone()).collect();
        let hangar = env.dirs.as_ref().map_or_else(|| self.st.hangar_dir.clone(), |dirs| dirs.home.join(".hangar"));
        let home = env.dirs.as_ref().map(|dirs| dirs.home.clone());
        let (procs, rows_owned) = (self.procs.clone(), rows.clone());
        let pane_pids: Vec<u32> = panes.iter().flat_map(|panes| panes.iter()).filter_map(|pane| pane.pid).collect();
        let counted = tokio::task::spawn_blocking(move || {
            let mut all: Vec<SessionRow> = rows_owned.iter().cloned().collect();
            if let Some(home) = &home {
                let live: Vec<&SessionRow> = rows_owned.iter().collect();
                all.extend(crate::session_open::fallen::rows(home, &live));
            }
            let (mut bound, mut headless) = (HashSet::new(), HashMap::new());
            sidecar_roots(&hangar, &mut headless, &mut bound);
            prewarm_canos(&hangar.join("prewarm"), &mut bound);
            let waits = queue_files.into_values().filter_map(|(name, path, created)| Some((name, queue_wait(&path, created)?))).collect();
            let queues = without_working_age(waits, &working);
            let mut guard = lock(&procs);
            let system = guard.get_or_insert_with(|| {
                // A CPU de um processo só vale na segunda leitura.
                let mut system = sysinfo::System::new();
                refresh(&mut system);
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                system
            });
            refresh(system);
            let children = children_of(system);
            let block = roots.map(|mut roots| {
                // Depois dos panes: as raízes das sessões sem terminal e o app-server do Codex com terminal.
                for (name, list) in headless { roots.entry(name).or_default().extend(list); }
                let facts = hangar_status::sessions::Facts { rows: &all, pids: &roots, tmux_server: tmux_server(system, &pane_pids),
                    children: &children, queues: &queues, now: now.timestamp_millis() as f64 / 1000.0 };
                hangar_status::sessions::sessions_block(&facts, &*system)
            });
            let canos = hangar_status::leftovers::canos_in(system, u64::try_from(now.timestamp()).unwrap_or(0));
            let listed: HashSet<String> = all.iter().map(|row| row.name.clone()).collect();
            (block, canos, bound, listed)
        }).await;
        let Ok((block, canos, bound, listed)) = counted else { return (Err("status_task_failed"), Err("status_task_failed")) };
        let tmux = self.tmux_orphans(&listed).await;
        let tmux = {
            let mut seen = lock(&self.tmux_seen);
            let confirmed = tmux.as_deref().and_then(|now| confirmed_orphans(seen.as_ref(), now));
            *seen = tmux.map(|now| now.into_iter().collect());
            confirmed
        };
        // Na subida, cano de sessão que ainda vai ser religada não é resto.
        let canos = self.st.startup.is_ready().then(|| hangar_status::leftovers::cano_orphans(&canos, &bound));
        (block.map(Some), Ok(hangar_status::leftovers::block(tmux, canos)))
    }

    /// Sessões do multiplexador abertas pelo Hangar sem linha na lista. `CP_PROVIDER` só vale
    /// confirmado na própria sessão (o formato cai no ambiente global); multiplexador fora: `None`.
    async fn tmux_orphans(&self, listed: &HashSet<String>) -> Option<Vec<String>> {
        let env = self.st.list.env();
        let mut command = crate::terminal_input::child_command(&env.capture_program);
        command.args(["list-sessions", "-F", "#{session_name}\t#{@cp_hidden}\t#{@cp_shortcut_key}"])
            .stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(5), command.output()).await.ok()?.ok()?;
        if !out.status.success() {
            return crate::list::mux::is_absence(&String::from_utf8_lossy(&out.stderr)).then(Vec::new);
        }
        let mut sessions = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut fields = line.split('\t');
            let name = fields.next().unwrap_or_default();
            if name.is_empty() { continue; }
            let hidden = fields.next().is_some_and(|v| !v.is_empty() && v != "0");
            let shortcut = fields.next().is_some_and(|v| !v.is_empty());
            let candidate = !hidden && !shortcut && !listed.contains(name);
            let hangar = candidate && env.mux.session_env(name, "CP_PROVIDER").await.is_some();
            sessions.push(MuxSession { name: name.to_owned(), hangar, hidden, shortcut });
        }
        Some(hangar_status::leftovers::tmux_orphans(&sessions, listed))
    }

    /// Esta máquina, os peers do `peers.json` e os apps nativos que reportam aqui.
    async fn machines(&self, ours: Option<&str>, now: DateTime<Utc>) -> Block<Vec<Machine>> {
        let service = crate::machine_peers::Service::from_state(&self.st).map_err(|_| "status_peers_unavailable")?;
        let peers = peer_entries(&service.store.tolerant());
        let probes = futures_util::future::join_all(peers.iter().map(|peer| {
            let service = &service;
            async move { if peer.enabled { Some(probe(service, peer).await) } else { None } }
        })).await;
        let mut apps = crate::presence::apps(std::time::Instant::now());
        let mut take_app = |id: &str| apps.iter().position(|app| app.machine_id.as_deref() == Some(id)).map(|i| apps.remove(i).report);
        let this_id = Some(self.st.current_server_id()).filter(|id| !id.is_empty())
            .or_else(sysinfo::System::host_name).unwrap_or_default();
        let mut machines = vec![Machine { id: this_id.clone(), label: this_id.clone(), this_machine: true, state: MachineState::Ok,
            down_since: None, latency_ms: None, server_version: ours.map(str::to_owned), outdated: false, lan: false,
            app: take_app(this_id.as_str()) }];
        let mut down = lock(&self.st.status.down_since);
        for (peer, probe) in peers.iter().zip(probes) {
            let state = match &probe {
                None => MachineState::Unknown,
                Some(probe) if probe.up => MachineState::Ok,
                Some(_) => MachineState::Down,
            };
            let down_since = match state {
                MachineState::Down => Some(stamp(*down.entry(peer.id.clone()).or_insert(now))),
                MachineState::Ok => { down.remove(&peer.id); None }
                MachineState::Unknown => None,
            };
            let server_version = probe.as_ref().and_then(|probe| probe.version.clone());
            let outdated = server_version.as_deref().zip(ours).is_some_and(|(theirs, ours)| hangar_status::attention::older(theirs, ours));
            machines.push(Machine { id: peer.id.clone(), label: peer.label.clone(), this_machine: false, state, down_since,
                latency_ms: probe.as_ref().filter(|probe| probe.up).and_then(|probe| probe.ms), server_version, outdated,
                lan: probe.as_ref().is_some_and(|probe| probe.up && lan_address(&peer.base_url)),
                app: take_app(peer.id.as_str()) });
        }
        // Máquina que saiu do `peers.json` não guarda hora de queda.
        down.retain(|id, _| peers.iter().any(|peer| peer.id == *id));
        drop(down);
        // App sem servidor conhecido aqui vira cartão só de app.
        machines.extend(apps.into_iter().map(|app| Machine { id: format!("app:{}", app.device), label: app.report.host.clone(),
            this_machine: false, state: MachineState::Unknown, down_since: None, latency_ms: None, server_version: None,
            outdated: false, lan: false, app: Some(app.report) }));
        Ok(Some(machines))
    }
}

impl Collect for Live {
    fn collect(&self, due: Due) -> BoxFuture<'_, Reading> {
        Box::pin(async move {
            let now = Utc::now();
            let package_version = crate::update::running_package().map(|package| package.version.clone());
            let medium = async { if due.medium { Some(self.sessions_and_leftovers(now).await) } else { None } };
            let slow = async {
                if due.slow { Some(tokio::join!(self.machines(package_version.as_deref(), now), self.update())) } else { None }
            };
            let (((exits, slow_requests, host), outside), medium, slow) =
                tokio::join!(async { tokio::join!(self.diary_and_host(now), self.outside(due.slow)) }, medium, slow);
            let (sessions, leftovers) = medium.map_or((None, None), |(s, l)| (Some(s), Some(l)));
            let (machines, update) = slow.map_or((None, None), |(m, u)| (Some(m), Some(u)));
            Reading {
                package_version: package_version.clone(), server: Some(self.server()), exits: Some(exits), update, machines, sessions,
                slow: Some(slow_requests), host: Some(host), outside: Some(Ok(Some(outside))), leftovers,
            }
        })
    }
}

/// O bloco Atualização a partir do corpo do `GET /api/atualizacao`.
fn update_block(body: &Value, last_check: Option<DateTime<Utc>>) -> Option<UpdateBlock> {
    let installed = body["versao_legivel"]["repo"].as_str().or_else(|| body["versoes"]["repo"].as_str())?.to_owned();
    let available = body["atualizacao_disponivel"].as_bool().unwrap_or(false)
        .then(|| body["versao_legivel"]["remoto"].as_str().map(str::to_owned)).flatten();
    let estado = &body["estado"];
    let last_result = (estado["fase"] == "pronto").then(|| {
        let ok = estado["ok"].as_bool()?;
        let ts = DateTime::parse_from_rfc3339(estado["ts"].as_str()?).ok()?.with_timezone(&Utc);
        Some(UpdateResult { ts: stamp(ts), ok, step: (!ok).then(|| estado["etapa"].as_str().map(str::to_owned)).flatten() })
    }).flatten();
    Some(UpdateBlock { channel: body["canal"].as_str().unwrap_or_default().to_owned(), installed, available,
        last_check: last_check.map(stamp), last_result })
}

struct PeerEntry { id: String, label: String, base_url: String, token: String, enabled: bool }

/// Máquinas do `peers.json` com endereço; `enabled: false` fica sem sonda (estado desconhecido).
fn peer_entries(book: &serde_json::Map<String, Value>) -> Vec<PeerEntry> {
    let mut peers: Vec<PeerEntry> = book.iter().filter_map(|(id, cfg)| {
        let base_url = cfg.get("base_url")?.as_str().filter(|url| !url.is_empty())?.trim_end_matches('/').to_owned();
        let text = |key: &str| cfg.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned);
        Some(PeerEntry { id: id.clone(), label: text("label").unwrap_or_else(|| id.clone()), base_url,
            token: text("token").unwrap_or_default(), enabled: cfg.get("enabled").and_then(Value::as_bool) != Some(false) })
    }).collect();
    peers.sort_by(|a, b| a.id.cmp(&b.id));
    peers
}

struct Probe { up: bool, ms: Option<u32>, version: Option<String> }

/// O endereço que o `check` sondou é da rede local: IP privado ou de enlace, ou nome `.local`. O
/// Tailscale (100.64.0.0/10, `*.ts.net`) e o Connect não são.
fn lan_address(url: &str) -> bool {
    let Some(host) = reqwest::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_ascii_lowercase)) else { return false };
    match host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_unique_local() || ip.is_unicast_link_local(),
        Err(_) => host.trim_end_matches('.').ends_with(".local"),
    }
}

/// O mesmo `check` de `/api/peers/check` (tempo e identidade, também pelo Connect); de pé, a versão
/// sai da saúde dele. Recusa de credencial ainda é máquina de pé.
async fn probe(service: &crate::machine_peers::Service, peer: &PeerEntry) -> Probe {
    let checked = crate::machine_peers::network::check(service, &peer.base_url, &peer.id).await;
    let estado = checked["estado"].as_str().unwrap_or_default();
    let up = matches!(estado, "ok" | "recusou");
    let ms = checked["tempo_ms"].as_u64().and_then(|ms| u32::try_from(ms).ok());
    let version = if estado == "ok" { peer_version(&peer.base_url, &peer.token).await } else { None };
    Probe { up, ms, version }
}

/// `package_version` da saúde do peer; ausente no modo desenvolvedor dele. Sem seguir redirect: o
/// token é daquela máquina.
async fn peer_version(base_url: &str, token: &str) -> Option<String> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    let client = CLIENT.get_or_init(|| crate::http_client().timeout(PEER_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none()).build().ok()).as_ref()?;
    let mut request = client.get(format!("{base_url}/__hangar_server/health"));
    if !token.is_empty() { request = request.bearer_auth(token); }
    let response = request.send().await.ok()?;
    if !response.status().is_success() { return None; }
    let body: Value = serde_json::from_slice(&response.bytes().await.ok()?).ok()?;
    body["package_version"].as_str().filter(|v| !v.is_empty()).map(str::to_owned)
}

fn refresh(system: &mut sysinfo::System) {
    // O usuário de cada processo: sem ele, o cano de outro usuário da máquina entraria nos restos.
    let kind = sysinfo::ProcessRefreshKind::nothing().with_cpu().with_memory().with_user(sysinfo::UpdateKind::OnlyIfNotSet);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, kind);
}

fn children_of(system: &sysinfo::System) -> Children {
    let mut children: Children = HashMap::new();
    for (pid, process) in system.processes() {
        if process.thread_kind().is_some() { continue; }
        if let Some(parent) = process.parent() {
            children.entry(i64::from(parent.as_u32())).or_default().push(i64::from(pid.as_u32()));
        }
    }
    children
}

/// O servidor do tmux é o pai dos panes; no psmux não há um processo só.
fn tmux_server(system: &sysinfo::System, pane_pids: &[u32]) -> Option<u32> {
    if cfg!(windows) { return None; }
    let parent = system.process(sysinfo::Pid::from_u32(*pane_pids.first()?))?.parent()?;
    let name = system.process(parent)?.name().to_string_lossy().into_owned();
    (ProcessGroup::from_executable(&name) == Some(ProcessGroup::Tmux)).then_some(parent.as_u32())
}

/// Os panes de cada sessão e, depois deles, o agente: a pilha da soma visita primeiro o agente, que
/// leva o grupo do provedor mesmo com o executável chamado pela versão. Sem a lista de panes ou com
/// um agente que não foi lido, os números sairiam menores que os reais: o bloco falha.
fn terminal_roots(panes: Option<&[crate::list::mux::Pane]>, agents: &[(String, String, Result<Option<u32>, ()>)])
    -> Result<HashMap<String, Vec<Root>>, &'static str> {
    let panes = panes.ok_or("status_mux_unavailable")?;
    let unread = agents.iter().filter(|(_, _, agent)| agent.is_err()).count();
    if unread > 0 {
        tracing::warn!(code = "status_agent_unread", unread, "tela Status: agentes sem leitura");
        return Err("status_agent_unread");
    }
    let mut roots: HashMap<String, Vec<Root>> = HashMap::new();
    for pane in panes {
        if let Some(pid) = pane.pid { roots.entry(pane.session.clone()).or_default().push(Root { pid, group: None }); }
    }
    for (name, provider, agent) in agents {
        if let Ok(Some(pid)) = agent {
            roots.entry(name.clone()).or_default().push(Root { pid: *pid, group: ProcessGroup::of_provider(provider) });
        }
    }
    Ok(roots)
}

/// A idade da fila só conta com a sessão parada: numa rodada longa a mensagem só espera a vez dela.
fn without_working_age(waits: Vec<(String, QueueWait)>, working: &HashSet<String>) -> Vec<QueueWait> {
    waits.into_iter().map(|(name, wait)| if working.contains(&name) { QueueWait { oldest_ts: None, ..wait } } else { wait }).collect()
}

/// Resto só visto em duas rodadas seguidas: sessão recém-aberta ainda sem linha na lista não conta.
/// Sem rodada anterior, ainda não se sabe.
fn confirmed_orphans(previous: Option<&HashSet<String>>, now: &[String]) -> Option<Vec<String>> {
    let previous = previous?;
    Some(now.iter().filter(|name| previous.contains(*name)).cloned().collect())
}

/// O diário pode ser lido? Desligado (`diary_dir` sem pasta com a base dos logs conhecida) é `Ok`
/// e os blocos saem ausentes pelo leitor; sem base dos logs ou pasta ilegível é falha.
fn diary_readable(dir: Option<&Path>, log_base: Option<&Path>) -> Result<(), &'static str> {
    match dir {
        None if log_base.is_none() => Err("status_diary_dir_unknown"),
        None => Ok(()),
        Some(dir) => match std::fs::read_dir(dir) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err("status_diary_unreadable"),
            _ => Ok(()),
        },
    }
}

/// Raízes das sessões sem terminal e os canos ligados, pelos sidecars do Claude e do Codex.
fn sidecar_roots(hangar: &Path, roots: &mut HashMap<String, Vec<Root>>, bound: &mut HashSet<u32>) {
    let mut problems = crate::list::discover::Problems::default();
    for (dir, provider) in [("claude-headless", "claude"), ("codex-sessions", "codex")] {
        let found = crate::list::discover_other::sidecars(&hangar.join(dir), "sidecar lido pela tela Status", &mut problems);
        for (_, meta) in found.iter() {
            let Some(name) = meta.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { continue };
            let pid = |value: Option<&Value>| value.and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()).filter(|p| *p != 0);
            let list = roots.entry(name.to_owned()).or_default();
            if let Some(cano) = pid(meta.get("cano").and_then(|cano| cano.get("pid"))) {
                bound.insert(cano);
                list.push(Root { pid: cano, group: Some(ProcessGroup::Cano) });
            }
            // Depois do cano: visitados antes dele, levam o próprio grupo.
            if let Some(agent) = pid(meta.get("agent_pid")) { list.push(Root { pid: agent, group: ProcessGroup::of_provider(provider) }); }
            if provider == "codex" && let Some(app) = pid(meta.get("app_pid")) {
                list.push(Root { pid: app, group: Some(ProcessGroup::CodexAppServer) });
            }
        }
    }
}

/// Canos das reservas pré-aquecidas: ainda sem sidecar, mas com dono.
fn prewarm_canos(dir: &Path, bound: &mut HashSet<u32>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|x| x == "json")) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(receipt) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(pid) = receipt["cano"]["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) { bound.insert(pid); }
    }
}

/// Quantas esperam e desde quando, pelo arquivo de estado da fila; ilegível fica de fora.
fn queue_wait(path: &Path, created: f64) -> Option<QueueWait> {
    let bytes = crate::runtime::queue::read_replaced(path).ok()?;
    let (state, _) = crate::runtime::queue::State::load(&bytes).ok()?;
    let waiting = crate::runtime::queue::waiting_ids(&state, created).len();
    Some(QueueWait { waiting: u32::try_from(waiting).unwrap_or(u32::MAX), oldest_ts: crate::runtime::queue::oldest_waiting(&state, created) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Roteiro: a rodada `n` falha o bloco Máquina quando `fails[n]`; o resto vem com um Servidor fixo.
    struct Script { rounds: AtomicUsize, fails: Vec<bool> }

    impl Script {
        fn new(fails: Vec<bool>) -> Arc<Self> { Arc::new(Self { rounds: AtomicUsize::new(0), fails }) }
        fn rounds(&self) -> usize { self.rounds.load(Ordering::SeqCst) }
    }

    fn server_block() -> ServerBlock {
        ServerBlock { version: "0.1.0.10824".into(), commit: None, pid: 7, started_at: "2026-11-17T10:00:00Z".into(), rss_bytes: 1,
            cpu_pct: None, rss_series: vec![], cpu_series: vec![], rss_avg_bytes: None, cpu_avg_pct: None }
    }

    impl Collect for Script {
        fn collect(&self, _due: Due) -> BoxFuture<'_, Reading> {
            let round = self.rounds.fetch_add(1, Ordering::SeqCst);
            let fail = self.fails.get(round).copied().unwrap_or(false);
            Box::pin(async move {
                Reading { package_version: Some("0.1.0.10824".into()), server: Some(Ok(Some(server_block()))),
                    host: Some(if fail { Err("status_test_failed") } else { Ok(None) }), ..Reading::default() }
            })
        }
    }

    fn quiet() -> Report { Arc::new(|_, _| {}) }

    async fn settle_tasks() {
        for _ in 0..20 { tokio::task::yield_now().await; }
    }

    #[tokio::test(start_paused = true)]
    async fn status_hub_runs_only_while_someone_watches() {
        let hub = Arc::new(Hub::default());
        let script = Script::new(vec![true, true, true, false, true]);
        assert!(!hub.running(), "sem tela, sem produtor");

        let make = { let script = script.clone(); move || script as Arc<dyn Collect> };
        let mut first = hub.subscribe(make, quiet());
        assert!(hub.running(), "liga com o primeiro assinante");
        assert!(first.first().await.is_some());
        let second = hub.subscribe(|| unreachable!("o produtor já está ligado"), quiet());

        while script.rounds() < 5 { tokio::time::sleep(Duration::from_secs(1)).await; }
        assert_eq!(hub.reported.load(Ordering::Relaxed), 2, "três rodadas em falha são um episódio; a quinta abre outro");

        drop(first);
        settle_tasks().await;
        assert!(hub.running(), "ainda há uma tela aberta");
        drop(second);
        settle_tasks().await;
        assert!(!hub.running(), "para com o último, sem esperar a cadência");
        let stopped_at = script.rounds();
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(script.rounds(), stopped_at, "parado não coleta");
        assert!(hub.tx.borrow().is_none(), "quem abrir depois não recebe o retrato antigo");

        let again = Script::new(vec![]);
        let mut back = hub.subscribe({ let again = again.clone(); move || again as Arc<dyn Collect> }, quiet());
        assert!(back.first().await.is_some());
        assert!(hub.running() && again.rounds() >= 1, "quem abre depois religa");
    }

    #[tokio::test]
    async fn status_snapshot_keeps_the_last_reading_and_drops_a_failing_block() {
        let hub = Hub::default();
        let mut parts = Parts::default();
        let host = Some(Ok(Some(hangar_api::status::HostBlock {
            mem_total: 1, mem_available: 1, swap_total: 0, swap_used: 0, load: None, cpus: 1,
            disk_home: hangar_api::status::DiskUse { used: 95, total: 100 }, disk_tmp: None,
            dir_hangar: hangar_api::status::DirSize { bytes: None, computing: true },
            dir_build_cache: hangar_api::status::DirSize { bytes: None, computing: true },
        })));
        parts.absorb(&hub, Reading { host, server: Some(Ok(Some(server_block()))), ..Reading::default() }, &quiet());
        let snapshot = parts.snapshot(Utc::now());
        assert!(snapshot.host.is_some() && snapshot.server.is_some());
        assert_eq!(snapshot.attention.iter().map(|a| a.code.as_str()).collect::<Vec<_>>(), vec!["disk_full"]);

        parts.absorb(&hub, Reading::default(), &quiet());
        assert!(parts.snapshot(Utc::now()).host.is_some(), "bloco não lido na rodada fica como estava");
        parts.absorb(&hub, Reading { host: Some(Err("status_test_failed")), ..Reading::default() }, &quiet());
        let snapshot = parts.snapshot(Utc::now());
        assert!(snapshot.host.is_none() && snapshot.attention.is_empty(), "bloco que falha sai ausente, sem ponto inventado");
        assert!(snapshot.server.is_some());
    }

    #[test]
    fn lan_comes_from_the_probed_address() {
        for url in ["http://192.168.1.20:8765", "http://10.0.0.5:8765", "http://172.20.1.1", "http://169.254.3.4:8765",
            "http://[fd12::1]:8765", "http://notebook.local:8765"] {
            assert!(lan_address(url), "{url}");
        }
        for url in ["http://100.64.0.2:8765", "https://maquina.exemplo.ts.net", "https://casa.exemplo.hangar.dev.br",
            "http://127.0.0.1:8765", "http://8.8.8.8", "não é url"] {
            assert!(!lan_address(url), "{url}");
        }
    }

    #[test]
    fn status_update_block_comes_from_the_update_screen_body() {
        let body = json!({"modo": "pacote", "canal": "latest", "versoes": {"repo": "0.1.0.10824", "backend": "0.1.0.10824"},
            "versao_legivel": {"repo": "0.1.0.10824", "backend": "0.1.0.10824", "remoto": "0.1.0.10833"},
            "atualizacao_disponivel": true,
            "estado": {"fase": "pronto", "ok": false, "etapa": "baixar", "ts": "2026-11-17T09:40:00-03:00"}});
        let checked = DateTime::parse_from_rfc3339("2026-11-17T14:11:05Z").unwrap().with_timezone(&Utc);
        assert_eq!(update_block(&body, Some(checked)), Some(UpdateBlock { channel: "latest".into(), installed: "0.1.0.10824".into(),
            available: Some("0.1.0.10833".into()), last_check: Some("2026-11-17T14:11:05Z".into()),
            last_result: Some(UpdateResult { ts: "2026-11-17T12:40:00Z".into(), ok: false, step: Some("baixar".into()) }) }));

        let running = json!({"canal": "latest", "versao_legivel": {"repo": "0.1.0.10824", "remoto": "0.1.0.10824"},
            "atualizacao_disponivel": false, "estado": {"fase": "rodando", "ok": null}});
        let block = update_block(&running, None).unwrap();
        assert_eq!((block.available, block.last_check, block.last_result), (None, None, None));
    }

    #[test]
    fn status_peers_without_an_address_are_left_out_and_disabled_ones_are_not_probed() {
        let book = json!({
            "vps": {"base_url": "https://vps.example/", "token": "t", "enabled": false},
            "casa": {"base_url": "http://100.64.0.2:8765", "token": "t", "label": "Casa"},
            "app-only": {"app": {}},
        });
        let peers = peer_entries(book.as_object().unwrap());
        let seen: Vec<(&str, &str, &str, bool)> = peers.iter().map(|p| (p.id.as_str(), p.label.as_str(), p.base_url.as_str(), p.enabled)).collect();
        assert_eq!(seen, vec![("casa", "Casa", "http://100.64.0.2:8765", true), ("vps", "vps", "https://vps.example", false)]);
    }

    fn pane(session: &str, pid: u32) -> crate::list::mux::Pane {
        crate::list::mux::Pane { session: session.into(), pid: Some(pid), ..Default::default() }
    }

    #[test]
    fn status_sessions_fail_instead_of_undercounting() {
        let agent = |name: &str, read: Result<Option<u32>, ()>| (name.to_owned(), "claude".to_owned(), read);
        assert_eq!(terminal_roots(None, &[]), Err("status_mux_unavailable"), "sem panes, nenhuma sessão com terminal entra");
        assert_eq!(terminal_roots(Some(&[pane("a", 10)]), &[agent("a", Ok(Some(11))), agent("b", Err(()))]),
            Err("status_agent_unread"), "um agente sem leitura derruba o bloco");

        let roots = terminal_roots(Some(&[pane("a", 10)]), &[agent("a", Ok(Some(11))), agent("b", Ok(None))]).unwrap();
        assert_eq!(roots["a"], vec![Root { pid: 10, group: None }, Root { pid: 11, group: Some(ProcessGroup::Claude) }],
            "o agente vem depois do pane");
        assert!(!roots.contains_key("b"), "sessão sem agente achado não inventa raiz");
    }

    #[test]
    fn status_queue_age_only_counts_for_a_stopped_session() {
        let wait = QueueWait { waiting: 1, oldest_ts: Some(1_000.0) };
        let working = HashSet::from(["ocupada".to_owned()]);
        let queues = without_working_age(vec![("ocupada".into(), wait), ("parada".into(), wait)], &working);
        assert_eq!(queues, vec![QueueWait { waiting: 1, oldest_ts: None }, wait], "a mensagem conta, a idade não");

        let rows: Vec<SessionRow> = ["ocupada", "parada"].iter().map(|name| serde_json::from_value(json!({
            "name": name, "provider": "claude", "state": if *name == "ocupada" { "working" } else { "idle" }})).unwrap()).collect();
        struct Nothing;
        impl hangar_status::sessions::ProcessReading for Nothing {
            fn read(&self, _pid: u32) -> Option<hangar_status::sessions::ProcStat> { None }
        }
        let facts = hangar_status::sessions::Facts { rows: &rows, pids: &HashMap::new(), tmux_server: None,
            children: &HashMap::new(), queues: &queues[..1], now: 1_000.0 + 3_600.0 };
        let block = hangar_status::sessions::sessions_block(&facts, &Nothing);
        assert_eq!((block.queued, block.oldest_queued_secs), (1, None), "rodada longa não acende fila parada");
    }

    #[test]
    fn status_tmux_leftover_needs_two_rounds() {
        assert_eq!(confirmed_orphans(None, &["x".to_owned()]), None, "primeira rodada ainda não sabe");
        let previous = HashSet::from(["x".to_owned(), "y".to_owned()]);
        assert_eq!(confirmed_orphans(Some(&previous), &["x".to_owned(), "z".to_owned()]), Some(vec!["x".to_owned()]));
    }

    #[test]
    fn status_diary_unreadable_is_a_failure_and_off_is_not() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(diary_readable(None, None), Err("status_diary_dir_unknown"));
        assert_eq!(diary_readable(None, Some(base.path())), Ok(()), "desligado");
        assert_eq!(diary_readable(Some(&base.path().join("ainda-nao-existe")), Some(base.path())), Ok(()));
        let file = base.path().join("arquivo");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(diary_readable(Some(&file), Some(base.path())), Err("status_diary_unreadable"), "pasta que não abre");
    }

    fn route_state(script: Arc<Script>) -> Arc<AppState> {
        let mut state = AppState::new(crate::config::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
        });
        state.status = Arc::new(Status::with_collector(Arc::new(move |_: Arc<AppState>| script.clone() as Arc<dyn Collect>)));
        Arc::new(state)
    }

    fn get(path: &str, token: Option<&str>) -> Request {
        let mut builder = Request::builder().uri(path);
        if let Some(token) = token { builder = builder.header("authorization", format!("Bearer {token}")); }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn status_route_owner_gets_c1_guest_is_refused_and_failure_is_reported_once() {
        let st = route_state(Script::new(vec![true, true, true]));
        let peer: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let response = snapshot(State(st.clone()), ConnectInfo(peer), get("/api/status", Some("fixture-owner"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let raw = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let parsed: StatusSnapshot = serde_json::from_slice(&raw).expect("o corpo é o contrato C1");
        assert!(parsed.server.is_some(), "bloco lido sai");
        assert!(parsed.host.is_none(), "bloco que falha sai ausente");
        assert_eq!(st.status.hub.reported.load(Ordering::Relaxed), 1);

        let again = snapshot(State(st.clone()), ConnectInfo(peer), get("/api/status", Some("fixture-owner"))).await;
        assert_eq!(again.status(), StatusCode::OK);
        assert_eq!(st.status.hub.reported.load(Ordering::Relaxed), 1, "a mesma falha não vai ao diário de novo");

        let mut guest = get("/api/status", Some("guest-token"));
        guest.extensions_mut().insert(crate::share::caller::Caller::Guest(Arc::new(crate::share::caller::GuestGrant {
            token: "guest-token".into(), token_hash: String::new(), sessions: HashMap::new(), pair_roots: vec![], invite: true })));
        let refused = snapshot(State(st.clone()), ConnectInfo(peer), guest).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let raw = axum::body::to_bytes(refused.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&raw).unwrap()["detail"]["code"], "status_owner_only");

        let mut guest_events = get("/api/status/events", Some("guest-token"));
        guest_events.extensions_mut().insert(crate::share::caller::Caller::Guest(Arc::new(crate::share::caller::GuestGrant {
            token: "guest-token".into(), token_hash: String::new(), sessions: HashMap::new(), pair_roots: vec![], invite: false })));
        assert_eq!(events(State(st.clone()), ConnectInfo(peer), guest_events).await.status(), StatusCode::FORBIDDEN);

        let anonymous = snapshot(State(st), ConnectInfo(peer), get("/api/status", None)).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    }
}
