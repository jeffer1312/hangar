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
/// Falhas da observação que são a troca de conversa em curso (`/clear`, reabertura): o vínculo muda e a
/// entrada fecha e reabre. Como o Python, essas rodadas são puladas; a que dura além disto aparece.
const SWITCHING: [&str; 2] = ["state_binding_changed", "runtime_binding"];
const SWITCH_GRACE: u8 = 8;
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
    pub questions: Vec<Value>,
    pub binding: Option<crate::interactions::InteractionBinding>,
    pub native_selector: Option<String>,
    /// O plano do `ExitPlanMode` cujo diálogo o pane mostra (`claude_plan_pending`).
    pub plan_pending: Option<serde_json::Map<String, Value>>,
    pub in_transfer: bool,
    pub permission_op: bool,
    pub permission_uncertain: bool,
    pub permission_revision: Option<permission::MemoryRevision>,
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
            questions: Vec::new(), binding: None, native_selector: None, plan_pending: None,
            in_transfer: r.in_transfer(now),
            permission_op: r.facts.permission_op,
            permission_uncertain: false, permission_revision: None,
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
    /// A tela mostra o "trust this folder?" que a abertura devia ter evitado: só dispara, e quem
    /// implementa registra o que o `.claude.json` diz da pasta naquela hora.
    fn trust_dialog(&self);
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
    permission_revision: Option<permission::MemoryRevision>,
    /// Em falha da observação: a tentativa do pool já conferida com `has-session`.
    failure: Option<Option<u32>>,
    preview: preview::Slot,
    held_form: Option<Value>,
    native_form: Option<Value>,
    binding: Option<crate::interactions::InteractionBinding>,
    /// Rodadas seguidas puladas pela troca de conversa.
    switch_misses: u8,
    /// Última linha de status sem métrica de contexto já registrada.
    ctx_warned: Option<String>,
    /// O "trust this folder?" na tela já foi avisado.
    trust_reported: bool,
}

/// Cabeçalho do diálogo de confiança do Claude Code, que a linha de status carrega enquanto ele está aberto.
const TRUST_DIALOG: &str = "Accessing workspace:";

static CTX_PAIR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"([\d.,]+)\s*[kKmM]?\s*/\s*([\d.,]+)\s*[kKmM]?").unwrap());
static CTX_LABELED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\bctx\s*[\d.,]+\s*[kKmM]?\s*/\s*[\d.,]+\s*[kKmM]?").unwrap());

/// `sse.context_pairs`: pares numéricos no segmento 💬 (2 ou mais: há métrica); "ctx x/y" conta como 2.
fn context_pairs(line: &str) -> usize {
    let Some(segment) = line.split_once('💬').map(|(_, rest)| rest.split('│').next().unwrap_or("")) else { return 0 };
    let n = CTX_PAIR.find_iter(segment).count();
    if CTX_LABELED.is_match(segment) { n.max(2) } else { n }
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
                Step::Sleep => self.idle(None, &runtime, &hub).await,
                Step::Wait { .. } => self.idle(Some(notified.as_mut()), &runtime, &hub).await,
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
    async fn ask(&mut self, event: &mut StateEvent) -> Option<Step> {
        let form = event.codex_question.as_ref().map(|question| Value::Object(question.clone()));
        if form != self.mem.held_form {
            self.mem.held_form = form.clone();
            if !self.src.emit("ask_question", form.clone().unwrap_or(Value::Null)).await {
                return Some(Step::Exit(Exit::Closed));
            }
        }
        if form.is_some() { return None; }
        if event.state != "awaiting_input" {
            if event.problema.is_none() && self.mem.native_form.take().is_some()
                && !self.src.emit("ask_question", Value::Null).await { return Some(Step::Exit(Exit::Closed)); }
            return None;
        }
        let epoch = self.src.epoch();
        let payload = self.src.ask_payload().await;
        if self.src.epoch() != epoch { return Some(Step::Again); }
        let failed = match payload {
            Ok(Some(payload)) if ask::matches(event, &payload) && payload.interaction_id.as_deref().is_none_or(str::is_empty) => "erro_interacao_indisponivel".to_owned(),
            Ok(Some(payload)) if ask::matches(event, &payload) => match serde_json::to_value(&payload) {
                Ok(data) => {
                    if self.mem.native_form.as_ref() != Some(&data) {
                        self.mem.native_form = Some(data.clone());
                        return (!self.src.emit("ask_question", data).await).then_some(Step::Exit(Exit::Closed));
                    }
                    return None;
                }
                Err(_) => "askq_serialize".to_owned(),
            },
            Ok(None) if event.problema.is_none() => {
                if self.mem.native_form.take().is_some() && !self.src.emit("ask_question", Value::Null).await {
                    return Some(Step::Exit(Exit::Closed));
                }
                return None;
            }
            Ok(_) => return None,
            Err(code) => code,
        };
        if crate::warn_limit::allow(Some(self.src.name()), &failed) {
            tracing::warn!(session = self.src.name(), code = failed.as_str(), "estado: pergunta nativa sem sidecar legível");
        }
        event.problema.get_or_insert_with(|| UNAVAILABLE.into());
        event.problema_detalhe.get_or_insert(failed);
        None
    }

    async fn publish(&mut self, mut event: StateEvent) -> Option<Step> {
        if let Some(step) = self.ask(&mut event).await { return Some(step); }
        let key = key_of(&event);
        if self.mem.key.as_ref() == Some(&key) { return None; }
        self.mem.key = Some(key);
        self.mem.preview.state = Some(event.state.clone());
        // Diagnóstico do "medição indisponível" (`sse.py`): a linha de status crua, uma vez por linha distinta.
        if let Some(line) = event.status_line.as_deref().filter(|line| context_pairs(line) < 2 && self.mem.ctx_warned.as_deref() != Some(*line)) {
            tracing::info!(session = self.src.name(), statusline = line, "estado: sem métrica de contexto");
            self.mem.ctx_warned = Some(line.to_owned());
        }
        // Uma vez por aparição: o diálogo que volta depois de sumir é outra ocorrência.
        let trust = event.status_line.as_deref().is_some_and(|line| line.contains(TRUST_DIALOG));
        if trust && !self.mem.trust_reported { self.src.trust_dialog(); }
        self.mem.trust_reported = trust;
        // Como o Python, a borda vale com problema na linha; só o estado que não foi observado não decide.
        if event.problema.as_deref() != Some(OBSERVATION_FAILED) && self.edges.deliverable(&event) {
            self.src.deliverable();
        }
        self.mem.last = Some(event.clone());
        (!self.src.publish(event).await).then_some(Step::Exit(Exit::Closed))
    }

    /// O ator de entrada publicou: o problema dele entra ou sai do último estado sem rodada nova
    /// (a memória temporal não anda). Problema da observação ou dos fatos fica até a rodada boa.
    /// O problema do ator quando é de tela (mensagem presa, troca de conta): vence o diagnóstico da rodada.
    fn screen_runtime_problem(&self) -> Option<(String, String)> {
        self.src.runtime_problem().filter(|(code, _)| !crate::problem_gate::is_diagnostic(code))
    }

    async fn runtime_changed(&mut self) -> Option<Step> {
        let last = self.mem.last.as_ref()?;
        let now = self.src.runtime_problem();
        // Problema da observação fica até a rodada boa, salvo quando chega um de tela, que ele não pode tapar.
        if last.problema.as_deref().is_some_and(|p| !edges::is_runtime_problem(p)) && now.as_ref().is_none_or(|(code, _)| crate::problem_gate::is_diagnostic(code)) {
            return None;
        }
        let same = match &now {
            Some((p, d)) => last.problema.as_deref() == Some(p.as_str()) && last.problema_detalhe.as_deref() == Some(d.as_str()),
            None => last.problema.is_none(),
        };
        if same {
            return None;
        }
        let (problema, problema_detalhe) = now.unzip();
        let event = StateEvent { problema, problema_detalhe, ..last.clone() };
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
        if facts.binding.is_some() && self.mem.binding != facts.binding {
            self.mem = Memory { binding: facts.binding.clone(), ..Default::default() };
        }
        if facts.permission_uncertain || self.mem.permission_revision != facts.permission_revision {
            self.mem.permission = permission::Watch::default();
            self.mem.permission_revision = facts.permission_revision;
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
            Err(f) if SWITCHING.contains(&f.code.as_str()) && self.mem.switch_misses < SWITCH_GRACE => {
                self.mem.switch_misses += 1;
                Step::Sleep
            }
            Err(f) => {
                let step = self.fail(OBSERVATION_FAILED, f.code, &facts).await;
                // O arquivo do hook não depende da captura: a prévia segue andando com a observação caída.
                if !preview::tick(&self.src, &mut self.mem.preview, None, epoch).await { return Step::Exit(Exit::Closed); }
                step
            }
            Ok(frame) => {
                self.mem.switch_misses = 0;
                self.reduce(frame, facts, epoch).await
            }
        }
    }

    /// O estado fica no último evento, com o problema; um evento por código.
    async fn fail(&mut self, problem: &str, detail: String, facts: &RoundFacts) -> Step {
        // A prévia fica com o texto que tinha e espera a próxima rodada boa.
        self.mem.preview.fast = false;
        let mode = self.mem.permission.current.as_ref().map(|(mode, _)| mode.clone());
        if self.mem.last.as_ref().is_some_and(|l| l.problema_detalhe.as_deref() == Some(detail.as_str()) && l.claude_permission_mode == mode) {
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
        let (problema, problema_detalhe) = self.screen_runtime_problem().unwrap_or((problem.to_owned(), detail));
        let event = StateEvent { problema: Some(problema), problema_detalhe: Some(problema_detalhe), claude_permission_mode: mode, ..base };
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
            // A operação pode terminar depois dos fatos; cada quadro valida sua revisão.
            match self.src.observe_permission(&key, observed).await {
                Ok(answer) => self.mem.permission.answered(&key, observed, facts.permission_op, answer),
                Err(code) => {
                    if matches!(code.as_str(), "permission_capture_stale" | "permission_mode_unknown") {
                        self.mem.permission = permission::Watch::default();
                    }
                    if crate::warn_limit::allow(Some(&name), PERMISSION_FAILED) {
                        tracing::warn!(session = name.as_str(), code = PERMISSION_FAILED, detail = code.as_str(), "estado: permission.observe falhou");
                    }
                    problem.get_or_insert((PERMISSION_FAILED, code));
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
        let selectors: Vec<_> = facts.questions.iter().filter(|q| q["id"].as_str().is_some_and(|id| id.starts_with("perm:"))).cloned().collect();
        let selector = super::facts::plugin_surfaces(&selectors).ok().and_then(|(question, _)| question.cloned()).or_else(|| facts.question.clone());
        let reducer_facts = ReducerFacts {
            open_question: files.open_question, plugin_question: selector, plugin_state: facts.plugin_state,
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
        // O problema do runtime entra na chave: muda o evento mesmo com a tela parada. O de tela (mensagem presa)
        // vence o diagnóstico daqui, que o portão da saída esconderia.
        let runtime = self.src.runtime_problem();
        let problem = if runtime.as_ref().is_some_and(|(code, _)| !crate::problem_gate::is_diagnostic(code)) { runtime }
            else { problem.map(|(p, d)| (p.to_owned(), d)).or(runtime) };
        let (problema, problema_detalhe) = problem.unzip();
        let mut event = StateEvent {
            session: name, state: a.state, label: a.label, question: a.question, options: a.options,
            status_line: a.status_line, overlay: a.overlay, login: a.login, limited: a.limit_reset.is_some(),
            limit_reset: a.limit_reset, loop_status: loop_info.status, loop_iter: loop_info.iter, loop_max: loop_info.max,
            claude_permission_mode: mode, claude_previous_non_plan: previous, shells: files.shells,
            problema, problema_detalhe, interaction_id: facts.native_selector.clone(),
            claude_plan_pending: facts.plan_pending.clone(), ..Default::default()
        };
        let projection = super::facts::project_plugin_state(&mut event, &facts.questions);
        if let Err(code) = &projection && !crate::problem_gate::keeps(event.problema.as_deref(), UNAVAILABLE) {
            event.problema = Some(UNAVAILABLE.into());
            event.problema_detalhe = Some(code.clone());
        }
        // Sem o aviso do plano não há cartão a guardar: o anterior seria de um diálogo que já fechou.
        if (projection.is_err() || facts.unavailable.as_deref().is_some_and(|code| code != "plan_notice_missing") || files.unavailable.is_some())
            && let Some(previous) = &self.mem.last {
            super::facts::preserve_cards(&mut event, previous);
        }
        if self.src.epoch() != epoch { return Step::Again; }
        if let Some(step) = self.publish(event).await { return step; }
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
        trust_dialogs: Mutex<u32>,
        files_failed: Mutex<Option<String>>,
        permissions: Mutex<Option<permission::PermissionStore>>,
        captured_permission: Mutex<Option<permission::MemoryRevision>>,
        finish_mode: Mutex<Option<String>>,
        hook: Mutex<Vec<HookFile>>,
        previews: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(frames: Vec<Result<&'static str, CaptureFailed>>) -> Arc<Self> {
            Arc::new(Self { frames, round: AtomicU32::new(0), has_session: AtomicU32::new(0), facts: Mutex::default(),
                wake: Arc::default(), epoch: AtomicU64::new(0), events: Mutex::default(), others: Mutex::default(),
                runtime: Mutex::default(), runtime_wake: Arc::default(), ask: Mutex::default(), ask_reads: AtomicU32::new(0),
                deliveries: Mutex::default(), trust_dialogs: Mutex::default(), files_failed: Mutex::default(), permissions: Mutex::default(), captured_permission: Mutex::default(), finish_mode: Mutex::default(),
                hook: Mutex::default(), previews: Mutex::default() })
        }
        fn rounds(&self) -> u32 { self.round.load(Ordering::SeqCst) }
        fn states(&self) -> Vec<String> { self.events.lock().unwrap().iter().map(|(_, e)| e.state.clone()).collect() }
    }

    impl Sources for Arc<Fake> {
        fn name(&self) -> &str { "s" }
        fn sid(&self) -> Option<String> { Some("sid".into()) }
        fn epoch(&self) -> u64 { self.epoch.load(Ordering::SeqCst) }
        fn wake(&self) -> Arc<Notify> { self.wake.clone() }
        async fn facts(&self) -> RoundFacts {
            let mut facts = self.facts.lock().unwrap().clone();
            if let Some(memory) = self.permissions.lock().unwrap().as_ref() {
                let memory = memory.lock().unwrap();
                facts.permission_revision = Some(memory.revision("sid"));
                facts.permission_uncertain = memory.is_uncertain("sid");
            }
            facts
        }
        async fn capture(&self) -> Result<Frame, CaptureFailed> {
            if let Some(memory) = self.permissions.lock().unwrap().as_ref() {
                *self.captured_permission.lock().unwrap() = Some(memory.lock().unwrap().revision("sid"));
            }
            let i = self.round.fetch_add(1, Ordering::SeqCst) as usize;
            let frame = self.frames[i.min(self.frames.len() - 1)].clone();
            if let Some(mode) = self.finish_mode.lock().unwrap().take() {
                let memory = self.permissions.lock().unwrap().as_ref().unwrap().clone();
                let mut memory = memory.lock().unwrap();
                assert!(memory.begin("sid"));
                memory.finish("sid", &mode);
            }
            frame.map(|t| Frame { text: t.into(), analysis: terminal_state::analyze(t) })
        }
        async fn has_session(&self) -> Option<bool> { self.has_session.fetch_add(1, Ordering::SeqCst); Some(true) }
        async fn dead(&self) -> Result<Dead, String> { Ok(Dead::Ok) }
        async fn observe_permission(&self, _: &str, mode: &str) -> Result<(String, String), String> {
            if let Some(memory) = self.permissions.lock().unwrap().as_ref() {
                let revision = self.captured_permission.lock().unwrap().unwrap();
                let view = memory.lock().unwrap().observe_capture("sid", mode, revision).map_err(str::to_owned)?;
                return Ok((view.current, view.previous_non_plan));
            }
            Ok((mode.into(), "manual".into()))
        }
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
        fn trust_dialog(&self) { *self.trust_dialogs.lock().unwrap() += 1; }
        fn hub_wake(&self) -> Arc<Notify> { Arc::default() }
        async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { None }
        async fn preview_files(&self, _: &str) -> Vec<HookFile> { self.hook.lock().unwrap().clone() }
        fn committed(&self) -> Option<Arc<str>> { None }
        async fn publish_preview(&self, event: PreviewEvent) -> bool { self.previews.lock().unwrap().push(event.text); true }
        fn wall(&self) -> f64 { wall_now() }
    }

    const SPINNER: &str = "✻ Thinking…\n────────────\n❯\n────────────";

    #[tokio::test]
    async fn known_finish_invalidates_unchanged_mode_frame_without_removing_form() {
        let fake = Fake::new(vec![Ok("────────────\n❯\n────────────\n⏵ manual mode on"),
            Ok("────────────\n❯\n────────────\n⏵ manual mode on"), Ok("────────────\n❯\n────────────\n⏵ plan mode on")]);
        let memory: permission::PermissionStore = Arc::default();
        *fake.permissions.lock().unwrap() = Some(memory);
        fake.facts.lock().unwrap().questions = vec![json!({"id":"ask:b","interaction_id":"form-b","questions":[{"question":"Pasta?","options":[]} ]})];
        let mut monitor = Monitor::new(fake.clone());
        monitor.round().await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.claude_permission_mode.as_deref(),Some("manual"));
        *fake.finish_mode.lock().unwrap() = Some("plan".into());
        monitor.round().await;
        let invalid = fake.events.lock().unwrap().last().unwrap().1.clone();
        assert!(invalid.claude_permission_mode.is_none(), "quadro manual anterior ao finish não é o modo atual");
        assert_eq!(invalid.problema_detalhe.as_deref(),Some("permission_capture_stale"));
        assert_eq!(invalid.codex_question.as_ref().unwrap()["interaction_id"],"form-b");
        monitor.round().await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.claude_permission_mode.as_deref(),Some("plan"));
    }

    #[tokio::test]
    async fn independent_held_surfaces_keep_ids_and_form_survives_selector_conflict() {
        let fake = Fake::new(vec![Ok(SPINNER), Ok(MENU)]);
        fake.facts.lock().unwrap().questions = vec![
            json!({"id":"ask:b","interaction_id":"form-b","questions":[{"question":"Pasta?","options":[]}]}),
        ];
        let mut monitor = Monitor::new(fake.clone());
        monitor.round().await;
        fake.facts.lock().unwrap().questions.push(json!({"id":"perm:a","interaction_id":"selector-a","questions":[],"tool":"Bash","resumo":"cmd"}));
        monitor.round().await;
        let events = fake.events.lock().unwrap();
        let last = &events.last().unwrap().1;
        assert_eq!(last.codex_question.as_ref().unwrap()["interaction_id"], "form-b");
        assert_eq!(last.problema.as_deref(), Some(UNAVAILABLE));
        assert!(!fake.others.lock().unwrap().iter().any(|(_, kind, data)| *kind == "ask_question" && data.is_null()));
    }

    #[tokio::test]
    async fn unavailable_store_keeps_cards_only_until_hub_generation_changes() {
        let fake = Fake::new(vec![Ok(SPINNER)]);
        fake.facts.lock().unwrap().questions = vec![
            json!({"id":"perm:a","interaction_id":"selector-a","questions":[],"tool":"Bash","resumo":"cmd"}),
            json!({"id":"ask:b","interaction_id":"form-b","questions":[{"question":"Pasta?","options":[]}]}),
        ];
        let mut monitor = Monitor::new(fake.clone());
        monitor.round().await;
        let first = fake.events.lock().unwrap().last().unwrap().1.clone();
        assert_eq!(first.interaction_id.as_deref(), Some("selector-a"));
        assert_eq!(first.codex_question.as_ref().unwrap()["interaction_id"], "form-b");
        *fake.facts.lock().unwrap() = RoundFacts { unavailable: Some("plugin_store_unavailable".into()), ..Default::default() };
        monitor.round().await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.codex_question, first.codex_question);
        fake.epoch.fetch_add(1, Ordering::SeqCst);
        monitor.round().await;
        assert!(fake.events.lock().unwrap().last().unwrap().1.codex_question.is_none());
    }

    /// Fatos indisponíveis ou observação caída junto com mensagem presa: o estado leva a mensagem presa, que é
    /// de tela; o diagnóstico não a tapa (o portão da saída o tiraria e a pessoa não veria nada).
    #[tokio::test]
    async fn a_stuck_message_wins_over_unavailable_facts() {
        let fail = Err(CaptureFailed { code: "terminal observer EOF".into(), attempt: None });
        let fake = Fake::new(vec![Ok(SPINNER), fail]);
        *fake.runtime.lock().unwrap() = Some(("terminal_input_stalled".into(), "presa".into()));
        *fake.facts.lock().unwrap() = RoundFacts { unavailable: Some("plugin_store_unavailable".into()), ..Default::default() };
        let mut monitor = Monitor::new(fake.clone());
        let last = || fake.events.lock().unwrap().last().unwrap().1.problema.clone();
        monitor.round().await;
        assert_eq!(last().as_deref(), Some("terminal_input_stalled"), "fatos indisponíveis");
        monitor.round().await;
        assert_eq!(last().as_deref(), Some("terminal_input_stalled"), "observação caída");
    }

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
        // O Store pode abrir a primeira pergunta sem long-poll anterior.
        fake.facts.lock().unwrap().alive = false;
        tokio::time::sleep(POLL).await;
        let before = fake.rounds();
        fake.wake.notify_waiters();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.rounds(), before + 1, "primeira pergunta local também acorda");
        tokio::time::sleep(POLL).await;
        assert_eq!(fake.rounds(), before + 2);
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

    /// Parity B7: as rodadas da troca de conversa (`/clear`: vínculo trocando, entrada fechada e reaberta)
    /// não publicam problema; só a falha que dura além da troca aparece.
    #[tokio::test(start_paused = true)]
    async fn conversation_switch_rounds_do_not_flicker_a_problem() {
        let switching = |code: &str| Err(CaptureFailed { code: code.into(), attempt: None });
        let mut frames = vec![Ok(SPINNER), switching("state_binding_changed"), switching("runtime_binding"), Ok(SPINNER)];
        frames.extend(std::iter::repeat_n(switching("state_binding_changed"), SWITCH_GRACE as usize + 1));
        let fake = Fake::new(frames);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * (SWITCH_GRACE as u32 + 6)).await;
        let problems: Vec<_> = fake.events.lock().unwrap().iter().map(|(_, e)| e.problema.clone()).collect();
        assert_eq!(problems.first(), Some(&None));
        assert_eq!(problems.iter().filter(|p| p.is_some()).count(), 1, "só a troca que não termina aparece: {problems:?}");
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
        AskQuestion { interaction_id: Some("native-form".into()), questions: vec![hangar_api::ask::AskQuestionItem { header: "Cor".into(), question: "Qual cor?".into(),
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
        let asks: Vec<_> = others(&fake, "ask_question").into_iter().filter(|(_, data)| !data.is_null()).collect();
        assert_eq!(asks.len(), 2, "{asks:?}");
        assert_eq!(asks[0].1, serde_json::to_value(payload(&["Azul", "Verde"])).unwrap());
        assert!(fake.ask_reads.load(Ordering::SeqCst) >= 7, "revisão conferida enquanto a pergunta está na tela");
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
    async fn trust_dialog_is_reported_once_per_appearance() {
        const TRUST: &str = "────────────\n Accessing workspace:\n /w/x\n Quick safety check: Is this a project you created or one you trust?\n ❯ No, continue without these permissions\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel";
        let fake = Fake::new(vec![Ok(IDLE), Ok(TRUST), Ok(TRUST), Ok(IDLE), Ok(TRUST), Ok(IDLE)]);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 6).await;
        assert_eq!(*fake.trust_dialogs.lock().unwrap(), 2, "duas aparições, cada uma avisada uma vez: {:?}", fake.states());
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

    /// Parity estado B1 (`test_drain_fires_once_on_overlay_to_idle`): a borda "voltou a aceitar texto"
    /// drena mesmo com um problema do runtime na linha, como o Python.
    #[tokio::test(start_paused = true)]
    async fn deliverable_edge_drains_with_a_runtime_problem() {
        let mut frames = vec![Ok(MENU), Ok(MENU)];
        frames.extend(std::iter::repeat_n(Ok(IDLE), 6));
        let fake = Fake::new(frames);
        *fake.runtime.lock().unwrap() = Some(("terminal_input_composer_busy".into(), "composer_busy".into()));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 8).await;
        assert_eq!(fake.deliveries.lock().unwrap().len(), 1, "{:?}", fake.states());
        task.abort();
    }

    /// Parity estado B4: a contagem do diagnóstico de contexto é a do Python.
    #[test]
    fn context_pairs_like_python() {
        assert_eq!(context_pairs("🤖 Opus │ 💬 46k/200k │ 💵 1"), 1);
        assert_eq!(context_pairs("💬 46k/200k · 12k/50k"), 2);
        assert_eq!(context_pairs("💬 ctx 46k/200k"), 2);
        assert_eq!(context_pairs("sem segmento"), 0);
    }

    /// Parity estado B2: com a observação do pane falhando, a prévia do hook (MessageDisplay) segue andando:
    /// o arquivo não depende da captura, como no `PreviewBroker` do Python.
    #[tokio::test(start_paused = true)]
    async fn hook_preview_keeps_going_while_the_observation_fails() {
        let fail = || Err(CaptureFailed { code: "terminal observer EOF".into(), attempt: None });
        let fake = Fake::new(vec![Ok(SPINNER), fail()]);
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2).await;
        *fake.hook.lock().unwrap() = vec![HookFile { text: "resposta em andamento no hook".into(), ts: Some(wall_now()) }];
        tokio::time::sleep(POLL * 3).await;
        assert!(fake.previews.lock().unwrap().iter().any(|t| t == "resposta em andamento no hook"), "{:?}", fake.previews.lock().unwrap());
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
        // O problema de tela do runtime vence o diagnóstico dos fatos, que a saída esconderia.
        *fake.runtime.lock().unwrap() = Some(("runtime_falhou".into(), "queue_io".into()));
        fake.facts.lock().unwrap().unavailable = Some("state_facts_timeout".into());
        tokio::time::sleep(POLL).await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.problema.as_deref(), Some("runtime_falhou"));
        fake.runtime_wake.notify_one();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.events.lock().unwrap().last().unwrap().1.problema.as_deref(), Some("runtime_falhou"), "os fatos não tapam o problema do ator");
        task.abort();
    }

    #[tokio::test]
    async fn one_monitor_per_session() {
        use crate::side::{Binding, Hubs, SideCtx, fake_monitors, IDLE_PANE};
        use crate::transcript::Provider;
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_monitors(IDLE_PANE);
        let ctx = SideCtx {
            watchers: Default::default(), hubs: Hubs::default(), locate: std::sync::Arc::new(crate::locate::Unavailable), monitors: Some(spawn), mods: Default::default(), runtime: Default::default(), queue_dir: None, problem_hold: std::time::Duration::ZERO, shown_problems: Default::default(),
            diag: crate::diag::DiagClient::new("127.0.0.1:9".parse().unwrap(), "s".into()) };
        let binding = |p| Binding { provider: p, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
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
