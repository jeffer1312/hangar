//! A chamada de voz: o organizador e a conversa falada rodam aqui; o WebRTC é do aparelho, que manda a oferta e o nível do
//! microfone e recebe a resposta SDP.
use super::{SendVerdict, log, observe, organizer, usage};
use super::organizer::{Effective, FinishStep, MIC_VOICE_LEVEL, Mode, ModeModel, ModeModels, Planner, REPEAT_NOTE, REPEAT_WINDOW, Results, SEND_UNCONFIRMED, SpeechHold,
    Consent, SendConsent, SendGate, SpokenTurns, ToolCall, finish_request, parse_tool, repeated_handoff, send_allowed, settings_update, spoken_input, tool_reply,
    organizer_start, user_speech, ORGANIZER_PROMPT, VOICE_PROMPT};
use super::rpc::{Incoming, Rpc, RpcError, handshake};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::sync::{Notify, mpsc};

pub struct CallId(pub Value);
pub enum Phase { Connecting, Live, Closed }
#[derive(Debug, Clone)]
pub enum VoiceFailure { Microphone, Speaker, AppServer, Realtime(String), Network, Timeout, Organizer, ModelSwitch, OwnFolder, AudioStopped, Closed }
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Activity { #[default] Idle, Thinking, Searching, Working }
pub enum VoiceEvent {
    Phase(Phase), Draft(Option<String>), Activity(Activity), ReadSession(CallId), Failed(VoiceFailure),
    /// Resposta SDP para o aparelho dono da chamada (a primeira e a de cada passagem).
    Answer(String),
    /// `turn`: a fala que pediu. A tela não manda duas vezes à mesma sessão no mesmo turno.
    Send { call: CallId, request: String, session: Option<String>, turn: String },
    Mode(Mode), Plan { path: PathBuf, markdown: String }, AskSession(String), SendPlan { session: String, text: String },
    /// Nome pedido e a fala do turno: a tela só troca quando a fala pede essa sessão.
    SwitchSession { call: CallId, name: String, spoken: String, recent: String },
    /// Ações da tela do Hangar (catálogo e execução) e o `computer` para os outros programas.
    HangarActions(CallId), HangarAction { call: CallId, id: String, arg: Option<String> }, Computer(CallId, String),
    /// Leitura da tela do Hangar pela árvore de acessibilidade; `None` = a tela escolhe a área.
    ReadScreen(CallId, Option<String>),
    /// Clique pela árvore de acessibilidade; `turn` separa o pedido do sim nos botões arriscados.
    ClickScreen { call: CallId, id: String, confirmed: bool, turn: String },
    /// Processos e uso da máquina, lidos fora do isolamento do shell do organizador.
    Observe(CallId, observe::Request),
    /// Ferramentas de sessão: a tela resolve os nomes falados e responde por `Voice::reply`. `turn` separa o pedido do sim.
    ListSessions(CallId), OpenSession(CallId, organizer::OpenRequest),
    CloseSession { call: CallId, name: String, confirmed: bool, turn: String },
    PairSessions(CallId, String, String), UnpairSession(CallId, String),
    /// Acompanhar: as respostas da sessão são faladas mesmo fora da tela.
    FollowSession(CallId, String), UnfollowSession(CallId, String),
    /// Modelo, esforço e velocidade que a thread do organizador usa de fato.
    Organizer(Effective),
    /// Uma linha dos bastidores: o que chegou ao organizador e o que ele respondeu (só tela, nunca diário).
    Backstage(Backstage),
    /// Fala nova do usuário no turno `turn`: a tela pergunta ao Jev o que ela pede, em paralelo ao organizador. `recent`: a
    /// transcrição que veio com ela (falas da voz e do usuário desde a anterior).
    Heard { turn: String, speech: String, recent: String },
    /// Contexto da thread do organizador (não o da voz, que não é informado): input do último turno e a janela do modelo.
    OrganizerContext { used: u64, window: Option<u64> },
    AccountLimits { five_hour: usage::RateWindow, seven_day: usage::RateWindow },
    /// Só para a tela: pedaço do resumo do raciocínio, a ação em curso e o fim do turno, que limpa os dois.
    Thought(String), Action(Option<organizer::OrganizerAction>), TurnDone,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Backstage { Heard(String), Result(String), Answer(String) }

/// O que entra no organizador vira linha dos bastidores: a fala repassada ou o resultado de uma sessão.
fn backstage_input(text: &str) -> Backstage {
    let head = text.trim_start();
    if let Some(rest) = head.strip_prefix("[RESULTADO DA SESSÃO ") {
        return Backstage::Result(rest.split_once(']').map_or(rest, |(name, _)| name).to_owned());
    }
    if head.starts_with(organizer::ANSWER_PREFIX) { return Backstage::Result(String::new()); }
    Backstage::Heard(spoken_input(text).to_owned())
}

/// `cwd`: pasta da sessão na tela quando é desta máquina (o organizador lê o código dela); `target`: nome dessa sessão.
/// `organizer`: modelo e esforço do organizador por modo; a chamada nasce no Direto. `tools`: o catálogo do dono, fixo desde o
/// `thread/start` (a thread não aceita catálogo novo). `handoff_same_thread`: a passagem recomeça a conversa falada na mesma thread.
/// `voice_dir`: raiz da voz (`~/.hangar/voz` em produção), com a pasta própria do organizador e os planos.
pub struct CallOptions { pub voice: Option<String>, pub context: String, pub cwd: Option<PathBuf>, pub target: String, pub organizer: ModeModels,
    pub tools: Value, pub handoff_same_thread: bool, pub voice_dir: PathBuf }

/// Sobe o app-server: o filho de verdade em produção, um falso nos testes.
pub type Spawn = Box<dyn FnOnce() -> futures_util::future::BoxFuture<'static, Result<(Rpc, async_channel::Receiver<Incoming>), RpcError>> + Send>;

enum Command { Retarget(String, String, Option<PathBuf>), Result(String, String), Reply(Value, Value), SetMode(Mode), Models(ModeModels), Answer(String), PlanDelivered,
    Sessions(Vec<String>), Jev { turn: String, verdict: SendVerdict, speech: Option<String>, note: Option<String> },
    /// SDP e a nota da sessão na tela do aparelho que ofereceu.
    Offer(String, String), Live, Level(f32), Detached }

#[derive(Clone)]
pub struct Voice { commands: mpsc::UnboundedSender<Command>, stopped: Arc<AtomicBool>, stop: Arc<Notify> }

impl Voice {
    pub fn start(options: CallOptions, spawn: Spawn, events: async_channel::Sender<VoiceEvent>) -> Voice {
        let (commands, inbox) = mpsc::unbounded_channel();
        let (stopped, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(Notify::new()));
        tokio::spawn(call(options, spawn, events, inbox, stopped.clone(), stop.clone()));
        Voice { commands, stopped, stop }
    }
    /// Oferta WebRTC de um aparelho: a primeira abre a conversa falada; as seguintes são a passagem para ele.
    pub fn offer(&self, sdp: String, context: String) { let _ = self.commands.send(Command::Offer(sdp, context)); }
    /// O aparelho conectou o áudio.
    pub fn live(&self) { let _ = self.commands.send(Command::Live); }
    /// Nível do microfone do aparelho: segura o envio e a fala puxada enquanto o usuário fala.
    pub fn level(&self, input: f32) { let _ = self.commands.send(Command::Level(input)); }
    /// O aparelho dono caiu: a conversa falada pode fechar sem encerrar a chamada.
    pub fn detached(&self) { let _ = self.commands.send(Command::Detached); }
    /// `cwd`: pasta da nova sessão neste disco (`None` se for de outra máquina).
    pub fn retarget(&self, name: String, context: String, cwd: Option<PathBuf>) { let _ = self.commands.send(Command::Retarget(name, context, cwd)); }
    pub fn session_result(&self, session: String, text: String) { let _ = self.commands.send(Command::Result(session, text)); }
    pub fn reply(&self, call: CallId, reply: Value) { let _ = self.commands.send(Command::Reply(call.0, reply)); }
    pub fn set_mode(&self, mode: Mode) { let _ = self.commands.send(Command::SetMode(mode)); }
    /// Pares editados no cartão durante a chamada: o do modo atual vale já no próximo turno.
    pub fn set_models(&self, models: ModeModels) { let _ = self.commands.send(Command::Models(models)); }
    /// Resposta, recusa ou estouro de um `ask_session`: chega ao organizador como turno marcado.
    pub fn session_answer(&self, text: String) { let _ = self.commands.send(Command::Answer(text)); }
    /// A sessão aceitou o plano enviado: o organizador o esquece.
    pub fn plan_delivered(&self) { let _ = self.commands.send(Command::PlanDelivered); }
    /// Nomes das sessões que a busca enxerga: o `set_mode` recusa quando a fala cita uma delas.
    pub fn set_sessions(&self, names: Vec<String>) { let _ = self.commands.send(Command::Sessions(names)); }
    /// Veredito do Jev sobre a fala do turno. Quando a tela já agiu (trocou, abriu): `speech` a voz fala na hora e `note`
    /// diz ao organizador que não repita nem fale.
    pub fn jev_verdict(&self, turn: String, verdict: SendVerdict, speech: Option<String>, note: Option<String>) {
        let _ = self.commands.send(Command::Jev { turn, verdict, speech, note });
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        // notify_one guarda a licença mesmo sem ninguém esperando ainda.
        self.stop.notify_one();
    }
}

/// Só o tipo: a mensagem de `Realtime` vem do servidor e pode repetir o texto enviado.
fn failure_kind(failure: &VoiceFailure) -> String {
    match failure { VoiceFailure::Realtime(_) => "Realtime".to_owned(), other => format!("{other:?}") }
}

fn failed(step: &'static str) -> impl FnOnce(VoiceFailure) -> VoiceFailure {
    move |failure| { log(format!("{step} failed: {}", failure_kind(&failure))); failure }
}

/// Deltas vêm aos montes: só o primeiro de cada (método, papel) até virar o turno ou mudar o falante.
fn first_delta(last: &mut Option<(String, String)>, method: &str, role: &str) -> bool {
    if method == "turn/started" || method == "turn/completed" { *last = None; return true; }
    if !method.ends_with("/delta") && !method.ends_with("Delta") { return true; }
    let key = (method.to_owned(), role.to_owned());
    if last.as_ref() == Some(&key) { return false; }
    *last = Some(key);
    true
}

fn log_notification(method: &str, params: &Value, ours: bool, last_delta: &mut Option<(String, String)>) {
    let role = params["role"].as_str().unwrap_or_default();
    if !first_delta(last_delta, method, role) { return; }
    let mut line = format!("notification {method}");
    if !ours { line.push_str(" thread=other"); }
    if method.starts_with("thread/realtime/transcript") { line.push_str(&format!(" role={role}")); }
    if method.starts_with("item/") { line.push_str(&format!(" item={}", params["item"]["type"].as_str().unwrap_or("?"))); }
    if method == "turn/completed" { line.push_str(&format!(" status={}", params["turn"]["status"].as_str().unwrap_or("?"))); }
    log(line);
}

async fn call(options: CallOptions, spawn: Spawn, events: async_channel::Sender<VoiceEvent>, mut inbox: mpsc::UnboundedReceiver<Command>,
    stopped: Arc<AtomicBool>, stop: Arc<Notify>) {
    log(format!("call start voice={}", options.voice.as_deref().unwrap_or("default")));
    log("phase Connecting");
    let _ = events.send(VoiceEvent::Phase(Phase::Connecting)).await;
    // Parar vale em qualquer fase: largar o future derruba o app-server (kill_on_drop).
    let outcome = tokio::select! {
        outcome = run_call(options, spawn, &events, &mut inbox, &stopped) => outcome,
        _ = stop.notified() => { log("stop requested"); Ok(()) },
    };
    stopped.store(true, Ordering::Relaxed);
    match &outcome { Ok(()) => log("call end ok"), Err(failure) => log(format!("call end failure={}", failure_kind(failure))) }
    if let Err(failure) = outcome { let _ = events.send(VoiceEvent::Failed(failure)).await; }
    log("phase Closed");
    let _ = events.send(VoiceEvent::Phase(Phase::Closed)).await;
}

fn rpc_failure(error: RpcError) -> VoiceFailure {
    match error { RpcError::Timeout => VoiceFailure::Timeout, RpcError::Server(m) => VoiceFailure::Realtime(m), _ => VoiceFailure::AppServer }
}

/// Só o tipo: a mensagem do servidor pode repetir o texto enviado.
fn rpc_error_kind(error: &RpcError) -> &'static str {
    match error { RpcError::Spawn => "spawn", RpcError::Closed => "closed", RpcError::Timeout => "timeout", RpcError::Server(_) => "server" }
}

async fn run_call(options: CallOptions, spawn: Spawn, events: &async_channel::Sender<VoiceEvent>, inbox: &mut mpsc::UnboundedReceiver<Command>,
    stopped: &Arc<AtomicBool>) -> Result<(), VoiceFailure> {
    let (rpc, incoming) = spawn().await.map_err(rpc_failure).map_err(failed("app-server spawn"))?;
    log("app-server spawned");
    let config = handshake(&rpc).await.map_err(rpc_failure).map_err(failed("handshake"))?;
    log("handshake ok");
    // Pasta própria e fixa: o que o organizador grava fica entre chamadas, nada aqui a apaga.
    let own = options.voice_dir.join("arquivos");
    if let Err(error) = std::fs::create_dir_all(&own) {
        log(format!("own folder create failed kind={:?}", error.kind()));
        let _ = events.send(VoiceEvent::Failed(VoiceFailure::OwnFolder)).await;
    }
    let mut models = options.organizer.clone();
    let mut applied = models.direct.clone();
    let start = organizer_start(&config, &own, options.cwd.as_deref(), &options.context, &applied, options.tools.clone());
    log(format!("organizer model={} effort={} tier={}", if applied.model.is_some() { "chosen" } else { "config" }, applied.effort,
        applied.tier.as_deref().unwrap_or("config")));
    // A conta lida em paralelo, com prazo curto: falhar só deixa os limites ocultos até a primeira atualização.
    let limits = async {
        match tokio::time::timeout(Duration::from_secs(3), rpc.request("account/rateLimits/read", json!({}))).await {
            Ok(Ok(result)) => send_limits(events, usage::read_limits(&result)).await,
            Ok(Err(error)) => log(format!("rateLimits/read failed kind={}", rpc_error_kind(&error))),
            Err(_) => log("rateLimits/read timed out"),
        }
    };
    let (started, ()) = tokio::join!(rpc.request("thread/start", start), limits);
    let started = started.map_err(rpc_failure).map_err(failed("thread/start"))?;
    let thread = started["thread"]["id"].as_str().unwrap_or_default().to_owned();
    // Voltar ao modelo do config pede o nome dele: o do config, senão o que a thread abriu sem escolha.
    // A velocidade da conta só se sabe pela thread aberta sem escolha.
    let defaults = Defaults {
        model: config["model"].as_str().or_else(|| started["model"].as_str().filter(|_| applied.model.is_none())).map(str::to_owned),
        tier: started["serviceTier"].as_str().filter(|_| applied.tier.is_none()).or_else(|| config["service_tier"].as_str()).map(str::to_owned),
    };
    let mut effective = Effective::from_start(&started);
    log(format!("thread started id={thread} tier={}", effective.tier.as_deref().unwrap_or("none")));
    let _ = events.send(VoiceEvent::Organizer(effective.clone())).await;

    // Comando que chega antes da oferta não se perde: fica na fila e o laço o trata primeiro, na ordem. O estado do aparelho
    // (queda, áudio, nível) é de quem ainda não ofereceu: aplicado depois, marcaria como caído o aparelho que assumiu.
    let mut early: VecDeque<Command> = VecDeque::new();
    let offer_sdp = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            match inbox.recv().await {
                Some(Command::Offer(sdp, _)) => return Ok(sdp),
                Some(Command::Detached | Command::Live | Command::Level(_)) => {}
                Some(other) => early.push_back(other),
                None => return Err(VoiceFailure::Closed),
            }
        }
    }).await.map_err(|_| VoiceFailure::Timeout).and_then(|r| r).map_err(failed("device offer"))?;
    let mut realtime = json!({"threadId": thread, "version": "v3", "outputModality": "audio", "prompt": VOICE_PROMPT,
        // O aviso curto de que ouviu sai da própria voz; o resumo do raciocínio deixa a voz saber o que o organizador faz.
        "includeStartupContext": false, "delegationAckFiller": true, "backendReasoningStatus": true, "clientManagedHandoffs": false,
        "realtimeStartInstructions": ORGANIZER_PROMPT, "initialItems": [{"role": "developer", "text": options.context}]});
    if let Some(voice) = &options.voice { realtime["voice"] = json!(voice); }
    let realtime_start = realtime.clone();
    realtime["transport"] = json!({"type": "webrtc", "sdp": &offer_sdp});
    rpc.request("thread/realtime/start", realtime).await.map_err(rpc_failure).map_err(failed("thread/realtime/start"))?;
    log(format!("realtime/start sent offer_bytes={}", offer_sdp.len()));

    // A resposta SDP chega como notificação; até lá, só ela interessa.
    let mut last_delta = None;
    let answer = tokio::time::timeout(Duration::from_secs(20), async {
        while let Ok(item) = incoming.recv().await {
            match item {
                Incoming::Notification { method, params } if method == "thread/realtime/sdp" => return Ok(params["sdp"].as_str().unwrap_or_default().to_owned()),
                Incoming::Notification { method, params } if method == "thread/realtime/error" =>
                    return Err(VoiceFailure::Realtime(params["message"].as_str().unwrap_or_default().to_owned())),
                Incoming::Notification { method, params } => {
                    let ours = params["threadId"].as_str() == Some(thread.as_str());
                    log_notification(&method, &params, ours, &mut last_delta);
                }
                Incoming::Request { method, .. } => log(format!("request before sdp {method} (unanswered)")),
                Incoming::Exited => return Err(VoiceFailure::AppServer),
            }
        }
        Err(VoiceFailure::AppServer)
    }).await.map_err(|_| VoiceFailure::Timeout).and_then(|answer| answer).map_err(failed("sdp answer"))?;
    log(format!("sdp answer received bytes={}", answer.len()));
    let _ = events.send(VoiceEvent::Answer(answer)).await;

    let mut greeted = false;
    // `restarting`: passagem para outro aparelho em curso; `detached`: o dono caiu. Nos dois, o fim da conversa falada não
    // encerra a chamada: a thread do organizador espera a próxima oferta.
    let mut restarting = false;
    let mut detached = false;
    let mut results = Results::default();
    // Cada pedido leva o destino falado (vazio = a sessão da tela) e o turno da fala que o pediu.
    let mut gate: SendGate<(Value, Option<String>, String)> = SendGate::default();
    let mut organizer_busy = false;
    let mut spoken = SpokenTurns::default();
    let mut activity = Activity::Idle;
    let mut planner = Planner::new(options.voice_dir.join("planos"));
    let mut target = options.target.clone();
    let mut target_cwd = options.cwd.clone();
    let mut pending_context: Option<String> = None;
    let mut context_failures = 0u32;
    let mut session_names: Vec<String> = Vec::new();
    let mut consent = Consent::send();
    // Edição no projeto: a mesma trava, com os verbos de editar. `edit_pending` é a edição liberada que espera o turno
    // seguinte (o acesso vale do próximo turno em diante); `edit_turn`, o turno aberto para fazê-la.
    let mut edit_consent = Consent::edit();
    let mut edit_pending: Option<(String, PathBuf)> = None;
    let mut edit_turn: Option<String> = None;
    // O acesso completo não fechou: nenhuma edição nova é liberada enquanto ele estiver aberto sem dono.
    let mut edit_stuck = false;
    // Última fala repassada (para reconhecer a repetida) e os itens de fala já tratados.
    let mut last_input: Option<(String, Instant)> = None;
    let mut seen_items: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Speech::default();
    // Envio recusado por falta de pedido: se a fala seguinte o completar, o organizador é avisado para tentar de novo.
    let mut refused_send: Option<Instant> = None;
    let outcome = loop {
        for text in out.hold.due(Instant::now()) {
            if out.away { out.park(text, "held"); } else { append_speech(&rpc, &thread, &text, "held").await; }
        }
        // No Planejar nada sai pelo gate; ao entrar nele o envio pendente já foi cancelado.
        if planner.mode == Mode::Direct && let Some(((id, session, turn), request)) = gate.due(Instant::now()) {
            consent.used();
            log(format!("gate sent words={} named={}", request.split_whitespace().count(), session.is_some()));
            let _ = events.send(VoiceEvent::Draft(None)).await;
            let _ = events.send(VoiceEvent::Send { call: CallId(id), request, session, turn }).await;
        }
        tokio::select! {
            // Comandos antes das notificações: a passagem ou a queda do aparelho já valem quando chega o fim da conversa anterior.
            biased;
            command = async { match early.pop_front() { Some(command) => Some(command), None => inbox.recv().await } } => match command {
                Some(Command::Live) => {
                    log("phase Live");
                    let _ = events.send(VoiceEvent::Phase(Phase::Live)).await;
                    restarting = false;
                    out.away = detached;
                    // A voz V3 nunca fala primeiro: sem isto a pessoa não tem prova de que o alto-falante funciona.
                    if !greeted {
                        greeted = true;
                        let spoke = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": "Conectado. Pode falar."})).await;
                        log(format!("greeting appendSpeech ok={}", spoke.is_ok()));
                    }
                    // O que a voz ia falar durante a troca sai agora, na ordem; ela nunca fala primeiro, então se perderia.
                    if !out.away {
                        for text in std::mem::take(&mut out.parked) { speak(&rpc, &thread, &mut out, text, "parked").await; }
                    }
                }
                Some(Command::Level(input)) => if input >= MIC_VOICE_LEVEL { gate.heard_voice(Instant::now()); out.hold.heard_voice(Instant::now()); },
                Some(Command::Detached) => { log("device detached"); detached = true; out.away = true; }
                Some(Command::Offer(sdp, context)) => {
                    // Outro aparelho assumiu (ou o mesmo voltou): a conversa falada recomeça na mesma thread, com o histórico dela.
                    log(format!("handoff offer bytes={}", sdp.len()));
                    restarting = true;
                    detached = false;
                    out.away = true;
                    let mut again = realtime_start.clone();
                    again["transport"] = json!({"type": "webrtc", "sdp": sdp});
                    again["includeStartupContext"] = json!(true);
                    again["initialItems"] = json!([{"role": "developer", "text": context}]);
                    if let Err(error) = rpc.request("thread/realtime/start", again).await {
                        log(format!("handoff realtime/start failed kind={}", rpc_error_kind(&error)));
                        break Err(VoiceFailure::Realtime(String::new()));
                    }
                }
                Some(Command::Reply(id, reply)) => { log(format!("tool reply success={}", reply["success"])); let _ = rpc.respond(id, reply).await; }
                Some(Command::SetMode(Mode::Plan)) if target.is_empty() => {
                    log("mode plan refused: no target");
                    let _ = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": "Abra uma sessão primeiro."})).await;
                }
                Some(Command::SetMode(mode)) => {
                    log(format!("mode set {mode:?}"));
                    let mut note = switch_mode(&mut planner, mode, &target, &mut gate, &rpc, events).await;
                    if let Some(warn) = apply_models(&rpc, &thread, &mut applied, &models, mode, &defaults, &mut effective, events).await { note = format!("{note} {warn}"); }
                    // Para o organizador, na próxima fala; a voz só anuncia o modo.
                    pending_context = Some(match pending_context.take() { Some(old) => format!("{old}\n{note}"), None => note });
                    let speech = if mode == Mode::Plan { "Modo planejar." } else { "Modo direto." };
                    let _ = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": speech})).await;
                }
                Some(Command::Models(edited)) => {
                    models = edited;
                    // A falha já aparece no cartão; o modo não mudou, não há o que dizer ao organizador.
                    let _ = apply_models(&rpc, &thread, &mut applied, &models, planner.mode, &defaults, &mut effective, events).await;
                }
                Some(Command::PlanDelivered) => planner.delivered(),
                Some(Command::Jev { turn, verdict, speech, note }) => {
                    consent.jev(&turn, verdict, Instant::now());
                    // O Jev pode responder depois que o pedido entrou na espera: o bloqueio vale para ele também.
                    if verdict == SendVerdict::Block {
                        for ((old, ..), _) in gate.take_where(|_, (_, _, t)| *t == turn) {
                            log("gate cancelled: jev blocked the turn");
                            let _ = rpc.respond(old, tool_reply("Cancelado: o pedido foi entendido como conversa; nada foi enviado.", false)).await;
                        }
                    }
                    log(format!("jev verdict={verdict:?} note={} speech={}", note.is_some(), speech.is_some()));
                    if let Some(text) = speech { speak(&rpc, &thread, &mut out, text, "jev").await; }
                    // A tela já agiu: o organizador sabe no mesmo turno e não repete a ação.
                    if let Some(note) = note {
                        let steer = json!({"threadId": thread, "expectedTurnId": turn,
                            "input": [{"type": "text", "text": format!("{} {note}", organizer::NOTE_PREFIX)}]});
                        let steered = rpc.request("turn/steer", steer).await;
                        log(format!("jev note steered ok={}", steered.is_ok()));
                    }
                }
                Some(Command::Sessions(names)) => { log(format!("sessions known count={}", names.len())); session_names = names; }
                Some(Command::Answer(text)) => {
                    log(format!("session answer bytes={}", text.len()));
                    if let Some(input) = results.push(String::new(), text) { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                }
                Some(Command::Retarget(name, context, cwd)) => {
                    log("retarget");
                    // O envio retido sem destino sairia para a sessão que está na tela agora; o nomeado segue para a dele.
                    for ((old, ..), _) in gate.take_for("") {
                        let _ = rpc.respond(old, tool_reply("Cancelado: a sessão mudou; nada foi enviado.", false)).await;
                    }
                    target = name.clone();
                    target_cwd = cwd;
                    // Sem anúncio falado: a pessoa vê a tela. O contexto da sessão só entra quando ela voltar a falar; a troca
                    // de sessão substitui a nota da anterior, mas não a do modo.
                    let note = format!("{}\n{context}", organizer::code_note(target_cwd.as_deref(), &own));
                    pending_context = Some(match pending_context.take().filter(|old| old.starts_with("Modo ")) { Some(old) => format!("{old}\n{note}"), None => note });
                    context_failures = 0;
                }
                Some(Command::Result(session, text)) => {
                    log(format!("session result bytes={}", text.len()));
                    if let Some(input) = results.push(session, text) { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                }
                None => break Ok(()),
            },
            item = incoming.recv() => match item {
                Ok(Incoming::Request { id, method, params }) if method == "item/tool/call" => {
                    let tool = params["tool"].as_str().unwrap_or("?").to_owned();
                    let _ = events.send(VoiceEvent::Action(Some(organizer::OrganizerAction::Tool(tool.clone())))).await;
                    let outcome = match parse_tool(&params) {
                        ToolCall::ReadSession => { let _ = events.send(VoiceEvent::ReadSession(CallId(id))).await; "read" }
                        ToolCall::Send { .. } | ToolCall::Hold(_) if !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Pedido recusado: só uma fala do usuário pode gerar envio.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::FinishPlan { .. } | ToolCall::AskSession(_) | ToolCall::SetMode(_) | ToolCall::SwitchSession(_)
                            | ToolCall::OpenSession(_) | ToolCall::CloseSession { .. } | ToolCall::PairSessions(..) | ToolCall::UnpairSession(_)
                            | ToolCall::HangarAction { .. } | ToolCall::Computer(_) | ToolCall::ClickScreen { .. } | ToolCall::EditFiles(_)
                            if !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Só a pedido falado do usuário.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::Send { .. } | ToolCall::Hold(_) if !send_allowed(planner.mode) => {
                            let _ = rpc.respond(id, tool_reply("Modo Planejar: nada vai à sessão até finish_plan.", false)).await;
                            "refused-plan-mode"
                        }
                        ToolCall::UpdatePlan(_) if planner.mode != Mode::Plan => {
                            let _ = rpc.respond(id, tool_reply("Modo Direto: o plano já foi enviado ou não está aberto.", false)).await;
                            "refused-direct-mode"
                        }
                        ToolCall::UpdatePlan(_) | ToolCall::SetMode(Mode::Plan) if target.is_empty() => {
                            let _ = rpc.respond(id, tool_reply("Abra uma sessão primeiro.", false)).await;
                            "refused-no-target"
                        }
                        ToolCall::UpdatePlan(markdown) => {
                            let bytes = markdown.len();
                            let plan = planner.plan(&target);
                            let path = plan.path.clone();
                            match plan.write(&markdown) {
                                Ok(()) => {
                                    log(format!("plan saved bytes={bytes}"));
                                    let _ = rpc.respond(id, tool_reply(format!("Plano salvo em {}.", path.display()), true)).await;
                                    planner.plan_changed();
                                    let _ = events.send(VoiceEvent::Plan { path, markdown }).await;
                                    "plan-saved"
                                }
                                Err(error) => {
                                    log(format!("plan write failed kind={:?}", error.kind()));
                                    let _ = rpc.respond(id, tool_reply(format!("Não consegui salvar o plano: {error}"), false)).await;
                                    "plan-failed"
                                }
                            }
                        }
                        ToolCall::ReadPlan => {
                            match planner.read() {
                                Ok(text) => {
                                    let text = if text.trim().is_empty() { "Plano vazio.".to_owned() } else { text };
                                    let _ = rpc.respond(id, tool_reply(text, true)).await;
                                    "plan-read"
                                }
                                Err(error) => {
                                    log(format!("plan read failed kind={:?}", error.kind()));
                                    let _ = rpc.respond(id, tool_reply("Não consegui ler o plano; ele existe mas a leitura falhou.", false)).await;
                                    "plan-read-failed"
                                }
                            }
                        }
                        ToolCall::AskSession(question) => match planner.ask(params["turnId"].as_str().unwrap_or_default(), &question) {
                            Ok(line) => {
                                log(format!("session question bytes={}", line.len()));
                                let _ = events.send(VoiceEvent::AskSession(line)).await;
                                let _ = rpc.respond(id, tool_reply("Pergunta enviada; a resposta chega depois marcada [RESPOSTA DA SESSÃO À PERGUNTA].", true)).await;
                                "asked"
                            }
                            Err(why) => { let _ = rpc.respond(id, tool_reply(why, false)).await; "refused-ask" }
                        },
                        ToolCall::FinishPlan { action } => {
                            let content = match planner.read() {
                                Ok(content) => content,
                                Err(error) => {
                                    log(format!("plan read failed kind={:?}", error.kind()));
                                    let _ = rpc.respond(id, tool_reply("Não consegui ler o plano; nada foi enviado.", false)).await;
                                    log("tool call finish_plan outcome=plan-read-failed");
                                    continue;
                                }
                            };
                            if content.trim().is_empty() {
                                let _ = rpc.respond(id, tool_reply("Plano vazio; escreva o plano com update_plan antes.", false)).await;
                                "refused-empty-plan"
                            } else if planner.finish_step(action, params["turnId"].as_str().unwrap_or_default()) == FinishStep::Arm {
                                let _ = rpc.respond(id, tool_reply("Leia o resumo e peça confirmação; chame finish_plan de novo depois que o usuário confirmar.", true)).await;
                                "finish-armed"
                            } else {
                                let path = planner.path().map(|p| p.to_path_buf()).unwrap_or_default();
                                // Sessão de outra máquina não lê o arquivo daqui: o conteúdo vai junto.
                                let text = finish_request(&path, action, target_cwd.is_none().then_some(content.as_str()));
                                log(format!("plan sent bytes={}", text.len()));
                                let session = planner.session().unwrap_or(&target).to_owned();
                                let _ = events.send(VoiceEvent::SendPlan { session, text }).await;
                                planner.sent();
                                let warn = apply_models(&rpc, &thread, &mut applied, &models, Mode::Direct, &defaults, &mut effective, events).await;
                                let reply = format!("Plano enviado à sessão; o resultado chega depois. {}", warn.unwrap_or_default());
                                let _ = rpc.respond(id, tool_reply(reply.trim_end(), true)).await;
                                let _ = events.send(VoiceEvent::Mode(Mode::Direct)).await;
                                "plan-sent"
                            }
                        }
                        // A resposta vem da tela (`Voice::reply`), depois de resolver o nome.
                        ToolCall::SwitchSession(name) => {
                            let recent = spoken.recent(&params).unwrap_or_default().to_owned();
                            let spoken = spoken.text(&params).unwrap_or_default().to_owned();
                            let _ = events.send(VoiceEvent::SwitchSession { call: CallId(id), name, spoken, recent }).await;
                            "switch"
                        }
                        ToolCall::HangarActions => { let _ = events.send(VoiceEvent::HangarActions(CallId(id))).await; "hangar-actions" }
                        ToolCall::HangarAction { id: action, arg } => { let _ = events.send(VoiceEvent::HangarAction { call: CallId(id), id: action, arg }).await; "hangar-action" }
                        ToolCall::ReadScreen(area) => { let _ = events.send(VoiceEvent::ReadScreen(CallId(id), area)).await; "read-screen" }
                        ToolCall::ClickScreen { id: target, confirmed } => {
                            let turn = params["turnId"].as_str().unwrap_or_default().to_owned();
                            let _ = events.send(VoiceEvent::ClickScreen { call: CallId(id), id: target, confirmed, turn }).await;
                            "click-screen"
                        }
                        ToolCall::Observe(request) => { let _ = events.send(VoiceEvent::Observe(CallId(id), request)).await; "observe" }
                        ToolCall::EditFiles(_) if cfg!(windows) => {
                            let _ = rpc.respond(id, tool_reply("No Windows você roda sem shell e não consegue editar; mande o pedido à sessão.", false)).await;
                            "refused-windows"
                        }
                        ToolCall::EditFiles(_) if edit_stuck => {
                            let _ = rpc.respond(id, tool_reply("O acesso completo da edição anterior não fechou; nenhuma edição nova até \
                                encerrar a chamada. Mande o pedido à sessão.", false)).await;
                            "refused-edit-stuck"
                        }
                        ToolCall::EditFiles(_) if !edit_consent.check(params["turnId"].as_str().unwrap_or_default(), Instant::now()) => {
                            let _ = rpc.respond(id, tool_reply(organizer::EDIT_UNCONFIRMED, false)).await;
                            "refused-unconfirmed"
                        }
                        ToolCall::EditFiles(request) => {
                            // Acesso completo, como uma sessão normal: o arquivo pedido pode estar fora do projeto. A pasta de trabalho
                            // é o projeto da sessão na tela quando ele é desta máquina, senão a pasta pessoal.
                            let workdir = target_cwd.clone().or_else(std::env::home_dir).unwrap_or_else(|| own.clone());
                            // O acesso vale do turno seguinte em diante: este termina calado e o Hangar abre o da edição.
                            let open = json!({"threadId": thread, "sandboxPolicy": {"type": "dangerFullAccess"}, "cwd": workdir});
                            match rpc.request("thread/settings/update", open).await {
                                Ok(_) => {
                                    edit_consent.used();
                                    edit_pending = Some((request, workdir));
                                    log(format!("edit access granted in_project={}", target_cwd.is_some()));
                                    let _ = rpc.respond(id, tool_reply("Edição liberada. Termine este turno sem escrever nada: o Hangar abre em seguida \
                                        um turno com acesso completo para você fazer a edição.", true)).await;
                                    "edit-granted"
                                }
                                Err(error) => {
                                    log(format!("edit access failed kind={}", rpc_error_kind(&error)));
                                    let _ = rpc.respond(id, tool_reply("Não consegui liberar a edição; mande o pedido à sessão.", false)).await;
                                    "edit-failed"
                                }
                            }
                        }
                        ToolCall::Computer(objective) => { let _ = events.send(VoiceEvent::Computer(CallId(id), objective)).await; "computer" }
                        ToolCall::ListSessions => { let _ = events.send(VoiceEvent::ListSessions(CallId(id))).await; "list" }
                        ToolCall::OpenSession(request) => { let _ = events.send(VoiceEvent::OpenSession(CallId(id), request)).await; "open" }
                        ToolCall::CloseSession { name, confirmed } => {
                            let turn = params["turnId"].as_str().unwrap_or_default().to_owned();
                            let _ = events.send(VoiceEvent::CloseSession { call: CallId(id), name, confirmed, turn }).await;
                            if confirmed { "close-confirmed" } else { "close" }
                        }
                        ToolCall::PairSessions(a, b) => { let _ = events.send(VoiceEvent::PairSessions(CallId(id), a, b)).await; "pair" }
                        ToolCall::UnpairSession(name) => { let _ = events.send(VoiceEvent::UnpairSession(CallId(id), name)).await; "unpair" }
                        ToolCall::FollowSession(name) => { let _ = events.send(VoiceEvent::FollowSession(CallId(id), name)).await; "follow" }
                        ToolCall::UnfollowSession(name) => { let _ = events.send(VoiceEvent::UnfollowSession(CallId(id), name)).await; "unfollow" }
                        ToolCall::SetMode(_) if let Some(name) = organizer::mode_word_session(spoken.text(&params).unwrap_or_default(), &session_names) => {
                            let reply = format!("'{name}' é o nome de uma sessão, não o modo; use switch_session para ir até ela.");
                            let _ = rpc.respond(id, tool_reply(reply, false)).await;
                            "refused-session-name"
                        }
                        ToolCall::SetMode(mode) => {
                            let mut note = switch_mode(&mut planner, mode, &target, &mut gate, &rpc, events).await;
                            if let Some(warn) = apply_models(&rpc, &thread, &mut applied, &models, mode, &defaults, &mut effective, events).await { note = format!("{note} {warn}"); }
                            let _ = rpc.respond(id, tool_reply(note, true)).await;
                            "mode-set"
                        }
                        ToolCall::Send { request, session } => {
                            let turn = params["turnId"].as_str().unwrap_or_default().to_owned();
                            let to = session.clone().unwrap_or_default();
                            let said = spoken.text(&params).unwrap_or_default().to_owned();
                            let recent = spoken.recent(&params).unwrap_or_default().to_owned();
                            let named = session.as_deref().is_some_and(|name| organizer::send_target_authorized(&said, &recent, name, &target, &session_names));
                            // Pedido que ainda espera para o mesmo destino: o novo é correção dele, não envio a mais.
                            let verdict = if gate.waiting_for(&to) { SendConsent::Granted } else { consent.check_to(&turn, &to, named, Instant::now()) };
                            match verdict {
                                SendConsent::Granted => match gate.offer((id, session, turn.clone()), &to, request, Instant::now()) {
                                    Err(((id, ..), why)) => {
                                        // Nada saiu: o reenvio completo no mesmo turno não pode virar "já enviado".
                                        consent.release(&turn, &to);
                                        let _ = rpc.respond(id, tool_reply(why, false)).await;
                                        "refused-short"
                                    }
                                    Ok(Some(((old, ..), _))) => { let _ = rpc.respond(old, tool_reply("Substituído por um pedido mais recente; nada foi enviado.", false)).await; "offered-superseded" }
                                    Ok(None) => "offered",
                                },
                                refused => {
                                    // Só contagens: o texto da fala nunca vai ao diário.
                                    let (verbs, directed, sessions) = organizer::send_signals(&said, &session_names);
                                    log(format!("send refused why={refused:?} words={} send_verbs={verbs} directed={directed} sessions_named={sessions} named_target={named}",
                                        said.split_whitespace().count()));
                                    let reply = match refused { SendConsent::Duplicate => organizer::SEND_DUPLICATE, SendConsent::NotNamed => organizer::SEND_NOT_NAMED, _ => SEND_UNCONFIRMED };
                                    if refused == SendConsent::Unconfirmed { refused_send = Some(Instant::now()); }
                                    let _ = rpc.respond(id, tool_reply(reply, false)).await;
                                    match refused { SendConsent::Duplicate => "refused-duplicate", SendConsent::NotNamed => "refused-not-named", _ => "refused-unconfirmed" }
                                }
                            }
                        }
                        ToolCall::Hold(request) => {
                            // "Segura" dentro da janela de 1,5 s cancela o envio que ainda não saiu.
                            let cancelled = gate.user_spoke();
                            let outcome = if cancelled.is_empty() { "held" } else { "held-cancelled" };
                            for ((old, ..), _) in cancelled {
                                let _ = rpc.respond(old, tool_reply("Cancelado: o usuário pediu para segurar; nada foi enviado.", false)).await;
                            }
                            let _ = events.send(VoiceEvent::Draft(Some(request.clone()))).await;
                            let _ = rpc.respond(id, tool_reply(format!("Segurado, nada enviado: {request}. Envie com send_to_session quando o usuário liberar."), true)).await;
                            outcome
                        }
                        ToolCall::Discard => {
                            for ((old, ..), _) in gate.user_spoke() {
                                let _ = rpc.respond(old, tool_reply("Cancelado: o usuário desistiu; nada foi enviado.", false)).await;
                            }
                            let _ = events.send(VoiceEvent::Draft(None)).await;
                            let _ = rpc.respond(id, tool_reply("Rascunho descartado.", true)).await;
                            "discarded"
                        }
                        ToolCall::Unknown(name) => { let _ = rpc.respond(id, tool_reply(format!("Ferramenta inexistente: {name}"), false)).await; "unknown" }
                    };
                    log(format!("tool call {tool} outcome={outcome}"));
                }
                // Perguntas do organizador não têm tela: a resposta é pela voz.
                Ok(Incoming::Request { id, method, .. }) if method == "item/tool/requestUserInput" => {
                    log("request item/tool/requestUserInput answered empty");
                    let _ = rpc.respond(id, json!({"answers": {}})).await;
                }
                Ok(Incoming::Request { id, method, .. }) => { log(format!("request {method} answered empty")); let _ = rpc.respond(id, json!({})).await; }
                Ok(Incoming::Notification { method, params }) => {
                    let ours = params["threadId"].as_str() == Some(thread.as_str());
                    log_notification(&method, &params, ours, &mut last_delta);
                    // Limite da conta não leva threadId: tem de passar antes do filtro.
                    if method == "account/rateLimits/updated" { send_limits(events, usage::account_limits(&params["rateLimits"])).await; continue; }
                    if !ours { continue; }
                    if method == "thread/realtime/sdp" {
                        let _ = events.send(VoiceEvent::Answer(params["sdp"].as_str().unwrap_or_default().to_owned())).await;
                        continue;
                    }
                    if method == "item/started" || method == "item/completed" { spoken.item_started(&params); }
                    // Cada fala uma vez: o started pode vir sem texto e o completed repete o item.
                    let text = organizer::user_message_text(&params).filter(|t| !t.trim().is_empty());
                    let item_key = params["item"]["id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned).or_else(|| text.clone()).unwrap_or_default();
                    if (method == "item/started" || method == "item/completed")
                        && let Some(text) = text.clone()
                        && !organizer::from_hangar(&text) && !seen_items.contains(&item_key) {
                        if seen_items.len() >= 256 { seen_items.clear(); }
                        seen_items.insert(item_key);
                        let now = Instant::now();
                        let _ = events.send(VoiceEvent::Backstage(backstage_input(&text))).await;
                        let input = spoken_input(&text).to_owned();
                        // A voz encaminha a mesma fala de novo quando a transcrição final fecha: não é o usuário continuando.
                        let repeat = last_input.as_ref().is_some_and(|(prev, at)| now.saturating_duration_since(*at) < REPEAT_WINDOW && repeated_handoff(prev, &input));
                        log(format!("handoff repeat={repeat}"));
                        if repeat {
                            let steer = json!({"threadId": thread, "expectedTurnId": params["turnId"], "input": [{"type": "text", "text": REPEAT_NOTE}]});
                            let steered = rpc.request("turn/steer", steer).await;
                            log(format!("repeat note steered ok={}", steered.is_ok()));
                        } else {
                            // Só fala nova vale como pedido de envio: a repetida reabriria um pedido já usado.
                            let speech = user_speech(&text);
                            let asked = consent.heard(&speech, &session_names, now);
                            edit_consent.heard(&speech, &session_names, now);
                            if asked && refused_send.take().is_some_and(|at| now.saturating_duration_since(at) < organizer::CONFIRM_WINDOW) {
                                log("send completed by next speech");
                                let note = organizer::SEND_COMPLETED_NOTE.to_owned();
                                pending_context = Some(match pending_context.take() { Some(old) => format!("{old}\n{note}"), None => note });
                            }
                            if let Some(turn) = params["turnId"].as_str() {
                                let recent = organizer::transcript_delta(&text).to_owned();
                                let _ = events.send(VoiceEvent::Heard { turn: turn.to_owned(), speech, recent }).await;
                            }
                            last_input = Some((input, now));
                            // A troca de sessão ou de modo chega ao organizador no turno desta fala. Nunca pela voz: na v3
                            // o texto acrescentado a ela é falado, e o usuário ouvia o caminho da pasta e as regras.
                            if let Some(note) = pending_context.take() {
                                let steer = json!({"threadId": thread, "expectedTurnId": params["turnId"],
                                    "input": [{"type": "text", "text": format!("{} {note}", organizer::NOTE_PREFIX)}]});
                                match rpc.request("turn/steer", steer).await {
                                    Ok(_) => { log("context steered on user speech"); context_failures = 0; }
                                    // Sem o contexto o organizador fala da sessão errada: volta para a próxima fala tentar de novo.
                                    Err(error) => {
                                        context_failures += 1;
                                        log(format!("context steer failed kind={} attempt={context_failures}", rpc_error_kind(&error)));
                                        if context_failures < 2 { pending_context.get_or_insert(note); }
                                        else { let _ = events.send(VoiceEvent::Failed(VoiceFailure::Organizer)).await; }
                                    }
                                }
                            }
                            // A transcrição da fala chega atrasada e cancelava o próprio pedido: só uma fala nova
                            // encaminhada (outro userMessage) prova que o usuário continuou.
                            for ((id, ..), _) in gate.user_spoke() {
                                log("gate cancelled: user kept talking");
                                let _ = rpc.respond(id, tool_reply("O usuário continuou falando; nada foi enviado. Monte o pedido com a fala completa.", false)).await;
                            }
                        }
                    } else if let Some(text) = text
                        && method == "item/completed" && organizer::from_hangar(&text) && !text.trim_start().starts_with("[NOTA DO HANGAR]") {
                        let _ = events.send(VoiceEvent::Backstage(backstage_input(&text))).await;
                    }
                    if method == "item/completed" && params["item"]["type"] == "agentMessage"
                        && let Some(text) = params["item"]["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        let _ = events.send(VoiceEvent::Backstage(Backstage::Answer(text.to_owned()))).await;
                    }
                    if let Some(action) = organizer::organizer_action(&method, &params["item"]) { let _ = events.send(VoiceEvent::Action(action)).await; }
                    if let Some(delta) = organizer::reasoning_delta(&method, &params) { let _ = events.send(VoiceEvent::Thought(delta)).await; }
                    let kind = params["item"]["type"].as_str().unwrap_or_default();
                    let next = match method.as_str() {
                        // O turno só entra em SpokenTurns quando o userMessage chega, depois do turn/started.
                        "item/started" | "item/completed" if params["item"]["type"] == "userMessage" && (method == "item/started" || organizer_busy) && spoken.allows(&params) => Some(Activity::Thinking),
                        "item/started" if kind == "webSearch" => Some(Activity::Searching),
                        "item/started" if matches!(kind, "dynamicToolCall" | "commandExecution") => Some(Activity::Working),
                        "item/completed" if matches!(kind, "webSearch" | "dynamicToolCall" | "commandExecution") && activity != Activity::Idle => Some(Activity::Thinking),
                        "turn/completed" => Some(Activity::Idle),
                        _ => None,
                    };
                    if let Some(next) = next && next != activity {
                        activity = next;
                        log(format!("activity {activity:?}"));
                        let _ = events.send(VoiceEvent::Activity(activity)).await;
                    }
                    match method.as_str() {
                        "turn/started" => { organizer_busy = true; results.turn_started(); }
                        "turn/completed" => {
                            organizer_busy = false;
                            let _ = events.send(VoiceEvent::TurnDone).await;
                            spoken.turn_completed(&params);
                            // Turno interrompido ou falho não pode deixar um envio esperando a janela de 1,5 s.
                            if params["turn"]["status"] != "completed" {
                                for ((id, ..), _) in gate.user_spoke() {
                                    log("gate cancelled: turn interrupted");
                                    let _ = rpc.respond(id, tool_reply("O turno foi interrompido; nada foi enviado.", false)).await;
                                }
                            }
                            if params["turn"]["status"] == "failed" {
                                log(format!("organizer failure: {:?}", VoiceFailure::Organizer));
                                let _ = events.send(VoiceEvent::Failed(VoiceFailure::Organizer)).await;
                            }
                            // Acabou o turno da edição (concluído, interrompido ou falho): o acesso ao projeto fecha já.
                            if edit_turn.is_some() && edit_turn.as_deref() == params["turn"]["id"].as_str() {
                                edit_turn = None;
                                edit_stuck = !close_edit_access(&rpc, &thread, &own, events).await;
                            }
                            if let Some((request, project)) = edit_pending.take() {
                                let text = format!("{} Edição liberada pelo usuário, com acesso completo a esta máquina (pasta de trabalho {}): \
                                    {request}\nEdite agora pelo shell só os arquivos deste pedido, dentro ou fora do projeto, confira o resultado e diga \
                                    numa frase o que mudou. Ao terminar, você volta a gravar só na sua pasta.", organizer::NOTE_PREFIX, project.display());
                                match rpc.request("turn/start", json!({"threadId": thread, "input": [{"type": "text", "text": text}]})).await {
                                    Ok(result) => {
                                        if let Some(turn) = result["turn"]["id"].as_str() { edit_turn = Some(turn.to_owned()); results.mark_summary(turn.to_owned()); }
                                        // Os resultados de sessão esperam o turno da edição acabar.
                                        results.turn_started();
                                        log("edit turn started");
                                    }
                                    Err(error) => {
                                        log(format!("edit turn start failed kind={}", rpc_error_kind(&error)));
                                        edit_stuck = !close_edit_access(&rpc, &thread, &own, events).await;
                                        if let Some(input) = results.turn_completed() { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                                    }
                                }
                            } else if let Some(input) = results.turn_completed() { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                        }
                        "item/completed" if params["item"]["type"] == "agentMessage" && params["item"]["phase"] != "commentary" => {
                            if results.take_summary(params["turnId"].as_str().unwrap_or_default())
                                && let Some(text) = params["item"]["text"].as_str() {
                                speak(&rpc, &thread, &mut out, text.to_owned(), "summary").await;
                            }
                        }
                        "thread/tokenUsage/updated" => if let Some((used, window)) = usage::context_usage(&params) {
                            log(format!("organizer context used={used} window={window:?}"));
                            let _ = events.send(VoiceEvent::OrganizerContext { used, window }).await;
                        },
                        "thread/realtime/error" | "thread/realtime/closed" if restarting || detached => log("realtime ended while switching device"),
                        "thread/realtime/error" => break Err(failed("realtime")(VoiceFailure::Realtime(params["message"].as_str().unwrap_or_default().to_owned()))),
                        "thread/realtime/closed" => {
                            // Só um código curto vai ao diário; o texto livre pode trazer conteúdo.
                            let reason = params["reason"].as_str().filter(|r| r.len() <= 40 && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or("?");
                            log(format!("realtime closed by server reason={reason} stopped={}", stopped.load(Ordering::Relaxed)));
                            break if stopped.load(Ordering::Relaxed) { Ok(()) } else { Err(VoiceFailure::Closed) };
                        }
                        _ => {}
                    }
                }
                Ok(Incoming::Exited) | Err(_) => break Err(failed("app-server exited")(VoiceFailure::AppServer)),
            },
            // Acorda para soltar o envio que assentou.
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    };
    stopped.store(true, Ordering::Relaxed);
    let (held, bytes) = out.hold.pending();
    if held > 0 { log(format!("held speech dropped at call end count={held} bytes={bytes}")); }
    if !out.parked.is_empty() { log(format!("parked speech dropped at call end count={}", out.parked.len())); }
    let _ = rpc.request("thread/realtime/stop", json!({"threadId": thread})).await;
    outcome
}

async fn send_limits(events: &async_channel::Sender<VoiceEvent>, limits: Option<(usage::RateWindow, usage::RateWindow)>) {
    let Some((five_hour, seven_day)) = limits else { return };
    log(format!("account limits five_hour={} seven_day={}", five_hour.is_some(), seven_day.is_some()));
    let _ = events.send(VoiceEvent::AccountLimits { five_hour, seven_day }).await;
}

/// Entrar no Planejar cancela o envio que esperava a janela: a chamada pendente falha em vez de ficar sem resposta.
async fn switch_mode(planner: &mut Planner, mode: Mode, target: &str, gate: &mut SendGate<(Value, Option<String>, String)>, rpc: &Rpc,
    events: &async_channel::Sender<VoiceEvent>) -> String {
    if mode == Mode::Plan {
        for ((old, ..), _) in gate.user_spoke() {
            let _ = rpc.respond(old, tool_reply("Cancelado: modo Planejar; nada foi enviado.", false)).await;
        }
    }
    let note = planner.set_mode(mode, target);
    let _ = events.send(VoiceEvent::Mode(mode)).await;
    note
}

/// O que a conta usa quando o par não escolhe: o modelo do config e a velocidade com que a thread abriu.
struct Defaults { model: Option<String>, tier: Option<String> }

/// Leva a thread ao par do modo; vale a partir do próximo turno. Falha aparece na tela e o texto devolvido avisa o organizador.
async fn apply_models(rpc: &Rpc, thread: &str, applied: &mut ModeModel, models: &ModeModels, mode: Mode, defaults: &Defaults,
    effective: &mut Effective, events: &async_channel::Sender<VoiceEvent>) -> Option<&'static str> {
    let wanted = models.get(mode);
    // Voltar ao "modelo do config" sem saber qual é ele não troca nada: dizer, em vez de fingir que trocou.
    if wanted.model.is_none() && defaults.model.is_none() && applied.model.is_some() {
        log(format!("settings update skipped mode={mode:?} reason=unknown_default_model"));
        let _ = events.send(VoiceEvent::Failed(VoiceFailure::ModelSwitch)).await;
        return Some("O modo mudou, mas o modelo que pensa não trocou; segue o anterior.");
    }
    let Some(update) = settings_update(thread, applied, wanted, defaults.model.as_deref(), defaults.tier.as_deref()) else {
        *applied = wanted.clone();
        return None;
    };
    if rpc.request("thread/settings/update", update.clone()).await.is_ok() {
        log(format!("settings update ok mode={mode:?} tier={}", update.get("serviceTier").map_or("same", |t| t.as_str().unwrap_or("default"))));
        *applied = wanted.clone();
        if let Some(model) = update["model"].as_str() { effective.model = Some(model.to_owned()); }
        if let Some(effort) = update["effort"].as_str() { effective.effort = Some(effort.to_owned()); }
        if let Some(tier) = update.get("serviceTier") { effective.tier = Some(tier.as_str().unwrap_or(organizer::TIER_STANDARD).to_owned()); }
        let _ = events.send(VoiceEvent::Organizer(effective.clone())).await;
        return None;
    }
    log(format!("settings update failed mode={mode:?}"));
    let _ = events.send(VoiceEvent::Failed(VoiceFailure::ModelSwitch)).await;
    Some("O modo mudou, mas o modelo que pensa não trocou; segue o anterior.")
}

/// Volta o organizador ao isolamento: grava só na pasta própria. Tenta duas vezes; falhando, avisa na tela, porque o acesso
/// completo ficaria aberto nos turnos seguintes.
async fn close_edit_access(rpc: &Rpc, thread: &str, own: &std::path::Path, events: &async_channel::Sender<VoiceEvent>) -> bool {
    let back = json!({"threadId": thread, "sandboxPolicy": {"type": "workspaceWrite", "writableRoots": []}, "cwd": own});
    for _ in 0..2 {
        if rpc.request("thread/settings/update", back.clone()).await.is_ok() { log("edit access closed"); return true; }
    }
    log("edit access close failed");
    let _ = events.send(VoiceEvent::Failed(VoiceFailure::Organizer)).await;
    false
}

/// Fala que a voz puxa sozinha: `hold` a segura enquanto o usuário fala; `parked` a guarda enquanto nenhum aparelho ouve
/// (dono caído ou passagem em curso), até o próximo `Live`.
#[derive(Default)]
struct Speech { hold: SpeechHold, away: bool, parked: VecDeque<String> }

impl Speech {
    fn park(&mut self, text: String, tag: &str) {
        log(format!("{tag} speech parked bytes={}", text.len()));
        self.parked.push_back(text);
    }
}

/// Fala que a voz puxa sozinha passa por aqui: com o usuário falando, fica guardada até ele terminar.
async fn speak(rpc: &Rpc, thread: &str, out: &mut Speech, text: String, tag: &str) {
    if out.away { out.park(text, tag); return; }
    let bytes = text.len();
    match out.hold.offer(text, Instant::now()) {
        Some(text) => append_speech(rpc, thread, &text, tag).await,
        None => log(format!("{tag} speech held bytes={bytes}")),
    }
}

async fn append_speech(rpc: &Rpc, thread: &str, text: &str, tag: &str) {
    let spoke = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": text})).await;
    log(format!("{tag} appendSpeech bytes={} ok={}", text.len(), spoke.is_ok()));
}

async fn start_summary(rpc: &Rpc, thread: &str, first: Value, results: &mut Results, out: &mut Speech, organizer_busy: bool) {
    let mut next = Some(first);
    // Laço, não recursão: um resumo recusado com o organizador ocioso solta o próximo da fila na hora.
    while let Some(mut input) = next.take() {
        input["threadId"] = json!(thread);
        match rpc.request("turn/start", input).await {
            Ok(result) => { if let Some(turn) = result["turn"]["id"].as_str() { results.mark_summary(turn.to_owned()); } }
            Err(error) => {
                log(format!("summary turn/start failed kind={} organizer_busy={organizer_busy}", rpc_error_kind(&error)));
                // Ocioso e recusado: nenhum turn/completed virá; fala o começo do texto e drena a fila.
                if let Some(text) = results.turn_start_failed(organizer_busy) {
                    let short: String = text.chars().take(400).collect();
                    speak(rpc, thread, out, format!("A sessão respondeu: {short}"), "summary fallback").await;
                    next = results.turn_completed();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::test_support::fake_app_server;
    use serde_json::json;

    fn options(tools: Value) -> CallOptions {
        // Nunca o HOME real: a chamada cria a pasta própria e os planos aqui.
        let voice_dir = std::env::temp_dir().join(format!("hangar-voice-call-{}", std::process::id()));
        CallOptions { voice: None, context: "ctx".into(), cwd: None, target: "hangar".into(), organizer: ModeModels::default(), tools, handoff_same_thread: true, voice_dir }
    }

    async fn next_answer(events: &async_channel::Receiver<VoiceEvent>) -> String {
        loop { if let VoiceEvent::Answer(sdp) = events.recv().await.unwrap() { return sdp; } }
    }

    #[test]
    fn deltas_log_once_per_speaker_turn() {
        let mut last = None;
        let delta = "thread/realtime/transcript/delta";
        assert!(first_delta(&mut last, delta, "user"));
        assert!(!first_delta(&mut last, delta, "user"));
        assert!(first_delta(&mut last, delta, "assistant"), "troca de falante loga");
        assert!(first_delta(&mut last, delta, "user"));
        assert!(first_delta(&mut last, "turn/started", ""));
        assert!(first_delta(&mut last, delta, "user"), "turno novo loga de novo");
        assert!(first_delta(&mut last, "item/started", ""), "não-delta sempre loga");
    }

    #[tokio::test]
    async fn device_offer_becomes_realtime_start_and_answer_goes_back() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer("v=0 offer A".into(), "ctx".into());
        assert_eq!(next_answer(&events).await, "v=0 answer 1");
        let mut start = None;
        while let Some(m) = seen.recv().await { if m["method"] == "thread/realtime/start" { start = Some(m); break; } }
        let start = start.unwrap();
        assert_eq!(start["params"]["transport"]["sdp"], "v=0 offer A");
        assert_eq!(start["params"]["includeStartupContext"], false);
    }

    #[tokio::test]
    async fn second_offer_restarts_realtime_on_the_same_thread() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer("v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.offer("v=0 offer B".into(), "A sessão na tela agora é web.".into());
        // O fechamento da conversa anterior não encerra a chamada.
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "replaced"}})).unwrap();
        assert_eq!(next_answer(&events).await, "v=0 answer 2");
        let all: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        let starts: Vec<&Value> = all.iter().filter(|m| m["method"] == "thread/realtime/start").collect();
        assert_eq!(starts.last().unwrap()["params"]["threadId"], "t1");
        assert_eq!(starts.last().unwrap()["params"]["includeStartupContext"], true);
        assert_eq!(starts.last().unwrap()["params"]["initialItems"][0]["text"], "A sessão na tela agora é web.");
        assert_eq!(all.iter().filter(|m| m["method"] == "thread/start").count(), 1, "uma thread só");
    }

    #[tokio::test]
    async fn detached_call_survives_realtime_closed_and_resumes() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer("v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.detached();
        // Aparelho caído: a OpenAI fecha a conversa falada; a chamada continua esperando outro aparelho.
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "peer_gone"}})).unwrap();
        push.send(json!({"method": "thread/realtime/error", "params": {"threadId": "t1", "message": "ice failed"}})).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        voice.offer("v=0 offer C".into(), "ctx".into());
        assert_eq!(next_answer(&events).await, "v=0 answer 2");
        while let Ok(e) = events.try_recv() { assert!(!matches!(e, VoiceEvent::Phase(Phase::Closed)), "não encerrou"); }
        let all: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        assert!(!all.iter().any(|m| m["method"] == "thread/realtime/stop"));
    }

    #[tokio::test]
    async fn thread_start_carries_the_owner_tools() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, _events) = async_channel::unbounded();
        let tools = crate::voice::organizer::tools_for(&["switch_session".into()]);
        let voice = Voice::start(options(tools.clone()), spawn, tx);
        voice.offer("v=0".into(), "ctx".into());
        loop { let m = seen.recv().await.unwrap(); if m["method"] == "thread/start" { assert_eq!(m["params"]["dynamicTools"], tools); break; } }
    }

    #[tokio::test]
    async fn speech_while_detached_waits_for_the_next_device() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer("v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.detached();
        voice.jev_verdict("turn-1".into(), SendVerdict::Unsure, Some("resposta guardada".into()), None);
        let spoke = |m: &Value| m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == "resposta guardada";
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| spoke(&m)), "sem aparelho, nada vai à voz");
        voice.offer("v=0 offer B".into(), "ctx".into());
        next_answer(&events).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before_live: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        assert!(before_live.iter().any(|m| m["method"] == "thread/realtime/start" && m["params"]["transport"]["sdp"] == "v=0 offer B"));
        assert!(!before_live.iter().any(|m| spoke(m)), "a passagem ainda não conectou o áudio");
        voice.live();
        loop { if spoke(&seen.recv().await.unwrap()) { break; } }
    }

    #[tokio::test]
    async fn detached_before_the_first_offer_does_not_stick() {
        let (spawn, _seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.detached();
        voice.offer("v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Com o aparelho presente, o fim da conversa falada encerra a chamada.
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "x"}})).unwrap();
        loop { if let VoiceEvent::Phase(Phase::Closed) = events.recv().await.unwrap() { break; } }
    }

    #[tokio::test]
    async fn commands_before_the_offer_are_kept() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.set_mode(Mode::Plan);
        voice.offer("v=0".into(), "ctx".into());
        next_answer(&events).await;
        loop { if let VoiceEvent::Mode(Mode::Plan) = events.recv().await.unwrap() { break; } }
        loop { let m = seen.recv().await.unwrap(); if m["method"] == "thread/realtime/appendSpeech" { assert_eq!(m["params"]["text"], "Modo planejar."); break; } }
    }
}
