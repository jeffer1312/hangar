//! Conversa por voz: o Codex local fala, a sessão aberta na tela trabalha.
pub mod audio;
pub mod computer;
pub mod organizer;
pub mod plan;
pub mod rpc;
pub mod rtc;
pub mod usage;

use organizer::{FinishStep, MIC_VOICE_LEVEL, Mode, ModeModel, ModeModels, Planner, Results, SendGate, SpokenTurns, ToolCall, finish_request, parse_tool, send_allowed, settings_update, tool_reply, organizer_start, ORGANIZER_PROMPT, VOICE_PROMPT};
use rpc::{Codex, Incoming, Rpc, RpcError, handshake};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::{runtime::Handle, sync::{Notify, mpsc}};

pub struct CallId(Value);
pub enum Phase { Connecting, Live, Closed }
#[derive(Debug, Clone)]
pub enum VoiceFailure { Microphone, Speaker, AppServer, Realtime(String), Network, Timeout, Organizer, ModelSwitch, OwnFolder, AudioStopped, Closed }
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Activity { #[default] Idle, Thinking, Searching, Working }
pub enum VoiceEvent {
    Phase(Phase), Levels(f32, f32), Draft(Option<String>), Activity(Activity), ReadSession(CallId), Send(CallId, String), Failed(VoiceFailure),
    Mode(Mode), Plan { path: PathBuf, markdown: String }, AskSession(String), SendPlan { session: String, text: String },
    /// Nome pedido e a fala do turno: a tela só troca quando a fala pede essa sessão.
    SwitchSession { call: CallId, name: String, spoken: String },
    /// Ações da tela do Hangar (catálogo e execução) e o `computer` para os outros programas.
    HangarActions(CallId), HangarAction { call: CallId, id: String, arg: Option<String> }, Computer(CallId, String),
    /// Leitura da tela do Hangar pela árvore de acessibilidade; `None` = a tela escolhe a área.
    ReadScreen(CallId, Option<String>),
    /// Ferramentas de sessão: a tela resolve os nomes falados e responde por `Voice::reply`. `turn` separa o pedido do sim.
    ListSessions(CallId), OpenSession(CallId, organizer::OpenRequest),
    CloseSession { call: CallId, name: String, confirmed: bool, turn: String },
    PairSessions(CallId, String, String), UnpairSession(CallId, String),
    /// Contexto da thread do organizador (não o da voz, que não é informado): input do último turno e a janela do modelo.
    OrganizerContext { used: u64, window: Option<u64> },
    AccountLimits { five_hour: usage::RateWindow, seven_day: usage::RateWindow },
    /// Só para a tela: pedaço do resumo do raciocínio, a ação em curso e o fim do turno, que limpa os dois.
    Thought(String), Action(Option<organizer::OrganizerAction>), TurnDone,
}
/// `cwd`: pasta da sessão na tela quando é desta máquina (o organizador lê o código dela); `target`: nome dessa sessão.
/// `organizer`: modelo e esforço do organizador por modo; a chamada nasce no Direto.
pub struct VoiceOptions { pub codex: Codex, pub voice: Option<String>, pub context: String, pub cwd: Option<PathBuf>, pub target: String,
    pub codex_home: Option<PathBuf>, pub organizer: ModeModels }

enum Command { Retarget(String, String, Option<PathBuf>), Result(String, String), Reply(Value, Value), SetMode(Mode), Models(ModeModels), Answer(String), PlanDelivered,
    Sessions(Vec<String>) }

pub struct Voice { commands: mpsc::UnboundedSender<Command>, muted: Arc<AtomicBool>, stopped: Arc<AtomicBool>, stop: Arc<Notify> }

impl Voice {
    pub fn start(runtime: &Handle, options: VoiceOptions, events: async_channel::Sender<VoiceEvent>) -> Voice {
        let (commands, inbox) = mpsc::unbounded_channel();
        let (muted, stopped, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(Notify::new()));
        runtime.spawn(call(options, events, inbox, muted.clone(), stopped.clone(), stop.clone()));
        Voice { commands, muted, stopped, stop }
    }
    pub fn set_muted(&self, muted: bool) { self.muted.store(muted, Ordering::Relaxed); }
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
    pub fn stop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        // notify_one guarda a licença mesmo sem ninguém esperando ainda.
        self.stop.notify_one();
    }
}

impl Drop for Voice { fn drop(&mut self) { self.stop(); } }

/// Diário da voz. Nunca recebe fala, transcrição, texto de pedido nem argumentos de ferramenta.
pub(crate) fn log(text: impl AsRef<str>) { crate::log_line(&format!("voice: {}", text.as_ref())); }

fn failed(step: &'static str) -> impl FnOnce(VoiceFailure) -> VoiceFailure {
    move |failure| { log(format!("{step} failed: {failure:?}")); failure }
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

async fn call(options: VoiceOptions, events: async_channel::Sender<VoiceEvent>, mut inbox: mpsc::UnboundedReceiver<Command>,
    muted: Arc<AtomicBool>, stopped: Arc<AtomicBool>, stop: Arc<Notify>) {
    log(format!("call start voice={}", options.voice.as_deref().unwrap_or("default")));
    log("phase Connecting");
    let _ = events.send(VoiceEvent::Phase(Phase::Connecting)).await;
    // Parar vale em qualquer fase: largar o future derruba o app-server (kill_on_drop) e o flag para a thread do RTC.
    let outcome = tokio::select! {
        outcome = run_call(options, &events, &mut inbox, &muted, &stopped) => outcome,
        _ = stop.notified() => { log("stop requested"); Ok(()) },
    };
    stopped.store(true, Ordering::Relaxed);
    match &outcome { Ok(()) => log("call end ok"), Err(failure) => log(format!("call end failure={failure:?}")) }
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

fn rtc_failure(error: rtc::RtcError) -> VoiceFailure {
    // Mídia parada é microfone trocado/desconectado ou codec, não rede: o texto de rede mandava olhar o firewall.
    match error { rtc::RtcError::Microphone => VoiceFailure::Microphone, rtc::RtcError::Speaker => VoiceFailure::Speaker,
        rtc::RtcError::Media => VoiceFailure::AudioStopped, _ => VoiceFailure::Network }
}

async fn run_call(options: VoiceOptions, events: &async_channel::Sender<VoiceEvent>, inbox: &mut mpsc::UnboundedReceiver<Command>,
    muted: &Arc<AtomicBool>, stopped: &Arc<AtomicBool>) -> Result<(), VoiceFailure> {
    let (rpc, incoming) = Rpc::spawn(&options.codex, options.codex_home.as_deref()).await.map_err(rpc_failure).map_err(failed("app-server spawn"))?;
    log("app-server spawned");
    let config = handshake(&rpc).await.map_err(rpc_failure).map_err(failed("handshake"))?;
    log("handshake ok");
    // Pasta própria e fixa: o que o organizador grava fica entre chamadas, nada aqui a apaga.
    let own = plan::files_dir();
    if let Err(error) = std::fs::create_dir_all(&own) {
        log(format!("own folder create failed kind={:?}", error.kind()));
        let _ = events.send(VoiceEvent::Failed(VoiceFailure::OwnFolder)).await;
    }
    let mut models = options.organizer.clone();
    let mut applied = models.direct.clone();
    let start = organizer_start(&config, &own, options.cwd.as_deref(), &options.context, applied.model.as_deref(), &applied.effort);
    log(format!("organizer model={} effort={}", if applied.model.is_some() { "chosen" } else { "config" }, applied.effort));
    // A conta lida em paralelo, com prazo curto: falhar só deixa os limites ocultos até a primeira atualização.
    let limits = async {
        match tokio::time::timeout(Duration::from_secs(3), rpc.request("account/rateLimits/read", json!({}))).await {
            Ok(Ok(result)) => send_limits(events, usage::read_limits(&result)).await,
            Ok(Err(error)) => log(format!("rateLimits/read failed: {error:?}")),
            Err(_) => log("rateLimits/read timed out"),
        }
    };
    let (started, ()) = tokio::join!(rpc.request("thread/start", start), limits);
    let started = started.map_err(rpc_failure).map_err(failed("thread/start"))?;
    let thread = started["thread"]["id"].as_str().unwrap_or_default().to_owned();
    // Voltar ao modelo do config pede o nome dele: o do config, senão o que a thread abriu sem escolha.
    let default_model = config["model"].as_str().or_else(|| started["model"].as_str().filter(|_| applied.model.is_none())).map(str::to_owned);
    log(format!("thread started id={thread}"));

    let offer = tokio::task::spawn_blocking(rtc::offer).await.map_err(|_| VoiceFailure::Network)
        .and_then(|offer| offer.map_err(rtc_failure)).map_err(failed("rtc offer"))?;
    let mut realtime = json!({"threadId": thread, "version": "v3", "outputModality": "audio", "prompt": VOICE_PROMPT,
        "includeStartupContext": false, "delegationAckFiller": false, "clientManagedHandoffs": false,
        "realtimeStartInstructions": ORGANIZER_PROMPT, "initialItems": [{"role": "developer", "text": options.context}],
        "transport": {"type": "webrtc", "sdp": offer.sdp.clone()}});
    if let Some(voice) = &options.voice { realtime["voice"] = json!(voice); }
    rpc.request("thread/realtime/start", realtime).await.map_err(rpc_failure).map_err(failed("thread/realtime/start"))?;
    log(format!("realtime/start sent offer_bytes={}", offer.sdp.len()));

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

    let (rtc_tx, rtc_rx) = async_channel::unbounded();
    let peer = rtc::run(offer, answer, muted.clone(), rtc_tx, stopped.clone());
    log("rtc thread started");
    let mut greeted = false;
    let mut results = Results::default();
    let mut gate: SendGate<Value> = SendGate::default();
    let mut organizer_busy = false;
    let mut spoken = SpokenTurns::default();
    let mut activity = Activity::Idle;
    let mut planner = Planner::default();
    let mut target = options.target.clone();
    let mut target_cwd = options.cwd.clone();
    let mut pending_context: Option<String> = None;
    let mut context_failures = 0u32;
    let mut session_names: Vec<String> = Vec::new();
    let outcome = loop {
        // No Planejar nada sai pelo gate; ao entrar nele o envio pendente já foi cancelado.
        if planner.mode == Mode::Direct && let Some((id, request)) = gate.due(Instant::now()) {
            log(format!("gate sent words={}", request.split_whitespace().count()));
            let _ = events.send(VoiceEvent::Draft(None)).await;
            let _ = events.send(VoiceEvent::Send(CallId(id), request)).await;
        }
        tokio::select! {
            event = rtc_rx.recv() => match event {
                Ok(rtc::RtcEvent::Connected) => {
                    log("phase Live");
                    let _ = events.send(VoiceEvent::Phase(Phase::Live)).await;
                    // A voz V3 nunca fala primeiro: sem isto a pessoa não tem prova de que o alto-falante funciona.
                    if !greeted {
                        greeted = true;
                        let spoke = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": "Conectado. Pode falar."})).await;
                        log(format!("greeting appendSpeech ok={}", spoke.is_ok()));
                    }
                }
                Ok(rtc::RtcEvent::Levels(i, o)) => {
                    if i >= MIC_VOICE_LEVEL { gate.heard_voice(Instant::now()); }
                    let _ = events.try_send(VoiceEvent::Levels(i, o));
                }
                Ok(rtc::RtcEvent::Failed(error)) => { log(format!("rtc failed error={error:?}")); break Err(failed("rtc")(rtc_failure(error))); }
                Ok(rtc::RtcEvent::Closed) => { log("rtc closed"); break Ok(()); }
                // A thread sempre manda o último evento antes de sair; canal fechado sem ele é thread morta.
                Err(_) => break Err(failed("rtc thread died")(VoiceFailure::Network)),
            },
            item = incoming.recv() => match item {
                Ok(Incoming::Request { id, method, params }) if method == "item/tool/call" => {
                    let tool = params["tool"].as_str().unwrap_or("?").to_owned();
                    let _ = events.send(VoiceEvent::Action(Some(organizer::OrganizerAction::Tool(tool.clone())))).await;
                    let outcome = match parse_tool(&params) {
                        ToolCall::ReadSession => { let _ = events.send(VoiceEvent::ReadSession(CallId(id))).await; "read" }
                        ToolCall::Send(_) | ToolCall::Hold(_) if !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Pedido recusado: só uma fala do usuário pode gerar envio.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::FinishPlan { .. } | ToolCall::AskSession(_) | ToolCall::SetMode(_) | ToolCall::SwitchSession(_)
                            | ToolCall::OpenSession(_) | ToolCall::CloseSession { .. } | ToolCall::PairSessions(..) | ToolCall::UnpairSession(_)
                            | ToolCall::HangarAction { .. } | ToolCall::Computer(_) if !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Só a pedido falado do usuário.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::Send(_) | ToolCall::Hold(_) if !send_allowed(planner.mode) => {
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
                                let warn = apply_models(&rpc, &thread, &mut applied, &models, Mode::Direct, default_model.as_deref(), events).await;
                                let reply = format!("Plano enviado à sessão; o resultado chega depois. {}", warn.unwrap_or_default());
                                let _ = rpc.respond(id, tool_reply(reply.trim_end(), true)).await;
                                let _ = events.send(VoiceEvent::Mode(Mode::Direct)).await;
                                "plan-sent"
                            }
                        }
                        // A resposta vem da tela (`Voice::reply`), depois de resolver o nome.
                        ToolCall::SwitchSession(name) => {
                            let spoken = spoken.text(&params).unwrap_or_default().to_owned();
                            let _ = events.send(VoiceEvent::SwitchSession { call: CallId(id), name, spoken }).await;
                            "switch"
                        }
                        ToolCall::HangarActions => { let _ = events.send(VoiceEvent::HangarActions(CallId(id))).await; "hangar-actions" }
                        ToolCall::HangarAction { id: action, arg } => { let _ = events.send(VoiceEvent::HangarAction { call: CallId(id), id: action, arg }).await; "hangar-action" }
                        ToolCall::ReadScreen(area) => { let _ = events.send(VoiceEvent::ReadScreen(CallId(id), area)).await; "read-screen" }
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
                        ToolCall::SetMode(_) if let Some(name) = organizer::mode_word_session(spoken.text(&params).unwrap_or_default(), &session_names) => {
                            let reply = format!("'{name}' é o nome de uma sessão, não o modo; use switch_session para ir até ela.");
                            let _ = rpc.respond(id, tool_reply(reply, false)).await;
                            "refused-session-name"
                        }
                        ToolCall::SetMode(mode) => {
                            let mut note = switch_mode(&mut planner, mode, &target, &mut gate, &rpc, events).await;
                            if let Some(warn) = apply_models(&rpc, &thread, &mut applied, &models, mode, default_model.as_deref(), events).await { note = format!("{note} {warn}"); }
                            let _ = rpc.respond(id, tool_reply(note, true)).await;
                            "mode-set"
                        }
                        ToolCall::Send(request) => match gate.offer(id, request, Instant::now()) {
                            Err((id, why)) => { let _ = rpc.respond(id, tool_reply(why, false)).await; "refused-short" }
                            Ok(Some((old, _))) => { let _ = rpc.respond(old, tool_reply("Substituído por um pedido mais recente; nada foi enviado.", false)).await; "offered-superseded" }
                            Ok(None) => "offered",
                        },
                        ToolCall::Hold(request) => {
                            // "Segura" dentro da janela de 1,5 s cancela o envio que ainda não saiu.
                            let cancelled = gate.user_spoke().map(|(old, _)| old);
                            let outcome = if cancelled.is_some() { "held-cancelled" } else { "held" };
                            if let Some(old) = cancelled {
                                let _ = rpc.respond(old, tool_reply("Cancelado: o usuário pediu para segurar; nada foi enviado.", false)).await;
                            }
                            let _ = events.send(VoiceEvent::Draft(Some(request.clone()))).await;
                            let _ = rpc.respond(id, tool_reply(format!("Segurado, nada enviado: {request}. Envie com send_to_session quando o usuário liberar."), true)).await;
                            outcome
                        }
                        ToolCall::Discard => {
                            if let Some((old, _)) = gate.user_spoke() {
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
                    if method == "thread/realtime/transcript/delta" && params["role"] == "user" && let Some(text) = pending_context.take() {
                        match rpc.request("thread/realtime/appendText", json!({"threadId": thread, "role": "developer", "text": text.clone()})).await {
                            Ok(_) => { log("context delivered on user speech"); context_failures = 0; }
                            // Sem o contexto o organizador fala da sessão errada: volta para a próxima fala tentar de novo.
                            Err(error) => {
                                context_failures += 1;
                                log(format!("context delivery failed kind={} attempt={context_failures}", rpc_error_kind(&error)));
                                // Cada tentativa prende o laço da chamada: na segunda falha avisa e desiste.
                                if context_failures < 2 { pending_context.get_or_insert(text); }
                                else { let _ = events.send(VoiceEvent::Failed(VoiceFailure::Organizer)).await; }
                            }
                        }
                    }
                    // A transcrição da fala chega atrasada e cancelava o próprio pedido: só uma fala nova
                    // encaminhada (outro userMessage) prova que o usuário continuou.
                    let user_spoke = method == "item/started" && params["item"]["type"] == "userMessage";
                    // Só registra a fala; cancelar envio pendente continua só no started e no delta.
                    if method == "item/started" || method == "item/completed" { spoken.item_started(&params); }
                    if user_spoke && let Some((id, _)) = gate.user_spoke() {
                        log("gate cancelled: user kept talking");
                        let _ = rpc.respond(id, tool_reply("O usuário continuou falando; nada foi enviado. Monte o pedido com a fala completa.", false)).await;
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
                            if params["turn"]["status"] != "completed" && let Some((id, _)) = gate.user_spoke() {
                                log("gate cancelled: turn interrupted");
                                let _ = rpc.respond(id, tool_reply("O turno foi interrompido; nada foi enviado.", false)).await;
                            }
                            if params["turn"]["status"] == "failed" {
                                log(format!("organizer failure: {:?}", VoiceFailure::Organizer));
                                let _ = events.send(VoiceEvent::Failed(VoiceFailure::Organizer)).await;
                            }
                            if let Some(input) = results.turn_completed() { start_summary(&rpc, &thread, input, &mut results, organizer_busy).await; }
                        }
                        "item/completed" if params["item"]["type"] == "agentMessage" && params["item"]["phase"] != "commentary" => {
                            if results.take_summary(params["turnId"].as_str().unwrap_or_default())
                                && let Some(text) = params["item"]["text"].as_str() {
                                let spoke = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": text})).await;
                                log(format!("summary appendSpeech bytes={} ok={}", text.len(), spoke.is_ok()));
                            }
                        }
                        "thread/tokenUsage/updated" => if let Some((used, window)) = usage::context_usage(&params) {
                            log(format!("organizer context used={used} window={window:?}"));
                            let _ = events.send(VoiceEvent::OrganizerContext { used, window }).await;
                        },
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
            command = inbox.recv() => match command {
                Some(Command::Reply(id, reply)) => { log(format!("tool reply success={}", reply["success"])); let _ = rpc.respond(id, reply).await; }
                Some(Command::SetMode(Mode::Plan)) if target.is_empty() => {
                    log("mode plan refused: no target");
                    let _ = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": "Abra uma sessão primeiro."})).await;
                }
                Some(Command::SetMode(mode)) => {
                    log(format!("mode set {mode:?}"));
                    let mut note = switch_mode(&mut planner, mode, &target, &mut gate, &rpc, events).await;
                    if let Some(warn) = apply_models(&rpc, &thread, &mut applied, &models, mode, default_model.as_deref(), events).await { note = format!("{note} {warn}"); }
                    let _ = rpc.request("thread/realtime/appendText", json!({"threadId": thread, "role": "developer", "text": note})).await;
                    let speech = if mode == Mode::Plan { "Modo planejar." } else { "Modo direto." };
                    let _ = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": speech})).await;
                }
                Some(Command::Models(edited)) => {
                    models = edited;
                    // A falha já aparece no cartão; o modo não mudou, não há o que dizer ao organizador.
                    let _ = apply_models(&rpc, &thread, &mut applied, &models, planner.mode, default_model.as_deref(), events).await;
                }
                Some(Command::PlanDelivered) => planner.delivered(),
                Some(Command::Sessions(names)) => { log(format!("sessions known count={}", names.len())); session_names = names; }
                Some(Command::Answer(text)) => {
                    log(format!("session answer bytes={}", text.len()));
                    if let Some(input) = results.push(String::new(), text) { start_summary(&rpc, &thread, input, &mut results, organizer_busy).await; }
                }
                Some(Command::Retarget(name, context, cwd)) => {
                    log("retarget");
                    // O envio retido sairia para a sessão que está na tela agora, não para a do pedido.
                    if let Some((old, _)) = gate.user_spoke() {
                        let _ = rpc.respond(old, tool_reply("Cancelado: a sessão mudou; nada foi enviado.", false)).await;
                    }
                    target = name.clone();
                    target_cwd = cwd;
                    // Sem anúncio falado: a pessoa vê a tela. O contexto da sessão só entra quando ela voltar a falar.
                    pending_context = Some(format!("{}\n{context}", organizer::code_note(target_cwd.as_deref(), &own)));
                    context_failures = 0;
                }
                Some(Command::Result(session, text)) => {
                    log(format!("session result bytes={}", text.len()));
                    if let Some(input) = results.push(session, text) { start_summary(&rpc, &thread, input, &mut results, organizer_busy).await; }
                }
                None => break Ok(()),
            },
            // Acorda para soltar o envio que assentou.
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    };
    stopped.store(true, Ordering::Relaxed);
    let _ = rpc.request("thread/realtime/stop", json!({"threadId": thread})).await;
    if !matches!(tokio::task::spawn_blocking(move || peer.join()).await, Ok(Ok(()))) { log("rtc thread join failed (panic)"); }
    outcome
}

async fn send_limits(events: &async_channel::Sender<VoiceEvent>, limits: Option<(usage::RateWindow, usage::RateWindow)>) {
    let Some((five_hour, seven_day)) = limits else { return };
    log(format!("account limits five_hour={} seven_day={}", five_hour.is_some(), seven_day.is_some()));
    let _ = events.send(VoiceEvent::AccountLimits { five_hour, seven_day }).await;
}

/// Entrar no Planejar cancela o envio que esperava a janela: a chamada pendente falha em vez de ficar sem resposta.
async fn switch_mode(planner: &mut Planner, mode: Mode, target: &str, gate: &mut SendGate<Value>, rpc: &Rpc,
    events: &async_channel::Sender<VoiceEvent>) -> String {
    if mode == Mode::Plan && let Some((old, _)) = gate.user_spoke() {
        let _ = rpc.respond(old, tool_reply("Cancelado: modo Planejar; nada foi enviado.", false)).await;
    }
    let note = planner.set_mode(mode, target);
    let _ = events.send(VoiceEvent::Mode(mode)).await;
    note
}

/// Leva a thread ao par do modo; vale a partir do próximo turno. Falha aparece na tela e o texto devolvido avisa o organizador.
async fn apply_models(rpc: &Rpc, thread: &str, applied: &mut ModeModel, models: &ModeModels, mode: Mode, default_model: Option<&str>,
    events: &async_channel::Sender<VoiceEvent>) -> Option<&'static str> {
    let wanted = models.get(mode);
    // Voltar ao "modelo do config" sem saber qual é ele não troca nada: dizer, em vez de fingir que trocou.
    if wanted.model.is_none() && default_model.is_none() && applied.model.is_some() {
        log(format!("settings update skipped mode={mode:?} reason=unknown_default_model"));
        let _ = events.send(VoiceEvent::Failed(VoiceFailure::ModelSwitch)).await;
        return Some("O modo mudou, mas o modelo que pensa não trocou; segue o anterior.");
    }
    let Some(update) = settings_update(thread, applied, wanted, default_model) else { *applied = wanted.clone(); return None };
    if rpc.request("thread/settings/update", update).await.is_ok() {
        log(format!("settings update ok mode={mode:?}"));
        *applied = wanted.clone();
        return None;
    }
    log(format!("settings update failed mode={mode:?}"));
    let _ = events.send(VoiceEvent::Failed(VoiceFailure::ModelSwitch)).await;
    Some("O modo mudou, mas o modelo que pensa não trocou; segue o anterior.")
}

async fn start_summary(rpc: &Rpc, thread: &str, first: Value, results: &mut Results, organizer_busy: bool) {
    let mut next = Some(first);
    // Laço, não recursão: um resumo recusado com o organizador ocioso solta o próximo da fila na hora.
    while let Some(mut input) = next.take() {
        input["threadId"] = json!(thread);
        match rpc.request("turn/start", input).await {
            Ok(result) => { if let Some(turn) = result["turn"]["id"].as_str() { results.mark_summary(turn.to_owned()); } }
            Err(error) => {
                log(format!("summary turn/start failed: {error:?} organizer_busy={organizer_busy}"));
                // Ocioso e recusado: nenhum turn/completed virá; fala o começo do texto e drena a fila.
                if let Some(text) = results.turn_start_failed(organizer_busy) {
                    let short: String = text.chars().take(400).collect();
                    let _ = rpc.request("thread/realtime/appendSpeech", json!({"threadId": thread, "text": format!("A sessão respondeu: {short}")})).await;
                    next = results.turn_completed();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

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
}
