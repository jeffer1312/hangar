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
use crate::state::facts::{FactsStore, SNAPSHOT_REFRESH, StateFactsClient};
use crate::state::published::Published;

/// Mesma janela da lista: rajada de deltas vira uma publicação.
pub const COALESCE: Duration = Duration::from_millis(150);
/// Retrato que falhou: tenta de novo sem martelar o Python.
const RETRY: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(5) };

/// De onde o feed do Claude lê a sugestão do plugin: os fatos que o Python empurra, com o interesse
/// renovado como o `Monitor` faz.
#[derive(Clone)]
pub struct FeedFacts { pub store: Arc<FactsStore>, pub client: StateFactsClient, pub diag: crate::diag::DiagClient }

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
    /// Estado da sessão Claude parada, lido do sidecar quando o feed acorda com ela parada.
    parked: Parked,
    /// Pasta do sidecar; `None` = o HOME do processo.
    home: Option<std::path::PathBuf>,
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

fn idle(name: &str) -> StateEvent { StateEvent { session: name.into(), state: "idle".into(), headless: true, ..Default::default() } }

/// Problema do próprio feed. Sai como `runtime_falhou`, o código que web, app e nativo traduzem;
/// o motivo vai no detalhe. Código novo sem tradução some da tela do web.
fn feed_problem(name: &str, code: &str, message: &str) -> StateEvent {
    StateEvent { problema: Some("runtime_falhou".into()), problema_detalhe: Some(actor_problem(code, message)), ..idle(name) }
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
            claude, parked: Parked::Absent, home: None }
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
        // Sem sidecar: `dead`, salvo na troca de modo, em que o Python não emite nada. Sem fatos, não se sabe: `idle` puro.
        // Retrato falhando: o último fato pode ser velho e não decide `dead` nem segura o estado.
        let facts = self.facts.as_ref().filter(|w| w.error.is_none()).and_then(|w| w.src.store.get(&self.name));
        match facts {
            Some(received) if received.in_transfer(std::time::Instant::now()) => Parked::Hold,
            Some(_) => Parked::State(StateEvent { state: "dead".into(), ..idle(&self.name) }),
            None => Parked::Absent,
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
            if !self.round() {
                return;
            }
            self.wait(pushed, retry_in).await;
            tokio::time::sleep(COALESCE).await;
        }
    }

    /// Relê o retrato dos fatos quando vence (o que renova o interesse no Python), quando faltou
    /// sequência ou depois de falha; devolve em quanto tempo precisa de novo.
    async fn refresh_facts(&mut self) -> Option<Duration> {
        let w = self.facts.as_mut()?;
        let failed = w.error.is_some();
        // Pulo de sequência relê na hora, mas não fura a espera depois de falha.
        if refresh_in(w.at, failed, Instant::now()).is_zero() || (!failed && w.src.store.needs_snapshot(&self.name)) {
            w.at = Some(Instant::now());
            match w.src.client.snapshot(&self.name).await {
                Ok(facts) => {
                    w.src.store.snapshot(&self.name, facts, std::time::Instant::now());
                    w.error = None;
                }
                Err(code) => {
                    if crate::warn_limit::allow(Some(&self.name), "rust.state_facts_failed") {
                        tracing::warn!(session = self.name.as_str(), code = code.as_str(), "estado: retrato dos fatos do Python não veio");
                        w.src.diag.report("rust.state_facts_failed", &self.name, &code, "estado: retrato dos fatos do Python não veio");
                    }
                    w.error = Some(code);
                }
            }
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
            };
            if !live_closed {
                return;
            }
            // Registro sumiu (servidor encerrando): ficam o hub, os fatos e o prazo.
            self.live = None;
        }
    }

    /// Publica o que mudou; `false` com o hub fechado.
    fn round(&mut self) -> bool {
        let Some(hub) = self.hub.upgrade() else { return false };
        let Some(generation) = hub.generation() else { return false };
        if self.sent.generation != Some(generation) {
            self.sent = Sent { generation: Some(generation), ..Sent::default() };
        }
        let value = self.live.as_mut().map(|rx| rx.borrow_and_update().clone());
        let (mut state, texts) = match &value {
            None => (feed_problem(&self.name, "runtime_absent", "o servidor não tem o runtime ligado"), Default::default()),
            // Sessão parada (não aberta no Rust, encerrada, ou abrindo): `idle`, como o Python. Abertura
            // que falhou e vida que acabou com erro chegam como `Some` com o erro.
            // Claude parado leva o que o sidecar diz dele; sem sidecar (ou Codex) fica o `idle` puro.
            Some(None) => match &self.parked {
                Parked::State(state) => (state.clone(), Default::default()),
                Parked::Hold => return true,
                Parked::Absent => (idle(&self.name), Default::default()),
            },
            Some(Some(live)) => {
                let mut state = if live.public_state.is_null() { idle(&self.name) } else {
                    serde_json::from_value(live.public_state.clone()).unwrap_or_else(|_| {
                        if crate::warn_limit::allow(Some(&self.name), "state_feed_invalid") {
                            tracing::warn!(session = self.name.as_str(), code = "state_feed_invalid", "estado: vista do ator não é um estado");
                        }
                        feed_problem(&self.name, "state_feed_invalid", "a vista do ator não é um estado")
                    })
                };
                if let Some((code, message)) = &live.error {
                    state.problema = Some("runtime_falhou".into());
                    state.problema_detalhe = Some(actor_problem(code, message));
                }
                (state, [live.preview.clone(), live.thinking.clone(), live.tool.clone()])
            }
        };
        // Como no `Monitor`: sem retrato, a sugestão e a sessão parada podem estar velhas, e a tela diz isso.
        if let Some(code) = self.facts.as_ref().and_then(|w| w.error.as_ref()) {
            if state.problema.is_none() {
                state.problema = Some(super::facts::UNAVAILABLE.into());
                state.problema_detalhe = Some(code.clone());
            }
        }
        let [preview, thinking, tool] = texts;
        let preview = match hub.committed() {
            Some(committed) if super::preview::is_committed(&preview, &committed) => String::new(),
            _ => preview,
        };
        // A sugestão só muda por fato novo; vazia desde o início não é mudança.
        let suggestion = self.facts.as_ref().and_then(|w| w.src.store.get(&self.name)).map(|r| r.facts.suggestion.clone());
        if let Some(text) = suggestion.filter(|t| t != self.sent.suggestion.as_deref().unwrap_or("")) {
            if !hub.publish_own("suggest", &serde_json::json!({"text": text}).to_string()) { return false; }
            self.sent.suggestion = Some(text);
        }
        // Mesma ordem do `sse.py`: a pergunta sai antes do estado que a abre.
        let question = serde_json::to_string(&state.codex_question).unwrap_or_else(|_| "null".into());
        if !self.emit(&hub, "ask_question", question, |s| &mut s.question) { return false; }
        let data = serde_json::to_string(&state).unwrap_or_default();
        if self.sent.state.as_deref() != Some(data.as_str()) {
            let sid = hub.jsonl().and_then(|j| j.file_stem().and_then(|s| s.to_str()).map(str::to_owned));
            if !self.emit(&hub, "state", data, |s| &mut s.state) { return false; }
            self.published.set(self.owner, &self.name, sid, Arc::new(state));
        }
        let preview = serde_json::to_string(&PreviewEvent { session: self.name.clone(), text: preview, md: true, full: true, vivo: true }).unwrap_or_default();
        self.emit(&hub, "preview", preview, |s| &mut s.preview)
            && self.emit(&hub, "pensamento", serde_json::json!({"text": thinking}).to_string(), |s| &mut s.thinking)
            && self.emit(&hub, "ferramenta", serde_json::json!({"text": tool}).to_string(), |s| &mut s.tool)
    }

    /// Publica `data` se mudou desde o último deste evento; `false` com o hub fechado.
    fn emit(&mut self, hub: &Hub, event: &str, data: String, slot: impl FnOnce(&mut Sent) -> &mut Option<String>) -> bool {
        let last = slot(&mut self.sent);
        if last.as_deref() == Some(data.as_str()) {
            return true;
        }
        if !hub.publish_own(event, &data) {
            return false;
        }
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
    let state = feed_problem(&hub.name, "state_feed_failed", "o estado da sessão caiu; volta ao reabrir o chat");
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

    struct Fixture { _dir: tempfile::TempDir, lease: crate::side::Lease, published: Arc<Published> }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx();
        let binding = Binding { provider: Provider::Codex, jsonl: dir.path().join("rollout-abc.jsonl"), key: "thread-1".into(), headless: true };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        Fixture { _dir: dir, lease, published: Arc::default() }
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
    async fn no_registry_is_runtime_absent() {
        let f = fixture();
        let mut rx = f.lease.hub.tx.subscribe();
        let _feed = spawn(&f, None);
        let got = collect(&mut rx, Duration::from_millis(50)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!(state["problema"], "runtime_falhou");
        assert!(state["problema_detalhe"].as_str().unwrap().starts_with("runtime_absent:"), "{state}");
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
        let binding = Binding { provider: Provider::ClaudeHeadless, jsonl: dir.path().join("sid.jsonl"), key: "sid".into(), headless: false };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        Fixture { _dir: dir, lease, published: Arc::default() }
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
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let read = s.read(&mut buf).await.unwrap_or(0);
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
            client: crate::state::facts::StateFactsClient::new(py.addr, "s".into()),
            diag: crate::diag::DiagClient::new(py.addr, "s".into()),
        }
    }

    fn hits(py: &FakePython) -> usize { py.hits.load(std::sync::atomic::Ordering::SeqCst) }

    fn texts(got: &[(Instant, String, Value)]) -> Vec<Value> {
        of(got, "suggest").into_iter().map(|e| e.2.clone()).collect()
    }

    // Tempo real: o Python de mentira é um socket de verdade, e relógio pausado adianta o prazo do pedido.
    #[tokio::test]
    async fn claude_suggestion_follows_pushed_facts() {
        let f = claude_fixture();
        let py = fake_python().await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(live("idle")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        let first = collect(&mut rx, Duration::from_millis(400)).await;
        assert_eq!(hits(&py), 1, "lê o retrato uma vez e registra o interesse");
        assert!(of(&first, "suggest").is_empty(), "vazia desde o início não é mudança");
        let now = std::time::Instant::now();
        store.push("s", facts(2, "roda os testes"), now);
        assert_eq!(texts(&collect(&mut rx, Duration::from_millis(400)).await), [json!({"text": "roda os testes"})]);
        store.push("s", facts(3, "roda os testes"), now);
        assert!(texts(&collect(&mut rx, Duration::from_millis(400)).await).is_empty(), "igual não repete");
        store.push("s", facts(4, ""), now);
        assert_eq!(texts(&collect(&mut rx, Duration::from_millis(400)).await), [json!({"text": ""})], "turno novo zera a sugestão");
        assert_eq!(hits(&py), 1);
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
        assert_eq!(py.snapshots.load(std::sync::atomic::Ordering::SeqCst), 2, "a primeira falhou e a segunda registrou o interesse");
        assert_eq!(texts(&got), [json!({"text": "depois da falha"})]);
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
        assert!(attempts >= 2 && attempts <= windows, "{attempts} tentativas em ~1 s com retentativa de {RETRY:?}");
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
        let windows = (Duration::from_millis(1050).as_millis() / RETRY.as_millis()) as usize + 1;
        let attempts = py.snapshots.load(std::sync::atomic::Ordering::SeqCst);
        assert!(attempts >= 2 && attempts <= windows, "{attempts} tentativas em ~1 s com retentativa de {RETRY:?}");
    }

    #[tokio::test]
    async fn failing_snapshot_shows_on_the_state() {
        let f = claude_fixture();
        let py = fake_python_with(usize::MAX, "").await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(Some(live("working")));
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!((state["state"].as_str(), state["problema"].as_str()), (Some("working"), Some(crate::state::facts::UNAVAILABLE)), "{got:?}");
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
        assert_eq!((state["state"].as_str(), state["problema"].as_str()), (Some("idle"), Some(crate::state::facts::UNAVAILABLE)), "{got:?}");
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
        assert_eq!(hits(&py), 2, "faltou sequência: relê o retrato");
    }

    #[tokio::test]
    async fn dropping_the_feed_stops_watching() {
        let f = claude_fixture();
        let py = fake_python().await;
        let store = Arc::new(crate::state::facts::FactsStore::default());
        let (_tx, live_rx) = channel(Some(live("idle")));
        let feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), Some(feed_facts(&py, &store))).run());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(store.get("s").is_some());
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
    async fn stopped_claude_without_sidecar_stays_bare_idle() {
        let f = claude_fixture();
        let home = tempfile::tempdir().unwrap();
        let mut rx = f.lease.hub.tx.subscribe();
        let (_tx, live_rx) = channel(None);
        let _feed = tokio::spawn(RuntimeFeed::new(&f.lease.hub, Some(live_rx), f.published.clone(), None).with_home(home.path()).run());
        let got = collect(&mut rx, Duration::from_millis(500)).await;
        let state = &of(&got, "state")[0].2;
        assert_eq!(state["state"], "idle");
        assert!(state["status_line"].is_null());
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
    async fn stopped_claude_without_sidecar_is_dead() {
        let got = stopped_without_sidecar(false).await;
        assert_eq!(of(&got, "state")[0].2["state"], "dead", "{got:?}");
    }

    #[tokio::test]
    async fn stopped_claude_without_sidecar_during_a_mode_swap_publishes_nothing() {
        let got = stopped_without_sidecar(true).await;
        assert!(of(&got, "state").is_empty(), "{got:?}");
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
