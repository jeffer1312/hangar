//! Fonte de produção do `Monitor`: o hub da sessão (época, conversa, publicação), a captura pelo
//! pool, os fatos do Python (retrato e empurrão), os arquivos da sessão e o ator de entrada.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use hangar_api::ask::AskQuestion;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use hangar_api::preview::PreviewEvent;
use hangar_api::state::StateEvent;
use serde_json::Value;
use tokio::sync::Notify;

use super::agent_pane::AgentPanes;
use super::capture::{MuxProcess, PaneCapture};
use super::edges::RuntimeView;
use super::facts::{Dead, FactsStore, SNAPSHOT_REFRESH, StateFactsClient};
use super::monitor::{CaptureFailed, FileFacts, Frame, LoopInfo, Monitor, RoundFacts, Sources, wall_now};
use super::preview::{HookFile, HookFiles};
use crate::list::bridge::{ListBridge, pid_alive};
use crate::list::facts_files::{self, HookStates};
use crate::runtime::gateway::RuntimeRegistry;
use crate::side::{Hub, SpawnMonitor};
use crate::terminal_control::TerminalPool;
use crate::terminal_state::TerminalQuestion;

/// Leitura dos arquivos da sessão numa rodada; estourou, a rodada segue sem eles e avisa.
const FILES_TIMEOUT: Duration = Duration::from_secs(2);
const ASK_TIMEOUT: Duration = Duration::from_secs(1);
/// Pasta de estado sem observador: marcadores e registro nativo relidos no máximo assim.
const HOOKS_TTL: Duration = Duration::from_millis(250);
/// Pasta sem observador (ainda não existe, recusada): nova tentativa de armar.
const REARM: Duration = Duration::from_secs(30);

/// Marcadores e registro nativo de todas as contas, compartilhados pelos `Monitor`s. Um observador
/// das pastas de estado marca a cópia como velha: N chats abertos não releem pasta nenhuma enquanto
/// nada muda, e a escrita vale já na rodada seguinte (o Python a via pelo inotify).
struct Hooks {
    states: HookStates,
    dirty: Arc<AtomicBool>,
    watcher: Option<RecommendedWatcher>,
    watched: Vec<std::path::PathBuf>,
    /// Alguma pasta sem observador: vale o prazo `HOOKS_TTL`.
    partial: bool,
    armed: Option<Instant>,
    read: Option<Instant>,
}

impl Hooks {
    fn new(states: HookStates) -> Self {
        Self { states, dirty: Arc::new(AtomicBool::new(true)), watcher: None, watched: Vec::new(), partial: true, armed: None, read: None }
    }

    fn fresh(&mut self, config_dirs: &[std::path::PathBuf]) {
        let dirs = facts_files::state_dirs(config_dirs);
        if dirs != self.watched || (self.partial && self.armed.is_none_or(|t| t.elapsed() >= REARM)) {
            self.arm(dirs);
        }
        let due = self.dirty.swap(false, Ordering::SeqCst) || (self.partial && self.read.is_none_or(|t| t.elapsed() >= HOOKS_TTL));
        if due {
            self.states.refresh(config_dirs);
            self.read = Some(Instant::now());
        }
    }

    fn arm(&mut self, dirs: Vec<std::path::PathBuf>) {
        let dirty = self.dirty.clone();
        dirty.store(true, Ordering::SeqCst);
        (self.watched, self.armed, self.partial) = (dirs, Some(Instant::now()), false);
        // O notify assina abrir e fechar: a leitura da própria releitura sujaria a cópia sem fim.
        let made = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(ev) if ev.kind.is_access() => {}
            _ => dirty.store(true, Ordering::SeqCst),
        });
        match made {
            Ok(mut w) => {
                for d in &self.watched {
                    // Pasta do Hangar ausente numa conta que existe: criada, senão ela ficaria relida a cada
                    // `HOOKS_TTL` para sempre. Conta que não existe mais não é recriada.
                    if !d.exists() && d.parent().is_some_and(|p| p.is_dir())
                        && let Err(e) = std::fs::create_dir(d)
                        && e.kind() != std::io::ErrorKind::AlreadyExists
                        && crate::warn_limit::allow(None, "state_hooks_mkdir")
                    {
                        tracing::warn!(code = "state_hooks_mkdir", kind = ?e.kind(), "estado: pasta de estado não criada; relê pelo prazo");
                    }
                    // Pasta que ainda não existe é o caso comum (conta sem hook); vale o prazo.
                    if let Err(e) = w.watch(d, RecursiveMode::NonRecursive) {
                        self.partial = true;
                        // O inotify devolve a pasta ausente como `Io(NotFound)`, não `PathNotFound`.
                        let missing = matches!(&e.kind, notify::ErrorKind::PathNotFound)
                            || matches!(&e.kind, notify::ErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound);
                        if !missing && crate::warn_limit::allow(None, "state_hooks_watch") {
                            tracing::warn!(code = "state_hooks_watch", kind = ?e.kind, "estado: pasta de estado sem observador; relê pelo prazo");
                        }
                    }
                }
                self.watcher = Some(w);
            }
            Err(e) => {
                self.watcher = None;
                self.partial = true;
                if crate::warn_limit::allow(None, "state_hooks_watch") {
                    tracing::warn!(code = "state_hooks_watch", kind = ?e.kind, "estado: pastas de estado sem observador; relê pelo prazo");
                }
            }
        }
    }
}

/// O que todos os `Monitor`s compartilham.
pub struct StateEnv {
    pub pool: TerminalPool,
    pub list: Arc<ListBridge>,
    pub client: StateFactsClient,
    pub diag: crate::diag::DiagClient,
    /// Ligado na subida, quando o servidor tem o runtime (ator de entrada das sessões).
    pub runtime: OnceLock<Arc<RuntimeRegistry>>,
    agents: AgentPanes,
    hooks: Mutex<Hooks>,
}

impl StateEnv {
    pub fn new(pool: TerminalPool, list: Arc<ListBridge>, client: StateFactsClient, diag: crate::diag::DiagClient) -> Self {
        let mut hooks = HookStates::default();
        hooks.set_demoted(list.demoted.clone());
        Self { pool, list, client, diag, runtime: OnceLock::new(), agents: AgentPanes::default(), hooks: Mutex::new(Hooks::new(hooks)) }
    }

    fn facts(&self) -> &FactsStore { &self.list.state_facts }

    fn program(&self) -> std::ffi::OsString { self.list.env().capture_program.clone() }
}

/// A fábrica do hub: um `Monitor` de produção por hub de Claude com terminal e um feed do runtime
/// por hub de Claude ou Codex sem terminal.
pub fn spawner(env: Arc<StateEnv>) -> SpawnMonitor {
    Arc::new(move |hub: &Arc<Hub>| {
        if hub.binding().is_some_and(|b| b.runtime_feed()) {
            let live = env.runtime.get().map(|registry| registry.live(&hub.name));
            let facts = super::runtime_feed::FeedFacts { store: env.list.state_facts.clone(), client: env.client.clone(), diag: env.diag.clone() };
            let feed = super::runtime_feed::RuntimeFeed::new(hub, live, env.list.published.clone(), Some(facts));
            let diag = env.diag.clone();
            return tokio::spawn(super::runtime_feed::guarded(Arc::downgrade(hub), feed.run(), move |name| {
                diag.report("rust.state_feed_failed", name, "state_feed_panic", "o estado da sessão sem terminal caiu; volta com o próximo assinante");
            }));
        }
        let src = LiveSources::new(env.clone(), hub);
        let diag = env.diag.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            let name = src.name.clone();
            // Pânico sem isto sumiria com a tarefa: o hub a dá por acabada e o Python volta a falar.
            match std::panic::AssertUnwindSafe(Monitor::new(src).run()).catch_unwind().await {
                Ok(exit) => tracing::info!(session = name.as_str(), exit = ?exit, "estado: Monitor terminou"),
                Err(_) => {
                    tracing::error!(session = name.as_str(), code = "state_monitor_panic", "estado: Monitor caiu");
                    diag.report("rust.state_monitor_failed", &name, "state_monitor_panic", "o Monitor de estado caiu; volta com o próximo assinante");
                }
            }
        })
    })
}

static NEXT_OWNER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Dono novo no mapa que a lista lê: cresce a cada `Monitor` ou feed.
pub(crate) fn next_owner() -> u64 { NEXT_OWNER.fetch_add(1, Ordering::Relaxed) }

struct CaptureSlot { generation: u64, target: String, capture: Arc<PaneCapture> }

#[derive(Default)]
struct Snapshot { at: Option<Instant>, error: Option<String> }

pub struct LiveSources {
    env: Arc<StateEnv>,
    hub: Weak<Hub>,
    name: String,
    wake: Arc<Notify>,
    hub_wake: Arc<Notify>,
    runtime_wake: Arc<Notify>,
    runtime_view: Arc<Mutex<RuntimeView>>,
    runtime_task: Option<tokio::task::AbortHandle>,
    capture: tokio::sync::Mutex<Option<CaptureSlot>>,
    snapshot: Mutex<Snapshot>,
    hook_files: Arc<Mutex<HookFiles>>,
    /// Dono da entrada deste `Monitor` no mapa que a lista lê (`Published`).
    owner: u64,
}

impl LiveSources {
    pub fn new(env: Arc<StateEnv>, hub: &Arc<Hub>) -> Self {
        let name = hub.name.clone();
        let wake = env.facts().watch(&name);
        let (runtime_wake, runtime_view) = (Arc::new(Notify::new()), Arc::new(Mutex::new(RuntimeView::default())));
        let runtime_task = env.runtime.get().map(|registry| {
            tokio::spawn(watch_runtime(registry.clone(), name.clone(), runtime_view.clone(), runtime_wake.clone())).abort_handle()
        });
        Self { hub_wake: hub.wake(), hub: Arc::downgrade(hub), name, wake, runtime_wake, runtime_view, runtime_task, env,
            capture: tokio::sync::Mutex::new(None), snapshot: Mutex::default(), hook_files: Arc::default(),
            owner: next_owner() }
    }

    fn publish_raw(&self, event: &str, data: &str) -> bool { self.hub.upgrade().is_some_and(|h| h.publish_own(event, data)) }

    /// Pane do agente (cache de 60 s); sem resposta da descoberta, `=nome:` sem guardar.
    async fn target(&self) -> String {
        let fallback = || format!("={}:", self.name);
        if let Some(t) = self.env.agents.cached(&self.name, Instant::now()) {
            return t.unwrap_or_else(fallback);
        }
        match self.env.list.agent_target(&self.name).await {
            Ok(t) => self.env.agents.target(&self.name, Instant::now(), || t).unwrap_or_else(fallback),
            Err(e) => {
                if crate::warn_limit::allow(Some(&self.name), "state_agent_pane_failed") {
                    tracing::warn!(session = self.name.as_str(), code = e.code, "estado: descoberta do pane do agente falhou; usa a janela ativa");
                }
                fallback()
            }
        }
    }

    /// Captura da época atual: troca de época ou de pane recria a fonte (o pool solta o vínculo
    /// anterior do mesmo consumidor sozinho).
    async fn pane(&self) -> Arc<PaneCapture> {
        let generation = self.epoch();
        let target = self.target().await;
        let mut slot = self.capture.lock().await;
        if let Some(s) = slot.as_ref().filter(|s| s.generation == generation && s.target == target) {
            return s.capture.clone();
        }
        let sid = self.sid().unwrap_or_default();
        let capture = Arc::new(PaneCapture::new(self.env.pool.clone(), self.env.program(), &self.name, &sid, target.clone()));
        *slot = Some(CaptureSlot { generation, target, capture: capture.clone() });
        capture
    }

    fn report(&self, event: &'static str, code: &str, reason: &'static str) {
        if crate::warn_limit::allow(Some(&self.name), event) {
            tracing::warn!(session = self.name.as_str(), code, "{reason}");
            self.env.diag.report(event, &self.name, code, reason);
        }
    }
}

impl Drop for LiveSources {
    fn drop(&mut self) {
        if let Some(t) = self.runtime_task.take() {
            t.abort();
        }
        self.env.facts().forget_watcher(&self.name, &self.wake);
        // Sem `Monitor`, a lista volta a classificar sozinha: estado dele parado aqui mentiria.
        self.env.list.published.clear(self.owner, &self.name);
        let slot = self.capture.get_mut().take();
        // O consumidor do pool vive até o fim do aluguel se ninguém o soltar.
        if let (Some(slot), Ok(rt)) = (slot, tokio::runtime::Handle::try_current()) {
            rt.spawn(async move { slot.capture.release().await });
        }
    }
}

impl Sources for LiveSources {
    fn name(&self) -> &str { &self.name }
    fn sid(&self) -> Option<String> { self.hub.upgrade()?.session_key() }
    /// Hub fechado não tem época: `u64::MAX` invalida a rodada, e a publicação seguinte encerra.
    fn epoch(&self) -> u64 { self.hub.upgrade().and_then(|h| h.generation()).unwrap_or(u64::MAX) }
    fn wake(&self) -> Arc<Notify> { self.wake.clone() }
    fn hub_wake(&self) -> Arc<Notify> { self.hub_wake.clone() }

    async fn facts(&self) -> RoundFacts {
        let store = self.env.facts();
        let due = {
            let s = self.snapshot.lock().unwrap();
            s.error.is_some() || s.at.is_none_or(|at| at.elapsed() >= SNAPSHOT_REFRESH)
        } || store.needs_snapshot(&self.name);
        if due {
            let result = self.env.client.snapshot(&self.name).await;
            let mut s = self.snapshot.lock().unwrap();
            match result {
                Ok(facts) => {
                    store.snapshot(&self.name, facts, Instant::now());
                    *s = Snapshot { at: Some(Instant::now()), error: None };
                }
                Err(code) => {
                    drop(s);
                    self.report("rust.state_facts_failed", &code, "estado: retrato dos fatos do Python não veio");
                    s = self.snapshot.lock().unwrap();
                    s.error = Some(code);
                }
            }
        }
        let error = self.snapshot.lock().unwrap().error.clone();
        let received = store.get(&self.name);
        // Retrato que nunca chegou é problema, nunca estado inventado de fatos vazios.
        let unavailable = error.or_else(|| received.is_none().then(|| "state_facts_missing".to_owned()));
        RoundFacts::from_received(received.as_ref(), Instant::now(), unavailable)
    }

    async fn capture(&self) -> Result<Frame, CaptureFailed> { self.pane().await.capture().await }

    async fn has_session(&self) -> Option<bool> {
        MuxProcess::new(self.env.program(), super::capture::TIMEOUT).has_session(&self.name).await
    }

    async fn dead(&self) -> Result<Dead, String> {
        let dead = self.env.client.dead(&self.name).await;
        if matches!(dead, Ok(Dead::Ok)) {
            self.env.agents.forget(&self.name);
        }
        dead
    }

    async fn observe_permission(&self, key: &str, mode: &str) -> Result<(String, String), String> {
        self.env.client.observe_permission(&self.name, key, mode).await
    }

    async fn files(&self, sid: Option<&str>) -> FileFacts {
        let (env, name, sid) = (self.env.clone(), self.name.clone(), sid.map(str::to_owned));
        let read = tokio::task::spawn_blocking(move || read_files(&env, &name, sid.as_deref()));
        match tokio::time::timeout(FILES_TIMEOUT, read).await {
            Ok(Ok(Ok(files))) => files,
            Ok(Ok(Err(code))) => {
                self.report("rust.state_files_failed", code, "estado: arquivos da sessão ilegíveis");
                FileFacts { unavailable: Some(code.to_owned()), ..FileFacts::default() }
            }
            Ok(Err(e)) => {
                let code = if e.is_panic() { "state_files_panic" } else { "state_files_cancelled" };
                self.report("rust.state_files_failed", code, "estado: leitura dos arquivos da sessão caiu");
                FileFacts { unavailable: Some(code.to_owned()), ..FileFacts::default() }
            }
            Err(_) => {
                self.report("rust.state_files_failed", "state_files_timeout", "estado: leitura dos arquivos da sessão passou do prazo");
                FileFacts { unavailable: Some("state_files_timeout".to_owned()), ..FileFacts::default() }
            }
        }
    }

    async fn publish(&self, event: StateEvent) -> bool {
        match serde_json::to_string(&event) {
            Ok(data) => {
                let sid = self.sid();
                let published = &self.env.list.published;
                // Hub fechado não publica, e a lista não pode ficar com o estado que ninguém viu.
                let sent = self.publish_raw("state", &data);
                // `dead` sai do mapa: a lista nunca mostra sessão morta, a linha some com a descoberta.
                if sent && event.state != "dead" {
                    published.set(self.owner, &self.name, sid, Arc::new(event));
                } else {
                    published.clear(self.owner, &self.name);
                }
                sent
            }
            Err(_) => {
                self.report("rust.state_publish_failed", "state_serialize", "estado: evento não serializou");
                true
            }
        }
    }

    async fn emit(&self, event: &'static str, data: Value) -> bool { self.publish_raw(event, &data.to_string()) }

    fn runtime_wake(&self) -> Arc<Notify> { self.runtime_wake.clone() }

    fn runtime_problem(&self) -> Option<(String, String)> {
        super::edges::runtime_problem(&self.runtime_view.lock().unwrap_or_else(|e| e.into_inner()))
    }

    async fn ask_payload(&self) -> Result<Option<AskQuestion>, String> {
        let Some(jsonl) = self.hub.upgrade().and_then(|h| h.jsonl()) else { return Ok(None) };
        match tokio::time::timeout(ASK_TIMEOUT, tokio::task::spawn_blocking(move || super::ask::read_pending(&jsonl))).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("askq_task_failed".to_owned()),
            Err(_) => Err("askq_timeout".to_owned()),
        }
    }

    fn deliverable(&self) {
        let (client, name, diag) = (self.env.client.clone(), self.name.clone(), self.env.diag.clone());
        tokio::spawn(async move {
            if let Err(code) = client.deliverable(&name).await
                && crate::warn_limit::allow(Some(&name), "rust.state_deliver_failed")
            {
                tracing::warn!(session = name.as_str(), code = code.as_str(), "estado: session.deliverable falhou");
                diag.report("rust.state_deliver_failed", &name, &code, "a entrega da fila pedida pelo estado falhou");
            }
        });
    }

    async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { Some(self.capture().await) }

    async fn preview_files(&self, stem: &str) -> Vec<HookFile> {
        let (list, files, stem) = (self.env.list.clone(), self.hook_files.clone(), stem.to_owned());
        let read = tokio::task::spawn_blocking(move || {
            let (dirs, _) = list.state_dirs_blocking().map_err(|e| e.code)?;
            Ok::<_, &'static str>(files.lock().unwrap_or_else(|e| e.into_inner()).read(&dirs, &stem))
        });
        match tokio::time::timeout(FILES_TIMEOUT, read).await {
            Ok(Ok(Ok(files))) => files,
            Ok(Ok(Err(code))) => {
                self.report("rust.state_preview_failed", code, "prévia: pastas das contas indisponíveis");
                Vec::new()
            }
            Ok(Err(_)) | Err(_) => {
                self.report("rust.state_preview_failed", "preview_files_timeout", "prévia: arquivo do hook não foi lido no prazo");
                Vec::new()
            }
        }
    }

    fn committed(&self) -> Option<Arc<str>> { self.hub.upgrade()?.committed() }

    async fn publish_preview(&self, event: PreviewEvent) -> bool {
        match serde_json::to_string(&event) {
            Ok(data) => self.publish_raw("preview", &data),
            Err(_) => {
                self.report("rust.state_publish_failed", "preview_serialize", "prévia: evento não serializou");
                true
            }
        }
    }

    fn wall(&self) -> f64 { wall_now() }
}

/// Marcador, pergunta aberta, statusline publicada, loop e shells da sessão. Lê disco e processos.
fn read_files(env: &StateEnv, name: &str, sid: Option<&str>) -> Result<FileFacts, &'static str> {
    let (config_dirs, dirs) = env.list.state_dirs_blocking().map_err(|e| e.code)?;
    let procs = env.list.env().procs.clone();
    let alive = |pid: i64| pid_alive(&*procs, pid);
    let (marker, shell_pid) = {
        let mut hooks = env.hooks.lock().unwrap_or_else(|e| e.into_inner());
        hooks.fresh(&config_dirs);
        (hooks.states.get_state(sid, alive), hooks.states.shell_pid(sid, alive))
    };
    let open_question = facts_files::open_question(sid, &config_dirs)
        .map(|q| TerminalQuestion { question: Some(q.question), options: q.options });
    let status_line = facts_files::published_status(sid, &config_dirs, wall_now()).map(|p| p.line);
    let mut row = crate::list::links::blank_row(name);
    if let Some(p) = crate::list::links::fill_loop(&mut row, &dirs)
        && crate::warn_limit::allow(Some(name), p.code)
    {
        tracing::warn!(session = name, code = p.code, "estado: sidecar do loop recusado");
    }
    let loop_info = row.loop_status.is_some().then(|| LoopInfo { status: row.loop_status, iter: row.loop_iter, max: row.loop_max });
    let shells = shell_pid.map(|pid| super::shells::shells_of(pid, &*procs)).unwrap_or_default();
    Ok(FileFacts { marker: marker.as_ref().map(|m| m.state.clone()), marker_ts: marker.map(|m| m.ts), open_question, status_line, loop_info, shells, unavailable: None })
}

/// Segue o ator de entrada terminal da sessão: erro e escritor parado viram o problema do estado.
async fn watch_runtime(registry: Arc<RuntimeRegistry>, name: String, view: Arc<Mutex<RuntimeView>>, wake: Arc<Notify>) {
    let mut rx = registry.subscribe();
    let mut keys: HashMap<String, bool> = HashMap::new();
    let set = |next: RuntimeView| {
        let mut v = view.lock().unwrap_or_else(|e| e.into_inner());
        let changed = (v.error.as_deref(), v.message.as_deref(), v.input_stalled.as_deref())
            != (next.error.as_deref(), next.message.as_deref(), next.input_stalled.as_deref());
        *v = next;
        if changed {
            wake.notify_one();
        }
    };
    let from_snapshot = |data: &Value, current: &RuntimeView| {
        let error = data["error"].as_str().map(str::to_owned);
        // A frase veio no `problem`; o retrato só traz o código.
        let message = current.message.clone().filter(|_| error.is_some() && error == current.error);
        RuntimeView { error, message, input_stalled: data["view"]["input_stalled"].as_str().map(str::to_owned) }
    };
    let current = || view.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some((key, data)) = registry.terminal_view(&name).await {
        keys.insert(key, true);
        set(from_snapshot(&data, &current()));
    }
    loop {
        let event = match rx.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                match registry.terminal_view(&name).await {
                    Some((key, data)) => {
                        keys.insert(key, true);
                        set(from_snapshot(&data, &current()));
                    }
                    None => set(RuntimeView::default()),
                }
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::warn!(session = name.as_str(), code = "state_runtime_closed", "estado: eventos do runtime acabaram; o problema da entrada fica no último valor");
                return;
            }
        };
        let mine = match keys.get(&event.key) {
            Some(mine) => *mine,
            // O ator publica antes de entrar no registro: chave ainda sem dono não é guardada.
            None => match registry.terminal_name(&event.key).await {
                None => false,
                Some(owner) => {
                    if keys.len() > 256 {
                        keys.clear();
                    }
                    let mine = owner == name;
                    keys.insert(event.key.clone(), mine);
                    mine
                }
            },
        };
        if !mine {
            continue;
        }
        match event.channel.as_str() {
            "snapshot" => set(from_snapshot(&event.data, &current())),
            "problem" => {
                let cur = current();
                set(RuntimeView { error: event.data["error_code"].as_str().map(str::to_owned),
                    message: event.data["message"].as_str().map(str::to_owned), input_stalled: cur.input_stalled });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_write_is_seen_on_the_next_read_without_rereading_idle_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join(".claude");
        let marker = cfg.join(".hangar-state/s1.json");
        for d in [".hangar-state", "sessions", ".hangar-askq"] {
            std::fs::create_dir_all(cfg.join(d)).unwrap();
        }
        std::fs::write(&marker, r#"{"state": "idle", "ts": 1}"#).unwrap();
        let mut hooks = Hooks::new(HookStates::default());
        let dirs = vec![cfg.clone()];
        hooks.fresh(&dirs);
        assert!(!hooks.partial, "as três pastas existem: todas observadas");
        assert_eq!(hooks.states.get_state(Some("s1"), |_| false).map(|m| m.state).as_deref(), Some("idle"));
        // O FSEvents do macOS entrega com atraso a criação das pastas e do marcador, feita logo antes do
        // observador nascer: espera ele assentar (uma janela sem releitura) antes de exigir nenhuma.
        let deadline = Instant::now() + Duration::from_secs(3);
        let read = loop {
            let read = hooks.read;
            std::thread::sleep(Duration::from_millis(300));
            hooks.fresh(&dirs);
            if hooks.read == read { break read; }
            assert!(Instant::now() < deadline, "o observador não assentou depois da criação das pastas");
        };
        std::thread::sleep(Duration::from_millis(300));
        hooks.fresh(&dirs);
        assert_eq!(hooks.read, read, "nada mudou: nenhuma releitura, nem passado o prazo");
        std::fs::write(&marker, r#"{"state": "working", "ts": 2}"#).unwrap();
        let seen = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            hooks.fresh(&dirs);
            hooks.states.get_state(Some("s1"), |_| false).map(|m| m.state).as_deref() == Some("working")
        });
        assert!(seen, "a escrita suja a cópia e a leitura seguinte a vê");
    }
}
