//! Fontes Rust do Monitor: vínculo do runtime, Store do plugin, arquivos e captura compartilhada.
//! Fatos externos só decoram os recursos que permanecem fora deste recorte.
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
use super::facts::{Dead, FactsStore};
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
    pub diag: crate::diag::DiagClient,
    /// Ligado na subida, quando o servidor tem o runtime (ator de entrada das sessões).
    pub runtime: OnceLock<Arc<RuntimeRegistry>>,
    pub permissions: super::permission::PermissionStore,
    /// Ligação obrigatória da unidade lifecycle; ausência nunca significa limpeza aceita.
    pub lifecycle: Arc<OnceLock<Arc<dyn Lifecycle>>>,
    agents: AgentPanes,
    hooks: Mutex<Hooks>,
}

pub trait Lifecycle: Send + Sync {
    fn dead<'a>(&'a self, name: &'a str, key: &'a str, conversation: &'a str, generation: u64)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Dead, String>> + Send + 'a>>;
}

impl StateEnv {
    pub fn new(pool: TerminalPool, list: Arc<ListBridge>, diag: crate::diag::DiagClient) -> Self {
        let mut hooks = HookStates::default();
        hooks.set_demoted(list.demoted.clone());
        Self { pool, list, diag, runtime: OnceLock::new(), permissions: Arc::default(), lifecycle: Arc::default(), agents: AgentPanes::default(), hooks: Mutex::new(Hooks::new(hooks)) }
    }

    fn facts(&self) -> &FactsStore { &self.list.state_facts }

    fn program(&self) -> std::ffi::OsString { self.list.env().capture_program.clone() }
}

/// A fábrica do hub: um `Monitor` de produção por hub de Claude com terminal e um feed do runtime
/// por hub de Claude ou Codex sem terminal.
pub fn spawner(env: Arc<StateEnv>) -> SpawnMonitor {
    Arc::new(move |hub: &Arc<Hub>| {
        // Sem terminal, ou Codex com terminal cujo ator é do Rust: o feed do runtime.
        if hub.runtime_feed() {
            let live = env.runtime.get().map(|registry| registry.live(&hub.name));
            let facts = super::runtime_feed::FeedFacts { store: env.list.state_facts.clone(), diag: env.diag.clone() };
            let feed = super::runtime_feed::RuntimeFeed::new(hub, live, env.list.published.clone(), Some(facts)).with_registry(env.runtime.get().cloned()).with_lifecycle(env.lifecycle.clone());
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

fn cut_of(view: &crate::mods::state::TerminalView) -> (Option<u32>, Option<String>) {
    let columns = view.crop_columns.filter(|_| view.panes.iter().any(|pane| pane.placement == "dock")).and_then(|c| u32::try_from(c).ok());
    (columns, crate::mods::tree::anchor(&view.above))
}

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
    last_frame: Mutex<Option<(u64, crate::interactions::InteractionBinding, super::permission::MemoryRevision, Frame)>>,
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
            tokio::spawn(watch_runtime(registry.clone(), name.clone(), runtime_view.clone(), runtime_wake.clone(), wake.clone())).abort_handle()
        });
        Self { hub_wake: hub.wake(), hub: Arc::downgrade(hub), name, wake, runtime_wake, runtime_view, runtime_task, env,
            capture: tokio::sync::Mutex::new(None), last_frame: Mutex::default(), hook_files: Arc::default(),
            owner: next_owner() }
    }

    fn publish_raw(&self, event: &str, data: &str) -> bool { self.hub.upgrade().is_some_and(|h| h.publish_own(event, data)) }

    /// Pane do agente (cache de 60 s); sem resposta da descoberta, `=nome:` sem guardar.
    async fn target(&self) -> String {
        if let Some(registry) = self.env.runtime.get()
            && let Ok(Some(target)) = registry.terminal_target(&self.name).await {
            return target.binding.pane;
        }
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
    /// Onde a conversa acaba no pane, dos mods do Rust (`preview.py`: `transcript_columns`, `band_anchor`):
    /// largura só com painel ancorado ao lado, e o começo da faixa.
    fn preview_cut(&self) -> (Option<u32>, Option<String>) {
        let Some(hub) = self.hub.upgrade() else { return (None, None) };
        let mods = hub.mods();
        let Some(view) = mods.life(&self.name).and_then(|life| mods.terminal_view_in(&self.name, life)) else { return (None, None) };
        cut_of(&view)
    }

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
        // Só largura/âncora de mods permanecem externos; pergunta, modo e vida não vêm desse retrato.
        let (body_columns, band_anchor) = self.preview_cut();
        let mut facts = RoundFacts { body_columns, band_anchor, ..Default::default() };
        // Sem entrada no Rust (claude aberto à mão, que só entra no primeiro envio pelo app): o estado sai
        // só do pane e dos arquivos, como no Python; não há vínculo de plugin, pergunta nem modo a ler.
        if let Some(registry) = self.env.runtime.get() && registry.hand_opened(&self.name).await { return facts; }
        let mut local = match self.env.runtime.get() {
            Some(registry) => super::facts::plugin_reading(registry, &self.name).await,
            None => Err("plugin_runtime_absent".into()),
        };
        // Vida fechada com o processo de pé (sessão reaberta): adota o Claude do pane como a lista, com o mesmo teto.
        if local.as_ref().is_err_and(|code| code == "runtime_binding") && let Some(registry) = self.env.runtime.get()
            && crate::list::classify::adoption_due(&self.name, Instant::now()) {
            match registry.adopt_claude_terminal(&self.name).await {
                Ok(()) => local = super::facts::plugin_reading(registry, &self.name).await,
                Err(error) => if crate::warn_limit::allow(Some(&self.name), "state_adopt_failed") {
                    tracing::warn!(session = %self.name, code = %error.code, "estado: adoção do Claude com terminal falhou");
                },
            }
        }
        match local {
            Ok(reading) => {
                let binding = reading.binding;
                facts.permission_op = match self.env.permissions.lock() {
                    Ok(memory) => {
                        facts.permission_revision = Some(memory.revision(&binding.conversation));
                        facts.permission_uncertain = memory.is_uncertain(&binding.conversation);
                        memory.active(&binding.conversation)
                    },
                    Err(_) => { facts.unavailable = Some("permission_store_unavailable".into()); false },
                };
                facts.binding = Some(binding);
                match reading.facts {
                    Ok(local) => {
                        facts.alive = local.alive();
                        facts.plugin_state = local.state();
                        facts.questions = local.questions;
                        facts.suggestion = Some(local.suggestion);
                    }
                    Err(code) => facts.unavailable = Some(code),
                }
            }
            Err(code) => facts.unavailable = Some(code),
        }
        if let (Some(binding), Some(registry), Some(jsonl)) = (facts.binding.clone(), self.env.runtime.get(), self.hub.upgrade().and_then(|h| h.jsonl())) {
            let frame = self.last_frame.lock().ok().and_then(|slot| slot.as_ref().filter(|(epoch, life, _, _)| *epoch == self.epoch() && life == &binding).map(|(_, _, _, frame)| frame.text.clone()));
            if let Some(pane) = frame {
                match registry.plugin() {
                    Ok(store) => {
                        let (store, life) = (store.clone(), binding.clone());
                        match tokio::task::spawn_blocking(move || super::ask::observe_screen_menu(&store, &life, &jsonl, &pane)).await {
                            Ok(Ok((id, plan))) => { facts.native_selector = id; facts.plan_pending = plan; },
                            Ok(Err(code)) => facts.unavailable = Some(code),
                            Err(_) => facts.unavailable = Some("askq_task_failed".into()),
                        }
                    }
                    Err(error) => facts.unavailable = Some(error.code),
                }
                if registry.interaction_binding_readonly(&self.name).await.ok().as_ref() != Some(&binding) {
                    facts = RoundFacts { unavailable: Some("erro_interacao_desatualizada".into()), ..Default::default() };
                }
            }
        }
        facts
    }

    async fn capture(&self) -> Result<Frame, CaptureFailed> {
        let epoch = self.epoch();
        *self.last_frame.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let registry = self.env.runtime.get().ok_or_else(|| CaptureFailed { code: "state_runtime_unbound".into(), attempt: None })?;
        if registry.hand_opened(&self.name).await { return self.pane().await.capture().await; }
        let binding = registry.interaction_binding_readonly(&self.name).await.map_err(|e| CaptureFailed { code: e.code, attempt: None })?;
        if self.sid().as_deref() != Some(binding.conversation.as_str()) { return Err(CaptureFailed { code: "state_binding_changed".into(), attempt: None }); }
        let revision = self.env.permissions.lock().map_err(|_| CaptureFailed { code: "permission_store_unavailable".into(), attempt: None })?
            .revision(&binding.conversation);
        let captured = self.pane().await.capture().await;
        if self.epoch() != epoch || registry.interaction_binding_readonly(&self.name).await.ok().as_ref() != Some(&binding) {
            return Err(CaptureFailed { code: "state_binding_changed".into(), attempt: None });
        }
        *self.last_frame.lock().unwrap_or_else(|e| e.into_inner()) = captured.as_ref().ok().map(|frame| (epoch, binding.clone(), revision, frame.clone()));
        captured
    }

    async fn has_session(&self) -> Option<bool> {
        MuxProcess::new(self.env.program(), super::capture::TIMEOUT).has_session(&self.name).await
    }

    async fn dead(&self) -> Result<Dead, String> {
        let lifecycle = self.env.lifecycle.get().ok_or("state_lifecycle_unbound")?;
        let registry = self.env.runtime.get().ok_or("plugin_runtime_absent")?;
        // `claude` aberto à mão (nunca foi do Rust): o multiplexador sem ela basta, como no Python; não há fila
        // nem vínculo a limpar. A do Rust que perdeu a entrada segue com o código do vínculo.
        if registry.hand_opened(&self.name).await {
            self.env.agents.forget(&self.name);
            return Ok(Dead::Ok);
        }
        let binding = registry.interaction_binding_readonly(&self.name).await.map_err(|e| e.code)?;
        let dead = lifecycle.dead(&self.name, &binding.key, &binding.conversation, binding.generation).await;
        if matches!(dead, Ok(Dead::Ok)) { self.env.agents.forget(&self.name); }
        dead
    }

    async fn observe_permission(&self, key: &str, mode: &str) -> Result<(String, String), String> {
        let registry = self.env.runtime.get().ok_or("state_runtime_unbound")?;
        if registry.hand_opened(&self.name).await {
            // Sem executor que gire o modo, a leitura do rodapé é a confirmação (a memória é por conversa).
            let view = self.env.permissions.lock().map_err(|_| "permission_store_unavailable")?.observe(key, mode);
            return Ok((view.current, view.previous_non_plan));
        }
        let binding = registry.interaction_binding_readonly(&self.name).await.map_err(|e| e.code)?;
        if binding.conversation != key || self.sid().as_deref() != Some(key) { return Err("permission_binding_changed".into()); }
        let revision = self.last_frame.lock().map_err(|_| "permission_frame_unavailable")?.as_ref()
            .filter(|(epoch, life, _, _)| *epoch == self.epoch() && life == &binding).map(|(_, _, revision, _)| *revision)
            .ok_or("permission_binding_changed")?;
        let mut memory = self.env.permissions.lock().map_err(|_| "permission_store_unavailable")?;
        memory.observe_capture(key, mode, revision).map_err(str::to_owned)?;
        let view = memory.confirmed(key).map_err(str::to_owned)?.ok_or("permission_mode_missing")?;
        Ok((view.current, view.previous_non_plan))
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
        let epoch = self.epoch();
        let registry = self.env.runtime.get().ok_or("plugin_runtime_absent")?;
        let binding = registry.interaction_binding_readonly(&self.name).await.map_err(|e| e.code)?;
        let pane = self.last_frame.lock().map_err(|_| "askq_frame_unavailable")?.as_ref()
            .filter(|(generation, life, _, _)| *generation == epoch && life == &binding).map(|(_, _, _, frame)| frame.text.clone()).ok_or("askq_frame_unavailable")?;
        let store = registry.plugin().map_err(|e| e.code)?.clone();
        let Some(jsonl) = self.hub.upgrade().and_then(|h| h.jsonl()) else { return Err("askq_transcript_missing".into()) };
        let (path, life) = (jsonl.clone(), binding.clone());
        let read = tokio::time::timeout(ASK_TIMEOUT, tokio::task::spawn_blocking(move || {
            super::ask::observe_terminal(store.interactions(), &life, &path, &pane)
        })).await;
        let payload = match read {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => return Err("askq_task_failed".into()),
            Err(_) => return Err("askq_timeout".into()),
        };
        if self.epoch() != epoch || registry.interaction_binding_readonly(&self.name).await.map_err(|e| e.code)? != binding
            || self.hub.upgrade().and_then(|h| h.jsonl()).as_ref() != Some(&jsonl) {
            return Err("erro_interacao_desatualizada".into());
        }
        Ok(payload)
    }

    fn deliverable(&self) {
        let Some(registry) = self.env.runtime.get().cloned() else {
            self.report("rust.state_deliver_failed", "state_runtime_unbound", "a entrega não tem runtime ligado");
            return;
        };
        let (name, diag) = (self.name.clone(), self.env.diag.clone());
        tokio::spawn(async move {
            if let Err(error) = registry.drain(&name).await {
                diag.report("rust.state_deliver_failed", &name, &error.code, "a drenagem direta da fila falhou");
            }
        });
    }

    fn trust_dialog(&self) {
        let (name, diag) = (self.name.clone(), self.env.diag.clone());
        tokio::spawn(async move {
            let read = { let name = name.clone(); tokio::task::spawn_blocking(move || {
                let home = std::env::home_dir().unwrap_or_default();
                let spec = match crate::session_open::record::live(&home, &name) {
                    Ok(Some(spec)) => spec,
                    Ok(None) => return ("trust_spec_missing", String::new()),
                    Err(e) => return ("trust_spec_unreadable", e.to_string()),
                };
                let terminal = crate::session_open::terminal::TerminalEnv::local(&home, &std::env::vars().collect());
                let file = crate::session_open::claude::claude_json(&terminal, &home, spec.account.config_dir());
                let (code, detail) = crate::session_open::claude::trust_evidence(&file, &spec.cwd, terminal.windows);
                (code, format!("cwd={} {detail}", spec.cwd))
            }) };
            let (code, detail) = read.await.unwrap_or(("trust_evidence_panic", String::new()));
            tracing::warn!(session = name.as_str(), code, detail, "o Claude pediu para confiar na pasta");
            diag.report("rust.trust_dialog_shown", &name, code, "o Claude pediu para confiar na pasta");
        });
    }

    async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { Some(self.pane().await.capture().await) }

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
async fn watch_runtime(registry: Arc<RuntimeRegistry>, name: String, view: Arc<Mutex<RuntimeView>>, wake: Arc<Notify>, plugin_wake: Arc<Notify>) {
    let mut receiver = registry.terminal_status(&name);
    let mut plugin = registry.plugin().ok().map(|store| store.subscribe());
    // Falha de abertura ou ciclo de vida mora no canal `live`: vale até o ator publicar de novo.
    let mut lifecycle = registry.live(&name).borrow().as_ref().and_then(|state| state.error.clone());
    loop {
        let mut next = receiver.borrow_and_update().clone();
        if let Some((code, message)) = lifecycle.take() && next.error.is_none() {
            next = RuntimeView { error: Some(code), message: Some(message), ..Default::default() };
        }
        {
            let mut current = view.lock().unwrap_or_else(|e| e.into_inner());
            if *current != next {
                *current = next;
                wake.notify_one();
            }
        }
        tokio::select! {
            changed = receiver.changed() => if changed.is_err() {
                tracing::warn!(session = name.as_str(), code = "state_runtime_closed", "estado: canal terminal acabou; o problema da entrada fica no último valor");
                return;
            },
            event = async { match plugin.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                match event {
                    Ok(key) if registry.interaction_binding_readonly(&name).await.is_ok_and(|binding| binding.key == key) => plugin_wake.notify_waiters(),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => plugin_wake.notify_waiters(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => { plugin = None; plugin_wake.notify_waiters(); },
                    _ => {},
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_status_monitor_reads_initial_state_and_ignores_other_sessions() {
        let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "test".into(), "instance".into()));
        let sender = registry.terminal_status_sender("a");
        let initial = RuntimeView { error: Some("queue_io".into()), message: Some("fila indisponível".into()), input_stalled: None, limit: None };
        sender.send_replace(initial.clone());
        let view = Arc::new(Mutex::new(RuntimeView::default()));
        let wake = Arc::new(Notify::new());
        let task = tokio::spawn(watch_runtime(registry.clone(), "a".into(), view.clone(), wake.clone(), Arc::new(Notify::new())));
        tokio::time::timeout(Duration::from_secs(1), wake.notified()).await.unwrap();
        assert_eq!(*view.lock().unwrap(), initial);
        registry.terminal_status_sender("b").send_replace(RuntimeView { input_stalled: Some("composer_busy".into()), ..Default::default() });
        let headless = registry.live("headless");
        assert!(headless.borrow().is_none());
        assert!(tokio::time::timeout(Duration::from_millis(30), wake.notified()).await.is_err(), "outra sessão não acorda este monitor");
        assert_eq!(*view.lock().unwrap(), initial);
        sender.send_replace(RuntimeView::default());
        tokio::time::timeout(Duration::from_secs(1), wake.notified()).await.unwrap();
        assert_eq!(*view.lock().unwrap(), RuntimeView::default());
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn terminal_status_monitor_reconnects_to_the_latest_of_ten_updates() {
        let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "test".into(), "instance".into()));
        let sender = registry.terminal_status_sender("a");
        for i in 0..10 {
            sender.send_replace(RuntimeView { input_stalled: Some(format!("reason-{i}")), ..Default::default() });
        }
        for _ in 0..2 {
            let view = Arc::new(Mutex::new(RuntimeView::default()));
            let wake = Arc::new(Notify::new());
            let task = tokio::spawn(watch_runtime(registry.clone(), "a".into(), view.clone(), wake.clone(), Arc::new(Notify::new())));
            tokio::time::timeout(Duration::from_secs(1), wake.notified()).await.unwrap();
            assert_eq!(view.lock().unwrap().input_stalled.as_deref(), Some("reason-9"));
            task.abort();
            let _ = task.await;
        }
    }

    #[test]
    fn live_sources_do_not_call_migrated_http_policies() {
        let source = include_str!("live.rs").split("#[cfg(test)]").next().unwrap();
        for call in ["client.snapshot(", "client.dead(", "client.observe_permission(", "client.deliverable(", "registry.terminal_view("] {
            assert!(!source.contains(call), "fonte migrada não pode chamar {call}");
        }
        assert!(source.contains("registry.drain(&name)"));
    }

    /// Parity estado M1: a prévia corta no painel ancorado e na faixa dos mods com os dados do Rust.
    #[test]
    fn preview_cut_comes_from_the_rust_mods() {
        use crate::mods::state::{TerminalPane, TerminalView};
        let pane = |placement: &str| TerminalPane { id: "p".into(), title: String::new(), placement: placement.into(), columns: Some(40), tree: serde_json::Value::Null, data: None };
        let view = TerminalView { above: serde_json::json!({"type": "Box", "children": ["Migração do relatório 2/6"]}), columns: Some(80),
            panes: vec![pane("dock")], shown: None, caps: vec![], crop_columns: Some(85) };
        assert_eq!(cut_of(&view), (Some(85), Some("Migração do rela".into())));
        let inline = TerminalView { panes: vec![pane("inline")], ..view };
        assert_eq!(cut_of(&inline).0, None, "só com painel ancorado");
    }

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
