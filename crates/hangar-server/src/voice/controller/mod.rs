//! O controlador da chamada: as regras do `voice_ui.rs` nativo que não desenham nada (ferramentas de sessão, Jev,
//! acompanhar, respostas faladas, ferramentas de tela repassadas ao aparelho e o retrato do estado).
//! O laço nunca espera rede: cada chamada roda em tarefa própria e o que muda estado volta por `Done`.
mod tools;
mod watch;
#[cfg(test)]
mod tests;

use super::call::{Activity, Backstage, CallId, Phase, Voice, VoiceEvent, VoiceFailure};
use super::machines::{CallError, HERE, Machines};
use super::organizer::{ConfirmGate, Effective, Mode, OrganizerAction, tool_reply};
use super::protocol::{ClientMsg, Screen, ServerMsg, ToController};
use super::rules::{Delivery, SwitchOffer};
use super::{SendVerdict, jev, log, usage::RateWindow};
use crate::diag::DiagClient;
use hangar_api::{chat::ChatEvent, session::SessionRow};
use serde_json::{Value, json};
use std::{collections::{HashMap, HashSet, VecDeque}, path::PathBuf, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::sync::mpsc;

/// `(máquina, nome)`: `HERE` ou o id do peer.
type Key = (String, String);

/// No máximo um retrato do estado por intervalo.
const STATE_EVERY: Duration = Duration::from_millis(150);
const BACKSTAGE_KEEP: usize = 60;
const THOUGHT_KEEP: usize = 2000;
const MOVED: &str = "A chamada passou para outro aparelho; tente de novo.";
const NO_DEVICE: &str = "Nenhum aparelho conectado agora; tente de novo quando a voz voltar.";
const UNSUPPORTED_TOOL: &str = "Este aparelho não atende essa ferramenta; peça no PC.";

/// De onde e em que ritmo reler a trava do beta: desligar no meio encerra a chamada.
pub struct GateSource { pub home: PathBuf, pub claude_dir: PathBuf, pub every: Duration }

#[derive(Default)]
struct DeviceState { owner: Option<mpsc::UnboundedSender<ServerMsg>>, caps: Vec<String>, screen: Option<Screen> }

/// O aparelho dono agora: o hub troca o dono, as capacidades e a tela; o controlador manda a quem estiver lá.
#[derive(Clone, Default)]
pub struct DeviceLink { state: Arc<Mutex<DeviceState>> }

impl DeviceLink {
    /// Já com um dono, que recebe pelo receptor devolvido.
    pub fn channel(caps: Vec<String>, screen: Option<Screen>) -> (Self, mpsc::UnboundedReceiver<ServerMsg>) {
        let (out, rx) = mpsc::unbounded_channel();
        let link = Self::default();
        link.replace_owner(out, caps, screen);
        (link, rx)
    }
    /// Novo dono com as capacidades e a tela dele; devolve o anterior.
    pub fn replace_owner(&self, owner: mpsc::UnboundedSender<ServerMsg>, caps: Vec<String>, screen: Option<Screen>) -> Option<mpsc::UnboundedSender<ServerMsg>> {
        let mut s = self.lock();
        (s.caps, s.screen) = (caps, screen);
        s.owner.replace(owner)
    }
    /// O dono caiu; capacidades e tela ficam até o próximo.
    pub fn clear_owner(&self) { self.lock().owner = None; }
    pub fn send(&self, msg: ServerMsg) {
        match &self.lock().owner {
            Some(owner) => { let _ = owner.send(msg); }
            None => log("device message dropped: no owner"),
        }
    }
    pub fn caps(&self) -> Vec<String> { self.lock().caps.clone() }
    pub fn screen(&self) -> Option<Screen> { self.lock().screen.clone() }
    pub fn set_caps(&self, caps: Vec<String>) { self.lock().caps = caps; }
    pub fn set_screen(&self, screen: Option<Screen>) { self.lock().screen = screen; }
    fn can(&self, tool: &str) -> bool { self.lock().caps.iter().any(|c| c == tool) }
    fn lock(&self) -> std::sync::MutexGuard<'_, DeviceState> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }
}

/// Ferramenta pedida ao aparelho, esperando o `tool_result`.
enum Forwarded {
    Reply(CallId),
    Switch(CallId, Key),
    Opened { call: CallId, name: String, sent: Option<Result<Delivery, String>> },
    /// O Jev agiu: o veredito sai com a confirmação falada só quando o aparelho confirma.
    JevSwitch { turn: String, verdict: SendVerdict, key: Key, machine: String },
    JevAction { turn: String, verdict: SendVerdict, label: String },
}

/// Ferramenta que espera a lista de sessões fresca para resolver o nome falado.
enum Want {
    List(CallId),
    Switch { call: CallId, name: String, said: String, recent: String },
    Close { call: CallId, name: String, confirmed: bool, turn: String },
    Pair(CallId, String, String),
    Unpair(CallId, String),
    Follow(CallId, String, bool),
    Send { call: CallId, request: String, name: String, turn: String },
}

enum Done {
    Rows { rows: Vec<(String, SessionRow)>, unreachable: Vec<String>, want: Want },
    Watch { rows: Vec<(String, SessionRow)>, fired: Vec<(Key, SessionRow)> },
    History { key: Key, row: SessionRow, result: Result<Vec<ChatEvent>, String> },
    Sent { key: Key, turn: String, error: Option<CallError> },
    Closed { call: CallId, key: Key, result: Result<(), String> },
    /// A sessão aceitou o plano: o próximo nasce para a sessão da tela de então.
    PlanDelivered,
    Opened { call: CallId, machine: String, name: String, sent: Option<Result<Delivery, String>> },
    AskFailed(Key),
    Jev(tools::JevAsked, Result<jev::Decision, String>),
    Failed { code: &'static str, text: String },
    Gate(bool),
}

/// O que uma tarefa de rede leva consigo.
#[derive(Clone)]
struct Ctx { voice: Voice, machines: Arc<Machines>, done: mpsc::UnboundedSender<Done>, backoff: watch::Backoff }

impl Ctx {
    /// Falha de ação: volta ao organizador e aparece no cartão (pelo laço).
    fn fail(&self, call: CallId, code: &'static str, text: String) {
        self.voice.reply(call, tool_reply(text.clone(), false));
        let _ = self.done.send(Done::Failed { code, text });
    }
}

#[derive(Default)]
struct Snapshot {
    phase: &'static str, mode: Mode, activity: Activity, draft: Option<String>, error: Option<(String, String)>,
    plan: Option<(PathBuf, String)>, effective: Option<Effective>, context: Option<(u64, Option<u64>)>,
    five_hour: RateWindow, seven_day: RateWindow, backstage: VecDeque<Backstage>, thought: String, action: Option<OrganizerAction>,
}

pub struct Controller {
    voice: Voice, machines: Arc<Machines>, jev: Option<jev::Config>, device: DeviceLink, gate: GateSource, diag: DiagClient,
    done: mpsc::UnboundedSender<Done>, done_rx: Option<mpsc::UnboundedReceiver<Done>>, backoff: watch::Backoff,
    client: String,
    /// Catálogo de ações de tela do nativo, para o Jev.
    actions: Vec<Value>,
    /// A última lista lida (vigia ou busca): o Jev e a pasta da sessão na tela leem daqui, sem esperar rede.
    rows: Vec<(String, SessionRow)>,
    followed: HashSet<Key>,
    /// Sessões que receberam pedido da voz: o fim do turno delas é falado.
    watched: HashSet<Key>,
    talked: HashMap<Key, Instant>,
    jev_switched: Option<(String, String, Instant)>,
    close_gate: ConfirmGate<Key>,
    closing: bool,
    switch_offer: SwitchOffer,
    sent_turn: Option<(String, HashSet<Key>)>,
    pending_question: Option<(Key, Instant)>,
    /// Ids de resposta já falados.
    spoken: HashSet<String>,
    heard_recent: String,
    session_names: Vec<String>,
    /// A sessão da tela já passada à chamada (`retarget`).
    target: Option<Screen>,
    /// Sessão `(máquina, nome)` do plano em curso: o `SendPlan` só traz o nome.
    plan_key: Option<Key>,
    computer: Option<tokio::task::JoinHandle<()>>,
    /// O dono caiu e nenhum outro assumiu: ferramentas de tela recusam até o próximo `hello`/oferta.
    detached: bool,
    /// Falha do evento anterior: a que vem logo antes do `Closed` foi a que encerrou a chamada.
    last_failure: Option<(&'static str, String)>,
    forwarded: HashMap<u64, Forwarded>,
    next_tool: u64,
    /// Sobe a cada aparelho que assume.
    owner_epoch: u64,
    next_offer: u64,
    /// `(seq, dono)` da última oferta repassada: só a resposta dela vai ao aparelho.
    latest_offer: Option<(u64, u64)>,
    snap: Snapshot,
    sent_state: Option<Value>,
    sent_at: Option<Instant>,
    dirty: bool,
}

impl Controller {
    pub fn new(voice: Voice, machines: Arc<Machines>, jev: Option<jev::Config>, device: DeviceLink, gate: GateSource, diag: DiagClient) -> Self {
        let (done, done_rx) = mpsc::unbounded_channel();
        let target = device.screen();
        Self { voice, machines, jev, device, gate, diag, done, done_rx: Some(done_rx), backoff: Arc::default(), client: String::new(),
            actions: Vec::new(), rows: Vec::new(), followed: HashSet::new(), watched: HashSet::new(), talked: HashMap::new(), jev_switched: None,
            close_gate: ConfirmGate::default(), closing: false, switch_offer: SwitchOffer::default(), sent_turn: None, pending_question: None,
            spoken: HashSet::new(), heard_recent: String::new(), session_names: Vec::new(), target, plan_key: None, computer: None, detached: false, last_failure: None, forwarded: HashMap::new(),
            next_tool: 0, owner_epoch: 0, next_offer: 0, latest_offer: None, snap: Snapshot { phase: "connecting", ..Snapshot::default() }, sent_state: None, sent_at: None, dirty: true }
    }

    fn ctx(&self) -> Ctx { Ctx { voice: self.voice.clone(), machines: self.machines.clone(), done: self.done.clone(), backoff: self.backoff.clone() } }

    pub async fn run(mut self, events: async_channel::Receiver<VoiceEvent>, mut from_device: mpsc::UnboundedReceiver<ToController>) {
        let Some(mut done_rx) = self.done_rx.take() else { return };
        let watcher = tokio::spawn(watch::watcher(self.machines.clone(), self.backoff.clone(), self.done.clone()));
        let mut ask_tick = tokio::time::interval(Duration::from_secs(1));
        let mut gate_tick = tokio::time::interval_at(tokio::time::Instant::now() + self.gate.every, self.gate.every);
        loop {
            let flush = self.dirty.then(|| self.sent_at.map_or_else(Instant::now, |at| at + STATE_EVERY));
            let go = tokio::select! {
                event = events.recv() => match event { Ok(event) => self.on_event(event), Err(_) => false },
                msg = from_device.recv() => match msg {
                    Some(msg) => self.on_device(msg),
                    None => { log("device channel closed"); self.voice.stop(); false }
                },
                Some(done) = done_rx.recv() => self.on_done(done),
                _ = ask_tick.tick() => { self.ask_expiry(); true }
                _ = gate_tick.tick() => { self.check_gate(); true }
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(flush.unwrap_or_else(Instant::now))), if flush.is_some() => {
                    self.flush_state();
                    true
                }
            };
            if !go { break; }
        }
        // Fechado já: o hub não entrega um aparelho novo a esta chamada que está acabando.
        drop(from_device);
        self.flush_state();
        watcher.abort();
        self.stop_computer();
        log("controller end");
    }

    fn on_device(&mut self, msg: ToController) -> bool {
        match msg {
            ToController::Detached => {
                self.voice.detached();
                self.detached = true;
                self.drop_forwarded("device detached");
            }
            ToController::OwnerChanged => {
                self.owner_epoch += 1;
                self.drop_forwarded("owner changed");
            }
            ToController::Models(models) => self.voice.set_models(models),
            ToController::Device(msg) => match msg {
                ClientMsg::Hello { client, screen, caps, actions } => {
                    log(format!("device hello client={client} caps={} actions={}", caps.len(), actions.len()));
                    self.detached = false;
                    self.client = client;
                    self.actions = actions;
                    self.device.set_caps(caps);
                    self.on_screen(screen);
                    // O aparelho novo começa com o painel vazio: recebe o retrato inteiro, mesmo sem mudança.
                    self.sent_state = None;
                    self.dirty = true;
                }
                ClientMsg::Offer { sdp } => {
                    self.detached = false;
                    let note = match self.device.screen() {
                        Some(screen) => format!("A sessão na tela agora é {}.", screen.name),
                        None => "Nenhuma sessão aberta na tela.".to_owned(),
                    };
                    self.next_offer += 1;
                    self.latest_offer = Some((self.next_offer, self.owner_epoch));
                    self.voice.offer(self.next_offer, sdp, note);
                }
                ClientMsg::Live => self.voice.live(),
                ClientMsg::Level { input } => self.voice.level(input),
                ClientMsg::Screen { screen } => self.on_screen(screen),
                ClientMsg::ToolResult { call, ok, text } => self.on_tool_result(call, ok, text),
                ClientMsg::Mode { mode } => self.voice.set_mode(if matches!(mode.as_str(), "plan" | "planejar") { Mode::Plan } else { Mode::Direct }),
                ClientMsg::Stop => { log("stop by device"); self.voice.stop(); return false; }
                ClientMsg::Ping => self.device.send(ServerMsg::Pong),
            },
        }
        true
    }

    fn on_event(&mut self, event: VoiceEvent) -> bool {
        self.dirty = true;
        let last_failure = self.last_failure.take();
        match event {
            VoiceEvent::Phase(Phase::Closed) => {
                // A chamada morreu por falha: o aparelho recebe o código, não só o retrato.
                if let Some((code, text)) = last_failure {
                    self.device.send(ServerMsg::Error { code: code.to_owned(), detail: Some(text) });
                }
                self.stop_computer();
                self.pending_question = None;
                let s = &mut self.snap;
                (s.phase, s.mode, s.draft, s.activity, s.action) = ("closed", Mode::Direct, None, Activity::Idle, None);
                s.thought.clear();
                self.flush_state();
                self.device.send(ServerMsg::Closed);
                return false;
            }
            VoiceEvent::Phase(Phase::Live) => { self.snap.error = None; self.snap.phase = "live"; }
            VoiceEvent::Phase(Phase::Connecting) => self.snap.phase = "connecting",
            VoiceEvent::Draft(draft) => self.snap.draft = draft,
            VoiceEvent::Activity(activity) => self.snap.activity = activity,
            VoiceEvent::Thought(delta) => push_thought(&mut self.snap.thought, &delta),
            VoiceEvent::Action(action) => self.snap.action = action,
            VoiceEvent::TurnDone => (self.snap.thought, self.snap.action) = (String::new(), None),
            VoiceEvent::Failed(failure) => self.call_failed(&failure),
            VoiceEvent::Mode(mode) => {
                if mode == Mode::Plan { self.mark_plan(); }
                self.snap.mode = mode;
            }
            VoiceEvent::Plan { path, markdown } => { self.mark_plan(); self.snap.plan = Some((path, markdown)); }
            VoiceEvent::OrganizerContext { used, window } => self.snap.context = Some((used, window)),
            // Atualização com uma janela só não apaga a outra.
            VoiceEvent::AccountLimits { five_hour, seven_day } => {
                self.snap.five_hour = five_hour.or(self.snap.five_hour);
                self.snap.seven_day = seven_day.or(self.snap.seven_day);
            }
            VoiceEvent::Organizer(effective) => self.snap.effective = Some(effective),
            VoiceEvent::Backstage(line) => {
                if self.snap.backstage.len() >= BACKSTAGE_KEEP { self.snap.backstage.pop_front(); }
                self.snap.backstage.push_back(line);
            }
            VoiceEvent::Answer(seq, sdp) => self.answer(seq, sdp),
            VoiceEvent::ReadSession(call) => self.read_session(call),
            VoiceEvent::Send { call, request, session, turn } => self.send(call, request, session, turn),
            VoiceEvent::AskSession(question) => self.ask(question),
            VoiceEvent::SendPlan { session, text } => self.send_plan(session, text),
            VoiceEvent::SwitchSession { call, name, spoken, recent } => self.lookup(Want::Switch { call, name, said: spoken, recent }),
            VoiceEvent::HangarActions(call) => self.forward(call, "hangar_actions", json!({})),
            VoiceEvent::HangarAction { call, id, arg } => self.forward(call, "hangar_action", json!({"id": id, "arg": arg})),
            VoiceEvent::ReadScreen(call, area) => self.forward(call, "read_screen", json!({"area": area})),
            VoiceEvent::ClickScreen { call, id, confirmed, turn } => self.forward(call, "click_screen", json!({"id": id, "confirmed": confirmed, "turn": turn})),
            VoiceEvent::Computer(call, objective) => self.computer(call, objective),
            VoiceEvent::Observe(call, request) => self.observe(call, request),
            VoiceEvent::Heard { turn, speech, recent } => {
                self.heard_recent = format!("{speech}\n{recent}");
                self.jev_ask(turn, speech, recent);
            }
            VoiceEvent::ListSessions(call) => self.lookup(Want::List(call)),
            VoiceEvent::OpenSession(call, request) => self.open(call, request),
            VoiceEvent::CloseSession { call, name, confirmed, turn } => self.lookup(Want::Close { call, name, confirmed, turn }),
            VoiceEvent::PairSessions(call, a, b) => self.lookup(Want::Pair(call, a, b)),
            VoiceEvent::UnpairSession(call, name) => self.lookup(Want::Unpair(call, name)),
            VoiceEvent::FollowSession(call, name) => self.lookup(Want::Follow(call, name, true)),
            VoiceEvent::UnfollowSession(call, name) => self.lookup(Want::Follow(call, name, false)),
        }
        true
    }

    fn on_done(&mut self, done: Done) -> bool {
        match done {
            Done::Rows { rows, unreachable, want } => self.on_rows(rows, unreachable, want),
            Done::Watch { rows, fired } => self.on_watch(rows, fired),
            Done::History { key, row, result } => self.on_history(key, row, result),
            Done::Sent { key, turn, error: Some(error) } => {
                // Recusa certa não entregou nada: a sessão volta a poder receber o reenvio do mesmo turno e deixa de ser
                // vigiada. Incerto fica marcado e vigiado, porque pode ter chegado.
                if error.certain {
                    if let Some((sent, keys)) = &mut self.sent_turn && *sent == turn { keys.remove(&key); }
                    self.watched.remove(&key);
                }
                self.action_failed("send_to_session", error.text);
            }
            Done::Sent { error: None, .. } => {}
            Done::PlanDelivered => self.plan_key = None,
            Done::Closed { call, key, result } => {
                self.closing = false;
                match result {
                    Ok(()) => {
                        log("close_session closed");
                        self.voice.reply(call, tool_reply(format!("Sessão {} fechada.", key.1), true));
                        self.followed.remove(&key);
                    }
                    Err(text) => self.fail(call, "close_session", text),
                }
            }
            Done::Opened { call, machine, name, sent } => self.opened(call, machine, name, sent),
            Done::AskFailed(key) => if self.pending_question.as_ref().is_some_and(|(k, _)| *k == key) {
                self.pending_question = None;
                log("ask_session delivery failed");
                self.voice.session_answer("A pergunta não chegou à sessão.".into());
            },
            Done::Jev(asked, result) => self.on_jev(asked, result),
            Done::Failed { code, text } => self.action_failed(code, text),
            Done::Gate(true) => {}
            Done::Gate(false) => {
                log("voice beta turned off: stopping the call");
                self.device.send(ServerMsg::Error { code: "disabled".into(), detail: None });
                self.voice.stop();
                return false;
            }
        }
        self.dirty = true;
        true
    }

    /// Só a resposta da última oferta do dono atual vai a ele; a de uma oferta anterior quebraria a conexão dele.
    fn answer(&mut self, seq: u64, sdp: String) {
        if self.latest_offer != Some((seq, self.owner_epoch)) {
            log("sdp answer dropped: stale offer");
            return;
        }
        self.latest_offer = None;
        self.device.send(ServerMsg::Answer { sdp });
    }

    /// Sessão nova na tela do aparelho: a chamada passa a falar dela.
    fn on_screen(&mut self, screen: Option<Screen>) {
        self.device.set_screen(screen.clone());
        if screen == self.target { return; }
        self.target = screen.clone();
        if self.pending_question.take().is_some() {
            log("ask_session timeout switched");
            self.voice.session_answer("A sessão não respondeu: a conversa trocou de sessão.".into());
        }
        let name = screen.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        // A pasta só vale neste disco: o organizador lê o código dela pelo caminho.
        let cwd = screen.as_ref().filter(|s| s.server == HERE)
            .and_then(|s| self.rows.iter().find(|(m, r)| m == HERE && r.name == s.name))
            .and_then(|(_, r)| r.cwd.clone()).map(PathBuf::from);
        self.voice.retarget(name.clone(), format!("A sessão na tela agora é {name}."), cwd);
    }

    /// O plano nasce para a sessão que a chamada tem como alvo (`target`) e fica com ela até ser entregue.
    fn mark_plan(&mut self) {
        if self.plan_key.is_none() { self.plan_key = self.target.as_ref().map(|s| (s.server.clone(), s.name.clone())); }
    }

    fn screen_key(&self) -> Option<Key> { self.device.screen().map(|s| (s.server, s.name)) }

    /// Sem dono vale "sim": as capacidades são do aparelho que caiu, e o `send_tool` recusa com o motivo certo.
    fn can(&self, tool: &str) -> bool { self.detached || self.device.can(tool) }

    /// Ferramenta de tela: pede ao aparelho dono; quem não a atende recusa já.
    fn forward(&mut self, call: CallId, tool: &'static str, args: Value) {
        if !self.can(tool) {
            log(format!("{tool} refused: device lacks it"));
            self.voice.reply(call, tool_reply(format!("Este aparelho não atende {tool}; peça no PC."), false));
            return;
        }
        self.send_tool(tool, args, Forwarded::Reply(call));
    }

    fn send_tool(&mut self, name: &str, args: Value, pending: Forwarded) {
        // Sem dono, a ferramenta iria às capacidades do aparelho que caiu e ninguém responderia.
        if self.detached {
            log(format!("{name} refused: no device"));
            self.abandon(pending, NO_DEVICE);
            return;
        }
        self.next_tool += 1;
        let call = self.next_tool;
        self.forwarded.insert(call, pending);
        log(format!("tool to device {name} call={call}"));
        self.device.send(ServerMsg::Tool { call, name: name.to_owned(), args });
    }

    fn on_tool_result(&mut self, call: u64, ok: bool, text: String) {
        let Some(pending) = self.forwarded.remove(&call) else { log(format!("tool result unknown call={call}")); return };
        log(format!("tool result call={call} ok={ok}"));
        match pending {
            // A recusa sem texto é do aparelho; o idioma da fala é decidido aqui, não no cliente.
            Forwarded::Reply(id) if !ok && text.trim().is_empty() => self.voice.reply(id, tool_reply(UNSUPPORTED_TOOL, false)),
            Forwarded::Reply(id) => self.voice.reply(id, tool_reply(text, ok)),
            Forwarded::Switch(id, key) => self.switched(id, key, ok, text),
            Forwarded::Opened { call, name, sent } => match super::rules::opened_reply(&name, ok, sent.as_ref()) {
                Ok(reply) => self.voice.reply(call, tool_reply(reply, true)),
                Err(reply) => self.fail(call, "open_session", reply),
            },
            Forwarded::JevSwitch { turn, verdict, key, machine } => self.jev_switched(turn, verdict, key, machine, ok),
            Forwarded::JevAction { turn, verdict, label } => self.jev_acted(turn, verdict, label, ok, text),
        }
    }

    /// O aparelho que recebeu as ferramentas não vai responder (caiu ou outro assumiu): cada uma volta ao organizador agora,
    /// senão o turno dele fica preso esperando.
    fn drop_forwarded(&mut self, why: &str) {
        log(format!("{why} pending_tools={}", self.forwarded.len()));
        let pending: Vec<Forwarded> = self.forwarded.drain().map(|(_, p)| p).collect();
        for p in pending { self.abandon(p, MOVED); }
    }

    /// Ferramenta que o aparelho não atendeu: a recusa vai ao organizador; o Jev só manda o veredito, sem fala.
    fn abandon(&mut self, pending: Forwarded, text: &str) {
        match pending {
            Forwarded::Reply(call) | Forwarded::Switch(call, _) => self.voice.reply(call, tool_reply(text, false)),
            // A sessão já existe: tentar de novo criaria outra.
            Forwarded::Opened { call, name, .. } => self.voice.reply(call, tool_reply(
                format!("A sessão {name} foi criada, mas não abriu na tela: {text} Não crie outra; peça para trocar para ela."), false)),
            Forwarded::JevSwitch { turn, verdict, .. } | Forwarded::JevAction { turn, verdict, .. } => self.voice.jev_verdict(turn, verdict, None, None),
        }
    }

    fn call_failed(&mut self, failure: &VoiceFailure) {
        let (code, text) = failure_text(failure);
        log(format!("call failure code={code}"));
        self.diag.report("rust.voice_failed", "voice", code, "a chamada de voz falhou");
        // Estas não encerram a chamada: ficam só no retrato.
        if !matches!(failure, VoiceFailure::Organizer | VoiceFailure::ModelSwitch | VoiceFailure::OwnFolder | VoiceFailure::AudioLost) {
            self.last_failure = Some((code, text.clone()));
        }
        self.snap.error = Some((code.to_owned(), text));
    }

    /// Falha de ação (servidor, conexão): volta ao organizador e aparece no cartão. Nome ambíguo não passa por aqui.
    fn fail(&mut self, call: CallId, code: &'static str, text: String) {
        self.voice.reply(call, tool_reply(text.clone(), false));
        self.action_failed(code, text);
    }

    fn action_failed(&mut self, code: &'static str, text: String) {
        log(format!("voice action failed code={code}"));
        self.diag.report("rust.voice_failed", "voice", code, "uma ação pedida por voz falhou");
        self.snap.error = Some((code.to_owned(), format!("A ação pedida por voz falhou: {text}")));
        self.dirty = true;
    }

    fn check_gate(&self) {
        let (home, claude_dir, done) = (self.gate.home.clone(), self.gate.claude_dir.clone(), self.done.clone());
        tokio::spawn(async move {
            // Leitura que não terminou não derruba a chamada.
            let enabled = tokio::task::spawn_blocking(move || super::settings::read_gate(&home, &claude_dir, None).enabled).await.unwrap_or(true);
            let _ = done.send(Done::Gate(enabled));
        });
    }

    fn stop_computer(&mut self) {
        if let Some(task) = self.computer.take() && !task.is_finished() { log("computer aborted"); task.abort(); }
    }

    fn state_json(&self) -> Value {
        let s = &self.snap;
        let window = |w: RateWindow| w.map(|(used, resets_at)| json!({"used_percent": used, "resets_at": resets_at}));
        let mut followed: Vec<&Key> = self.followed.iter().collect();
        followed.sort();
        json!({
            "phase": s.phase,
            "mode": match s.mode { Mode::Direct => "direct", Mode::Plan => "plan" },
            "activity": match s.activity { Activity::Idle => "idle", Activity::Thinking => "thinking", Activity::Searching => "searching", Activity::Working => "working" },
            "draft": s.draft,
            "error": s.error.as_ref().map(|(code, text)| json!({"code": code, "text": text})),
            "plan": s.plan.as_ref().map(|(path, markdown)| json!({"path": path.display().to_string(), "markdown": markdown})),
            "effective": s.effective.as_ref().map(|e| json!({"model": e.model, "effort": e.effort, "tier": e.tier})),
            "context": s.context.map(|(used, window)| json!({"used": used, "window": window})),
            "limits": {"five_hour": window(s.five_hour), "seven_day": window(s.seven_day)},
            "backstage": s.backstage.iter().map(|line| match line {
                Backstage::Heard(text) => json!({"kind": "heard", "text": text}),
                Backstage::Result(text) => json!({"kind": "result", "text": text}),
                Backstage::Answer(text) => json!({"kind": "answer", "text": text}),
            }).collect::<Vec<_>>(),
            "thought": s.thought,
            "action": s.action.as_ref().map(|a| match a {
                OrganizerAction::Tool(text) => json!({"kind": "tool", "text": text}),
                OrganizerAction::Search(text) => json!({"kind": "search", "text": text}),
                OrganizerAction::Command(text) => json!({"kind": "command", "text": text}),
            }),
            "followed": followed.iter().map(|(server, name)| json!({"server": server, "name": name})).collect::<Vec<_>>(),
            "server": self.machines.own_label,
            "client": self.client,
        })
    }

    /// Só retrato que mudou vai ao aparelho.
    fn flush_state(&mut self) {
        self.dirty = false;
        let state = self.state_json();
        if self.sent_state.as_ref() == Some(&state) { return; }
        self.sent_state = Some(state.clone());
        self.sent_at = Some(Instant::now());
        self.device.send(ServerMsg::State { state });
    }
}

fn push_thought(thought: &mut String, delta: &str) {
    thought.push_str(delta);
    let excess = thought.len().saturating_sub(THOUGHT_KEEP);
    if excess > 0 {
        let cut = (excess..thought.len()).find(|i| thought.is_char_boundary(*i)).unwrap_or(thought.len());
        thought.drain(..cut);
    }
}

/// Código para o diário e o texto do cartão; o detalhe do `Realtime` vem do servidor e só vai à tela.
fn failure_text(failure: &VoiceFailure) -> (&'static str, String) {
    match failure {
        VoiceFailure::Microphone => ("microphone", "Sem acesso ao microfone".into()),
        VoiceFailure::Speaker => ("speaker", "Não foi possível tocar o áudio no alto-falante.".into()),
        VoiceFailure::AppServer => ("app_server", "O Codex local parou de responder.".into()),
        VoiceFailure::Realtime(detail) => ("realtime", format!("Não foi possível manter a chamada. Confira a conexão e tente conectar novamente. {detail}").trim_end().to_owned()),
        VoiceFailure::Network => ("network", "Sem conexão de áudio com a OpenAI; a rede ou o firewall pode estar bloqueando UDP.".into()),
        VoiceFailure::Timeout => ("timeout", "A chamada parou de responder e foi encerrada. Tente conectar novamente.".into()),
        VoiceFailure::Organizer => ("organizer", "O organizador da voz falhou nesta fala; a conversa continua.".into()),
        VoiceFailure::ModelSwitch => ("model_switch", "O modelo do organizador não trocou; ele segue com o anterior até a próxima troca de modo.".into()),
        VoiceFailure::OwnFolder => ("own_folder", "Não consegui criar a pasta do organizador (~/.hangar/voz/arquivos); ele conversa, mas não consegue gravar arquivos.".into()),
        VoiceFailure::AudioStopped => ("audio_stopped", "O áudio da chamada parou: o microfone pode ter sido trocado ou desconectado. Conecte de novo.".into()),
        VoiceFailure::Closed => ("closed", "A OpenAI encerrou a chamada.".into()),
        VoiceFailure::AudioLost => ("audio_lost", "O áudio da chamada caiu; conecte de novo para continuar a mesma conversa.".into()),
        VoiceFailure::AudioLostEnded => ("audio_lost", "O áudio da chamada caiu e nenhum aparelho reconectou a tempo; a chamada foi encerrada.".into()),
    }
}
