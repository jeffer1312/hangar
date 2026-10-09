//! `Monitor` de estado de uma sessão Claude com terminal: porte do `StateMonitor` (`state.py`).
//! Uma rodada a cada `POLL`, ou antes quando o plugin vivo acorda (empurrão dos fatos). A memória
//! temporal do `reduce` anda por rodada, contadas como o Python conta, para as sequências gravadas
//! por ele valerem como prova. Ninguém cria um `Monitor` em produção antes da troca de dono do
//! estado: até lá só os testes o exercitam.
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_api::ask::AskQuestion;
use hangar_api::preview::PreviewEvent;
use hangar_api::state::{ShellVivo, StateEvent};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::edges::{self, Edges};
use super::facts::{Dead, Received, UNAVAILABLE};
use super::{ask, permission};
use super::preview::{self, HookFile};
use crate::terminal_control::{CaptureRequest, TerminalPool};
use crate::terminal_state::{self, PaneAnalysis, ReducerFacts, ReducerMemory, TerminalQuestion};

pub const POLL: Duration = Duration::from_millis(750);
/// Rodadas sem spinner depois das quais o marcador `working` deixa de valer (`HOOK_WORKING_GRACE`).
pub const HOOK_GRACE: u32 = 8;
pub const OBSERVATION_FAILED: &str = "terminal_observacao_falhou";
pub const PERMISSION_FAILED: &str = "permission_observe_failed";

/// Quadro capturado e a análise que o pool já fez dele.
#[derive(Clone, Debug)]
pub struct Frame { pub text: String, pub analysis: PaneAnalysis }

/// Captura que falhou. `attempt` é a tentativa guardada do pool (`None`: falha que ele não guarda):
/// enquanto não muda, o pool só repete a mesma falha sem tentar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureFailed { pub code: String, pub attempt: Option<u32> }

/// Fatos do Python valendo agora.
#[derive(Clone, Debug, Default)]
pub struct RoundFacts {
    pub alive: bool,
    pub plugin_state: Option<String>,
    /// A pergunta segurada no formato de `pergunta_pendente` (`id`, `questions`, `tool`, `resumo`).
    pub question: Option<Value>,
    pub in_transfer: bool,
    pub permission_op: bool,
    /// A sugestão do plugin; `None` sem fato nenhum (nada muda).
    pub suggestion: Option<String>,
    /// O retrato não veio: código, e o último valor fica.
    pub unavailable: Option<String>,
    /// Largura da conversa com painel ancorado e começo da faixa dos mods: cortes da prévia.
    pub body_columns: Option<u32>,
    pub band_anchor: Option<String>,
}

impl RoundFacts {
    pub fn from_received(r: Option<&Received>, now: Instant, unavailable: Option<String>) -> Self {
        let Some(r) = r else { return Self { unavailable, ..Self::default() } };
        Self {
            alive: r.alive(now),
            plugin_state: r.plugin_state(now).map(|p| p.state.clone()),
            question: r.question(now).map(|q| json!({"id": q.id, "questions": q.questions, "tool": q.tool, "resumo": q.resumo})),
            in_transfer: r.in_transfer(now),
            permission_op: r.facts.permission_op,
            suggestion: Some(r.facts.suggestion.clone()),
            unavailable,
            body_columns: r.facts.body_columns,
            band_anchor: r.facts.band_anchor.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopInfo { pub status: Option<String>, pub iter: Option<u32>, pub max: Option<u32> }

/// Arquivos da sessão lidos na rodada; sem session-id, só o loop.
#[derive(Clone, Debug, Default)]
pub struct FileFacts {
    pub marker: Option<String>,
    /// `ts` do marcador, no relógio de parede.
    pub marker_ts: Option<f64>,
    pub open_question: Option<TerminalQuestion>,
    pub status_line: Option<String>,
    pub loop_info: Option<LoopInfo>,
    pub shells: Vec<ShellVivo>,
    /// Leitura que falhou (código): os campos acima ficaram vazios e o estado sai com o problema.
    pub unavailable: Option<String>,
}

/// O que o `Monitor` lê e a quem publica. Quem implementa faz a E/S bloqueante fora do runtime.
/// Nenhum método tem corpo padrão: a fonte de produção que esquecer um não compila.
pub trait Sources: Send + Sync {
    fn name(&self) -> &str;
    fn sid(&self) -> Option<String>;
    /// Muda no `rebind` do hub (`/clear`, troca do filho): a rodada em curso não vale mais.
    fn epoch(&self) -> u64;
    /// Avisado quando o Python empurra fatos novos desta sessão.
    fn wake(&self) -> Arc<Notify>;
    /// Avisado (`notify_one`) pelo hub no `rebind` (rodada já) e quando uma resposta entra no
    /// transcript (a prévia que a repetia sai já); lido uma vez no `run`.
    fn hub_wake(&self) -> Arc<Notify>;
    fn facts(&self) -> impl Future<Output = RoundFacts> + Send;
    fn capture(&self) -> impl Future<Output = Result<Frame, CaptureFailed>> + Send;
    /// `None` = o multiplexador não respondeu, que não é sessão morta.
    fn has_session(&self) -> impl Future<Output = Option<bool>> + Send;
    fn dead(&self) -> impl Future<Output = Result<Dead, String>> + Send;
    fn observe_permission(&self, key: &str, mode: &str) -> impl Future<Output = Result<(String, String), String>> + Send;
    fn files(&self, sid: Option<&str>) -> impl Future<Output = FileFacts> + Send;
    /// `false`: ninguém mais ouve, e o `Monitor` acaba.
    fn publish(&self, event: StateEvent) -> impl Future<Output = bool> + Send;
    /// Os outros eventos da sessão (`suggest`, `ask_question`), mesma regra do `publish`.
    fn emit(&self, event: &'static str, data: Value) -> impl Future<Output = bool> + Send;
    /// Avisado (`notify_one`) quando o ator de entrada da sessão publica; lido uma vez no `run`.
    fn runtime_wake(&self) -> Arc<Notify>;
    /// `edges::runtime_problem` do que o ator publicou por último; `None` fora do Rust.
    fn runtime_problem(&self) -> Option<(String, String)>;
    /// O sidecar do AskUserQuestion (`ask::read_pending`, fora do runtime).
    fn ask_payload(&self) -> impl Future<Output = Result<Option<AskQuestion>, String>> + Send;
    /// `session.deliverable`: só dispara; quem implementa não segura a rodada e registra a falha.
    fn deliverable(&self);
    /// Captura só para a prévia, entre as rodadas de estado; `None`: a fonte não as faz.
    fn preview_capture(&self) -> impl Future<Output = Option<Result<Frame, CaptureFailed>>> + Send;
    /// `.hangar-preview/<stem>.json` legíveis, na ordem das pastas de config. Chamado a cada toque
    /// rápido: quem implementa lê o disco fora do runtime (`HookFiles` em `spawn_blocking`).
    fn preview_files(&self, stem: &str) -> impl Future<Output = Vec<HookFile>> + Send;
    /// Última resposta já gravada no transcript, normalizada (`preview::norm`).
    fn committed(&self) -> Option<Arc<str>>;
    fn publish_preview(&self, event: PreviewEvent) -> impl Future<Output = bool> + Send;
    /// Relógio de parede em segundos, o do `ts` do arquivo do hook e do marcador.
    fn wall(&self) -> f64;
}

pub fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Exit { Dead, Closed }

enum Step { Exit(Exit), Again, Sleep, Wait { alive: bool } }

#[derive(Default)]
struct Memory {
    reducer: ReducerMemory,
    /// Último evento publicado, com os shells reduzidos aos pids (o tempo deles corre sozinho).
    key: Option<StateEvent>,
    last: Option<StateEvent>,
    permission: permission::Watch,
    /// Em falha da observação: a tentativa do pool já conferida com `has-session`.
    failure: Option<Option<u32>>,
    preview: preview::Slot,
    /// `ask_question` já saiu para a pergunta na tela; zera quando ela sai e no `/clear`.
    asked: bool,
}

pub struct Monitor<S> { src: S, poll: Duration, mem: Memory, epoch: u64, edges: Edges }

fn key_of(event: &StateEvent) -> StateEvent {
    let shells = event.shells.iter().map(|s| ShellVivo { pid: s.pid, ..ShellVivo::default() }).collect();
    StateEvent { shells, ..event.clone() }
}

fn anchor(state: Option<&str>) -> Option<&str> { state.filter(|s| matches!(*s, "working" | "idle")) }

impl<S: Sources> Monitor<S> {
    pub fn new(src: S) -> Self { Self::with_poll(src, POLL) }

    pub fn with_poll(src: S, poll: Duration) -> Self {
        let epoch = src.epoch();
        Self { src, poll, mem: Memory::default(), epoch, edges: Edges::default() }
    }

    pub async fn run(mut self) -> Exit {
        let wake = self.src.wake();
        let runtime = self.src.runtime_wake();
        let hub = self.src.hub_wake();
        loop {
            // Armado antes da rodada: empurrão que chega durante ela acorda a espera seguinte.
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let exit = match self.round().await {
                Step::Exit(exit) => Some(exit),
                Step::Again => None,
                Step::Sleep | Step::Wait { alive: false } => self.idle(None, &runtime, &hub).await,
                Step::Wait { alive: true } => self.idle(Some(notified.as_mut()), &runtime, &hub).await,
            };
            if let Some(exit) = exit {
                return exit;
            }
        }
    }

    /// Espera a próxima rodada de estado (o relógio, ou o empurrão quando `woken` vem). Enquanto
    /// a prévia corre, toques de `preview::FAST` só para ela, fora da contagem das rodadas. O ator
    /// de entrada acorda só para o problema dele, também fora da contagem. O hub acorda para a
    /// rodada da época nova (`rebind`) ou para tirar a prévia que acabou de ser gravada.
    async fn idle(&mut self, mut woken: Option<std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>>, runtime: &Notify, hub: &Notify) -> Option<Exit> {
        let deadline = tokio::time::Instant::now() + self.poll;
        loop {
            let until = if self.mem.preview.fast { (tokio::time::Instant::now() + preview::FAST).min(deadline) } else { deadline };
            let wake = async {
                match woken.as_mut() {
                    Some(n) => n.as_mut().await,
                    None => std::future::pending().await,
                }
            };
            enum Woke { Clock, Actor, Hub }
            let woke = tokio::select! {
                () = tokio::time::sleep_until(until) => Woke::Clock,
                () = wake => return None,
                () = runtime.notified() => Woke::Actor,
                () = hub.notified() => Woke::Hub,
            };
            match woke {
                Woke::Clock => {}
                Woke::Actor => {
                    if let Some(Step::Exit(exit)) = self.runtime_changed().await {
                        return Some(exit);
                    }
                    continue;
                }
                Woke::Hub if self.src.epoch() != self.epoch => return None,
                Woke::Hub => {
                    if !preview::committed_changed(&self.src, &mut self.mem.preview, self.epoch).await {
                        return Some(Exit::Closed);
                    }
                    continue;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            if !preview::tick(&self.src, &mut self.mem.preview, None, self.epoch).await {
                return Some(Exit::Closed);
            }
        }
    }

    /// `ask_question` uma vez por pergunta na tela. Enquanto ela não sai, o sidecar é relido a cada
    /// rodada, como o Python a cada tique: o hook pode gravá-lo depois de o menu aparecer.
    async fn ask(&mut self, event: &StateEvent) -> Option<Step> {
        if event.state != "awaiting_input" {
            self.mem.asked = false;
            return None;
        }
        if self.mem.asked {
            return None;
        }
        let failed = match self.src.ask_payload().await {
            Ok(Some(payload)) if ask::matches(event, &payload) => match serde_json::to_value(&payload) {
                Ok(data) => {
                    self.mem.asked = true;
                    return (!self.src.emit("ask_question", data).await).then_some(Step::Exit(Exit::Closed));
                }
                Err(_) => "askq_serialize".to_owned(),
            },
            Ok(_) => return None,
            Err(code) => code,
        };
        if crate::warn_limit::allow(Some(self.src.name()), &failed) {
            tracing::warn!(session = self.src.name(), code = failed.as_str(), "estado: pergunta nativa sem sidecar legível");
        }
        None
    }

    async fn publish(&mut self, event: StateEvent) -> Option<Step> {
        // Como o laço do `sse.py`: a pergunta nativa sai antes do estado que a mostra.
        if let Some(step) = self.ask(&event).await {
            return Some(step);
        }
        if self.edges.deliverable(&event) {
            self.src.deliverable();
        }
        self.mem.last = Some(event.clone());
        (!self.src.publish(event).await).then_some(Step::Exit(Exit::Closed))
    }

    /// O ator de entrada publicou: o problema dele entra ou sai do último estado sem rodada nova
    /// (a memória temporal não anda). Problema da observação ou dos fatos fica até a rodada boa.
    async fn runtime_changed(&mut self) -> Option<Step> {
        let last = self.mem.last.as_ref()?;
        if last.problema.as_deref().is_some_and(|p| !edges::is_runtime_problem(p)) {
            return None;
        }
        let now = self.src.runtime_problem();
        let same = match &now {
            Some((p, d)) => last.problema.as_deref() == Some(p.as_str()) && last.problema_detalhe.as_deref() == Some(d.as_str()),
            None => last.problema.is_none(),
        };
        if same {
            return None;
        }
        let (problema, problema_detalhe) = now.unzip();
        let event = StateEvent { problema, problema_detalhe, ..last.clone() };
        self.mem.key = Some(key_of(&event));
        self.publish(event).await
    }

    async fn round(&mut self) -> Step {
        let epoch = self.src.epoch();
        if epoch != self.epoch {
            // `/clear` ou troca do filho: quadro, chave e memória temporal são da conversa anterior.
            // A prévia zera sem publicar: o `rebind` do hub apaga o retrato e o app recebe `reset`.
            (self.mem, self.epoch) = (Memory::default(), epoch);
        }
        let captured = self.src.capture().await;
        let facts = self.src.facts().await;
        if self.src.epoch() != epoch {
            return Step::Again;
        }
        // A sugestão sai quando muda, mesmo com o estado parado.
        if let Some(text) = self.edges.suggestion(facts.suggestion.as_deref())
            && !self.src.emit("suggest", json!({"text": text})).await
        {
            return Step::Exit(Exit::Closed);
        }
        // `has-session` só separa "morreu" de "pane em branco": no quadro vazio, e na falha ao
        // entrar nela e a cada nova tentativa do pool (até lá ele repete a mesma falha sem tentar).
        // Falha que o pool não guarda é tentativa nova a cada rodada.
        let probe = match &captured {
            Ok(frame) => {
                self.mem.failure = None;
                frame.text.is_empty()
            }
            Err(f) => f.attempt.is_none() || self.mem.failure != Some(f.attempt),
        };
        if probe {
            match self.src.has_session().await {
                // Só a resposta definitiva conta como conferida; o resto repete no tique seguinte.
                Some(true) => if let Err(f) = &captured { self.mem.failure = Some(f.attempt) },
                None => {
                    if crate::warn_limit::allow(Some(self.src.name()), "state_mux_no_answer") {
                        tracing::warn!(session = self.src.name(), code = "state_mux_no_answer", "estado: has-session sem resposta");
                    }
                }
                Some(false) if facts.in_transfer => return Step::Sleep,
                Some(false) => match self.src.dead().await {
                    Ok(Dead::Ok) => {
                        let _ = self.src.publish(StateEvent { session: self.src.name().to_owned(), state: "dead".into(), ..Default::default() }).await;
                        return Step::Exit(Exit::Dead);
                    }
                    Ok(Dead::InTransfer) => return Step::Sleep,
                    Err(code) => return self.fail(UNAVAILABLE, code, &facts).await,
                },
            }
        }
        match captured {
            Err(f) => self.fail(OBSERVATION_FAILED, f.code, &facts).await,
            Ok(frame) => self.reduce(frame, facts, epoch).await,
        }
    }

    /// O estado fica no último evento, com o problema; um evento por código.
    async fn fail(&mut self, problem: &str, detail: String, facts: &RoundFacts) -> Step {
        // A prévia fica com o texto que tinha e espera a próxima rodada boa.
        self.mem.preview.fast = false;
        if self.mem.last.as_ref().is_some_and(|l| l.problema_detalhe.as_deref() == Some(detail.as_str())) {
            return Step::Sleep;
        }
        let base = match self.mem.last.take() {
            Some(last) => last,
            None => {
                // Sem quadro anterior, o estado sai das âncoras que não dependem do pane.
                let sid = self.src.sid();
                let found = match (&facts.plugin_state, &sid) {
                    (Some(p), _) => Some(p.clone()),
                    (None, Some(sid)) => self.src.files(Some(sid)).await.marker,
                    (None, None) => None,
                };
                let state = anchor(found.as_deref()).unwrap_or(&self.mem.reducer.held_state).to_owned();
                StateEvent { session: self.src.name().to_owned(), state, ..Default::default() }
            }
        };
        if crate::warn_limit::allow(Some(self.src.name()), problem) {
            tracing::warn!(session = self.src.name(), code = problem, detail, "estado: rodada sem quadro");
        }
        let event = StateEvent { problema: Some(problem.to_owned()), problema_detalhe: Some(detail), ..base };
        // A rodada boa seguinte publica de novo, mesmo igual à de antes da falha.
        self.mem.key = None;
        self.publish(event).await.unwrap_or(Step::Sleep)
    }

    async fn reduce(&mut self, frame: Frame, facts: RoundFacts, epoch: u64) -> Step {
        let name = self.src.name().to_owned();
        let sid = self.src.sid();
        let files = self.src.files(sid.as_deref()).await;
        // Arquivo da sessão que não se leu também é fato que faltou: sem ele o estado viria sem
        // marcador nem pergunta aberta, como se fosse verdade.
        let mut problem = facts.unavailable.clone().or_else(|| files.unavailable.clone()).map(|d| (UNAVAILABLE, d));
        if let Some(observed) = permission::parse_permission_mode(&frame.text) {
            let key = sid.clone().unwrap_or_else(|| name.clone());
            if self.mem.permission.due(&key, observed, facts.permission_op) {
                match self.src.observe_permission(&key, observed).await {
                    Ok(answer) => self.mem.permission.answered(&key, observed, facts.permission_op, answer),
                    Err(code) => {
                        if crate::warn_limit::allow(Some(&name), PERMISSION_FAILED) {
                            tracing::warn!(session = name.as_str(), code = PERMISSION_FAILED, detail = code.as_str(), "estado: permission.observe falhou");
                        }
                        problem.get_or_insert((PERMISSION_FAILED, code));
                    }
                }
            }
        }
        if self.src.epoch() != epoch {
            return Step::Again;
        }
        let marker = files.marker.clone().zip(files.marker_ts);
        self.mem.preview.set_view(facts.body_columns, facts.band_anchor.clone(), marker);
        if !preview::tick(&self.src, &mut self.mem.preview, Some(&frame), epoch).await {
            return Step::Exit(Exit::Closed);
        }
        let reducer_facts = ReducerFacts {
            open_question: files.open_question, plugin_question: facts.question, plugin_state: facts.plugin_state,
            hook_state: files.marker, hook_grace: Some(HOOK_GRACE), status_line: files.status_line,
        };
        let (reduced, diagnostic) = terminal_state::reduce_analysis(frame.analysis, std::mem::take(&mut self.mem.reducer), reducer_facts);
        self.mem.reducer = reduced.memory;
        if let Some((plugin, pane)) = diagnostic.divergence {
            tracing::info!(session = name.as_str(), code = "state_plugin_diverged", plugin, pane, "estado: o plugin corrigiu o pane");
        }
        let a = reduced.analysis;
        let (mode, previous) = self.mem.permission.current.clone().unzip();
        let loop_info = files.loop_info.unwrap_or_default();
        // O problema do runtime entra na chave: muda o evento mesmo com a tela parada.
        let problem = problem.map(|(p, d)| (p.to_owned(), d)).or_else(|| self.src.runtime_problem());
        let (problema, problema_detalhe) = problem.unzip();
        let event = StateEvent {
            session: name, state: a.state, label: a.label, question: a.question, options: a.options,
            status_line: a.status_line, overlay: a.overlay, login: a.login, limited: a.limit_reset.is_some(),
            limit_reset: a.limit_reset, loop_status: loop_info.status, loop_iter: loop_info.iter, loop_max: loop_info.max,
            claude_permission_mode: mode, claude_previous_non_plan: previous, shells: files.shells,
            problema, problema_detalhe, ..Default::default()
        };
        let key = key_of(&event);
        if self.mem.key.as_ref() != Some(&key) {
            self.mem.key = Some(key);
            if let Some(step) = self.publish(event).await {
                return step;
            }
        } else if let Some(step) = self.ask(&event).await {
            return step;
        }
        Step::Wait { alive: facts.alive }
    }
}

/// Captura em processo pelo `TerminalPool` (cliente `-C`), sem HTTP.
pub struct PoolCapture { pool: TerminalPool, request: CaptureRequest }

impl PoolCapture {
    /// `binding` é o session-id da conversa; `target` o pane do agente ou `=nome:`.
    pub fn new(pool: TerminalPool, name: &str, binding: &str, target: String) -> Self {
        Self::with_consumer(pool, format!("monitor:{name}"), name, binding, target)
    }

    /// Consumidor próprio: quem solta o vínculo ao terminar não pode ser o do `Monitor` vivo da sessão,
    /// que dividiria o mesmo observador.
    pub fn with_consumer(pool: TerminalPool, consumer: String, name: &str, binding: &str, target: String) -> Self {
        let request = CaptureRequest { consumer, name: name.to_owned(), provider: "claude".into(),
            binding: binding.to_owned(), target, started: 0.0, lines: 200, colors: false, join: false };
        Self { pool, request }
    }

    pub async fn capture(&self) -> Result<Frame, CaptureFailed> {
        match self.pool.capture(self.request.clone()).await {
            Ok(r) => Ok(Frame { text: r.text, analysis: r.analysis }),
            Err(e) => Err(CaptureFailed { code: e.0.to_owned(), attempt: self.pool.failure_attempts(&self.request, &e).await }),
        }
    }

    pub async fn release(&self) {
        if let Err(e) = self.pool.release(&self.request.consumer).await {
            tracing::warn!(session = self.request.name.as_str(), code = e.0, "estado: liberar a observação falhou");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    /// Fonte de mentira: um quadro por rodada, repetindo o último quando acabam.
    struct Fake {
        frames: Vec<Result<&'static str, CaptureFailed>>,
        round: AtomicU32,
        has_session: AtomicU32,
        facts: Mutex<RoundFacts>,
        wake: Arc<Notify>,
        epoch: AtomicU64,
        events: Mutex<Vec<(u32, StateEvent)>>,
        others: Mutex<Vec<(u32, &'static str, Value)>>,
        runtime: Mutex<Option<(String, String)>>,
        runtime_wake: Arc<Notify>,
        ask: Mutex<Option<AskQuestion>>,
        ask_reads: AtomicU32,
        deliveries: Mutex<Vec<u32>>,
        files_failed: Mutex<Option<String>>,
    }

    impl Fake {
        fn new(frames: Vec<Result<&'static str, CaptureFailed>>) -> Arc<Self> {
            Arc::new(Self { frames, round: AtomicU32::new(0), has_session: AtomicU32::new(0), facts: Mutex::default(),
                wake: Arc::default(), epoch: AtomicU64::new(0), events: Mutex::default(), others: Mutex::default(),
                runtime: Mutex::default(), runtime_wake: Arc::default(), ask: Mutex::default(), ask_reads: AtomicU32::new(0),
                deliveries: Mutex::default(), files_failed: Mutex::default() })
        }
        fn rounds(&self) -> u32 { self.round.load(Ordering::SeqCst) }
        fn states(&self) -> Vec<String> { self.events.lock().unwrap().iter().map(|(_, e)| e.state.clone()).collect() }
    }

    impl Sources for Arc<Fake> {
        fn name(&self) -> &str { "s" }
        fn sid(&self) -> Option<String> { Some("sid".into()) }
        fn epoch(&self) -> u64 { self.epoch.load(Ordering::SeqCst) }
        fn wake(&self) -> Arc<Notify> { self.wake.clone() }
        async fn facts(&self) -> RoundFacts { self.facts.lock().unwrap().clone() }
        async fn capture(&self) -> Result<Frame, CaptureFailed> {
            let i = self.round.fetch_add(1, Ordering::SeqCst) as usize;
            let frame = self.frames[i.min(self.frames.len() - 1)].clone();
            frame.map(|t| Frame { text: t.into(), analysis: terminal_state::analyze(t) })
        }
        async fn has_session(&self) -> Option<bool> { self.has_session.fetch_add(1, Ordering::SeqCst); Some(true) }
        async fn dead(&self) -> Result<Dead, String> { Ok(Dead::Ok) }
        async fn observe_permission(&self, _: &str, mode: &str) -> Result<(String, String), String> { Ok((mode.into(), "manual".into())) }
        async fn files(&self, _: Option<&str>) -> FileFacts {
            FileFacts { unavailable: self.files_failed.lock().unwrap().clone(), ..FileFacts::default() }
        }
        async fn publish(&self, event: StateEvent) -> bool {
            self.events.lock().unwrap().push((self.rounds(), event));
            true
        }
        async fn emit(&self, event: &'static str, data: Value) -> bool {
            self.others.lock().unwrap().push((self.rounds(), event, data));
            true
        }
        fn runtime_wake(&self) -> Arc<Notify> { self.runtime_wake.clone() }
        fn runtime_problem(&self) -> Option<(String, String)> { self.runtime.lock().unwrap().clone() }
        async fn ask_payload(&self) -> Result<Option<AskQuestion>, String> {
            self.ask_reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.ask.lock().unwrap().clone())
        }
        fn deliverable(&self) { self.deliveries.lock().unwrap().push(self.rounds()); }
        fn hub_wake(&self) -> Arc<Notify> { Arc::default() }
        async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { None }
        async fn preview_files(&self, _: &str) -> Vec<HookFile> { Vec::new() }
        fn committed(&self) -> Option<Arc<str>> { None }
        async fn publish_preview(&self, _: PreviewEvent) -> bool { true }
        fn wall(&self) -> f64 { wall_now() }
    }

    const SPINNER: &str = "✻ Thinking…\n────────────\n❯\n────────────";

    #[tokio::test(start_paused = true)]
    async fn rounds_counted_like_python() {
        // Spinner parado: STALE_LIMIT rodadas iguais viram idle. Com o plugin vivo acordando, as
        // rodadas acordadas contam igual às do relógio, e o idle chega antes de 3 × POLL.
        let fake = Fake::new(vec![Ok(SPINNER)]);
        fake.facts.lock().unwrap().alive = true;
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            fake.wake.notify_waiters();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.rounds(), 5, "uma rodada no início e uma por empurrão");
        assert_eq!(fake.states(), ["working", "idle"]);
        assert_eq!(fake.events.lock().unwrap()[1].0, 4, "idle na 4ª rodada, como o Python");
        // Sem plugin vivo, o empurrão não acorda: só o relógio.
        fake.facts.lock().unwrap().alive = false;
        tokio::time::sleep(POLL).await;
        let before = fake.rounds();
        fake.wake.notify_waiters();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.rounds(), before, "empurrão sem long-poll vivo não conta rodada");
        tokio::time::sleep(POLL).await;
        assert_eq!(fake.rounds(), before + 1);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn has_session_only_on_failure_entry_and_retry() {
        let fail = |attempt| Err(CaptureFailed { code: "terminal observer EOF".into(), attempt });
        let mut frames = vec![Ok(SPINNER)];
        frames.extend(std::iter::repeat_n(fail(Some(0)), 5));
        frames.extend(std::iter::repeat_n(fail(Some(1)), 3));
        frames.extend(std::iter::repeat_n(fail(None), 3));
        frames.push(Ok(SPINNER));
        frames.push(fail(Some(0)));
        let fake = Fake::new(frames);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 14).await;
        assert!(fake.rounds() >= 14);
        // Entrada (tentativa 0), nova tentativa (1), falha que o pool não guarda (toda rodada: 3),
        // e a entrada de novo depois do quadro bom.
        assert_eq!(fake.has_session.load(Ordering::SeqCst), 6);
        let problems: Vec<_> = fake.events.lock().unwrap().iter().map(|(_, e)| e.problema.clone()).collect();
        assert_eq!(problems, [None, Some(OBSERVATION_FAILED.into()), None, Some(OBSERVATION_FAILED.into())],
            "um evento por código; a volta publica de novo");
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn rebind_resets_memory_and_publishes_again() {
        let fake = Fake::new(vec![Ok(SPINNER)]);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 4 + Duration::from_millis(10)).await;
        assert_eq!(fake.states(), ["working", "idle"]);
        fake.epoch.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(POLL + Duration::from_millis(10)).await;
        assert_eq!(fake.states(), ["working", "idle", "working"], "memória nova: o spinner conta do zero");
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_facts_become_the_problem() {
        let fake = Fake::new(vec![Ok(SPINNER)]);
        fake.facts.lock().unwrap().unavailable = Some("state_facts_status:503".into());
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(Duration::from_millis(10)).await;
        fake.facts.lock().unwrap().unavailable = None;
        tokio::time::sleep(POLL).await;
        let events: Vec<_> = fake.events.lock().unwrap().iter().map(|(_, e)| (e.state.clone(), e.problema_detalhe.clone())).collect();
        assert_eq!(events, [("working".into(), Some("state_facts_status:503".into())), ("working".into(), None)]);
        assert_eq!(fake.events.lock().unwrap()[0].1.problema.as_deref(), Some(UNAVAILABLE));
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn unreadable_session_files_become_the_problem() {
        let fake = Fake::new(vec![Ok(SPINNER)]);
        *fake.files_failed.lock().unwrap() = Some("state_files_timeout".into());
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(Duration::from_millis(10)).await;
        let first = fake.events.lock().unwrap()[0].1.clone();
        assert_eq!((first.problema.as_deref(), first.problema_detalhe.as_deref()), (Some(UNAVAILABLE), Some("state_files_timeout")));
        *fake.files_failed.lock().unwrap() = None;
        tokio::time::sleep(POLL).await;
        assert!(fake.events.lock().unwrap().last().unwrap().1.problema.is_none(), "leitura boa limpa o problema");
        task.abort();
    }

    const IDLE: &str = "────────────\n❯\n────────────";
    const MENU: &str = "   Qual cor?\n\n ❯ 1. Azul\n   2. Verde\n   3. Type something.\n";

    fn payload(labels: &[&str]) -> AskQuestion {
        AskQuestion { questions: vec![hangar_api::ask::AskQuestionItem { header: "Cor".into(), question: "Qual cor?".into(),
            multi_select: false, options: labels.iter().map(|l| hangar_api::ask::AskOption { label: (*l).into(), ..Default::default() }).collect() }] }
    }

    fn others(fake: &Fake, kind: &str) -> Vec<(u32, Value)> {
        fake.others.lock().unwrap().iter().filter(|(_, k, _)| *k == kind).map(|(r, _, d)| (*r, d.clone())).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn ask_question_once_per_prompt() {
        // Pergunta na tela por várias rodadas, sai (resposta), volta outra: uma emissão por pergunta.
        let mut frames = vec![Ok(SPINNER)];
        frames.extend(std::iter::repeat_n(Ok(MENU), 4));
        frames.push(Ok(SPINNER));
        frames.extend(std::iter::repeat_n(Ok(MENU), 3));
        let fake = Fake::new(frames);
        *fake.ask.lock().unwrap() = Some(payload(&["Azul", "Verde"]));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 9 + Duration::from_millis(10)).await;
        let asks = others(&fake, "ask_question");
        assert_eq!(asks.len(), 2, "{asks:?}");
        assert_eq!(asks[0].1, serde_json::to_value(payload(&["Azul", "Verde"])).unwrap());
        assert_eq!(fake.ask_reads.load(Ordering::SeqCst), 2, "o sidecar só é lido quando a pergunta aparece");
        // O sidecar chega depois do menu: a rodada seguinte o relê, com a tela parada.
        task.abort();
        let _ = task.await;
        let fake = Fake::new(vec![Ok(MENU)]);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2 + Duration::from_millis(10)).await;
        assert!(others(&fake, "ask_question").is_empty());
        *fake.ask.lock().unwrap() = Some(payload(&["Azul", "Verde"]));
        tokio::time::sleep(POLL).await;
        assert_eq!(others(&fake, "ask_question").len(), 1);
        assert_eq!(fake.events.lock().unwrap().len(), 1, "o estado não sai de novo");
        // Sidecar velho de outra pergunta: não abre o stepper.
        task.abort();
        let _ = task.await;
        let fake = Fake::new(vec![Ok(MENU)]);
        *fake.ask.lock().unwrap() = Some(payload(&["Sim", "Nao"]));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2).await;
        assert!(others(&fake, "ask_question").is_empty());
        task.abort();
        let _ = task.await;
        // `/clear` com a mesma pergunta na tela: a conversa nova emite de novo.
        let fake = Fake::new(vec![Ok(MENU)]);
        *fake.ask.lock().unwrap() = Some(payload(&["Azul", "Verde"]));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2).await;
        fake.epoch.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(POLL * 2).await;
        assert_eq!(others(&fake, "ask_question").len(), 2, "uma antes e uma depois do /clear");
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn deliverable_edge_calls_service_once() {
        // Nasce entregável (uma), pergunta (não), volta (uma), segue entregável (nenhuma).
        let mut frames = vec![Ok(IDLE), Ok(IDLE), Ok(MENU), Ok(MENU), Ok(SPINNER)];
        frames.extend(std::iter::repeat_n(Ok(IDLE), 6));
        let fake = Fake::new(frames);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 11).await;
        let deliveries = fake.deliveries.lock().unwrap().clone();
        assert_eq!(deliveries.len(), 2, "{deliveries:?} {:?}", fake.states());
        // O `/clear` não é borda: a fila não muda de dono.
        fake.epoch.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(POLL * 2).await;
        assert_eq!(fake.deliveries.lock().unwrap().len(), 2);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn runtime_problem_in_key_and_wakes() {
        let fake = Fake::new(vec![Ok(IDLE)]);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2 + Duration::from_millis(10)).await;
        assert_eq!(fake.events.lock().unwrap().len(), 1);
        let rounds = fake.rounds();
        // O ator publica: sai na hora, sem rodada a mais (a memória temporal não anda).
        *fake.runtime.lock().unwrap() = Some(("terminal_input_composer_busy".into(), "composer_busy".into()));
        fake.runtime_wake.notify_one();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.rounds(), rounds, "acordar pelo ator não captura");
        let last = fake.events.lock().unwrap().last().unwrap().1.clone();
        assert_eq!((last.problema.as_deref(), last.problema_detalhe.as_deref()), (Some("terminal_input_composer_busy"), Some("composer_busy")));
        assert_eq!(last.state, "idle");
        // Igual nas rodadas seguintes: a chave já tem o problema, nada sai de novo.
        tokio::time::sleep(POLL * 3).await;
        assert_eq!(fake.events.lock().unwrap().len(), 2);
        // Resolvido: limpa.
        *fake.runtime.lock().unwrap() = None;
        tokio::time::sleep(POLL).await;
        assert_eq!(fake.events.lock().unwrap().len(), 3);
        assert!(fake.events.lock().unwrap()[2].1.problema.is_none());
        // Problema da observação vence o do runtime.
        *fake.runtime.lock().unwrap() = Some(("runtime_falhou".into(), "queue_io".into()));
        fake.facts.lock().unwrap().unavailable = Some("state_facts_timeout".into());
        tokio::time::sleep(POLL).await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.problema.as_deref(), Some(UNAVAILABLE));
        fake.runtime_wake.notify_one();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.problema.as_deref(), Some(UNAVAILABLE), "o ator não apaga o problema dos fatos");
        task.abort();
    }

    #[tokio::test]
    async fn one_monitor_per_session() {
        use crate::side::{Binding, Hubs, SideCtx, fake_monitors, IDLE_PANE};
        use crate::transcript::Provider;
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_monitors(IDLE_PANE);
        let ctx = SideCtx { upstream: "127.0.0.1:9".parse().unwrap(), secret: "s".into(), http: crate::proxy::client(),
            watchers: Default::default(), hubs: Hubs::default(), infos: Default::default(), monitors: Some(spawn), mods: Default::default() };
        let binding = |p| Binding { provider: p, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false };
        // Dono no celular, dono no desktop e o canal do convidado: um hub, um Monitor.
        let leases: Vec<_> = (0..3).map(|_| ctx.hubs.acquire("s", binding(Provider::Claude), &ctx)).collect();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let first = leases[0].hub.clone();
        drop(leases);
        assert!(first.monitor.lock().unwrap().is_none(),"o último assinante saiu: o Monitor para com o hub");
        let again = ctx.hubs.acquire("s", binding(Provider::Claude), &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 2, "volta com o próximo assinante");
        // Codex com terminal não tem dono do estado no Rust.
        let _codex = ctx.hubs.acquire("c", binding(Provider::Codex), &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        // Claude e Codex sem terminal ganham um feed, e só um com vários assinantes.
        let _headless = ctx.hubs.acquire("h", binding(Provider::ClaudeHeadless), &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 3);
        let feeds: Vec<_> = (0..2).map(|_| ctx.hubs.acquire("x", Binding { headless: true, ..binding(Provider::Codex) }, &ctx)).collect();
        assert_eq!(count.load(Ordering::SeqCst), 4);
        drop(feeds);
        drop(again);
    }

    #[tokio::test(start_paused = true)]
    async fn suggest_on_change() {
        let fake = Fake::new(vec![Ok(IDLE)]);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL).await;
        assert!(others(&fake, "suggest").is_empty(), "sem fato e sem sugestão, nada sai");
        fake.facts.lock().unwrap().suggestion = Some(String::new());
        tokio::time::sleep(POLL).await;
        assert!(others(&fake, "suggest").is_empty(), "vazia desde o início não é mudança");
        fake.facts.lock().unwrap().suggestion = Some("roda os testes".into());
        tokio::time::sleep(POLL * 3).await;
        fake.facts.lock().unwrap().suggestion = None;
        tokio::time::sleep(POLL * 2).await;
        fake.facts.lock().unwrap().suggestion = Some(String::new());
        tokio::time::sleep(POLL).await;
        let texts: Vec<_> = others(&fake, "suggest").into_iter().map(|(_, d)| d).collect();
        assert_eq!(texts, [json!({"text": "roda os testes"}), json!({"text": ""})], "uma por mudança, mesmo sem o estado mudar");
        assert_eq!(fake.events.lock().unwrap().len(), 1);
        task.abort();
    }
}
