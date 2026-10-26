//! Estado ao vivo de Claude e Codex sem terminal: o hub publica o último valor que o ator escreveu no canal
//! em processo (`RuntimeRegistry::live`), coalescido, no lugar do `Monitor` e sem passar pelo Python.
use std::sync::{Arc, Weak};
use std::time::Duration;

use hangar_api::preview::PreviewEvent;
use hangar_api::state::StateEvent;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::runtime::protocol::LiveReceiver;
use crate::side::Hub;
use crate::state::facts::{FactsStore, SNAPSHOT_REFRESH};
use crate::state::published::Published;

/// Mesma janela da lista: rajada de deltas vira uma publicação.
pub const COALESCE: Duration = Duration::from_millis(150);
/// Leitura do plugin que falhou: tenta de novo sem martelar o Store.
const RETRY: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(5) };

/// Ligação do feed do Claude aos fatos: a leitura vem do `plugin::Store` do Rust; `store` só acorda o feed
/// quando a ponte da lista recebe um empurrão.
#[derive(Clone)]
pub struct FeedFacts { pub store: Arc<FactsStore>, pub diag: crate::diag::DiagClient }

struct Watching { src: FeedFacts, at: Option<Instant>, error: Option<String> }

/// Quanto falta para reler o retrato.
fn refresh_in(at: Option<Instant>, failed: bool, now: Instant) -> Duration {
    let every = if failed { RETRY } else { SNAPSHOT_REFRESH };
    at.map_or(Duration::ZERO, |at| every.saturating_sub(now.saturating_duration_since(at)))
}

pub struct RuntimeFeed {
    hub: Weak<Hub>,
    name: String,
    wake: Arc<Notify>,
    live: Option<LiveReceiver>,
    published: Arc<Published>,
    owner: u64,
    /// Só o Claude tem plugin: o Codex não pede fatos.
    facts: Option<Watching>,
    /// Aviso de empurrão aceito; sem fatos, um que nunca dispara.
    facts_wake: Arc<Notify>,
    /// Último dado publicado de cada evento, na época `generation`: troca de ligação republica tudo.
    sent: Sent,
    claude: bool,
    /// Sem pane: o estado parado sai com `headless: true`; o do Codex com terminal, com `false`.
    headless: bool,
    /// Estado da sessão Claude parada, lido do sidecar quando o feed acorda com ela parada.
    parked: Parked,
    /// Pasta do sidecar; `None` = o HOME do processo.
    home: Option<std::path::PathBuf>,
    registry: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
    plugin_events: Option<tokio::sync::broadcast::Receiver<String>>,
    plugin_reading: Option<crate::state::facts::PluginReading>,
    source_binding: Option<(u64, crate::side::Binding)>,
    last_binding: Option<crate::interactions::InteractionBinding>,
    lifecycle: Arc<std::sync::OnceLock<Arc<dyn super::live::Lifecycle>>>,
}

/// O que a sessão Claude parada publica: o estado do sidecar (ou `dead`), nada (troca de modo em curso, o
/// Python também não emite) ou o `idle` puro.
enum Parked { Absent, State(StateEvent), Hold }

#[derive(Default)]
struct Sent { generation: Option<u64>, question: Option<String>, state: Option<String>, preview: Option<String>, thinking: Option<String>, tool: Option<String>, suggestion: Option<String> }

/// Problema do ator, como o `runtime_problem` do Python: `<código>: <frase>` com teto de 300.
fn actor_problem(code: &str, message: &str) -> String {
    let text = if message.is_empty() { code.to_owned() } else { format!("{code}: {message}") };
    text.chars().take(300).collect()
}

/// `headless` diz se há pane: o Codex com terminal também chega aqui quando o ator dele é do Rust.
fn idle(name: &str, headless: bool) -> StateEvent { StateEvent { session: name.into(), state: "idle".into(), headless, ..Default::default() } }

/// Sem terminal: Claude sem terminal, ou Codex cujo sidecar não tem pane.
fn binding_headless(hub: &Hub) -> bool {
    hub.binding().is_none_or(|b| b.provider == crate::transcript::Provider::ClaudeHeadless || b.headless)
}

/// Problema do próprio feed. Sai como `runtime_falhou`, o código que web, app e nativo traduzem;
/// o motivo vai no detalhe. Código novo sem tradução some da tela do web.
fn feed_problem(name: &str, headless: bool, code: &str, message: &str) -> StateEvent {
    StateEvent { problema: Some("runtime_falhou".into()), problema_detalhe: Some(actor_problem(code, message)), ..idle(name, headless) }
}

impl RuntimeFeed {
    /// `live`: `None` quando o servidor não tem o runtime ligado.
    pub fn new(hub: &Arc<Hub>, live: Option<LiveReceiver>, published: Arc<Published>, facts: Option<FeedFacts>) -> Self {
        let name = hub.name.clone();
        let claude = hub.binding().is_some_and(|b| b.provider == crate::transcript::Provider::ClaudeHeadless);
        let facts = facts.filter(|_| claude);
        let facts_wake = facts.as_ref().map_or_else(Arc::default, |f| f.store.watch(&name));
        Self { hub: Arc::downgrade(hub), name, wake: hub.wake(), live, published, owner: super::live::next_owner(),
            facts: facts.map(|src| Watching { src, at: None, error: None }), facts_wake, sent: Sent::default(),
            claude, headless: binding_headless(hub), parked: Parked::Absent, home: None,
            registry: None, plugin_events: None, plugin_reading: None, source_binding: None, last_binding: None, lifecycle: Arc::default() }
    }

    pub fn with_registry(mut self, registry: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>) -> Self {
        self.plugin_events = registry.as_ref().and_then(|r| r.plugin().ok()).map(|store| store.subscribe());
        self.registry = registry;
        self
    }

    pub fn with_lifecycle(mut self, lifecycle: Arc<std::sync::OnceLock<Arc<dyn super::live::Lifecycle>>>) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    #[cfg(test)]
    fn with_home(mut self, home: &std::path::Path) -> Self {
        self.home = Some(home.to_path_buf());
        self
    }

    /// Sessão Claude parada: o que o sidecar diz dela. Roda quando o feed acorda, não por tique, e fora do executor.
    async fn load_parked(&self) -> Parked {
        if !self.claude || !self.live.as_ref().is_some_and(|rx| rx.borrow().is_none()) {
            return Parked::Absent;
        }
        let Some(home) = self.home.clone().or_else(std::env::home_dir) else { return Parked::Absent };
        let name = self.name.clone();
        let read = tokio::task::spawn_blocking(move || super::parked::parked_state_at(&name, &home)).await.ok().flatten();
        if let Some(state) = read { return Parked::State(state); }
        let Some(lifecycle) = self.lifecycle.get() else {
            return Parked::State(feed_problem(&self.name, self.headless, "state_lifecycle_unbound", "o encerramento da vida não tem controlador ligado"));
        };
        let Some(binding) = self.last_binding.as_ref() else {
            return Parked::State(feed_problem(&self.name, self.headless, "state_lifecycle_binding_missing", "a vida ausente não tem vínculo comprovado"));
        };
        match lifecycle.dead(&self.name, &binding.key, &binding.conversation, binding.generation).await {
            Ok(super::facts::Dead::Ok) => Parked::State(StateEvent { state: "dead".into(), ..idle(&self.name, self.headless) }),
            Ok(super::facts::Dead::InTransfer) => Parked::Hold,
            Err(code) => Parked::State(feed_problem(&self.name, self.headless, &code, "o encerramento da vida não foi confirmado")),
        }
    }

    pub async fn run(mut self) {
        loop {
            // Arma o aviso antes de ler os fatos: empurrão no meio da rodada não se perde.
            let wake = self.facts_wake.clone();
            let mut pushed = std::pin::pin!(wake.notified());
            pushed.as_mut().enable();
            let retry_in = self.refresh_facts().await;
            self.parked = self.load_parked().await;
            if !self.round().await {
                return;
            }
            self.wait(pushed, retry_in).await;
            tokio::time::sleep(COALESCE).await;
        }
    }

    /// Relê o retrato dos fatos quando vence (o que renova o interesse no Python), quando faltou
    /// sequência ou depois de falha; devolve em quanto tempo precisa de novo.
    async fn refresh_facts(&mut self) -> Option<Duration> {
        let hub = self.hub.upgrade()?;
        let source = hub.source_binding()?;
        self.source_binding = Some(source.clone());
        if self.facts.is_none() { return None; }
        let read = match &self.registry {
            Some(registry) => crate::state::facts::plugin_reading(registry, &self.name).await,
            None => Err("state_runtime_unbound".into()),
        };
        let w = self.facts.as_mut()?;
        w.at = Some(Instant::now());
        if hub.source_binding().as_ref() != Some(&source)
            || read.as_ref().is_ok_and(|reading| reading.binding.conversation != source.1.key) {
            self.plugin_reading = None;
            w.error = Some("state_plugin_binding_changed".into());
            return Some(RETRY);
        }
        match read {
            Ok(reading) => {
                self.last_binding = Some(reading.binding.clone());
                w.error = reading.facts.as_ref().err().cloned();
                self.plugin_reading = Some(reading);
            }
            Err(code) => { self.plugin_reading = None; w.error = Some(code); },
        }
        Some(refresh_in(w.at, w.error.is_some(), Instant::now()))
    }

    /// Acorda pelo canal do ator, pelo hub (resposta gravada, religação), por empurrão de fatos ou
    /// pelo prazo de reler o retrato.
    async fn wait(&mut self, mut pushed: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>, retry_in: Option<Duration>) {
        let mut timer = std::pin::pin!(async { match retry_in { Some(d) => tokio::time::sleep(d).await, None => std::future::pending().await } });
        loop {
            let live_closed = tokio::select! {
                closed = async { match self.live.as_mut() { Some(rx) => rx.changed().await.is_err(), None => std::future::pending().await } } => closed,
                _ = self.wake.notified() => false,
                _ = &mut pushed => false,
                _ = &mut timer => false,
                event = async { match self.plugin_events.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                    match event {
                        Ok(key) if self.plugin_reading.as_ref().is_none_or(|r| r.binding.key == key) => false,
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => false,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => { self.plugin_events = None; false },
                    }
                },
            };
            if !live_closed {
                return;
            }
            // Registro sumiu (servidor encerrando): ficam o hub, os fatos e o prazo.
            self.live = None;
        }
    }

    /// Publica o que mudou; `false` com o hub fechado.
    async fn round(&mut self) -> bool {
        let Some(hub) = self.hub.upgrade() else { return false };
        let Some(source) = hub.source_binding() else { return false };
        let generation = source.0;
        if self.source_binding.is_none() && self.facts.is_none() { self.source_binding = Some(source.clone()); }
        if self.source_binding.as_ref() != Some(&source) {
            self.plugin_reading = None;
            self.wake.notify_one();
            return true;
        }
        if self.sent.generation != Some(generation) {
            self.sent = Sent { generation: Some(generation), ..Sent::default() };
        }
        let runtime_binding = if self.claude {
            match &self.registry {
                Some(registry) => match registry.interaction_binding_readonly(&self.name).await {
                    Ok(binding) if binding.conversation == source.1.key => Some(binding),
                    _ => {
                        if crate::warn_limit::allow(Some(&self.name), "state_feed_binding_unavailable") {
                            tracing::warn!(session = self.name.as_str(), code = "state_feed_binding_unavailable", "estado: vínculo da rodada não confirmado");
                        }
                        None
                    }
                },
                None => None,
            }
        } else { None };
        let value = self.live.as_mut().map(|rx| rx.borrow_and_update().clone());
        let observed_binding = value.as_ref().and_then(|live| live.as_ref()).and_then(|live| live.binding.as_ref());
        let interaction_unavailable = self.claude && self.registry.is_some() && runtime_binding.is_none();
        let display_binding_confirmed = if interaction_unavailable {
            match (&self.registry, observed_binding) {
                (Some(registry), Some(binding)) if binding.conversation == source.1.key => {
                    let snapshots = registry.list_snapshots_readonly().await;
                    snapshots.get(&binding.key).is_some_and(|snapshot| snapshot["key"] == binding.key
                        && snapshot["generation"].as_u64() == Some(binding.generation)
                        && snapshot["view"]["conversation"].as_str() == Some(binding.conversation.as_str())
                        && snapshot["view"]["public_state"].is_object())
                }
                _ => false,
            }
        } else { true };
        if interaction_unavailable { self.plugin_reading = None; }
        if self.plugin_reading.as_ref().is_some_and(|reading| runtime_binding.as_ref().is_some_and(|binding| binding != &reading.binding)) {
            self.plugin_reading = None;
            self.wake.notify_one();
            return true;
        }
        let live_binding_changed = runtime_binding.as_ref().is_some_and(|binding|
            value.as_ref().and_then(|live| live.as_ref()).is_some_and(|live| live.binding.as_ref() != Some(binding)));
        if live_binding_changed { self.plugin_reading = None; }
        // Sem estado provado, nada sai: a tela fica com o último, sem `idle` inventado, e o motivo vai ao diário.
        let unproven = |code: &str, message: &str| {
            crate::problem_gate::report_hidden(hub.diag(), &self.name, code, Some(message));
            true
        };
        let (mut state, texts) = match &value {
            None => return unproven("runtime_absent", "o servidor não tem o runtime ligado"),
            // Sessão parada (não aberta no Rust, encerrada, ou abrindo): `idle`, como o Python. Abertura
            // que falhou e vida que acabou com erro chegam como `Some` com o erro.
            // Claude parado leva o que o sidecar diz dele; sem sidecar (ou Codex) fica o `idle` puro.
            Some(None) => match &self.parked {
                Parked::State(state) => (state.clone(), Default::default()),
                Parked::Hold => return true,
                Parked::Absent => (idle(&self.name, self.headless), Default::default()),
            },
            Some(Some(live)) if live_binding_changed || interaction_unavailable && (!display_binding_confirmed || live.binding.as_ref().is_none_or(|binding| binding.conversation != source.1.key)) => {
                // Erro da vida desta conversa (ou de antes do vínculo) é falha real e sai; o de outra conversa, não.
                let own = live.binding.as_ref().is_none_or(|binding| binding.conversation == source.1.key);
                let state = if !live_binding_changed && own && let Some((code, message)) = &live.error {
                    feed_problem(&self.name, self.headless, code, message)
                } else {
                    return unproven("state_feed_binding_unavailable", "o estado não prova a conversa atual");
                };
                (state, Default::default())
            }
            Some(Some(live)) => {
                let mut state = if live.public_state.is_null() { idle(&self.name, self.headless) } else {
                    match serde_json::from_value(live.public_state.clone()) {
                        Ok(state) => state,
                        Err(_) => return unproven("state_feed_invalid", "a vista do ator não é um estado"),
                    }
                };
                if let Some((code, message)) = &live.error {
                    state.problema = Some("runtime_falhou".into());
                    state.problema_detalhe = Some(actor_problem(code, message));
                }
                (state, [live.preview.clone(), live.thinking.clone(), live.tool.clone()])
            }
        };
        // Como no `Monitor`: sem retrato, a sugestão e a sessão parada podem estar velhas, e a tela diz isso.
        // Sessão parada (sem ator) não tem vínculo nem plugin a ler: é o normal dela, não estado velho.
        let parked = matches!(value, Some(None));
        if let Some(code) = self.facts.as_ref().and_then(|w| w.error.as_ref()).filter(|code| !(parked && *code == "runtime_binding")) {
            if state.problema.is_none() {
                state.problema = Some(super::facts::UNAVAILABLE.into());
                state.problema_detalhe = Some(code.clone());
            }
        }
        // Sem terminal, o cartão é só o do canal nativo (que o motor responde): o `ask:`/`perm:` do plugin
        // seria respondido com 409, e o `ask.ts` encerra a própria pergunta quando o nativo vence.
        let [preview, thinking, tool] = texts;
        // A prévia daqui é o texto exato do bloco, não a tela pintada: o começo da resposta gravada é ela
        // mesma lida antes do commit, em qualquer tamanho.
        let preview = match hub.committed() {
            Some(committed) if super::preview::is_committed(&preview, &committed) => String::new(),
            Some(committed) if { let n = super::preview::norm(&preview); !n.is_empty() && committed.starts_with(&n) } => String::new(),
            _ => preview,
        };
        // A sugestão só muda por fato novo; vazia desde o início não é mudança.
        let suggestion = self.plugin_reading.as_ref().and_then(|r| r.facts.as_ref().ok()).map(|r| r.suggestion.clone());
        if let Some(text) = suggestion.filter(|t| t != self.sent.suggestion.as_deref().unwrap_or("")) {
            if !self.emit(&hub, &source, runtime_binding.as_ref(), "suggest", serde_json::json!({"text":text}).to_string(), |s| &mut s.suggestion).await { return hub.generation().is_some(); }
        }
        // Mesma ordem do `sse.py`: a pergunta sai antes do estado que a abre.
        let question = serde_json::to_string(&state.codex_question).unwrap_or_else(|_| "null".into());
        if !interaction_unavailable && !live_binding_changed && !self.emit(&hub, &source, runtime_binding.as_ref(), "ask_question", question, |s| &mut s.question).await { return hub.generation().is_some(); }
        let data = serde_json::to_string(&state).unwrap_or_default();
        if self.sent.state.as_deref() != Some(data.as_str()) {
            let sid = source.1.jsonl.file_stem().and_then(|s| s.to_str()).map(str::to_owned);
            let (published, owner, name) = (self.published.clone(), self.owner, self.name.clone());
            if !self.emit_with(&hub, &source, runtime_binding.as_ref(), "state", data, |s| &mut s.state,
                move || published.set(owner, &name, sid, Arc::new(state))).await { return hub.generation().is_some(); }
        }
        let preview = serde_json::to_string(&PreviewEvent { session: self.name.clone(), text: preview, md: true, full: true, vivo: true }).unwrap_or_default();
        let sent = self.emit(&hub, &source, runtime_binding.as_ref(), "preview", preview, |s| &mut s.preview).await
            && self.emit(&hub, &source, runtime_binding.as_ref(), "pensamento", serde_json::json!({"text": thinking}).to_string(), |s| &mut s.thinking).await
            && self.emit(&hub, &source, runtime_binding.as_ref(), "ferramenta", serde_json::json!({"text": tool}).to_string(), |s| &mut s.tool).await;
        sent || hub.generation().is_some()
    }

    /// Publica `data` se mudou desde o último deste evento; `false` com o hub fechado.
    async fn emit(&mut self, hub: &Hub, source: &(u64, crate::side::Binding), binding: Option<&crate::interactions::InteractionBinding>,
        event: &str, data: String, slot: impl FnOnce(&mut Sent) -> &mut Option<String>) -> bool {
        self.emit_with(hub, source, binding, event, data, slot, || {}).await
    }

    async fn emit_with(&mut self, hub: &Hub, source: &(u64, crate::side::Binding), binding: Option<&crate::interactions::InteractionBinding>,
        event: &str, data: String, slot: impl FnOnce(&mut Sent) -> &mut Option<String>, published: impl FnOnce()) -> bool {
        if let Some(binding) = binding && let Some(registry) = &self.registry {
            match registry.interaction_binding_readonly(&self.name).await {
                Ok(current) if current == *binding => {},
                _ => {
                    if let Some(facts) = &mut self.facts { facts.error = Some("state_feed_binding_changed".into()); }
                    return false;
                }
            }
        }
        if hub.source_binding().as_ref() != Some(source) { return false; }
        let last = slot(&mut self.sent);
        if last.as_deref() == Some(data.as_str()) { return true; }
        if !hub.publish_own_bound_with(source, event, &data, published) { return false; }
        *last = Some(data);
        true
    }
}

impl Drop for RuntimeFeed {
    fn drop(&mut self) {
        if let Some(w) = &self.facts {
            w.src.store.forget_watcher(&self.name, &self.facts_wake);
        }
        // Sem feed a lista volta ao fato do Python: o estado parado aqui mentiria.
        self.published.clear(self.owner, &self.name);
    }
}

/// Roda o feed; pânico vira diário (`on_panic`) e `state` com problema, e o hub o dá por acabado
/// (volta com o próximo assinante).
pub async fn guarded(hub: Weak<Hub>, run: impl std::future::Future<Output = ()>, on_panic: impl FnOnce(&str)) {
    use futures_util::FutureExt;
    if std::panic::AssertUnwindSafe(run).catch_unwind().await.is_ok() {
        return;
    }
    let Some(hub) = hub.upgrade() else { return };
    tracing::error!(session = hub.name.as_str(), code = "state_feed_panic", "estado: feed do runtime caiu");
    on_panic(&hub.name);
    let state = feed_problem(&hub.name, binding_headless(&hub), "state_feed_failed", "o estado da sessão caiu; volta ao reabrir o chat");
    if let Ok(data) = serde_json::to_string(&state) {
        hub.publish_own("state", &data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::protocol::{LiveSender, LiveState};
    use crate::side::{Binding, Out, test_ctx};
    use crate::transcript::Provider;
    use serde_json::{Value, json};
    use tokio::sync::broadcast;
    use tokio::time::Instant;

    struct Fixture { _dir: tempfile::TempDir, lease: crate::side::Lease, published: Arc<Published>, ctx: crate::side::SideCtx }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx();
        let binding = Binding { provider: Provider::Codex, jsonl: dir.path().join("rollout-abc.jsonl"), key: "thread-1".into(), headless: true, life: None };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        Fixture { _dir: dir, lease, published: Arc::default(), ctx }
    }

    fn live(state: &str) -> LiveState {
        LiveState { public_state: json!({"session": "s", "state": state, "headless": true}), ..Default::default() }
    }

    fn spawn(f: &Fixture, rx: Option<crate::runtime::protocol::LiveReceiver>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(RuntimeFeed::new(&f.lease.hub, rx, f.published.clone(), None).run())
    }

    /// (instante, evento, dado) do que o hub repassou até o prazo.
    async fn collect(rx: &mut broadcast::Receiver<Out>, limit: Duration) -> Vec<(Instant, String, Value)> {
        let mut out = Vec::new();
        let _ = tokio::time::timeout(limit, async {
            while let Ok(msg) = rx.recv().await {
                if let Out::Side(f) = msg {
                    let f = String::from_utf8_lossy(&f).into_owned();
                    let event = f.lines().next().unwrap_or("").trim_start_matches("event: ").to_owned();
                    let data = serde_json::from_str(f.lines().nth(1).unwrap_or("").trim_start_matches("data: ")).unwrap_or(Value::Null);
                    out.push((Instant::now(), event, data));
                }
            }
        }).await;
        out
    }

    fn of<'a>(got: &'a [(Instant, String, Value)], event: &str) -> Vec<&'a (Instant, String, Value)> {
        got.iter().filter(|(_, e, _)| e == event).collect()
    }

    fn channel(state: Option<LiveState>) -> (LiveSender, crate::runtime::protocol::LiveReceiver) {
        tokio::sync::watch::channel(state.map(Arc::new))
    }

    #[tokio::test]
    async fn headless_claude_shows_only_the_native_cards_never_the_plugin_ones() {
        use crate::interactions::InteractionBinding;
        use crate::state::facts::{PluginFacts, PluginReading};
        let f = claude_fixture();
        let binding = InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        let native = LiveState { public_state: json!({"session": "s", "state": "awaiting_input", "headless": true,
            "codex_question": {"provider":"claude","request_id":1,"interaction_id":"native-form","questions":[{"question":"Pasta?","options":[]}]}}),
            ..Default::default() };
        let (_tx, rx) = channel(Some(native));
        let mut feed = RuntimeFeed::new(&f.lease.hub, Some(rx), f.published.clone(), None);
        let local = PluginFacts { plugin_state: None, waiter_open: true, heartbeat_age_ms: Some(0), question: None,
            questions: vec![json!({"id":"ask:b","interaction_id":"b","questions":[{"question":"Pasta?","options":[]}]}),
                json!({"id":"perm:a","interaction_id":"a","questions":[],"tool":"Bash"})], suggestion: "".into() };
        feed.plugin_reading = Some(PluginReading { binding, facts: Ok(local) });
        assert!(feed.round().await);
        let question: Value = serde_json::from_str(feed.sent.question.as_deref().unwrap()).unwrap();
        assert_eq!(question["interaction_id"], "native-form");
        let state: Value = serde_json::from_str(feed.sent.state.as_deref().unwrap()).unwrap();
        assert!(state["problema"].is_null(), "cartão do plugin não disputa com o nativo: {state}");
        assert!(state["interaction_id"].is_null() && state["question"].is_null(), "perm: do plugin não vira cartão: {state}");
    }

    #[tokio::test]
    async fn real_runtime_live_error_is_published_without_authorizing_old_plugin_questions() {
        use crate::runtime::protocol::{CanoBinding, RuntimeTarget};
        let f = claude_fixture();
        let py = fake_python().await;
        let external = Arc::new(FactsStore::default());
        let interactions = Arc::new(crate::interactions::InteractionStore::default());
        let plugin = Arc::new(crate::plugin::Store::new(interactions.clone()));
        let registry = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(py.addr,"test".into(),"feed-error".into())
            .with_interactions(interactions).with_plugin(plugin));
        let endpoint = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = endpoint.local_addr().unwrap();
        drop(endpoint);
        let queues = tempfile::tempdir().unwrap();
        let error = registry.open(RuntimeTarget {key:"error-key".into(),generation:1,name:"s".into(),provider:"claude".into(),
            metadata:json!({"name":"s","headless":true,"session_id":"sid","initialized":true}),
            binding:CanoBinding {pid:42,escuta:format!("tcp:{address}"),token:"cano-test".into(),versao:2},
            lease_path:queues.path().join("error.lock"),state_path:queues.path().join("error.json"),
            projection_dir:queues.path().join("projection"),transcript:f._dir.path().join("sid.jsonl"),created:0.0}).await.unwrap_err();
        let live = registry.live("s");
        assert!(live.borrow().as_ref().unwrap().error.is_some(), "erro vem do produtor real do runtime");
        assert!(registry.interaction_binding_readonly("s").await.is_err(), "erro não autoriza interação");
        let mut rx = f.lease.hub.tx.subscribe();
        let mut feed = RuntimeFeed::new(&f.lease.hub,Some(live),f.published.clone(),Some(feed_facts(&py,&external)))
            .with_registry(Some(registry));
        feed.refresh_facts().await;
        assert!(feed.round().await);
        let got = collect(&mut rx,Duration::from_millis(20)).await;
        let state = &of(&got,"state")[0].2;
        assert_eq!(state["problema"],"runtime_falhou");
        assert!(state["problema_detalhe"].as_str().unwrap().contains(&error.code));
        assert_eq!(state["headless"],true);
        assert!(of(&got,"ask_question").is_empty());
        assert!(of(&got,"suggest").is_empty());
    }

    #[tokio::test]
    async fn parked_claude_without_actor_shows_no_stale_warning() {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        write_sidecar(home.path(),json!({"cwd":"/w","session_id":"sid","model":"claude-opus-5","effort":"high"}));
        let py = fake_python().await;
        let registry = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(py.addr,"test".into(),"feed-parked-quiet".into()));
        let external = Arc::new(FactsStore::default());
        let mut feed = RuntimeFeed::new(&f.lease.hub,Some(registry.live("s")),f.published.clone(),Some(feed_facts(&py,&external)))
            .with_registry(Some(registry.clone())).with_home(home.path());
        feed.refresh_facts().await;
        feed.parked = feed.load_parked().await;
        let mut rx = f.lease.hub.tx.subscribe();
        assert!(feed.round().await);
        let got = collect(&mut rx,Duration::from_millis(20)).await;
        let state = &of(&got,"state")[0].2;
        assert_eq!((state["state"].as_str(), &state["problema"]), (Some("idle"), &serde_json::Value::Null), "{got:?}");
    }

    #[tokio::test]
    async fn parked_claude_without_registry_entry_publishes_b_metadata_but_not_old_a() {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        write_sidecar(home.path(),json!({"cwd":"/w","session_id":"sid-b","model":"claude-opus-5","effort":"high",
            "permission_mode":"plan","previous_non_plan":"acceptEdits","problema":["limite_de_uso","volta às 14:00"]}));
        let py = fake_python().await;
        let registry = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(py.addr,"test".into(),"feed-parked".into()));
        let external = Arc::new(FactsStore::default());
        let mut feed = RuntimeFeed::new(&f.lease.hub,Some(registry.live("s")),f.published.clone(),Some(feed_facts(&py,&external)))
            .with_registry(Some(registry.clone())).with_home(home.path());
        feed.refresh_facts().await;
        let b = Binding {provider:Provider::ClaudeHeadless,jsonl:f._dir.path().join("sid-b.jsonl"),key:"sid-b".into(),headless:true,life:None};
        let _rebinding = f.ctx.hubs.acquire("s",b,&f.ctx);
        let mut rx = f.lease.hub.tx.subscribe();
        assert!(feed.round().await);
        assert!(of(&collect(&mut rx,Duration::from_millis(20)).await,"state").is_empty(), "a rodada de A não publica em B");
        feed.refresh_facts().await;
        feed.parked = feed.load_parked().await;
        assert!(registry.interaction_binding_readonly("s").await.is_err());
        assert!(feed.round().await);
        let got = collect(&mut rx,Duration::from_millis(20)).await;
        let state = &of(&got,"state")[0].2;
        assert_eq!(state["state"],"idle");
        assert_eq!(state["status_line"],"🤖 Opus5 (high)");
        assert_eq!(state["claude_permission_mode"],"plan");
        assert_eq!(state["claude_previous_non_plan"],"acceptEdits");
        assert_eq!(state["problema"],"limite_de_uso");
        assert!(of(&got,"ask_question").is_empty());
        assert!(!got.iter().any(|(_,_,data)| data.to_string().contains("ask:a")));
    }

    /// Hub ligado como no servidor (`SideCtx::info` pergunta ao runtime antes da descoberta): a rodada
    /// prova a conversa e publica a vista do ator, sem `state_feed_binding_unavailable`.
    #[tokio::test]
    async fn hub_bound_by_the_runtime_info_publishes_the_actor_view() {
        use crate::runtime::protocol::{CanoBinding, RuntimeTarget};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let boot = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let python = boot.local_addr().unwrap();
        let boot_server = tokio::spawn(async move {
            axum::serve(boot, axum::Router::new().fallback(|| async { crate::session_write::json_response(axum::http::StatusCode::OK, json!({"ok":true,"data":{}})) })).await.unwrap();
        });
        let cano = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = cano.local_addr().unwrap();
        let cano_server = tokio::spawn(async move {
            let (socket, _) = cano.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            let mut token = String::new();
            socket.read_line(&mut token).await.unwrap();
            let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
                "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
            socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
            let mut line = String::new();
            while socket.read_line(&mut line).await.unwrap_or(0) > 0 { line.clear(); }
        });
        let processes = Arc::new(RecipeProcesses {sid:std::sync::Mutex::new("sid".into()),config:dir.path().into()});
        let registry = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(python, "test".into(), "feed-info".into()).with_processes(processes));
        let ctx = test_ctx();
        registry.bind_hubs(ctx.hubs.downgrade());
        assert!(ctx.runtime.set(registry.clone()).is_ok());
        registry.open(RuntimeTarget { key:"runtime-key".into(), generation:1, name:"s".into(), provider:"claude".into(),
            metadata:json!({"name":"s","headless":true,"session_id":"sid","initialized":true}),
            binding:CanoBinding {pid:42,escuta:format!("tcp:{address}"),token:"cano-test".into(),versao:2},
            lease_path:dir.path().join("feed.lock"),state_path:dir.path().join("queue.json"),
            projection_dir:dir.path().join("projection"),transcript:dir.path().join("sid.jsonl"),created:0.0 }).await.unwrap();
        let info = ctx.info("s").await.unwrap().unwrap();
        let lease = ctx.hubs.acquire("s", Binding::from_info(&info).unwrap(), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        let mut feed = RuntimeFeed::new(&lease.hub, Some(registry.live("s")), Arc::default(), None).with_registry(Some(registry.clone()));
        assert!(feed.round().await);
        let got = collect(&mut rx, Duration::from_millis(20)).await;
        let state = &of(&got, "state")[0].2;
        assert!(state["problema"].is_null(), "rodada sem prova da conversa: {state}");
        registry.close("runtime-key", 1).await.unwrap();
        cano_server.abort();
        boot_server.abort();
    }

    struct RecipeProcesses {sid:std::sync::Mutex<String>,config:std::path::PathBuf}
    impl crate::list::procs::ProcessView for RecipeProcesses {
        fn children(&self,_:Duration)->std::io::Result<Arc<crate::list::procs::ChildrenMap>> {Ok(Arc::new(std::collections::HashMap::from([(42,vec![43])]))) }
        fn argv(&self,_:i64)->Vec<String> {vec![]}
        fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(self.config.clone())}
        fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
        fn start_time(&self,_:i64)->Option<f64> {Some(1000.0)}
        fn recipe(&self,pid:i64)->std::io::Result<crate::list::procs::ProcessRecipe> {
            let argv=match pid {42=>vec!["hangar-cano".into()],43=>vec!["claude".into(),"--session-id".into(),self.sid.lock().unwrap().clone().into()],_=>return Err(std::io::ErrorKind::NotFound.into())};
            Ok(crate::list::procs::ProcessRecipe {pid,birth:1000.0,executable:if pid==42 {"/fixture/hangar-cano".into()} else {"/fixture/claude".into()},cwd:self.config.clone(),argv,
                environ:vec![format!("HOME={}",self.config.display()).into(),format!("CLAUDE_CONFIG_DIR={}",self.config.display()).into()],cgroup:None})
        }
    }

    #[tokio::test]
    async fn refresh_a_rebind_b_discards_a_and_then_real_refresh_publishes_only_b() {
        use crate::runtime::protocol::{CanoBinding, RuntimeTarget};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let f = claude_fixture();
        let boot = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let py = FakePython { addr:boot.local_addr().unwrap(), hits:Arc::default(), snapshots:Arc::default() };
        let boot_server = tokio::spawn(async move {
            axum::serve(boot, axum::Router::new().fallback(|| async { crate::session_write::json_response(axum::http::StatusCode::OK, json!({"ok":true,"data":{}})) })).await.unwrap();
        });
        let external = Arc::new(crate::state::facts::FactsStore::default());
        let interactions = Arc::new(crate::interactions::InteractionStore::default());
        let plugin = Arc::new(crate::plugin::Store::new(interactions.clone()));
        let processes=Arc::new(RecipeProcesses {sid:std::sync::Mutex::new("sid".into()),config:f._dir.path().into()});
        let registry = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(py.addr, "test".into(), "feed-race".into())
            .with_interactions(interactions).with_plugin(plugin.clone()).with_processes(processes.clone()));
        registry.bind_hubs(f.ctx.hubs.downgrade());
        let queues = tempfile::tempdir().unwrap();
        let mut feed = RuntimeFeed::new(&f.lease.hub, Some(registry.live("s")), f.published.clone(), Some(feed_facts(&py,&external))).with_registry(Some(registry.clone()));
        let mut servers = Vec::new();
        for (conversation, generation) in [("sid", 1), ("sid-b", 2)] {
            if generation == 2 {
                registry.close("feed-key", 1).await.unwrap();
                let _lease=crate::runtime::queue::acquire_lease(&queues.path().join("feed.lock")).unwrap();
                let mut store=crate::runtime::queue::Store::open(&queues.path().join("queue.json"),&queues.path().join("projection"),crate::runtime::queue::State::new("feed-key",1,"s",vec![])).unwrap();
                store.exec(1,"fixture-new-life",crate::runtime::protocol::ClockSample {monotonic_s:0.0,epoch_s:1.0},crate::runtime::queue::Action::AdvanceLifecycle {generation:2,state:json!({})}).unwrap();
            }
            *processes.sid.lock().unwrap()=conversation.into();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            servers.push(tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut token = String::new();
                socket.read_line(&mut token).await.unwrap();
                assert_eq!(token, "cano-test\n");
                let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
                    "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
                socket.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
                loop {
                    let mut line = String::new();
                    if socket.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                    let envelope: Value = serde_json::from_str(&line).unwrap();
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    socket.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
                }
            }));
            registry.open(RuntimeTarget { key:"feed-key".into(), generation, name:"s".into(), provider:"claude".into(),
                metadata:json!({"name":"s","headless":true,"session_id":conversation,"initialized":true}),
                binding:CanoBinding {pid:42,escuta:format!("tcp:{address}"),token:"cano-test".into(),versao:2},
                lease_path:queues.path().join("feed.lock"),state_path:queues.path().join("queue.json"),
                projection_dir:queues.path().join("projection"),transcript:f._dir.path().join(format!("{conversation}.jsonl")),created:0.0 }).await.unwrap();
            if generation == 1 {
                let life = registry.interaction_binding_readonly("s").await.unwrap();
                let gate = crate::plugin::PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false};
                let _pending = plugin.begin_ask(&life, serde_json::from_value(json!({"id":"ask:a","questions":[{"question":"A?","options":[]}]})).unwrap(), gate, std::time::Instant::now()).unwrap();
                plugin.suggest(&life, "suggest-a".into(), true).unwrap();
                let mut rx = f.lease.hub.tx.subscribe();
                feed.refresh_facts().await;
                let b = Binding {provider:Provider::ClaudeHeadless,jsonl:f._dir.path().join("sid-b.jsonl"),key:"sid-b".into(),headless:true,life:None};
                let _rebinding = f.ctx.hubs.acquire("s", b, &f.ctx);
                assert!(feed.round().await);
                assert!(collect(&mut rx, Duration::from_millis(20)).await.iter().all(|(_,kind,_)| !matches!(kind.as_str(),"ask_question"|"state"|"suggest")));
            }
        }
        let life = registry.interaction_binding_readonly("s").await.unwrap();
        let gate = crate::plugin::PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false};
        let _pending = plugin.begin_ask(&life, serde_json::from_value(json!({"id":"ask:b","questions":[{"question":"B?","options":[]}]})).unwrap(), gate, std::time::Instant::now()).unwrap();
        plugin.suggest(&life, "suggest-b".into(), true).unwrap();
        let mut rx = f.lease.hub.tx.subscribe();
        feed.refresh_facts().await;
        assert!(feed.round().await);
        let got = collect(&mut rx, Duration::from_millis(20)).await;
        assert!(of(&got,"suggest").iter().any(|event| event.2["text"] == "suggest-b"));
        assert!(!got.iter().any(|(_,_,data)| data.to_string().contains("ask:b")), "sem terminal o cartão do plugin não aparece");
        assert!(!got.iter().any(|(_,_,data)| data.to_string().contains("ask:a") || data.to_string().contains("suggest-a")));
        let old_binding = crate::interactions::InteractionBinding { key:"feed-key".into(),conversation:"sid-b".into(),generation:1 };
        let (_old_tx, old_view) = channel(Some(LiveState { binding:Some(old_binding),
            public_state:json!({"session":"s","state":"awaiting_input","headless":true,"status_line":"metadata-a",
                "codex_question":{"request_id":"ask:a","interaction_id":"a","questions":[{"question":"A?"}]}}),
            error:Some(("old-a-error".into(),"old-a-detail".into())), ..Default::default() }));
        let mut stale = RuntimeFeed::new(&f.lease.hub,Some(old_view),Arc::default(),Some(feed_facts(&py,&external))).with_registry(Some(registry.clone()));
        let mut stale_rx = f.lease.hub.tx.subscribe();
        stale.refresh_facts().await;
        assert!(stale.round().await);
        let rejected = collect(&mut stale_rx,Duration::from_millis(20)).await;
        assert!(of(&rejected,"state").is_empty(), "sem prova da conversa, nenhum estado inventado: {rejected:?}");
        assert!(of(&rejected,"ask_question").is_empty());
        assert!(!rejected.iter().any(|(_,_,data)| data.to_string().contains("metadata-a") || data.to_string().contains("ask:a")));
        let mut observed = registry.live("s");
        let server = servers.pop().unwrap();
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // O socket caiu e o cano (pid de mentira) não é mais o desta vida: queda real, na vista do ator.
                if observed.borrow().as_ref().is_some_and(|live| live.public_state["problema"] == "headless_processo_caiu") { break; }
                observed.changed().await.unwrap();
            }
        }).await.expect("queda real do ator após perder a conexão");
        let mut rx = f.lease.hub.tx.subscribe();
        feed.refresh_facts().await;
        assert!(feed.round().await);
        let failed = collect(&mut rx,Duration::from_millis(20)).await;
        assert_eq!(of(&failed,"state")[0].2["problema"],"headless_processo_caiu", "{failed:?}");
        assert!(of(&failed,"ask_question").is_empty(), "erro não resolve nem republica o cartão B");
        assert!(!failed.iter().any(|(_,_,data)| data.to_string().contains("ask:a") || data.to_string().contains("suggest-a")));
        registry.close("feed-key", 2).await.unwrap();
        for server in servers { server.abort(); }
        boot_server.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn burst_coalesces_into_one_round() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (tx, live_rx) = channel(Some(live("working")));
        let _feed = spawn(&f, Some(live_rx));
        collect(&mut rx, Duration::from_millis(50)).await;
        let start = Instant::now();
        for i in 0..20 {
            tx.send_replace(Some(Arc::new(LiveState { preview: format!("parte {i}"), ..live("working") })));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let got = collect(&mut rx, Duration::from_millis(400)).await;
        let previews = of(&got, "preview");
        assert_eq!(previews.len(), 1, "{got:?}");
        assert_eq!(previews[0].2["text"], "parte 19");
        assert_eq!(previews[0].2["vivo"], true);
        assert!(previews[0].0 - start >= COALESCE, "sai depois da janela");
        assert!(of(&got, "state").is_empty(), "o estado não mudou");
    }

    #[tokio::test(start_paused = true)]
    async fn republishes_only_on_change() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (tx, live_rx) = channel(Some(live("idle")));
        let _feed = spawn(&f, Some(live_rx));
        let first = collect(&mut rx, Duration::from_millis(50)).await;
        assert_eq!(of(&first, "state").len(), 1);
        assert_eq!(of(&first, "ask_question").iter().map(|e| e.2.clone()).collect::<Vec<_>>(), [Value::Null], "primeiro retrato leva a pergunta vazia");
        tx.send_replace(Some(Arc::new(live("idle"))));
        let same = collect(&mut rx, Duration::from_millis(300)).await;
        assert!(of(&same, "state").is_empty() && of(&same, "ask_question").is_empty(), "{same:?}");
        let question = json!({"questions": [{"question": "Qual?"}]});
        let mut asking = live("awaiting_input");
        asking.public_state["codex_question"] = question.clone();
        tx.send_replace(Some(Arc::new(asking)));
        let asked = collect(&mut rx, Duration::from_millis(300)).await;
        assert_eq!(of(&asked, "ask_question")[0].2, question);
        assert_eq!(of(&asked, "state").len(), 1);
        tx.send_replace(Some(Arc::new(live("idle"))));
        let back = collect(&mut rx, Duration::from_millis(300)).await;
        assert_eq!(of(&back, "ask_question").iter().map(|e| e.2.clone()).collect::<Vec<_>>(), [Value::Null], "de volta a vazio");
    }

    #[tokio::test(start_paused = true)]
    async fn actor_error_is_runtime_falhou() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(LiveState { error: Some(("queue_io".into(), "fila recusou: x".into())), ..live("working") }));
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!((state["state"].as_str(), state["problema"].as_str(), state["problema_detalhe"].as_str()),
                   (Some("working"), Some("runtime_falhou"), Some("queue_io: fila recusou: x")));
    }

    /// Vínculo do runtime sem prova (entrada fora do registro): erro durável do ator desta conversa sai como falha
    /// real, com o código verdadeiro; o de outra conversa não sai.
    #[tokio::test]
    async fn own_conversation_actor_error_shows_and_another_conversations_does_not() {
        for (conversation, shown) in [("sid", true), ("outra", false)] {
            let f = claude_fixture();
            let py = fake_python().await;
            let registry = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(py.addr, "test".into(), format!("feed-own-{conversation}")));
            let binding = crate::interactions::InteractionBinding { key: "k".into(), conversation: conversation.into(), generation: 1 };
            let (_tx, live_rx) = channel(Some(LiveState { binding: Some(binding), public_state: json!({"session": "s"}),
                error: Some(("queue_io".into(), "fila recusou".into())), ..Default::default() }));
            let mut rx = f.lease.hub.tx.subscribe();
            let mut feed = RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), None).with_registry(Some(registry));
            assert!(feed.round().await);
            let got = collect(&mut rx, Duration::from_millis(20)).await;
            let states = of(&got, "state");
            if shown {
                assert_eq!((states[0].2["problema"].as_str(), states[0].2["problema_detalhe"].as_str()),
                    (Some("runtime_falhou"), Some("queue_io: fila recusou")), "{got:?}");
            } else {
                assert!(states.is_empty(), "erro de outra conversa não sai: {got:?}");
            }
        }
    }

    /// A vista do ator que não é um estado não vira `idle` inventado: nada sai e o motivo vai ao diário.
    #[tokio::test(start_paused = true)]
    async fn invalid_actor_view_publishes_no_invented_state() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(LiveState { public_state: json!("não é um estado"), ..Default::default() }));
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        assert!(of(&got, "state").is_empty(), "{got:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn absent_entry_is_idle() {
        // Sessão parada não é falha: reiniciar, encerrar ou reabrir não pode acender o problema.
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!(state["state"], "idle");
        assert!(state["problema"].is_null(), "{state}");
    }

    #[tokio::test(start_paused = true)]
    async fn no_registry_publishes_no_invented_state() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let _feed = spawn(&f, None);
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        // Sem runtime não há estado a provar: nada de `idle` com aviso; o motivo vai ao diário.
        assert!(of(&got, "state").is_empty(), "{got:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn committed_preview_goes_out_empty() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let answer = "Resposta já gravada no transcript da sessão.";
        f.lease.hub.set_committed(0, crate::state::preview::norm(answer), false);
        let (_tx, live_rx) = channel(Some(LiveState { preview: answer.into(), ..live("working") }));
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        assert_eq!(of(&got, "preview")[0].2["text"], "", "prévia igual à resposta gravada sai vazia");
    }

    /// O feed lê o canal depois de o transcript já ter entregado a resposta: o começo curto dela, que a regra
    /// de 16 caracteres deixaria passar, ressuscitaria a bolha no app, que ignora o vazio durante `working`.
    #[tokio::test(start_paused = true)]
    async fn a_short_start_of_the_committed_answer_goes_out_empty() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        f.lease.hub.set_committed(0, crate::state::preview::norm("Vou medir o `hangar-server` desta máquina."), false);
        let (_tx, live_rx) = channel(Some(LiveState { preview: "Vou med".into(), ..live("working") }));
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        assert_eq!(of(&got, "preview")[0].2["text"], "", "o começo da resposta gravada sai vazio");
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_preview_that_diverges_from_the_committed_answer_goes_out() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        f.lease.hub.set_committed(0, crate::state::preview::norm("Vou medir o `hangar-server` desta máquina."), false);
        let (_tx, live_rx) = channel(Some(LiveState { preview: "Vou ver".into(), ..live("working") }));
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        assert_eq!(of(&got, "preview")[0].2["text"], "Vou ver", "resposta nova que diverge da gravada aparece");
    }

    #[tokio::test(start_paused = true)]
    async fn never_publishes_suggest() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (tx, live_rx) = channel(Some(live("working")));
        let _feed = spawn(&f, Some(live_rx));
        tx.send_replace(Some(Arc::new(LiveState { thinking: "pensa".into(), tool: "{}".into(), ..live("idle") })));
        let got = collect(&mut rx, Duration::from_millis(400)).await;
        assert!(of(&got, "suggest").is_empty());
        assert_eq!(of(&got, "pensamento").last().unwrap().2, json!({"text": "pensa"}));
        assert_eq!(of(&got, "ferramenta").last().unwrap().2, json!({"text": "{}"}));
    }

    fn claude_fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx();
        let binding = Binding { provider: Provider::ClaudeHeadless, jsonl: dir.path().join("sid.jsonl"), key: "sid".into(), headless: false, life: None };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        Fixture { _dir: dir, lease, published: Arc::default(), ctx }
    }

    #[tokio::test(start_paused = true)]
    async fn claude_headless_publishes_the_actor_view() {
        let f = claude_fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let question = json!({"questions": [{"question": "Qual?"}]});
        let mut asking = live("awaiting_input");
        asking.public_state["codex_question"] = question.clone();
        let (_tx, live_rx) = channel(Some(LiveState { preview: "texto".into(), thinking: "pensa".into(), tool: "{}".into(), ..asking }));
        let _feed = spawn(&f, Some(live_rx));
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        assert_eq!(of(&got, "ask_question")[0].2, question);
        assert_eq!(of(&got, "state")[0].2["state"], "awaiting_input");
        assert_eq!(of(&got, "preview")[0].2["text"], "texto");
        assert_eq!(of(&got, "pensamento")[0].2, json!({"text": "pensa"}));
        assert_eq!(of(&got, "ferramenta")[0].2, json!({"text": "{}"}));
        assert!(of(&got, "suggest").is_empty(), "sem fatos empurrados nada sai");
        assert_eq!(f.published.get("s", Some("sid")).map(|e| e.state.clone()).as_deref(), Some("awaiting_input"));
    }

    #[tokio::test(start_paused = true)]
    async fn claude_turn_reopened_mid_stream_is_working_until_result() {
        // Rust reiniciado no meio de um turno: a vista do motor reaberto fica `working` até o `result`.
        use crate::runtime::claude::ClaudeEngine;
        use crate::runtime::protocol::{ClockSample, EngineInput};
        let clock = |s: f64| ClockSample { monotonic_s: s, epoch_s: 1_800_000_000.0 + s };
        let mut engine = ClaudeEngine::new(json!({"name": "s", "session_id": "sid", "initialized": true}), 1, clock(10.0));
        let view = |engine: &ClaudeEngine| LiveState { public_state: engine.view()["public_state"].clone(), ..Default::default() };
        let f = claude_fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let (tx, live_rx) = channel(Some(view(&engine)));
        let _feed = spawn(&f, Some(live_rx));
        assert_eq!(of(&collect(&mut rx, Duration::from_millis(50)).await, "state")[0].2["state"], "idle");
        let delta = json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "meio"}}});
        engine.apply(EngineInput::Line(delta), clock(11.0)).unwrap();
        tx.send_replace(Some(Arc::new(view(&engine))));
        assert_eq!(of(&collect(&mut rx, Duration::from_millis(300)).await, "state")[0].2["state"], "working");
        engine.apply(EngineInput::Line(json!({"type": "result", "subtype": "success"})), clock(20.0)).unwrap();
        tx.send_replace(Some(Arc::new(view(&engine))));
        assert_eq!(of(&collect(&mut rx, Duration::from_millis(300)).await, "state").last().unwrap().2["state"], "idle");
    }

    #[tokio::test(start_paused = true)]
    async fn closed_hub_ends_feed() {
        let f = fixture();
        let (tx, live_rx) = channel(Some(live("working")));
        let feed = spawn(&f, Some(live_rx));
        tokio::time::sleep(Duration::from_millis(50)).await;
        crate::side::close_hub(&f.lease.hub);
        tx.send_replace(Some(Arc::new(live("idle"))));
        tokio::time::timeout(Duration::from_secs(1), feed).await.expect("hub fechado: o feed acaba").unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn publishes_to_list_with_rollout_stem() {
        let f = fixture();
        let (tx, live_rx) = channel(Some(live("working")));
        let feed = spawn(&f, Some(live_rx));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.published.get("s", Some("rollout-abc")).map(|e| e.state.clone()).as_deref(), Some("working"));
        tx.send_replace(Some(Arc::new(live("idle"))));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(f.published.get("s", Some("rollout-abc")).map(|e| e.state.clone()).as_deref(), Some("idle"));
        feed.abort();
        let _ = feed.await;
        assert!(f.published.get("s", Some("rollout-abc")).is_none(), "feed que acaba sai da lista");
    }

    /// Python de mentira: responde o retrato de fatos e conta os pedidos.
    struct FakePython { addr: std::net::SocketAddr, hits: Arc<std::sync::atomic::AtomicUsize>, snapshots: Arc<std::sync::atomic::AtomicUsize> }

    async fn fake_python() -> FakePython { fake_python_with(0, "").await }

    /// Os primeiros `fail_first` pedidos levam 503; depois responde o retrato com `suggestion`.
    async fn fake_python_with(fail_first: usize, suggestion: &'static str) -> FakePython { fake_python_full(fail_first, suggestion, false).await }

    /// Como `fake_python_with`, com a janela de troca de modo aberta ou não nos fatos.
    async fn fake_python_full(fail_first: usize, suggestion: &'static str, transfer: bool) -> FakePython {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let snapshots = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (counted, snapped) = (hits.clone(), snapshots.clone());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let read = s.read(&mut buf).await.unwrap_or(0);
                // Qualquer processo da máquina pode sondar a porta nova (`HEAD /`): só rota do app conta.
                let path = std::str::from_utf8(&buf[..read]).ok().and_then(|r| r.split(' ').nth(1)).unwrap_or("");
                if !(path.starts_with("/internal/") || path.starts_with("/api/")) {
                    let _ = s.write_all(b"HTTP/1.1 404 X\r\ncontent-length: 0\r\n\r\n").await;
                    continue;
                }
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Retrato é GET; o relatório de diagnóstico é POST.
                if buf[..read].starts_with(b"GET") {
                    snapped.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                if n < fail_first {
                    let _ = s.write_all(b"HTTP/1.1 503 X\r\ncontent-length: 0\r\n\r\n").await;
                    continue;
                }
                let mut facts = facts_json(1, suggestion);
                facts["transfer_active"] = json!(transfer);
                let body = serde_json::to_string(&facts).unwrap();
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
            }
        });
        FakePython { addr, hits, snapshots }
    }

    fn facts_json(seq: u64, suggestion: &str) -> Value {
        json!({"seq": seq, "plugin_state": null, "waiter_open": false, "heartbeat_age_ms": null, "question": null,
               "suggestion": suggestion, "body_columns": null, "band_anchor": null, "in_transfer_ms": 0,
               "transfer_active": false, "permission_op": false})
    }

    fn facts(seq: u64, suggestion: &str) -> crate::state::facts::StateFacts {
        serde_json::from_value(facts_json(seq, suggestion)).unwrap()
    }

    fn feed_facts(py: &FakePython, store: &Arc<crate::state::facts::FactsStore>) -> FeedFacts {
        FeedFacts {
            store: store.clone(),
            diag: crate::diag::DiagClient::new(py.addr, "s".into()),
        }
    }

    fn hits(py: &FakePython) -> usize { py.hits.load(std::sync::atomic::Ordering::SeqCst) }

    fn texts(got: &[(Instant, String, Value)]) -> Vec<Value> {
        of(got, "suggest").into_iter().map(|e| e.2.clone()).collect()
    }

    // O retrato HTTP e os empurrões antigos não são fonte do plugin no Rust.
    #[tokio::test]
    async fn claude_does_not_request_migrated_http_facts() {
        let f = claude_fixture();
        let py = fake_python().await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(live("idle")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        let first = collect(&mut rx, Duration::from_millis(400)).await;
        assert_eq!(hits(&py), 0, "a fonte migrada não consulta HTTP");
        assert!(of(&first, "suggest").is_empty(), "vazia desde o início não é mudança");
        let now = std::time::Instant::now();
        store.push("s", facts(2, "roda os testes"), now);
        assert!(texts(&collect(&mut rx, Duration::from_millis(400)).await).is_empty(), "empurrão externo não publica sugestão do plugin");
        store.push("s", facts(3, "roda os testes"), now);
        assert!(texts(&collect(&mut rx, Duration::from_millis(400)).await).is_empty(), "igual não repete");
        store.push("s", facts(4, ""), now);
        assert!(texts(&collect(&mut rx, Duration::from_millis(400)).await).is_empty());
        assert_eq!(hits(&py), 0);
    }

    #[tokio::test]
    async fn failed_snapshot_is_retried() {
        let f = claude_fixture();
        let py = fake_python_with(1, "depois da falha").await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(live("idle")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        let got = collect(&mut rx, RETRY + Duration::from_millis(800)).await;
        assert_eq!(py.snapshots.load(std::sync::atomic::Ordering::SeqCst), 0, "não há reserva HTTP");
        assert!(texts(&got).is_empty());
    }

    #[tokio::test]
    async fn failing_snapshot_is_not_polled_on_every_wake() {
        let f = claude_fixture();
        let py = fake_python_with(usize::MAX, "").await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let (tx, live_rx) = channel(Some(live("working")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        // Turno em streaming: o ator acorda o feed várias vezes por janela de retentativa.
        for i in 0..40 {
            tx.send_replace(Some(Arc::new(LiveState { preview: format!("parte {i}"), ..live("working") })));
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let windows = (Duration::from_millis(1000).as_millis() / RETRY.as_millis()) as usize + 1;
        let attempts = py.snapshots.load(std::sync::atomic::Ordering::SeqCst);
        let _ = windows;
        assert_eq!(attempts, 0, "nenhuma leitura de fatos migrados via HTTP");
    }

    #[tokio::test]
    async fn failing_snapshot_with_a_gap_is_not_polled_on_every_wake() {
        let f = claude_fixture();
        let py = fake_python_with(usize::MAX, "").await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let (tx, live_rx) = channel(Some(live("working")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = std::time::Instant::now();
        store.push("s", facts(1, ""), now);
        assert_eq!(store.push("s", facts(3, ""), now), crate::state::facts::Push::Accepted { gap: true });
        for i in 0..40 {
            tx.send_replace(Some(Arc::new(LiveState { preview: format!("parte {i}"), ..live("working") })));
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(store.needs_snapshot("s"), "o retrato segue falhando: o pulo continua aberto");
        // Pelo tempo que passou de fato: num runner lento as 40 esperas de 25 ms passam de 1 s, e cada
        // janela de retentativa a mais é uma tentativa legítima, não um pedido por acordada.
        let windows = ((now.elapsed() + Duration::from_millis(50)).as_millis() / RETRY.as_millis()) as usize + 1;
        let attempts = py.snapshots.load(std::sync::atomic::Ordering::SeqCst);
        let _ = windows;
        assert_eq!(attempts, 0, "nenhuma leitura de fatos migrados via HTTP");
    }

    #[tokio::test]
    async fn failing_snapshot_stays_off_the_state() {
        let f = claude_fixture();
        let py = fake_python_with(usize::MAX, "").await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(live("working")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        // O feed marca os fatos velhos (`state_facts_unavailable`); o portão do hub os tira da saída: é diagnóstico.
        assert_eq!((state["state"].as_str(), state["problema"].as_str()), (Some("working"), None), "{got:?}");
    }

    #[tokio::test]
    async fn stale_transfer_fact_does_not_hold_a_stopped_claude() {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        let py = fake_python_with(usize::MAX, "").await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let _watching = store.watch("s");
        let mut stale = facts(1, "");
        stale.transfer_active = true;
        store.snapshot("s", stale, std::time::Instant::now());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).with_home(home.path()).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!((state["state"].as_str(), state["problema"].as_str()), (Some("idle"), Some("runtime_falhou")), "{got:?}");
    }

    #[tokio::test]
    async fn skipped_sequence_rereads_the_snapshot() {
        let f = claude_fixture();
        let py = fake_python().await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(live("idle")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        collect(&mut rx, Duration::from_millis(400)).await;
        store.push("s", facts(5, "x"), std::time::Instant::now());
        collect(&mut rx, Duration::from_millis(400)).await;
        assert_eq!(hits(&py), 0, "sequência externa não aciona fatos migrados");
    }

    #[tokio::test]
    async fn dropping_the_feed_stops_watching() {
        let f = claude_fixture();
        let py = fake_python().await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let (_tx, live_rx) = channel(Some(live("idle")));
        let feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(store.get("s").is_none(), "o feed não hidrata a memória do plugin com HTTP");
        feed.abort();
        let _ = feed.await;
        assert!(store.get("s").is_none(), "sem feed, sem entrada");
    }

    #[tokio::test]
    async fn codex_feed_asks_no_facts() {
        let f = fixture();
        let py = fake_python().await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let (_tx, live_rx) = channel(Some(live("idle")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(hits(&py), 0, "Codex não tem plugin");
        assert_eq!(store.push("s", facts(1, "x"), std::time::Instant::now()), crate::state::facts::Push::Unwatched);
    }

    fn write_sidecar(home: &std::path::Path, body: Value) {
        let dir = home.join(".hangar").join("claude-headless");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s.json"), body.to_string()).unwrap();
    }

    #[tokio::test]
    async fn stopped_claude_state_carries_the_sidecar() {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        write_sidecar(home.path(), json!({"cwd": "/w", "session_id": "x", "model": "claude-opus-5", "effort": "high",
            "permission_mode": "plan", "previous_non_plan": "acceptEdits", "problema": ["limite_de_uso", "volta às 14:00"]}));
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), None).with_home(home.path()).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!(state["state"], "idle");
        assert_eq!(state["status_line"], "🤖 Opus5 (high)");
        assert_eq!(state["claude_permission_mode"], "plan");
        assert_eq!(state["claude_previous_non_plan"], "acceptEdits");
        assert_eq!(state["problema"], "limite_de_uso");
        assert_eq!(state["problema_detalhe"], "volta às 14:00");
    }

    #[tokio::test]
    async fn stopped_claude_without_sidecar_or_binding_is_idle_with_the_coded_problem() {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), None).with_home(home.path()).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!(state["state"], "idle");
        assert!(state["status_line"].is_null());
        // Sem vínculo comprovado não se decide `dead`: o problema aparece, nunca um estado inventado.
        assert_eq!(state["problema"], "runtime_falhou", "{state}");
        assert!(state["problema_detalhe"].as_str().is_some_and(|d| d.starts_with("state_lifecycle_unbound")), "{state}");
    }

    /// Claude parado sem sidecar, com os fatos do Python dizendo se há troca de modo em curso.
    async fn stopped_without_sidecar(transfer: bool) -> Vec<(Instant, String, Value)> {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        let py = fake_python_full(0, "", transfer).await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).with_home(home.path()).run());
        collect(&mut rx, Duration::from_millis(500)).await
    }

    #[tokio::test]
    async fn stopped_claude_without_sidecar_requires_lifecycle_controller() {
        let got = stopped_without_sidecar(false).await;
        assert_eq!(of(&got, "state")[0].2["problema"], "runtime_falhou", "{got:?}");
        assert!(of(&got, "state")[0].2["problema_detalhe"].as_str().unwrap().starts_with("state_lifecycle_unbound"));
    }

    #[tokio::test]
    async fn external_transfer_flag_cannot_decide_rust_lifecycle() {
        let got = stopped_without_sidecar(true).await;
        assert_eq!(of(&got, "state")[0].2["problema"], "runtime_falhou", "{got:?}");
        assert!(of(&got, "state")[0].2["problema_detalhe"].as_str().unwrap().starts_with("state_lifecycle_unbound"));
    }

    #[tokio::test]
    async fn stopped_codex_stays_bare_idle() {
        let f = fixture();
        let home = tempfile::tempdir().unwrap();
        write_sidecar(home.path(), json!({"cwd": "/w", "session_id": "x", "model": "claude-opus-5", "permission_mode": "plan"}));
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), None).with_home(home.path()).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        assert!(state["status_line"].is_null() && state["claude_permission_mode"].is_null(), "{state}");
    }

    #[test]
    fn refresh_delay() {
        let t = Instant::now();
        assert_eq!(refresh_in(None, false, t), Duration::ZERO, "nunca leu: agora");
        assert_eq!(refresh_in(Some(t), false, t + Duration::from_secs(10)), SNAPSHOT_REFRESH - Duration::from_secs(10));
        assert_eq!(refresh_in(Some(t), false, t + Duration::from_secs(40)), Duration::ZERO);
        assert_eq!(refresh_in(Some(t), true, t), RETRY, "falha tenta de novo logo, sem martelar");
    }
}
