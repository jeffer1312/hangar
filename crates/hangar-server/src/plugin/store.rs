use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot, watch};

use crate::interactions::{InteractionBinding, InteractionKind, InteractionStore, InteractionTarget, plugin_target, plugin_semantic};
use super::{PluginMode, PluginReply, PluginRequest, ServiceError};

const POLL: Duration = Duration::from_secs(25);
const NO_POLL: Duration = Duration::from_secs(5);
const ALIVE: Duration = Duration::from_secs(35);
/// Quanto o último `/pull` ainda prova a conversa, a mesma janela do estado recente.
const PULL_PROOF: Duration = Duration::from_secs(90);
const HOOK_ALIVE: Duration = Duration::from_secs(2);
const CONFIRM: Duration = Duration::from_secs(5);
const PUBLISH: Duration = Duration::from_secs(30);
const FOCUS: Duration = Duration::from_secs(1);
/// O `SendMessage` volta logo (fila do agente ou retomada); passado isso a entrega é desconhecida.
const AGENT_SEND: Duration = Duration::from_secs(15);
/// O modo que o plugin declara no `/pull` quando sabe entregar texto a um subagente.
pub const AGENT_MESSAGE_MODE: &str = "agent_message";
const AGENT_LATE_KEPT: usize = 32;

fn question_visible(interrupted: bool, poll_open: bool, age: Duration) -> bool {
    !interrupted && age <= ALIVE && (poll_open || age <= NO_POLL)
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PluginQuestion {
    pub id: String,
    pub interaction_id: String,
    pub questions: Value,
    pub tool: Option<String>,
    #[serde(rename = "resumo")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Receipt {
    pub publication_id: String,
    pub generation: u64,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Ask {
    pub id: String,
    pub questions: Option<Vec<Value>>,
    pub tool: Option<String>,
    pub janela_ms: Option<i64>,
    #[serde(rename = "resumo")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Pull {
    #[serde(default)] pub instance: String,
    #[serde(default)] pub modos: Vec<String>,
    pub estado: Option<String>,
    pub session_id: Option<String>,
}

/// Na dúvida sobre terminal ou presença, a permissão continua no CLI.
#[derive(Clone, Copy, Debug)]
pub struct PermissionGate {
    pub app_present: bool,
    pub terminal_attached: bool,
    pub mode_without_dialog: bool,
}
impl PermissionGate {
    fn hold(self) -> bool { self.app_present && !self.terminal_attached && !self.mode_without_dialog }
}

struct Waiter { id: u64, sender: Option<oneshot::Sender<Value>> }
impl Waiter {
    fn send(&mut self, value: Value) -> bool { self.sender.take().is_some_and(|sender| sender.send(value).is_ok()) }
}
/// `pulled`: início do `/pull` mais recente; `seen`, a última vez que se soube do plugin (início ou fim de um).
struct Owner { instance: String, modes: HashSet<String>, seen: Instant, pulled: Instant }
struct Publication {
    receipt: Receipt,
    mode: PluginMode,
    result: watch::Sender<Option<PluginReply>>,
    handed: bool,
    poll_id: u64,
    deadline: Instant,
}
struct Answer {
    receipt: Option<Receipt>,
    payload: Value,
    result: watch::Sender<Option<PluginReply>>,
    handed: bool,
    poll_id: Option<u64>,
    deadline: Instant,
}
struct Question {
    public: PluginQuestion,
    seen: Instant,
    interrupted: Option<Instant>,
    expired: bool,
    waiter: Option<Waiter>,
    answer: Option<Answer>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PullStatus { pub open: bool, pub last_seen_s: Option<u64>, pub reported: bool, pub ended: bool }
struct Session {
    binding: InteractionBinding,
    owner: Option<Owner>,
    waiter: Option<Waiter>,
    heartbeat: Option<Instant>,
    publication: Option<Publication>,
    questions: BTreeMap<String, Question>,
    closed: HashMap<String, (String, Option<Receipt>)>,
    state: Option<(Instant, String, Option<String>)>,
    state_revision: u64,
    suggestion: String,
    plan: Option<PlanNotice>,
    agents: HashMap<String, AgentLife>,
    agent_waits: HashMap<String, (String, oneshot::Sender<AgentSend>)>,
    /// Pedidos que passaram do prazo sem resposta: a confirmação ainda pode chegar.
    agent_late: VecDeque<(String, String)>,
    agent_outbox: VecDeque<(String, Value)>,
    agent_next: u64,
    /// O último `session.end` do plugin (`fim`): o estado seguinte não o apaga.
    ended: Option<Instant>,
    /// Conversa que o plugin do mesmo processo passou a informar no `/pull` (`/clear`, `/resume`).
    reported: Option<String>,
}

/// Ciclo de vida de um subagente, como o Claude Code o sinaliza ao plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState { Running, Finished, Failed }
#[derive(Clone, Debug, PartialEq)]
pub struct AgentLife { pub state: AgentState, pub ended_at: Option<String> }

/// Resultado de mandar texto a um subagente pelo plugin.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentSend { Delivered, Refused(String), NoPlugin, NoAnswer }

/// Aviso do `ExitPlanMode`: o hook não segura, o diálogo do terminal é que responde.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanNotice { pub tool_use_id: String, pub plan: String, pub path: Option<String> }
#[derive(Default)]
struct Memory { sessions: HashMap<String, Session>, next_waiter: u64, stopping: bool }

pub struct Store {
    interactions: Arc<InteractionStore>,
    memory: Mutex<Memory>,
    events: broadcast::Sender<String>,
}

fn target(id: &str) -> InteractionTarget {
    plugin_target(if id.starts_with("perm:") { InteractionKind::Approval } else { InteractionKind::Question }, id)
}
fn unavailable() -> ServiceError { ServiceError("plugin_store_unavailable") }
fn changed() -> ServiceError { ServiceError("erro_interacao_desatualizada") }
fn session<'a>(memory: &'a mut Memory, binding: &InteractionBinding) -> Result<&'a mut Session, ServiceError> {
    memory.sessions.get_mut(&binding.key).filter(|s| s.binding == *binding).ok_or_else(changed)
}
fn close_question(interactions: &InteractionStore, binding: &InteractionBinding, id: &str) -> Result<(), ServiceError> {
    interactions.observe(binding, &target(id), None).map(|_| ()).map_err(|error| ServiceError(error.code()))
}
fn finish_question(mut question: Question, reply: PluginReply) {
    if let Some(mut waiter) = question.waiter.take() { let _ = waiter.send(json!({"answers":null})); }
    if let Some(answer) = question.answer { answer.result.send_replace(Some(reply)); }
}
fn visible_questions(interactions: &InteractionStore, s: &mut Session, now: Instant) -> Result<Vec<PluginQuestion>, ServiceError> {
    let mut out = Vec::new();
    for q in s.questions.values_mut() {
        if question_visible(q.interrupted.is_some() || q.expired, q.waiter.is_some(), now.saturating_duration_since(q.seen)) {
            out.push(q.public.clone());
        } else {
            close_question(interactions, &s.binding, &q.public.id)?;
            q.expired = true;
        }
    }
    Ok(out)
}
fn discard_unhanded_answer(question: &mut Question, now: Instant) {
    if question.answer.as_ref().is_some_and(|a| !a.handed && a.poll_id.is_none() && now >= a.deadline)
        && let Some(answer) = question.answer.take() {
        answer.result.send_replace(Some(PluginReply::NotWritten));
    }
}
fn finish_session(session: Session) {
    if let Some(mut waiter) = session.waiter { let _ = waiter.send(json!({"text":null})); }
    if let Some(publication) = session.publication {
        publication.result.send_replace(Some(if publication.handed { PluginReply::Unknown } else { PluginReply::NotWritten }));
    }
    for (_, question) in session.questions { finish_question(question, PluginReply::Unknown); }
}

impl Store {
    pub fn new(interactions: Arc<InteractionStore>) -> Self {
        Self { interactions, memory: Mutex::new(Memory::default()), events: broadcast::channel(256).0 }
    }
    pub fn interactions(&self) -> &Arc<InteractionStore> { &self.interactions }
    pub fn subscribe(&self) -> broadcast::Receiver<String> { self.events.subscribe() }
    fn notify(&self, key: &str) { let _ = self.events.send(key.to_owned()); }

    /// O runtime já ativou esta vida na Arc; isto só liga a memória do plugin.
    pub fn bind(&self, binding: &InteractionBinding) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        if memory.sessions.get(&binding.key).is_some_and(|s| s.binding == *binding) { return Ok(()); }
        if let Some(old) = memory.sessions.get(&binding.key) {
            self.interactions.forget_binding(&old.binding).map_err(|error| ServiceError(error.code()))?;
        }
        if let Some(old) = memory.sessions.remove(&binding.key) { finish_session(old); }
        memory.sessions.insert(binding.key.clone(), Session { binding: binding.clone(), owner: None, waiter: None,
            heartbeat: None, publication: None, questions: BTreeMap::new(), closed: HashMap::new(), state: None, state_revision: 0, suggestion: String::new(), plan: None,
            agents: HashMap::new(), agent_waits: HashMap::new(), agent_late: VecDeque::new(), agent_outbox: VecDeque::new(), agent_next: 0, ended: None, reported: None });
        self.notify(&binding.key);
        Ok(())
    }

    pub fn forget(&self, key: &str) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        if let Some(old) = memory.sessions.get(key) {
            self.interactions.forget_binding(&old.binding).map_err(|error| ServiceError(error.code()))?;
        }
        if let Some(old) = memory.sessions.remove(key) { finish_session(old); }
        self.notify(key);
        Ok(())
    }

    pub fn stop(&self) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        memory.stopping = true;
        for session in memory.sessions.values_mut() {
            if let Some(mut waiter) = session.waiter.take() { let _ = waiter.send(json!({"text":null})); }
            for question in session.questions.values_mut() {
                if let Some(mut waiter) = question.waiter.take() { let _ = waiter.send(json!({"answers":null})); }
            }
        }
        Ok(())
    }

    pub fn begin_pull(&self, binding: &InteractionBinding, pull: Pull, now: Instant) -> Result<Poll<'_>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        if pull.session_id.as_deref() != Some(binding.conversation.as_str()) {
            // O processo trocou de conversa (`/clear`, `/resume`): o plugin já fala com o id novo, e é por
            // ele que o ciclo de vida sabe da troca, sem reler o registro nativo a cada tique. O `/pull` só de
            // subagentes vem de um `claude -p`, que pode ser filho da sessão com o ambiente dela: não conta.
            let agents_only = !pull.modos.is_empty() && pull.modos.iter().all(|mode| mode == AGENT_MESSAGE_MODE);
            if !agents_only && let Some(sid) = pull.session_id.filter(|sid| !sid.is_empty() && sid.len() <= 128 && sid.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)))
                && let Ok(s) = session(&mut memory, binding) && s.reported.as_deref() != Some(sid.as_str()) {
                s.reported = Some(sid);
                self.notify(&binding.key);
            }
            return Err(ServiceError("plugin_conversation_mismatch"));
        }
        if memory.stopping { return Ok(Poll::ready(json!({"text":null}))); }
        memory.next_waiter += 1;
        let id = memory.next_waiter;
        let s = session(&mut memory, binding)?;
        if s.owner.as_ref().is_some_and(|owner| owner.instance != pull.instance && now.saturating_duration_since(owner.seen) < ALIVE) {
            return Err(ServiceError("plugin_instance_conflict"));
        }
        if s.owner.as_ref().is_none_or(|owner| owner.instance != pull.instance)
            && s.publication.as_ref().is_some_and(|p| now >= p.deadline) {
            s.publication = None;
        }
        let modes = if pull.modos.is_empty() { HashSet::from(["fill".to_owned()]) } else { pull.modos.into_iter().collect() };
        s.owner = Some(Owner { instance: pull.instance, modes, seen: now, pulled: now });
        if s.state.is_none() { s.state = pull.estado.map(|state| (now, state, None)); }
        if let Some((_, payload)) = s.agent_outbox.pop_front() {
            s.heartbeat = Some(now);
            return Ok(Poll::ready(payload));
        }
        let (sender, receiver) = oneshot::channel();
        s.waiter = Some(Waiter { id, sender: Some(sender) });
        s.heartbeat = Some(now);
        self.notify(&binding.key);
        Ok(Poll { guard: Some(PollGuard { store: self, binding: binding.clone(), request_id: None, id, handed: false }),
            receiver: Some(receiver), ready: None, timeout: POLL, pause: Duration::ZERO })
    }

    pub fn begin_ask(&self, binding: &InteractionBinding, ask: Ask, gate: PermissionGate, now: Instant) -> Result<Poll<'_>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        if memory.stopping { return Ok(Poll::ready(json!({"answers":null}))); }
        memory.next_waiter += 1;
        let poll_id = memory.next_waiter;
        let s = session(&mut memory, binding)?;
        if ask.id.starts_with("perm:") && !gate.hold() {
            close_question(&self.interactions, binding, &ask.id)?;
            if let Some(old) = s.questions.remove(&ask.id) {
                let reply = if old.answer.as_ref().is_some_and(|a| a.handed || a.poll_id.is_some()) { PluginReply::Unknown } else { PluginReply::NotWritten };
                finish_question(old, reply);
            }
            self.notify(&binding.key);
            return Ok(Poll::ready(json!({"soltar":true})));
        }
        let semantic = plugin_semantic(&json!({"questions":ask.questions.clone().unwrap_or_default(),"tool":ask.tool,"resumo":ask.summary}));
        let interrupted = s.questions.get(&ask.id).and_then(|q| q.interrupted);
        if let Some(at) = interrupted.filter(|at| now.saturating_duration_since(*at) <= HOOK_ALIVE) {
            // Como o Python, o poll seguinte espera a janela da interrupção em vez de voltar na hora.
            return Ok(Poll::later(json!({"answers":null}), HOOK_ALIVE.saturating_sub(now.saturating_duration_since(at))));
        }
        if s.questions.get(&ask.id).is_some_and(|q| !question_visible(q.interrupted.is_some() || q.expired, q.waiter.is_some(), now.saturating_duration_since(q.seen))) {
            close_question(&self.interactions, binding, &ask.id)?;
        }
        let interaction_id = self.interactions.observe(binding, &target(&ask.id), Some(semantic.clone()))
            .map_err(|error| ServiceError(error.code()))?.ok_or_else(unavailable)?;
        let replace = s.questions.get(&ask.id).is_none_or(|q| q.public.interaction_id != interaction_id);
        if replace {
            if let Some(old) = s.questions.remove(&ask.id) { finish_question(old, PluginReply::Unknown); }
            s.closed.remove(&ask.id);
            s.questions.insert(ask.id.clone(), Question {
                public: PluginQuestion { id: ask.id.clone(), interaction_id, questions: semantic["questions"].clone(), tool: ask.tool, summary: ask.summary },
                seen: now, interrupted: None, expired: false, waiter: None, answer: None,
            });
        }
        let q = s.questions.get_mut(&ask.id).ok_or_else(unavailable)?;
        discard_unhanded_answer(q, now);
        q.seen = now;
        q.interrupted = None;
        self.notify(&binding.key);
        if let Some(answer) = q.answer.as_mut().filter(|a| !a.handed && a.poll_id.is_none() && now < a.deadline) {
            answer.handed = true;
            return Ok(Poll::ready(answer.payload.clone()));
        }
        let (sender, receiver) = oneshot::channel();
        q.waiter = Some(Waiter { id: poll_id, sender: Some(sender) });
        let timeout = match ask.janela_ms { Some(ms) if ms != 0 => Duration::from_millis(ms.max(0) as u64).min(POLL), _ => POLL };
        Ok(Poll { guard: Some(PollGuard { store: self, binding: binding.clone(), request_id: Some(ask.id), id: poll_id, handed: false }),
            receiver: Some(receiver), ready: None, timeout, pause: Duration::ZERO })
    }

    pub fn pending_all(&self, binding: &InteractionBinding, now: Instant) -> Result<Vec<PluginQuestion>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        visible_questions(&self.interactions, s, now)
    }

    /// O consumidor singular não escolhe arbitrariamente entre dois alvos.
    pub fn pending(&self, binding: &InteractionBinding, now: Instant) -> Option<PluginQuestion> {
        match self.pending_all(binding, now) {
            Ok(mut questions) if questions.len() == 1 => questions.pop(),
            Ok(_) => None,
            Err(error) => { tracing::warn!(code = error.0, "pergunta do plugin indisponível"); None }
        }
    }

    pub fn interrupt(&self, binding: &InteractionBinding, request_id: &str) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        close_question(&self.interactions, binding, request_id)?;
        if let Some(q) = s.questions.get_mut(request_id) {
            q.interrupted = Some(Instant::now());
            if let Some(mut waiter) = q.waiter.take() { let _ = waiter.send(json!({"answers":null})); }
            if let Some(answer) = q.answer.take() { answer.result.send_replace(Some(PluginReply::Unknown)); }
        }
        self.notify(&binding.key);
        Ok(())
    }

    /// Corpo completo do hook: `{"permitir":true/false}`, `{"answers":{"Escolha":"A"}}`
    /// ou `{"deny":"motivo"}`. O recibo é acrescentado no topo, sem envolver o corpo em `answers`.
    pub async fn answer(&self, binding: &InteractionBinding, request_id: &str, interaction_id: &str, body: Value,
        receipt: Option<Receipt>) -> Result<PluginReply, ServiceError> {
        let (result, deadline) = {
            let mut memory = self.memory.lock().map_err(|_| unavailable())?;
            let s = session(&mut memory, binding)?;
            if let Some((closed_id, closed_receipt)) = s.closed.get(request_id)
                && closed_id == interaction_id && closed_receipt == &receipt { return Ok(PluginReply::Accepted); }
            let expected = self.interactions.validate(binding, Some(interaction_id)).map_err(|e| ServiceError(e.code()))?;
            if expected != target(request_id) { return Err(changed()); }
            if receipt.as_ref().is_some_and(|r| r.generation != binding.generation || r.session_id != binding.conversation) { return Err(changed()); }
            let q = s.questions.get_mut(request_id).ok_or_else(changed)?;
            let now = Instant::now();
            if !question_visible(q.interrupted.is_some() || q.expired, q.waiter.is_some(), now.saturating_duration_since(q.seen)) {
                close_question(&self.interactions, binding, request_id)?;
                q.expired = true;
                return Err(changed());
            }
            discard_unhanded_answer(q, now);
            if let Some(answer) = &q.answer {
                if answer.receipt != receipt { return Err(ServiceError("plugin_publication_conflict")); }
                (answer.result.subscribe(), answer.deadline)
            } else {
                if !body.is_object() { return Err(ServiceError("plugin_answer_body_invalid")); }
                let mut payload = body;
                if let Some(r) = &receipt {
                    payload["publication_id"] = json!(r.publication_id);
                    payload["generation"] = json!(r.generation);
                    payload["session_id"] = json!(r.session_id);
                }
                let (result, rx) = watch::channel(None);
                let deadline = now + CONFIRM;
                let mut answer = Answer { receipt, payload, result, deadline, handed: false, poll_id: None };
                if let Some(waiter) = q.waiter.as_mut() {
                    if waiter.send(answer.payload.clone()) { answer.poll_id = Some(waiter.id); }
                }
                q.answer = Some(answer);
                (rx, deadline)
            }
        };
        Ok(wait_result(result, deadline).await)
    }

    pub fn finish(&self, binding: &InteractionBinding, request_id: &str, winner: &str, receipt: Option<Receipt>) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        let Some(q) = s.questions.get(request_id) else { return Ok(()); };
        match &receipt {
            Some(r) if r.generation != binding.generation || r.session_id != binding.conversation
                || q.answer.as_ref().is_none_or(|a| a.receipt.as_ref() != Some(r)) => return Ok(()),
            // Sem recibo, o hook legado só identifica o pedido; não confirma uma publicação com recibo.
            None if q.answer.as_ref().is_some_and(|a| a.receipt.is_some()) => return Ok(()),
            _ => {},
        }
        close_question(&self.interactions, binding, request_id)?;
        let q = s.questions.remove(request_id).ok_or_else(changed)?;
        let accepted = winner == "app" && q.answer.as_ref().is_some_and(|a| a.handed);
        if accepted { s.closed.insert(request_id.into(), (q.public.interaction_id.clone(), receipt)); }
        finish_question(q, if accepted { PluginReply::Accepted } else { PluginReply::Unknown });
        self.notify(&binding.key);
        Ok(())
    }

    pub async fn publish(&self, binding: &InteractionBinding, request: PluginRequest) -> Result<PluginReply, ServiceError> {
        let (result, deadline) = {
            let mut memory = self.memory.lock().map_err(|_| unavailable())?;
            let stopping = memory.stopping;
            let s = session(&mut memory, binding)?;
            if s.publication.as_ref().is_some_and(|p| p.mode == PluginMode::Focus && Instant::now() >= p.deadline) {
                s.publication = None;
            }
            if let Some(p) = &s.publication {
                if p.receipt.publication_id == request.id { (p.result.subscribe(), p.deadline) }
                else { return Ok(PluginReply::Unknown); }
            } else {
                if stopping || s.waiter.is_none() { return Ok(PluginReply::Unavailable); }
                if s.owner.as_ref().is_none_or(|o| !o.modes.contains("receipt_v2") || request.mode == PluginMode::Focus && !o.modes.contains("focus")) {
                    return Ok(PluginReply::NotWritten);
                }
                let receipt = Receipt { publication_id: request.id, generation: binding.generation, session_id: binding.conversation.clone() };
                let payload = json!({"text":request.text,"modo":request.mode,"publication_id":receipt.publication_id,
                    "generation":receipt.generation,"session_id":receipt.session_id});
                let deadline = Instant::now() + if request.mode == PluginMode::Focus { FOCUS } else { PUBLISH };
                let (result, rx) = watch::channel(None);
                let waiter = s.waiter.as_mut().ok_or_else(unavailable)?;
                if !waiter.send(payload) { return Ok(PluginReply::NotWritten); }
                s.publication = Some(Publication { receipt, mode: request.mode, result, deadline, handed: false, poll_id: waiter.id });
                (rx, deadline)
            }
        };
        let reply = wait_result(result, deadline).await;
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        if let Ok(s) = session(&mut memory, binding)
            && s.publication.as_ref().is_some_and(|p| p.deadline == deadline && p.mode == PluginMode::Focus) {
            s.publication = None;
        }
        Ok(reply)
    }

    /// A entrega já se provou no transcript: a publicação sai sem esperar o aviso, que chega depois
    /// dos hooks da conta e cai no vazio, como no Python.
    pub fn abandon(&self, binding: &InteractionBinding, publication_id: &str) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        if s.publication.as_ref().is_some_and(|p| p.receipt.publication_id == publication_id) { s.publication = None; }
        Ok(())
    }

    pub fn acknowledge(&self, binding: &InteractionBinding, receipt: Option<&Receipt>, mode: PluginMode, ok: bool) -> Result<(), ServiceError> {
        let reply = if !ok { PluginReply::Unknown } else if mode == PluginMode::Fill { PluginReply::Filled } else { PluginReply::Accepted };
        self.acknowledge_result(binding, receipt, mode, reply)
    }

    pub fn submission_dropped(&self, binding: &InteractionBinding, receipt: Option<&Receipt>) -> Result<(), ServiceError> {
        self.acknowledge_result(binding, receipt, PluginMode::User, PluginReply::NotWritten)
    }

    fn acknowledge_result(&self, binding: &InteractionBinding, receipt: Option<&Receipt>, mode: PluginMode, reply: PluginReply) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        if let Some(p) = &s.publication {
            let expected_mode = if p.mode == PluginMode::Focus { PluginMode::Fill } else { p.mode.clone() };
            if receipt == Some(&p.receipt) && expected_mode == mode && p.handed {
                p.result.send_replace(Some(reply));
                s.publication = None;
            }
        }
        Ok(())
    }

    pub fn waiting(&self, binding: &InteractionBinding) -> Result<bool, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        Ok(session(&mut memory, binding)?.waiter.is_some())
    }
    pub fn declared_modes(&self, binding: &InteractionBinding) -> Result<HashSet<String>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        Ok(session(&mut memory, binding)?.owner.as_ref().map(|o| o.modes.clone()).unwrap_or_default())
    }
    pub fn alive(&self, binding: &InteractionBinding, now: Instant) -> Result<bool, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        Ok(s.waiter.is_some() || s.heartbeat.is_some_and(|at| now.saturating_duration_since(at) < ALIVE))
    }
    /// O plugin prova que a conversa do vínculo é a atual: um `/pull` aberto agora, que só entra com o
    /// `session_id` dela, e nenhum `session.end` (`fim`, o `/clear` incluso) depois de ele começar.
    /// Para o relatório de diagnóstico: o plugin pedindo ou não, e o que ele informou por último.
    pub fn pull_status(&self, binding: &InteractionBinding, now: Instant) -> Result<PullStatus, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        let seen = s.owner.as_ref().map(|owner| owner.seen);
        Ok(PullStatus { open: s.waiter.is_some(), last_seen_s: seen.map(|seen| now.saturating_duration_since(seen).as_secs()),
            reported: s.reported.is_some(), ended: s.ended.is_some_and(|ended| seen.is_none_or(|seen| ended >= seen)) })
    }
    /// O `/pull` aberto, ou visto há pouco: entre um `/pull` e o seguinte a prova não pisca. O visto há
    /// pouco não diz a conversa de agora: só vale onde nenhuma outra leitura a contradiz.
    pub fn conversation_proved(&self, binding: &InteractionBinding, now: Instant) -> Result<bool, ServiceError> {
        self.proof(binding, Some(now))
    }
    /// Só o `/pull` aberto agora, que o Claude Code aceita com a conversa atual.
    pub fn conversation_pulling(&self, binding: &InteractionBinding) -> Result<bool, ServiceError> {
        self.proof(binding, None)
    }
    fn proof(&self, binding: &InteractionBinding, now: Option<Instant>) -> Result<bool, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        // Conversa nova informada pelo plugin desmente o `/pull` visto há pouco. O fim conta contra o início do
        // `/pull`: um aberto antes do `session.end` não prova a conversa que acabou, mesmo terminando depois. O
        // `/pull` só de subagentes vem de um `claude -p` com o ambiente da sessão: não é o plugin dela.
        let recent = |owner: &Owner| s.reported.is_none() && now.is_some_and(|now| now.saturating_duration_since(owner.seen) <= PULL_PROOF);
        Ok(s.owner.as_ref().is_some_and(|owner| (s.waiter.is_some() || recent(owner))
            && owner.modes.iter().any(|mode| mode != AGENT_MESSAGE_MODE)
            && s.ended.is_none_or(|ended| ended < owner.pulled)))
    }
    /// A conversa nova que o plugin informou depois de a do vínculo acabar, se houver.
    pub fn reported_conversation(&self, binding: &InteractionBinding) -> Result<Option<String>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        Ok(session(&mut memory, binding)?.reported.clone())
    }
    pub fn recent_state(&self, binding: &InteractionBinding, now: Instant) -> Result<Option<(String, Option<String>)>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        Ok(s.state.as_ref().filter(|(at, _, _)| s.waiter.is_some() || now.saturating_duration_since(*at) <= Duration::from_secs(90))
            .map(|(_, state, reason)| (state.clone(), reason.clone())))
    }

    pub fn record_state(&self, binding: &InteractionBinding, state: String, reason: Option<String>, now: Instant) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        if state == "working" { s.suggestion.clear(); }
        if state == "fim" { s.ended = Some(now); }
        s.state = Some((now, state, reason));
        s.state_revision += 1;
        self.notify(&binding.key);
        Ok(())
    }

    /// Só eventos do plugin provam o turno terminal; polling e alterações da fila não trocam esta marca.
    pub fn working_revision(&self, binding: &InteractionBinding, now: Instant) -> Result<Option<u64>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        Ok(s.state.as_ref().filter(|(at, state, _)| state == "working" && s.state_revision > 0
            && (s.waiter.is_some() || now.saturating_duration_since(*at) <= Duration::from_secs(90)))
            .map(|_| s.state_revision))
    }
    pub fn suggest(&self, binding: &InteractionBinding, text: String, shown: bool) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        session(&mut memory, binding)?.suggestion = if shown { text } else { String::new() };
        self.notify(&binding.key);
        Ok(())
    }
    pub fn notice_plan(&self, binding: &InteractionBinding, tool_use_id: String, plan: String, path: Option<String>) -> Result<(), ServiceError> {
        if tool_use_id.is_empty() { return Err(ServiceError("plugin_body_invalid")); }
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        session(&mut memory, binding)?.plan = Some(PlanNotice { tool_use_id, plan, path });
        self.notify(&binding.key);
        Ok(())
    }
    pub fn plan_notice(&self, binding: &InteractionBinding) -> Result<Option<PlanNotice>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        Ok(session(&mut memory, binding)?.plan.clone())
    }
    /// Só o aviso da mesma chamada sai: um mais novo que chegou no meio fica.
    pub fn drop_plan(&self, binding: &InteractionBinding, tool_use_id: &str) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        if s.plan.as_ref().is_some_and(|p| p.tool_use_id == tool_use_id) { s.plan = None; self.notify(&binding.key); }
        Ok(())
    }
    /// `running` reabre um subagente encerrado (retomada pelo `SendMessage`) e apaga o fim.
    pub fn record_agent(&self, binding: &InteractionBinding, agent_id: &str, state: AgentState, at_ms: Option<i64>) -> Result<(), ServiceError> {
        if agent_id.is_empty() || agent_id.len() > 128 { return Err(ServiceError("plugin_body_invalid")); }
        let at = match at_ms {
            Some(ms) => chrono::DateTime::from_timestamp_millis(ms).ok_or(ServiceError("plugin_body_invalid"))?,
            None => chrono::Utc::now(),
        };
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        let ended_at = (state != AgentState::Running).then(|| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        // ponytail: sem teto por conversa; a memória some com a vida da sessão.
        s.agents.insert(agent_id.to_owned(), AgentLife { state, ended_at });
        self.notify(&binding.key);
        Ok(())
    }
    pub fn agent_lives(&self, binding: &InteractionBinding) -> Result<HashMap<String, AgentLife>, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        Ok(session(&mut memory, binding)?.agents.clone())
    }
    /// Plugin vivo que declarou saber entregar a subagentes.
    pub fn can_message_agents(&self, binding: &InteractionBinding, now: Instant) -> bool {
        self.alive(binding, now).unwrap_or(false)
            && self.declared_modes(binding).is_ok_and(|modes| modes.contains(AGENT_MESSAGE_MODE))
    }

    /// Entrega pelo `/pull` aberto (ou o próximo) e espera a confirmação do plugin.
    /// Sem confirmação no prazo o pedido sai da fila: nunca chega atrasado depois do 504.
    pub async fn send_agent(&self, binding: &InteractionBinding, agent_id: &str, text: &str) -> Result<AgentSend, ServiceError> {
        if !self.can_message_agents(binding, Instant::now()) { return Ok(AgentSend::NoPlugin); }
        let (id, receiver) = {
            let mut memory = self.memory.lock().map_err(|_| unavailable())?;
            if memory.stopping { return Ok(AgentSend::NoPlugin); }
            let s = session(&mut memory, binding)?;
            s.agent_next += 1;
            // O relógio no id: o plugin descarta id já entregue, e um backend reiniciado recomeça a contagem.
            let id = format!("agent-msg-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(), s.agent_next);
            let payload = json!({"text":null,"agent_message":{"id":id,"to":agent_id,"text":text}});
            let (sender, receiver) = oneshot::channel();
            s.agent_waits.insert(id.clone(), (agent_id.to_owned(), sender));
            if !s.waiter.as_mut().is_some_and(|w| w.send(payload.clone())) { s.agent_outbox.push_back((id.clone(), payload)); }
            (id, receiver)
        };
        let reply = tokio::time::timeout(AGENT_SEND, receiver).await;
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let reply = match reply { Ok(Ok(reply)) => reply, _ => AgentSend::NoAnswer };
        if let Ok(s) = session(&mut memory, binding) {
            if s.agent_waits.remove(&id).is_some() && reply == AgentSend::NoAnswer {
                s.agent_late.push_back((id.clone(), agent_id.to_owned()));
                if s.agent_late.len() > AGENT_LATE_KEPT { s.agent_late.pop_front(); }
            }
            s.agent_outbox.retain(|(queued, _)| *queued != id);
            if reply == AgentSend::Delivered {
                s.agents.insert(agent_id.to_owned(), AgentLife { state: AgentState::Running, ended_at: None });
                self.notify(&binding.key);
            }
        }
        Ok(reply)
    }
    pub fn agent_sent(&self, binding: &InteractionBinding, id: &str, delivered: bool, reason: Option<String>) -> Result<(), ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        // Sem quem espere, a resposta não volta ao app, mas o que o plugin fez vale: entregue = rodando.
        let orphan = if let Some((agent, sender)) = s.agent_waits.remove(id) {
            let reply = if delivered { AgentSend::Delivered } else { AgentSend::Refused(reason.clone().unwrap_or_default()) };
            sender.send(reply).err().map(|_| agent)
        } else if let Some(at) = s.agent_late.iter().position(|(late, _)| late == id) {
            s.agent_late.remove(at).map(|(_, agent)| agent)
        } else {
            tracing::warn!(id, delivered, "confirmação do plugin para mensagem de subagente desconhecida");
            None
        };
        if let Some(agent) = orphan {
            tracing::warn!(id, agent = %agent, delivered, reason = reason.as_deref().unwrap_or(""), "confirmação do plugin chegou sem o pedido esperando");
            if delivered {
                s.agents.insert(agent, AgentLife { state: AgentState::Running, ended_at: None });
                self.notify(&binding.key);
            }
        }
        Ok(())
    }

    pub fn facts(&self, binding: &InteractionBinding, now: Instant) -> Result<Value, ServiceError> {
        let mut memory = self.memory.lock().map_err(|_| unavailable())?;
        let s = session(&mut memory, binding)?;
        let questions = visible_questions(&self.interactions, s, now)?;
        let age = |at| now.saturating_duration_since(at).as_millis().min(u64::MAX as u128) as u64;
        let question = if questions.len() == 1 { questions.first().map(|q| {
            let mut value = serde_json::to_value(q).expect("campos serializáveis");
            value["seen_age_ms"] = json!(age(s.questions[&q.id].seen)); value
        }) } else { None };
        Ok(json!({"plugin_state":s.state.as_ref().map(|(at,state,reason)| json!({"state":state,"reason":reason,"age_ms":age(*at)})),
            "waiter_open":s.waiter.is_some(),"heartbeat_age_ms":s.heartbeat.map(age),"question":question,
            "questions":questions,"suggestion":s.suggestion,
            "owner_instance":s.owner.as_ref().map(|owner| &owner.instance),
            "declared_modes":s.owner.as_ref().map(|owner| { let mut modes:Vec<_>=owner.modes.iter().collect();modes.sort();modes }).unwrap_or_default()}))
    }
}

async fn wait_result(mut result: watch::Receiver<Option<PluginReply>>, deadline: Instant) -> PluginReply {
    loop {
        if let Some(reply) = result.borrow().clone() { return reply; }
        match tokio::time::timeout_at(deadline.into(), result.changed()).await {
            Ok(Ok(())) => {},
            _ => return result.borrow().clone().unwrap_or(PluginReply::Unknown),
        }
    }
}

/// A guarda acompanha a conexão: um poll antigo nunca remove a espera que o substituiu.
pub struct Poll<'a> {
    guard: Option<PollGuard<'a>>,
    receiver: Option<oneshot::Receiver<Value>>,
    ready: Option<Value>,
    timeout: Duration,
    /// Espera antes da resposta pronta: o hook que repete o poll sem dormir não gira em laço.
    pause: Duration,
}
impl Poll<'_> {
    fn ready(value: Value) -> Self { Self { guard: None, receiver: None, ready: Some(value), timeout: Duration::ZERO, pause: Duration::ZERO } }
    fn later(value: Value, pause: Duration) -> Self { Self { pause, ..Self::ready(value) } }
    /// Espera no máximo `limit`.
    pub fn cap(mut self, limit: Duration) -> Self { self.timeout = self.timeout.min(limit); self }
    pub async fn wait(mut self) -> Value {
        if let Some(value) = self.ready.take() {
            if !self.pause.is_zero() { tokio::time::sleep(self.pause).await; }
            return value;
        }
        let empty = if self.guard.as_ref().is_some_and(|g| g.request_id.is_some()) { json!({"answers":null}) } else { json!({"text":null}) };
        let Some(receiver) = self.receiver.take() else { return empty; };
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(value)) => {
                if let Some(guard) = &mut self.guard { guard.handed = true; }
                value
            },
            _ => empty,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    fn binding() -> InteractionBinding { InteractionBinding { key: "key".into(), conversation: "sid".into(), generation: 2 } }
    fn setup() -> (Store, InteractionBinding) {
        let binding = binding();
        let store = Store::new(Arc::new(InteractionStore::default()));
        store.interactions.bind(&binding).unwrap();
        store.bind(&binding).unwrap();
        (store, binding)
    }
    fn gate() -> PermissionGate { PermissionGate { app_present: true, terminal_attached: false, mode_without_dialog: false } }
    fn ask(id: &str) -> Ask { Ask { id: id.into(), questions: Some(vec![json!({"question":"Escolha","options":["A","B"]})]), tool: None, janela_ms: None, summary: None } }
    fn pull(instance: &str) -> Pull { Pull { instance: instance.into(), modos: vec!["fill".into(),"user".into(),"focus".into(),"receipt_v2".into()], estado: Some("idle".into()), session_id: Some("sid".into()) } }
    fn receipt() -> Receipt { Receipt { publication_id: "publication-1".into(), generation: 2, session_id: "sid".into() } }
    fn request(mode: PluginMode) -> PluginRequest { PluginRequest { id: "publication-1".into(), text: "teste".into(), mode } }

    #[test]
    fn pull_status_tells_whether_the_plugin_is_pulling_and_what_it_reported() {
        let (store, binding) = setup();
        let at = |s: &Store| s.pull_status(&binding, Instant::now()).unwrap();
        assert_eq!(at(&store), PullStatus { open: false, last_seen_s: None, reported: false, ended: false }, "nunca fez /pull");
        let poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        assert_eq!(at(&store), PullStatus { open: true, last_seen_s: Some(0), reported: false, ended: false });
        drop(poll);
        let later = store.pull_status(&binding, Instant::now() + Duration::from_secs(120)).unwrap();
        assert_eq!((later.open, later.last_seen_s), (false, Some(120)), "parou de pedir há 2 min");
        let mut other = pull("i"); other.session_id = Some("sid-nova".into());
        assert!(store.begin_pull(&binding, other, Instant::now()).is_err());
        assert!(at(&store).reported, "conversa nova informada");
        store.record_state(&binding, "fim".into(), None, Instant::now() + Duration::from_millis(1)).unwrap();
        assert!(store.pull_status(&binding, Instant::now() + Duration::from_millis(2)).unwrap().ended);
    }

    #[test]
    fn a_recent_pull_stops_proving_once_the_plugin_reports_another_conversation() {
        let (store, binding) = setup();
        drop(store.begin_pull(&binding, pull("i"), Instant::now()).unwrap());
        let mut other = pull("i"); other.session_id = Some("sid-nova".into());
        assert!(store.begin_pull(&binding, other, Instant::now()).is_err());
        assert!(!store.conversation_proved(&binding, Instant::now()).unwrap(), "o plugin já pede com outra conversa");
    }

    #[test]
    fn a_pull_open_across_the_end_never_proves_the_ended_conversation() {
        let (store, binding) = setup();
        let poll = store.begin_pull(&binding, pull("i"), Instant::now() - Duration::from_millis(20)).unwrap();
        store.record_state(&binding, "fim".into(), None, Instant::now() - Duration::from_millis(10)).unwrap();
        assert!(!store.conversation_pulling(&binding).unwrap(), "aberto antes do fim");
        drop(poll);
        assert!(!store.conversation_proved(&binding, Instant::now()).unwrap(), "encerrado depois do fim");
    }

    #[test]
    fn a_pull_only_for_subagents_proves_nothing() {
        let (store, binding) = setup();
        let mut agents = pull("i"); agents.modos = vec![AGENT_MESSAGE_MODE.into()];
        let poll = store.begin_pull(&binding, agents, Instant::now()).unwrap();
        assert!(!store.conversation_pulling(&binding).unwrap());
        assert!(!store.conversation_proved(&binding, Instant::now()).unwrap());
        drop(poll);
    }

    #[test]
    fn conversation_stays_proved_between_pulls_until_the_window_or_the_end() {
        let (store, binding) = setup();
        assert!(!store.conversation_proved(&binding, Instant::now()).unwrap(), "sem /pull nenhum, nada prova");
        let poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        assert!(store.conversation_proved(&binding, Instant::now()).unwrap());
        drop(poll);
        let between = Instant::now();
        assert!(store.conversation_proved(&binding, between).unwrap(), "o intervalo entre dois /pull não tira a prova");
        assert!(store.conversation_proved(&binding, between + Duration::from_secs(89)).unwrap());
        assert!(!store.conversation_proved(&binding, between + Duration::from_secs(91)).unwrap(), "sem /pull há 90 s, o plugin não prova mais");
        store.record_state(&binding, "fim".into(), None, Instant::now() + Duration::from_millis(1)).unwrap();
        assert!(!store.conversation_proved(&binding, Instant::now() + Duration::from_millis(2)).unwrap(), "session.end depois do /pull");
    }

    #[test]
    fn conversation_held_plugin_memory_cannot_activate_the_public_identity() {
        let binding = binding();
        let store = Store::new(Arc::new(InteractionStore::default()));
        store.bind(&binding).unwrap();
        assert_eq!(store.begin_ask(&binding, ask("q"), gate(), Instant::now()).err().unwrap().0, "erro_interacao_indisponivel");
        store.interactions.bind(&binding).unwrap();
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        assert!(store.pending(&binding, Instant::now()).is_some());
        drop(poll);
    }

    #[test]
    fn conversation_held_question_expires_without_poll_and_on_interrupt() {
        assert!(question_visible(false, true, Duration::from_secs(25)));
        assert!(question_visible(false, true, ALIVE));
        assert!(question_visible(false, false, NO_POLL));
        assert!(!question_visible(false, false, Duration::from_secs(6)));
        assert!(!question_visible(true, true, Duration::ZERO));
        assert!(!question_visible(false, true, Duration::from_secs(36)));
    }

    #[tokio::test]
    async fn conversation_held_permissions_require_an_app_no_terminal_and_a_dialog_mode() {
        for gate in [PermissionGate { app_present: false, ..gate() }, PermissionGate { terminal_attached: true, ..gate() }, PermissionGate { mode_without_dialog: true, ..gate() }] {
            let (store, binding) = setup();
            let poll = store.begin_ask(&binding, ask("perm:tool"), super::tests::gate(), Instant::now()).unwrap();
            let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
            assert_eq!(store.begin_ask(&binding, ask("perm:tool"), gate, Instant::now()).unwrap().wait().await, json!({"soltar":true}));
            assert_eq!(poll.wait().await, json!({"answers":null}));
            assert!(store.interactions.validate(&binding, Some(&id)).is_err());
            assert!(store.pending(&binding, Instant::now()).is_none());
        }
    }

    #[tokio::test]
    async fn conversation_held_polls_do_not_rotate_tokens_or_close_siblings() {
        let (store, binding) = setup();
        let old = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        let first = store.pending(&binding, Instant::now()).unwrap();
        let new = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        drop(old);
        assert_eq!(first, store.pending(&binding, Instant::now()).unwrap());
        let other = store.begin_ask(&binding, ask("perm:p"), gate(), Instant::now()).unwrap();
        assert!(store.pending(&binding, Instant::now()).is_none());
        assert_eq!(store.pending_all(&binding, Instant::now()).unwrap().len(), 2);
        store.interrupt(&binding, "q").unwrap();
        assert!(store.interactions.validate(&binding, Some(&first.interaction_id)).is_err());
        assert_eq!(new.wait().await, json!({"answers":null}));
        assert_eq!(store.pending(&binding, Instant::now()).unwrap().id, "perm:p");
        drop(other);
    }

    /// Servidor que perdeu o espelho do plugin (reiniciado): o `/pull` volta logo com `faixa:false`, e o plugin
    /// reenvia o `/ui` sem esperar o long-poll inteiro.
    #[tokio::test(start_paused = true)]
    async fn capped_pull_returns_empty_before_the_long_poll() {
        let (store, binding) = setup();
        let started = tokio::time::Instant::now();
        let poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap().cap(Duration::from_secs(1));
        assert_eq!(poll.wait().await, json!({"text":null}));
        assert!(started.elapsed() <= Duration::from_secs(1), "{:?}", started.elapsed());
    }

    /// Parity P22 (`test_interrupcao_pelo_app_solta_a_pergunta_na_hora_e_acorda_o_long_poll`): depois da
    /// interrupção, o poll seguinte do mesmo hook espera a janela em vez de voltar na hora (laço sem pausa).
    #[tokio::test(start_paused = true)]
    async fn interrupted_question_next_poll_waits_instead_of_spinning() {
        let (store, binding) = setup();
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        store.interrupt(&binding, "q").unwrap();
        assert_eq!(poll.wait().await, json!({"answers":null}), "a interrupção acorda o long-poll na hora");
        let started = tokio::time::Instant::now();
        assert_eq!(store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap().wait().await, json!({"answers":null}));
        assert!(started.elapsed() >= HOOK_ALIVE - Duration::from_millis(50), "{:?}", started.elapsed());
    }

    #[test]
    fn conversation_held_expired_poll_cannot_revive_the_old_public_identity() {
        let (store, binding) = setup();
        let now = Instant::now();
        let old = store.begin_ask(&binding, ask("q"), gate(), now).unwrap();
        let id = store.pending(&binding, now).unwrap().interaction_id;
        assert!(store.pending(&binding, now + ALIVE + Duration::from_secs(1)).is_none());
        drop(old);
        assert!(store.pending(&binding, Instant::now()).is_none());
        assert!(store.interactions.validate(&binding, Some(&id)).is_err());
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        assert_ne!(store.pending(&binding, Instant::now()).unwrap().interaction_id, id);
        drop(poll);
    }

    #[tokio::test]
    async fn conversation_held_answer_is_single_and_requires_matching_receipt_and_winner() {
        let (store, binding) = setup();
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
        let mut first = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"A"}}), Some(receipt())));
        let mut duplicate = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"B"}}), Some(receipt())));
        assert!(first.as_mut().now_or_never().is_none());
        assert!(duplicate.as_mut().now_or_never().is_none());
        assert_eq!(poll.wait().await, json!({"publication_id":"publication-1","generation":2,"session_id":"sid","answers":{"Escolha":"A"}}));
        store.finish(&binding, "q", "app", Some(Receipt { generation: 1, ..receipt() })).unwrap();
        assert!(first.as_mut().now_or_never().is_none());
        store.finish(&binding, "q", "app", Some(receipt())).unwrap();
        assert_eq!(first.await.unwrap(), PluginReply::Accepted);
        assert_eq!(duplicate.await.unwrap(), PluginReply::Accepted);
        assert_eq!(store.answer(&binding, "q", &id, json!({}), Some(receipt())).await.unwrap(), PluginReply::Accepted);
        assert!(store.interactions.validate(&binding, Some(&id)).is_err());
    }

    #[tokio::test]
    async fn conversation_held_answer_preserves_the_hook_body_at_the_top_level() {
        for (request_id, body) in [
            ("perm:allow", json!({"permitir":true})),
            ("perm:deny", json!({"permitir":false})),
            ("ask:form", json!({"answers":{"Escolha":"A"}})),
            ("ask:skip", json!({"deny":"O usuário prefere conversar antes de responder."})),
        ] {
            for receipt in [None, Some(receipt())] {
                let (store, binding) = setup();
                let poll = store.begin_ask(&binding, ask(request_id), gate(), Instant::now()).unwrap();
                let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
                let mut answer = Box::pin(store.answer(&binding, request_id, &id, body.clone(), receipt.clone()));
                assert!(answer.as_mut().now_or_never().is_none());
                let mut expected = body.clone();
                if let Some(r) = &receipt {
                    expected["publication_id"] = json!(r.publication_id);
                    expected["generation"] = json!(r.generation);
                    expected["session_id"] = json!(r.session_id);
                }
                assert_eq!(poll.wait().await, expected);
                store.finish(&binding, request_id, "app", receipt).unwrap();
                assert_eq!(answer.await.unwrap(), PluginReply::Accepted);
            }
        }
    }

    #[tokio::test]
    async fn conversation_held_answer_rejects_a_non_object_before_publication() {
        let (store, binding) = setup();
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
        for body in [Value::Null, json!(true), json!([])] {
            assert_eq!(store.answer(&binding, "q", &id, body, Some(receipt())).await.unwrap_err().0, "plugin_answer_body_invalid");
        }
        assert!(store.memory.lock().unwrap().sessions[&binding.key].questions["q"].answer.is_none());
        drop(poll);
    }

    #[tokio::test]
    async fn conversation_held_late_receipt_cannot_finish_a_reopened_question_without_answer() {
        let (store, binding) = setup();
        let first_poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        let first_id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
        let mut first = Box::pin(store.answer(&binding, "q", &first_id, json!({"answers":{"Escolha":"A"}}), Some(receipt())));
        assert!(first.as_mut().now_or_never().is_none());
        first_poll.wait().await;
        store.finish(&binding, "q", "app", Some(receipt())).unwrap();
        assert_eq!(first.await.unwrap(), PluginReply::Accepted);

        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        let reopened = store.pending(&binding, Instant::now()).unwrap();
        assert_ne!(reopened.interaction_id, first_id);
        assert!(store.memory.lock().unwrap().sessions[&binding.key].questions["q"].answer.is_none());
        store.finish(&binding, "q", "app", Some(receipt())).unwrap();
        store.finish(&binding, "q", "terminal", Some(receipt())).unwrap();
        assert_eq!(store.pending(&binding, Instant::now()).unwrap(), reopened);
        assert_eq!(store.interactions.validate(&binding, Some(&reopened.interaction_id)).unwrap(), target("q"));

        let second_receipt = Receipt { publication_id: "publication-2".into(), ..receipt() };
        let mut second = Box::pin(store.answer(&binding, "q", &reopened.interaction_id, json!({"answers":{"Escolha":"B"}}), Some(second_receipt.clone())));
        assert!(second.as_mut().now_or_never().is_none());
        assert_eq!(poll.wait().await["answers"], json!({"Escolha":"B"}));
        store.finish(&binding, "q", "app", Some(receipt())).unwrap();
        store.finish(&binding, "q", "app", None).unwrap();
        assert!(second.as_mut().now_or_never().is_none());
        store.finish(&binding, "q", "app", Some(second_receipt)).unwrap();
        assert_eq!(second.await.unwrap(), PluginReply::Accepted);
    }

    #[tokio::test]
    async fn conversation_held_expired_unconsumed_answer_releases_both_duplicate_waiters() {
        for refresh_first in [false, true] {
            let (store, binding) = setup();
            let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
            let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
            let mut first = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"A"}}), Some(receipt())));
            let mut duplicate = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"B"}}), Some(receipt())));
            assert!(first.as_mut().now_or_never().is_none());
            assert!(duplicate.as_mut().now_or_never().is_none());
            drop(poll);
            {
                let mut memory = store.memory.lock().unwrap();
                let q = memory.sessions.get_mut(&binding.key).unwrap().questions.get_mut("q").unwrap();
                let answer = q.answer.as_mut().unwrap();
                assert!(!answer.handed && answer.poll_id.is_none());
                answer.deadline = Instant::now() - Duration::from_millis(1);
            }
            let mut next_poll = if refresh_first {
                let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
                assert!(store.memory.lock().unwrap().sessions[&binding.key].questions["q"].answer.is_none());
                Some(poll)
            } else { None };
            let second_receipt = Receipt { publication_id: "publication-2".into(), ..receipt() };
            let mut second = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"B"}}), Some(second_receipt.clone())));
            assert!(second.as_mut().now_or_never().is_none());
            assert_eq!(first.await.unwrap(), PluginReply::NotWritten);
            assert_eq!(duplicate.await.unwrap(), PluginReply::NotWritten);
            if next_poll.is_none() { next_poll = Some(store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap()); }
            assert_eq!(store.pending(&binding, Instant::now()).unwrap().interaction_id, id);
            let payload = next_poll.unwrap().wait().await;
            assert_eq!(payload["publication_id"], "publication-2");
            assert_eq!(payload["answers"], json!({"Escolha":"B"}));
            store.finish(&binding, "q", "app", Some(second_receipt)).unwrap();
            assert_eq!(second.await.unwrap(), PluginReply::Accepted);
        }
    }

    #[tokio::test]
    async fn conversation_held_expired_possible_delivery_keeps_receipt_and_never_replays() {
        for consumed in [false, true] {
            let (store, binding) = setup();
            let mut poll = Some(store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap());
            let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
            let mut answer = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"A"}}), Some(receipt())));
            assert!(answer.as_mut().now_or_never().is_none());
            if consumed { poll.take().unwrap().wait().await; }
            {
                let mut memory = store.memory.lock().unwrap();
                let answer = memory.sessions.get_mut(&binding.key).unwrap().questions.get_mut("q").unwrap().answer.as_mut().unwrap();
                assert!(answer.handed || answer.poll_id.is_some());
                answer.deadline = Instant::now() - Duration::from_millis(1);
            }
            let next_poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
            assert!(next_poll.ready.is_none());
            let other = Receipt { publication_id: "publication-2".into(), ..receipt() };
            assert_eq!(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"B"}}), Some(other)).await.unwrap_err().0, "plugin_publication_conflict");
            assert_eq!(store.answer(&binding, "q", &id, json!({}), Some(receipt())).await.unwrap(), PluginReply::Unknown);
            if let Some(poll) = poll { assert_eq!(poll.wait().await["publication_id"], "publication-1"); }
            store.finish(&binding, "q", "app", Some(receipt())).unwrap();
            assert_eq!(answer.await.unwrap(), PluginReply::Accepted);
            assert_eq!(next_poll.wait().await, json!({"answers":null}));
        }
    }

    #[tokio::test]
    async fn conversation_held_terminal_winner_and_unsolicited_finish_are_not_acceptance() {
        let (store, binding) = setup();
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
        let mut answer = Box::pin(store.answer(&binding, "q", &id, json!({}), None));
        assert!(answer.as_mut().now_or_never().is_none());
        poll.wait().await;
        store.finish(&binding, "q", "terminal", None).unwrap();
        assert_eq!(answer.await.unwrap(), PluginReply::Unknown);
        assert!(store.answer(&binding, "q", &id, json!({}), None).await.is_err());
    }

    #[tokio::test]
    async fn conversation_held_stored_answer_expires_and_cancel_does_not_republish() {
        let (store, binding) = setup();
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        drop(poll);
        let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
        let mut answer = Box::pin(store.answer(&binding, "q", &id, json!({"answers":{"Escolha":"A"}}), Some(receipt())));
        assert!(answer.as_mut().now_or_never().is_none());
        drop(answer);
        // A desconexão do app não perde o recibo de uma resposta que ainda pode chegar ao hook.
        let poll = store.begin_ask(&binding, ask("q"), gate(), Instant::now()).unwrap();
        assert_eq!(poll.wait().await["publication_id"], "publication-1");
        let mut again = ask("q"); again.janela_ms = Some(-1);
        assert_eq!(store.begin_ask(&binding, again, gate(), Instant::now()).unwrap().wait().await, json!({"answers":null}));
        store.finish(&binding, "q", "app", Some(receipt())).unwrap();
        assert_eq!(store.answer(&binding, "q", &id, json!({}), Some(receipt())).await.unwrap(), PluginReply::Accepted);

        let poll = store.begin_ask(&binding, ask("expired"), gate(), Instant::now()).unwrap();
        drop(poll);
        let id = store.pending(&binding, Instant::now()).unwrap().interaction_id;
        let mut answer = Box::pin(store.answer(&binding, "expired", &id, json!({}), None));
        assert!(answer.as_mut().now_or_never().is_none());
        drop(answer);
        let mut again = ask("expired"); again.janela_ms = Some(-1);
        assert_eq!(store.begin_ask(&binding, again, gate(), Instant::now() + CONFIRM + Duration::from_millis(1)).unwrap().wait().await, json!({"answers":null}));
    }

    #[test]
    fn conversation_held_old_poll_never_recreates_owner_or_overwrites_new_state() {
        let (store, binding) = setup();
        let old = store.begin_pull(&binding, pull("old"), Instant::now()).unwrap();
        store.forget(&binding.key).unwrap();
        drop(old);
        assert!(store.memory.lock().unwrap().sessions.is_empty());
        store.interactions.bind(&binding).unwrap();
        store.bind(&binding).unwrap();
        let old = store.begin_pull(&binding, pull("old"), Instant::now()).unwrap();
        let new = store.begin_pull(&binding, pull("new"), Instant::now() + ALIVE).unwrap();
        store.record_state(&binding, "working".into(), None, Instant::now()).unwrap();
        drop(old);
        let memory = store.memory.lock().unwrap();
        let s = &memory.sessions[&binding.key];
        assert_eq!(s.owner.as_ref().unwrap().instance, "new");
        assert_eq!(s.state.as_ref().unwrap().1, "working");
        assert!(s.waiter.is_some());
        drop(memory);
        drop(new);
    }

    #[test]
    fn conversation_held_closing_an_old_target_never_erases_the_new_life() {
        let (store, old) = setup();
        let poll = store.begin_ask(&old, ask("old"), gate(), Instant::now()).unwrap();
        let new = InteractionBinding { generation: old.generation + 1, ..old.clone() };
        store.interactions.bind(&new).unwrap();
        let token = store.interactions.observe(&new, &target("new"), Some(json!({"question":"B"}))).unwrap().unwrap();
        assert_eq!(store.interrupt(&old, "old").unwrap_err().0, "erro_interacao_desatualizada");
        drop(poll);
        assert_eq!(store.interactions.validate(&new, Some(&token)).unwrap(), target("new"));
    }

    #[test]
    fn conversation_held_bind_and_forget_preserve_a_new_life_already_observed() {
        for forget in [false, true] {
            let (store, old) = setup();
            let poll = store.begin_ask(&old, ask("old"), gate(), Instant::now()).unwrap();
            let new = InteractionBinding { generation: old.generation + 1, ..old.clone() };
            store.interactions.bind(&new).unwrap();
            let token = store.interactions.observe(&new, &target("new"), Some(json!({"question":"B"}))).unwrap().unwrap();
            if forget { store.forget(&old.key).unwrap(); } else { store.bind(&new).unwrap(); }
            drop(poll);
            assert_eq!(store.interactions.validate(&new, Some(&token)).unwrap(), target("new"));
            assert!(store.begin_ask(&old, ask("old"), gate(), Instant::now()).is_err());
        }
    }

    #[test]
    fn conversation_held_liveness_and_state_age_use_the_monotonic_clock() {
        let (store, binding) = setup();
        let now = Instant::now();
        store.record_state(&binding, "working".into(), None, now).unwrap();
        let poll = store.begin_pull(&binding, pull("i"), now).unwrap();
        assert!(store.waiting(&binding).unwrap());
        assert!(store.declared_modes(&binding).unwrap().contains("receipt_v2"));
        assert_eq!(store.recent_state(&binding, now + Duration::from_secs(91)).unwrap().unwrap().0, "working");
        drop(poll);
        let now = Instant::now();
        assert!(!store.waiting(&binding).unwrap());
        assert!(store.alive(&binding, now).unwrap());
        assert!(!store.alive(&binding, now + ALIVE).unwrap());
        assert!(store.recent_state(&binding, now + Duration::from_secs(91)).unwrap().is_none());
    }

    #[test]
    fn conversation_held_other_conversation_or_live_instance_cannot_take_pull() {
        let (store, binding) = setup();
        let first = store.begin_pull(&binding, pull("first"), Instant::now()).unwrap();
        assert_eq!(store.begin_pull(&binding, pull("second"), Instant::now()).err().unwrap().0, "plugin_instance_conflict");
        assert_eq!(store.reported_conversation(&binding).unwrap(), None);
        let mut wrong = pull("first"); wrong.session_id = Some("other".into());
        assert_eq!(store.begin_pull(&binding, wrong, Instant::now()).err().unwrap().0, "plugin_conversation_mismatch");
        // Parity item 3: a conversa nova que o plugin informa é o sinal da troca (`/clear`, `/resume`).
        assert_eq!(store.reported_conversation(&binding).unwrap().as_deref(), Some("other"));
        let future = InteractionBinding { generation: 3, ..binding.clone() };
        store.interactions.bind(&future).unwrap();
        store.bind(&future).unwrap();
        drop(first);
        assert!(store.begin_pull(&binding, pull("first"), Instant::now()).is_err());
    }

    #[tokio::test]
    async fn conversation_held_terminal_publications_track_disconnect_and_all_receipts() {
        for (mode, ack, reply) in [(PluginMode::Fill, PluginMode::Fill, PluginReply::Filled), (PluginMode::User, PluginMode::User, PluginReply::Accepted), (PluginMode::Focus, PluginMode::Fill, PluginReply::Filled)] {
            let (store, binding) = setup();
            let poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
            let mut publish = Box::pin(store.publish(&binding, request(mode)));
            assert!(publish.as_mut().now_or_never().is_none());
            assert_eq!(poll.wait().await["publication_id"], "publication-1");
            store.acknowledge(&binding, Some(&Receipt { session_id: "wrong".into(), ..receipt() }), ack.clone(), true).unwrap();
            assert!(publish.as_mut().now_or_never().is_none());
            store.acknowledge(&binding, Some(&receipt()), ack, true).unwrap();
            assert_eq!(publish.await.unwrap(), reply);
        }
        let (store, binding) = setup();
        let poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        let mut publish = Box::pin(store.publish(&binding, request(PluginMode::Fill)));
        assert!(publish.as_mut().now_or_never().is_none());
        drop(poll);
        assert_eq!(publish.await.unwrap(), PluginReply::NotWritten);
    }

    #[tokio::test]
    async fn conversation_held_canceled_publication_retains_unknown_until_receipt() {
        let (store, binding) = setup();
        let old = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        let poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        let mut publish = Box::pin(store.publish(&binding, request(PluginMode::Fill)));
        assert!(publish.as_mut().now_or_never().is_none());
        drop(old);
        assert!(publish.as_mut().now_or_never().is_none());
        poll.wait().await;
        drop(publish);
        let next = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        let other = PluginRequest { id: "other".into(), ..request(PluginMode::Fill) };
        assert_eq!(store.publish(&binding, other).await.unwrap(), PluginReply::Unknown);
        store.acknowledge(&binding, Some(&receipt()), PluginMode::Fill, true).unwrap();
        assert!(store.memory.lock().unwrap().sessions[&binding.key].publication.is_none());
        drop(next);
    }

    #[tokio::test]
    async fn conversation_held_capabilities_and_shutdown_never_claim_delivery() {
        let (store, binding) = setup();
        assert_eq!(store.publish(&binding, request(PluginMode::Fill)).await.unwrap(), PluginReply::Unavailable);
        let mut old = pull("i"); old.modos = vec!["fill".into()];
        let poll = store.begin_pull(&binding, old, Instant::now()).unwrap();
        assert_eq!(store.publish(&binding, request(PluginMode::Fill)).await.unwrap(), PluginReply::NotWritten);
        store.stop().unwrap();
        assert_eq!(poll.wait().await, json!({"text":null}));
        assert_eq!(store.publish(&binding, request(PluginMode::Fill)).await.unwrap(), PluginReply::Unavailable);
    }

    fn agent_pull() -> Pull { let mut p = pull("i"); p.modos.push(AGENT_MESSAGE_MODE.into()); p }

    #[test]
    fn agent_lifecycle_finishes_fails_and_resume_reopens() {
        let (store, binding) = setup();
        store.record_agent(&binding, "a1", AgentState::Running, None).unwrap();
        assert_eq!(store.agent_lives(&binding).unwrap()["a1"], AgentLife { state: AgentState::Running, ended_at: None });
        store.record_agent(&binding, "a1", AgentState::Finished, Some(1_791_900_509_038)).unwrap();
        assert_eq!(store.agent_lives(&binding).unwrap()["a1"].ended_at.as_deref(), Some("2026-10-13T14:08:29.038Z"));
        store.record_agent(&binding, "a1", AgentState::Running, None).unwrap();
        assert_eq!(store.agent_lives(&binding).unwrap()["a1"], AgentLife { state: AgentState::Running, ended_at: None }, "retomada");
        store.record_agent(&binding, "a2", AgentState::Failed, None).unwrap();
        assert!(store.agent_lives(&binding).unwrap()["a2"].ended_at.is_some());
        assert_eq!(store.record_agent(&binding, "", AgentState::Running, None).unwrap_err().0, "plugin_body_invalid");
        // Outra vida da sessão começa sem os sinais da anterior.
        let next = InteractionBinding { generation: 3, ..binding.clone() };
        store.interactions.bind(&next).unwrap();
        store.bind(&next).unwrap();
        assert!(store.agent_lives(&next).unwrap().is_empty());
        assert!(store.agent_lives(&binding).is_err());
    }

    #[tokio::test]
    async fn agent_message_round_trip_through_the_open_pull_marks_running() {
        let (store, binding) = setup();
        store.record_agent(&binding, "a1", AgentState::Finished, None).unwrap();
        let poll = store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap();
        assert!(store.can_message_agents(&binding, Instant::now()));
        let mut send = Box::pin(store.send_agent(&binding, "a1", "olá"));
        assert!(send.as_mut().now_or_never().is_none());
        let got = poll.wait().await;
        let id = msg_id(&got);
        assert_eq!(got["agent_message"], json!({"id":id,"to":"a1","text":"olá"}));
        assert_eq!(got["text"], Value::Null, "plugin antigo não confunde com prompt");
        store.agent_sent(&binding, &id, true, None).unwrap();
        assert_eq!(send.await.unwrap(), AgentSend::Delivered);
        assert_eq!(store.agent_lives(&binding).unwrap()["a1"].state, AgentState::Running);
    }

    fn msg_id(got: &Value) -> String { got["agent_message"]["id"].as_str().unwrap().to_owned() }

    /// O `claude -p` (sem terminal, ou filho de uma sessão que herdou a ponte) abre o `/pull` só com
    /// `agent_message`: não fala de troca de conversa nem recebe a entrada da sessão.
    #[tokio::test]
    async fn agent_only_pull_reports_no_conversation_and_takes_no_input() {
        let (store, binding) = setup();
        let only_agents = |sid: &str| Pull { instance: "p".into(), modos: vec![AGENT_MESSAGE_MODE.into()], estado: None, session_id: Some(sid.into()) };
        assert_eq!(store.begin_pull(&binding, only_agents("child"), Instant::now()).err().unwrap().0, "plugin_conversation_mismatch");
        assert_eq!(store.reported_conversation(&binding).unwrap(), None, "o filho não troca a conversa da sessão");
        let _poll = store.begin_pull(&binding, only_agents("sid"), Instant::now()).unwrap();
        assert!(store.can_message_agents(&binding, Instant::now()));
        assert_eq!(store.publish(&binding, request(PluginMode::User)).await.unwrap(), PluginReply::NotWritten);
        assert_eq!(store.recent_state(&binding, Instant::now()).unwrap(), None);
    }

    #[tokio::test]
    async fn agent_message_ids_do_not_repeat_across_backend_restarts() {
        // O plugin descarta id já entregue: um backend reiniciado não pode reusar o id da vida anterior.
        let mut ids = Vec::new();
        for _ in 0..2 {
            let (store, binding) = setup();
            let poll = store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap();
            let mut send = Box::pin(store.send_agent(&binding, "a1", "x"));
            assert!(send.as_mut().now_or_never().is_none());
            let id = msg_id(&poll.wait().await);
            store.agent_sent(&binding, &id, true, None).unwrap();
            send.await.unwrap();
            ids.push(id);
        }
        assert_ne!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn agent_message_queues_for_the_next_pull_and_reports_refusal() {
        let (store, binding) = setup();
        drop(store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap());
        let mut send = Box::pin(store.send_agent(&binding, "a1", "x"));
        assert!(send.as_mut().now_or_never().is_none());
        let got = store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap().wait().await;
        store.agent_sent(&binding, &msg_id(&got), false, Some("this session has no SendMessage tool".into())).unwrap();
        assert_eq!(send.await.unwrap(), AgentSend::Refused("this session has no SendMessage tool".into()));
        assert!(store.agent_lives(&binding).unwrap().is_empty());
    }

    #[tokio::test]
    async fn agent_message_needs_a_plugin_that_declared_it() {
        let (store, binding) = setup();
        assert_eq!(store.send_agent(&binding, "a1", "x").await.unwrap(), AgentSend::NoPlugin);
        let _poll = store.begin_pull(&binding, pull("i"), Instant::now()).unwrap();
        assert!(!store.can_message_agents(&binding, Instant::now()));
        assert_eq!(store.send_agent(&binding, "a1", "x").await.unwrap(), AgentSend::NoPlugin);
    }

    #[tokio::test(start_paused = true)]
    async fn agent_message_without_confirmation_is_unknown_and_leaves_the_queue() {
        let (store, binding) = setup();
        drop(store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap());
        assert_eq!(store.send_agent(&binding, "a1", "x").await.unwrap(), AgentSend::NoAnswer);
        let next = store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap();
        assert!(next.wait().now_or_never().is_none(), "texto expirado não sai no pull seguinte");
    }

    #[tokio::test(start_paused = true)]
    async fn agent_confirmation_after_the_deadline_still_marks_running() {
        let (store, binding) = setup();
        store.record_agent(&binding, "a1", AgentState::Finished, None).unwrap();
        let poll = store.begin_pull(&binding, agent_pull(), Instant::now()).unwrap();
        let (sent, got) = tokio::join!(store.send_agent(&binding, "a1", "x"), poll.wait());
        assert_eq!(sent.unwrap(), AgentSend::NoAnswer);
        store.agent_sent(&binding, &msg_id(&got), true, None).unwrap();
        assert_eq!(store.agent_lives(&binding).unwrap()["a1"].state, AgentState::Running, "chegou depois do 504, mas chegou");
        store.agent_sent(&binding, "agent-msg-9", true, None).unwrap();
    }

    #[test]
    fn agent_end_with_an_invalid_timestamp_is_refused() {
        let (store, binding) = setup();
        assert_eq!(store.record_agent(&binding, "a1", AgentState::Finished, Some(i64::MAX)).unwrap_err().0, "plugin_body_invalid");
        assert!(store.agent_lives(&binding).unwrap().is_empty());
    }
}

struct PollGuard<'a> { store: &'a Store, binding: InteractionBinding, request_id: Option<String>, id: u64, handed: bool }
impl Drop for PollGuard<'_> {
    fn drop(&mut self) {
        let Ok(mut memory) = self.store.memory.lock() else {
            tracing::error!(code = "plugin_store_unavailable", "não foi possível encerrar a espera do plugin"); return;
        };
        let Ok(s) = session(&mut memory, &self.binding) else { return; };
        let now = Instant::now();
        if let Some(id) = &self.request_id {
            if let Some(q) = s.questions.get_mut(id) {
                if q.waiter.as_ref().is_some_and(|w| w.id == self.id) { q.waiter = None; q.seen = now; }
                if let Some(answer) = q.answer.as_mut().filter(|a| a.poll_id == Some(self.id)) {
                    if self.handed { answer.handed = true; }
                    else { answer.poll_id = None; }
                }
            }
        } else {
            if s.waiter.as_ref().is_some_and(|w| w.id == self.id) {
                s.waiter = None;
                s.heartbeat = Some(now);
                if let Some(owner) = &mut s.owner { owner.seen = now; }
            }
            if let Some(p) = s.publication.as_mut().filter(|p| p.poll_id == self.id) {
                if self.handed { p.handed = true; }
                else if !p.handed { p.result.send_replace(Some(PluginReply::NotWritten)); s.publication = None; }
            }
        }
        self.store.notify(&self.binding.key);
    }
}
