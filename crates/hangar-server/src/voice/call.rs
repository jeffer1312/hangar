//! A chamada de voz: o organizador e a conversa falada rodam aqui; o WebRTC é do aparelho, que manda a oferta e o nível do
//! microfone e recebe a resposta SDP.
use super::{log, observe, organizer, usage};
use super::organizer::{Effective, FinishStep, MIC_VOICE_LEVEL, Mode, ModeModel, ModeModels, Planner, REPEAT_NOTE, Results, SpeechHold,
    ConfirmGate, SendGate, SpokenTurns, ToolCall, finish_request, parse_tool, send_allowed, settings_update, spoken_input, tool_reply,
    organizer_start};
use super::rpc::{Incoming, Rpc, RpcError, handshake};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::sync::{Notify, mpsc};

pub struct CallId(pub Value);
pub enum Phase { Connecting, Live, Closed }
#[derive(Debug, Clone)]
pub enum VoiceFailure { Microphone, Speaker, AppServer, Realtime(String), Network, Timeout, Organizer, ModelSwitch, OwnFolder, AudioStopped, Closed,
    /// A conversa falada caiu sem a chamada acabar: espera uma oferta nova.
    AudioLost,
    /// Nenhuma oferta nova veio no prazo depois da queda do áudio: a chamada acabou.
    AudioLostEnded }
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Activity { #[default] Idle, Thinking, Searching, Working }
pub enum VoiceEvent {
    Phase(Phase), Draft(Option<String>), Activity(Activity), Failed(VoiceFailure),
    /// `session`: nome falado (sem ele, a da tela); `user_turns`: de qual pedido do usuário começar, contando do fim.
    ReadSession { call: CallId, session: Option<String>, user_turns: usize },
    /// Resposta SDP à oferta `seq` (a primeira e a de cada passagem).
    Answer(u64, String),
    /// `turn`: a fala que pediu. A tela não manda duas vezes à mesma sessão no mesmo turno.
    Send { call: CallId, request: String, session: Option<String>, turn: String },
    Mode(Mode), Plan { path: PathBuf, markdown: String }, AskSession(String), SendPlan { session: String, text: String },
    /// Nome pedido pelo organizador num turno com fala do usuário; nome ambíguo volta com as opções.
    SwitchSession { call: CallId, name: String },
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
    /// Arquivo, pastas, atalhos, máquinas e interromper; `turn` separa o pedido do sim.
    Assist { call: CallId, assist: organizer::Assist, turn: String },
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
    /// `remember`/`forget` gravaram: o painel "O que a voz lembra" aberto relê a lista.
    MemoryChanged,
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
    pub tools: Value, pub handoff_same_thread: bool, pub voice_dir: PathBuf,
    /// Quanto a chamada espera uma oferta nova depois que o áudio cai: o mesmo prazo do hub para o aparelho que sai.
    pub audio_grace: Duration,
    /// Quanto um comando do organizador pode rodar antes de o turno ser interrompido (`COMMAND_LIMIT` em produção).
    pub command_limit: Duration,
    /// Entrada do MCP `hangar` com a identidade desta chamada; `None` deixa o MCP desligado.
    pub hangar_mcp: Option<Value>,
    /// Silêncio contínuo que encerra a fala dele para os resultados das sessões (`RESULT_SILENCE` em produção).
    pub result_silence: Duration,
    pub style: super::settings::Style }

/// Sobe o app-server: o filho de verdade em produção, um falso nos testes.
pub type Spawn = Box<dyn FnOnce() -> futures_util::future::BoxFuture<'static, Result<(Rpc, async_channel::Receiver<Incoming>), RpcError>> + Send>;

enum Command { Retarget(String, String, Option<PathBuf>), Result(String, String), Listen(String, bool), Heard(String, String), PlanFailed(String), Reply(Value, Value), SetMode(Mode), Models(ModeModels), Answer(String), PlanDelivered,
    Sessions(Vec<String>), Jev { turn: String, speech: Option<String>, note: Option<String> },
    /// Número da oferta, SDP e a nota da sessão na tela do aparelho que ofereceu.
    Offer(u64, String, String), Live, Level(f32), Detached }

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
    /// A resposta volta com o mesmo `seq`.
    pub fn offer(&self, seq: u64, sdp: String, context: String) { let _ = self.commands.send(Command::Offer(seq, sdp, context)); }
    /// O aparelho conectou o áudio.
    pub fn live(&self) { let _ = self.commands.send(Command::Live); }
    /// Nível do microfone do aparelho: segura o envio e a fala puxada enquanto o usuário fala.
    pub fn level(&self, input: f32) { let _ = self.commands.send(Command::Level(input)); }
    /// O aparelho dono caiu: a conversa falada pode fechar sem encerrar a chamada.
    pub fn detached(&self) { let _ = self.commands.send(Command::Detached); }
    /// `cwd`: pasta da nova sessão neste disco (`None` se for de outra máquina).
    pub fn retarget(&self, name: String, context: String, cwd: Option<PathBuf>) { let _ = self.commands.send(Command::Retarget(name, context, cwd)); }
    pub fn session_result(&self, session: String, text: String) { let _ = self.commands.send(Command::Result(session, text)); }
    /// Ele pediu (ou deixou de pedir) os resultados desta sessão.
    pub fn listen(&self, session: String, on: bool) { let _ = self.commands.send(Command::Listen(session, on)); }
    /// Ele ouviu a conversa desta sessão: o que dela estava guardado e aparece na leitura deixa a fila.
    pub fn heard(&self, session: String, read: String) { let _ = self.commands.send(Command::Heard(session, read)); }
    pub fn reply(&self, call: CallId, reply: Value) { let _ = self.commands.send(Command::Reply(call.0, reply)); }
    pub fn set_mode(&self, mode: Mode) { let _ = self.commands.send(Command::SetMode(mode)); }
    /// Pares editados no cartão durante a chamada: o do modo atual vale já no próximo turno.
    pub fn set_models(&self, models: ModeModels) { let _ = self.commands.send(Command::Models(models)); }
    /// Resposta, recusa ou estouro de um `ask_session`: chega ao organizador como turno marcado.
    pub fn session_answer(&self, text: String) { let _ = self.commands.send(Command::Answer(text)); }
    /// A sessão aceitou o plano enviado: o organizador o esquece.
    pub fn plan_delivered(&self) { let _ = self.commands.send(Command::PlanDelivered); }
    /// O plano não chegou à sessão; `why` vai ao organizador.
    pub fn plan_failed(&self, why: String) { let _ = self.commands.send(Command::PlanFailed(why)); }
    /// Nomes das sessões que a busca enxerga: o `set_mode` recusa quando a fala cita uma delas.
    pub fn set_sessions(&self, names: Vec<String>) { let _ = self.commands.send(Command::Sessions(names)); }
    /// A tela já agiu pelo Jev (trocou, abriu): `speech` a voz fala na hora e `note` diz ao organizador que não repita nem fale.
    pub fn jev_verdict(&self, turn: String, speech: Option<String>, note: Option<String>) {
        let _ = self.commands.send(Command::Jev { turn, speech, note });
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

/// Comando que espera (`--watch`, `tail -f`) prende o turno do organizador e a voz fica sem resposta: passou disto, interrompe.
pub const COMMAND_LIMIT: Duration = Duration::from_secs(60);
const INTERRUPT_WAIT: Duration = Duration::from_secs(3);
/// Final de fala depois disso sem pedaço novo é a de uma fala já encerrada.
const LATE_FINAL: Duration = Duration::from_millis(1500);
const OFFER_ACCEPTED_NOTE: &str = "[NOTA DO HANGAR] Ele disse sim ao aviso e o Hangar já soltou os resultados guardados: não chame quiet; \
    resuma este resultado agora, como manda a regra do [RESULTADO DA SESSÃO]. Os outros chegam depois, um por vez.";
const COMMAND_LIMIT_NOTE: &str = "O comando demorou mais de 60 s e foi interrompido; não use comando que espera — consulte uma vez e responda.";
const COMMAND_LIMIT_SPEECH: &str = "Parei o comando porque estava demorando demais.";
/// Interrupção recusada: nova tentativa enquanto o comando seguir rodando.
const COMMAND_RETRY: Duration = Duration::from_secs(5);

/// Quanto uma oferta espera a resposta SDP.
const ANSWER_WAIT: Duration = Duration::from_secs(20);

/// A passagem em curso acabou (respondida, falhou ou estourou): a oferta que esperava vai ao laço.
fn next_offer(answering: &mut Option<Instant>, waiting: &mut Option<(u64, String, String)>, early: &mut VecDeque<Command>) {
    *answering = None;
    if let Some((seq, sdp, context)) = waiting.take() { early.push_front(Command::Offer(seq, sdp, context)); }
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
    // Sem a regra, o comando que o Codex julga perigoso volta recusado ao organizador, que diz isso em voz.
    if let Err(error) = organizer::write_voice_rules(&own) { log(format!("voice rules write failed kind={:?}", error.kind())); }
    let available = super::rpc::account_models(&rpc).await;
    if available.is_none() { log("account model list unavailable; chosen models kept"); }
    let mut models = options.organizer.clone();
    for model in models.fit(available.as_deref()) { log(format!("model {model} not in the account; using the account default")); }
    let mut applied = models.direct.clone();
    let memory_file = super::memory::memory_path(&options.voice_dir);
    // Ilegível: abre sem memória e sem convite de perfil; a tela mostra o erro. Vazia (sem arquivo ou sem entrada): a voz
    // convida ao conectar, porque o organizador só fala depois de a pessoa falar.
    let today = chrono::Local::now().date_naive();
    // Quem fala também parafraseia: sem as preferências, ela resumia e enchia a espera apesar do que ele pediu.
    let (note, onboarding, voice_prefs) = match super::memory::load(&memory_file) {
        Some(entries) => {
            let prefs: Vec<_> = entries.iter().filter(|e| e.kind == super::memory::Kind::Preferencia).cloned().collect();
            (organizer::memory_note(&super::memory::prompt_block(&entries, today), !entries.is_empty()), entries.is_empty(),
                super::memory::prompt_block(&prefs, today))
        }
        None => { log("memory unreadable"); (organizer::memory_note("", true), false, String::new()) }
    };
    log(format!("memory onboarding={onboarding} style={:?}", options.style));
    // O estilo vem antes da memória: uma preferência guardada por ele vale sobre o estilo escolhido no cartão.
    let note = format!("{}\n\n{note}", organizer::organizer_style(options.style));
    let start = organizer_start(&config, &own, options.cwd.as_deref(), &options.context, &applied, options.tools.clone(), &note,
        options.hangar_mcp.as_ref());
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
    // Aparelho que caiu antes de oferecer (celular travado) tira o prazo: quem limita a espera passa a ser o do hub.
    let mut offer_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(45));
    let (offer_seq, offer_sdp) = loop {
        let command = match offer_deadline {
            Some(at) => tokio::time::timeout_at(at, inbox.recv()).await.map_err(|_| failed("device offer")(VoiceFailure::Timeout))?,
            None => inbox.recv().await,
        };
        match command {
            Some(Command::Offer(seq, sdp, _)) => break (seq, sdp),
            Some(Command::Detached) => offer_deadline = None,
            Some(Command::Live | Command::Level(_)) => {}
            Some(other) => early.push_back(other),
            None => return Err(failed("device offer")(VoiceFailure::Closed)),
        }
    };
    let mut realtime = json!({"threadId": thread, "version": "v3", "outputModality": "audio", "prompt": organizer::voice_prompt(onboarding, &options.tools, options.hangar_mcp.is_some(), &voice_prefs, options.style),
        // Sem aviso de espera a cada delegação ('um segundo'): ele pediu silêncio. O Codex não repassa a resposta do organizador
        // à voz: repassada na hora, ela saía por cima da fala dele e cortada em ~1.000 tokens; quem fala é o `SpeechHold`.
        "includeStartupContext": false, "delegationAckFiller": false, "backendReasoningStatus": false, "clientManagedHandoffs": true,
        "realtimeStartInstructions": organizer::REALTIME_START, "initialItems": [{"role": "developer", "text": options.context}]});
    if let Some(voice) = &options.voice { realtime["voice"] = json!(voice); }
    let realtime_start = realtime.clone();
    realtime["transport"] = json!({"type": "webrtc", "sdp": &offer_sdp});
    rpc.request("thread/realtime/start", realtime).await.map_err(rpc_failure).map_err(failed("thread/realtime/start"))?;
    log(format!("realtime/start sent offer_bytes={}", offer_sdp.len()));

    // A resposta SDP chega como notificação; até lá, só ela interessa.
    let mut last_delta = None;
    let answer = tokio::time::timeout(ANSWER_WAIT, async {
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
    let _ = events.send(VoiceEvent::Answer(offer_seq, answer)).await;

    let mut greeted = false;
    // Uma passagem por vez: a resposta SDP não diz de qual oferta é, então a próxima espera a anterior responder,
    // falhar ou estourar o prazo.
    // Áudio caído sem aparelho que ofereça de novo: o erro pode não ter derrubado o WebRTC, e aí nada mais acabaria a chamada.
    let mut audio_lost: Option<Instant> = None;
    let audio_grace = options.audio_grace;
    let command_limit = options.command_limit;
    let mut last_offer = offer_seq;
    let mut answering: Option<Instant> = None;
    let mut waiting_offer: Option<(u64, String, String)> = None;
    // `restarting`: passagem para outro aparelho em curso; `detached`: o dono caiu. Nos dois, o fim da conversa falada não
    // encerra a chamada: a thread do organizador espera a próxima oferta.
    let mut restarting = false;
    let mut detached = false;
    let mut results = Results::default();
    results.set_screen(&options.target);
    // Última voz dele (microfone ou transcrição) e se a conversa estava em curso na volta anterior do laço: resultado só
    // abre resumo depois de `result_silence` contínuo, e a fala que volta corta o resumo ainda sem mensagem.
    let result_silence = options.result_silence;
    let mut last_user_voice: Option<Instant> = None;
    // Último pedaço de fala dele: a final só conta como fala se veio logo depois de um.
    let mut last_user_delta: Option<Instant> = None;
    let mut was_talking = false;
    // O aviso de resultados guardados, para o organizador saber o que o "quero ouvir" responde.
    let mut offer_note: Option<String> = None;
    // Turno de resumo que a fala dele cortou: o fim dele não é falha de turno de fala.
    let mut cut_summary: Option<String> = None;
    // Preferências gravadas nesta chamada, que o prompt da abertura não tem.
    let mut new_prefs: Vec<String> = Vec::new();
    // Cada pedido leva o destino falado (vazio = a sessão da tela) e o turno da fala que o pediu.
    let mut gate: SendGate<(Value, Option<String>, String)> = SendGate::default();
    // O plano confirmado espera o silêncio como o envio direto; a resposta do finish_plan só sai com a entrega real.
    let mut plan_gate: SendGate<(Value, String)> = SendGate::default();
    let mut plan_call: Option<(Value, Option<String>)> = None;
    let mut organizer_busy = false;
    let mut spoken = SpokenTurns::default();
    let mut activity = Activity::Idle;
    let mut planner = Planner::new(options.voice_dir.join("planos"));
    let mut target = options.target.clone();
    let mut target_cwd = options.cwd.clone();
    let mut pending_context: Option<String> = None;
    // Notas ao organizador que a troca de sessão não pode descartar (fala perdida na queda, resto de leitura cortada).
    let mut carry: Vec<String> = Vec::new();
    let mut context_failures = 0u32;
    let mut session_names: Vec<String> = Vec::new();
    // Comando destrutivo do organizador recusado à espera do sim falado.
    let mut command_gate: ConfirmGate<String> = ConfirmGate::default();
    // Última fala repassada (para reconhecer a repetida) e os itens de fala já tratados.
    let mut last_input: Option<(String, Instant, Option<String>)> = None;
    // Por turno, quantas ferramentas que agem saíram dele sem voltar falha; e o turno de cada uma à espera da resposta.
    // Conta na saída porque parte delas responde na hora, sem passar pela resposta assíncrona.
    let mut acted: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut acting_calls: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut seen_items: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Turno da última fala encaminhada e o que o usuário falou depois dela sem nova delegação: o envio desse turno não sai
    // sem essa continuação.
    let mut delegated: Option<(String, Instant)> = None;
    let mut continuation: Vec<String> = Vec::new();
    // A continuação passou do teto e perdeu o começo: o envio volta pedindo confirmação, não sai com o pedido cortado.
    let mut continuation_cut = false;
    // Turnos abertos por uma fala repetida: ela já foi tratada no turno original, então nada que age sai deles.
    // Valor: o turno original, quando conhecido.
    let mut repeated: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    // Turno cuja fala seguiu em transcrição depois da delegação, e desde quando: o envio dele espera a final.
    let mut speaking: Option<(String, Instant)> = None;
    let mut out = Speech::default();
    let mut ack = organizer::WaitAck::default();
    let mut transcript = super::transcript::Transcript::open(&options.voice_dir, chrono::Local::now());
    // Comandos do organizador em curso, por item: turno e prazo (um turno pode rodar mais de um).
    let mut running_commands: std::collections::HashMap<String, (String, Instant)> = std::collections::HashMap::new();
    let outcome = loop {
        if audio_lost.is_some_and(|at| Instant::now() >= at) {
            break Err(failed("audio lost: no new offer")(VoiceFailure::AudioLostEnded));
        }
        if answering.is_some_and(|at| Instant::now() >= at) {
            log("handoff sdp answer timed out");
            next_offer(&mut answering, &mut waiting_offer, &mut early);
        }
        let now = Instant::now();
        if let Some(turn) = running_commands.values().find(|(_, at)| now >= *at).map(|(turn, _)| turn.clone()) {
            // Só o código: o texto do comando é conversa e não vai ao diário.
            // Prazo curto: esperar aqui para o laço inteiro, e recusa só adia a nova tentativa.
            let interrupted = tokio::time::timeout(INTERRUPT_WAIT,
                rpc.request("turn/interrupt", json!({"threadId": thread, "turnId": turn}))).await.is_ok_and(|r| r.is_ok());
            log(format!("command too long: turn interrupted ok={interrupted}"));
            if interrupted {
                running_commands.retain(|_, (t, _)| *t != turn);
                pending_context = Some(match pending_context.take() { Some(old) => format!("{old}\n{COMMAND_LIMIT_NOTE}"), None => COMMAND_LIMIT_NOTE.to_owned() });
                speak(&rpc, &thread, &mut out, COMMAND_LIMIT_SPEECH.to_owned(), "command limit").await;
            } else {
                // Recusado (o turno pode ter acabado agora): sem dizer que parou; tenta de novo se ainda estiver rodando.
                for (t, at) in running_commands.values_mut() { if *t == turn { *at = now + COMMAND_RETRY; } }
            }
        }
        if !out.away && let Some(text) = out.hold.due(Instant::now()) { append_speech(&rpc, &thread, &text, "held").await; }
        // Conversa em curso: fala dele dentro da janela, turno do organizador para a fala dele, ou nenhum aparelho ouvindo.
        // Só a transcrição conta como fala: ruído no microfone seguraria os avisos para sempre.
        // O turno da fala dele conta com teto: sem o `turn/completed` dele, os resultados ficariam presos para sempre.
        let talking = out.away || delegated.as_ref().is_some_and(|(_, at)| at.elapsed() < command_limit * 2)
            || last_user_voice.is_some_and(|at| at.elapsed() < result_silence);
        if talking != was_talking {
            was_talking = talking;
            if talking {
                // O Codex repassa à voz a mensagem do resumo assim que ela fica pronta, sem esperar ninguém: cortar o turno
                // antes é o único jeito de ela não tomar a vez dele.
                // A delegação que chega com o turno aberto entra nele (`StartOrSteer`): com a fala dele dentro, não corta.
                if let Some(turn) = results.running_summary().filter(|t| delegated.as_ref().is_none_or(|(d, _)| d != t)) {
                    let cut = tokio::time::timeout(INTERRUPT_WAIT,
                        rpc.request("turn/interrupt", json!({"threadId": thread, "turnId": turn}))).await.is_ok_and(|r| r.is_ok());
                    log(format!("summary cut by user speech ok={cut}"));
                    if cut { results.take_summary(&turn); results.interrupted(); cut_summary = Some(turn); }
                }
                results.set_user_talking(true);
            } else if let Some(input) = results.set_user_talking(false) {
                start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await;
            }
        }
        if let Some(offer) = results.take_offer() {
            log(format!("results offered held={}", results.held()));
            offer_note = Some(format!("A voz acabou de avisar: \"{offer}\" Se ele quiser ouvir, chame quiet com on false; se não \
                quiser agora, não chame nada: os resultados continuam guardados."));
            speak(&rpc, &thread, &mut out, offer, "results offer").await;
        }
        // No Planejar nada sai pelo gate; ao entrar nele o envio pendente já foi cancelado.
        let held = speaking.as_ref().filter(|(_, since)| since.elapsed() < organizer::STILL_SPEAKING_CAP).map(|(t, _)| t.clone());
        if planner.mode == Mode::Direct
            && let Some(((id, session, turn), request)) = gate.due_except(Instant::now(), |(_, _, t)| held.as_ref() == Some(t)) {
            log(format!("gate sent words={} named={}", request.split_whitespace().count(), session.is_some()));
            let _ = events.send(VoiceEvent::Draft(None)).await;
            let _ = events.send(VoiceEvent::Send { call: CallId(id), request, session, turn }).await;
        }
        if let Some(text) = ack.due(Instant::now(), last_user_voice) {
            if out.away { log("wait ack skipped: away"); } else { speak(&rpc, &thread, &mut out, text.to_owned(), "wait ack").await; }
        }
        if plan_call.is_none() && let Some(((id, session), text)) = plan_gate.due(Instant::now()) {
            log(format!("plan sent bytes={}", text.len()));
            let _ = events.send(VoiceEvent::SendPlan { session, text }).await;
            planner.sent();
            let warn = apply_models(&rpc, &thread, &mut applied, &models, Mode::Direct, &defaults, &mut effective, events).await;
            plan_call = Some((id, warn.map(str::to_owned)));
            let _ = events.send(VoiceEvent::Mode(Mode::Direct)).await;
        }
        // Comando parado não manda evento nenhum: sem este despertar, o prazo dele nunca seria conferido.
        let command_due = running_commands.values().map(|(_, at)| *at).min();
        let ack_due = ack.next_check(last_user_voice);
        tokio::select! {
            // Comandos antes das notificações: a passagem ou a queda do aparelho já valem quando chega o fim da conversa anterior.
            biased;
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(command_due.unwrap_or_else(Instant::now))), if command_due.is_some() => {}
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(ack_due.unwrap_or_else(Instant::now))), if ack_due.is_some() => {}
            command = async { match early.pop_front() { Some(command) => Some(command), None => inbox.recv().await } } => match command {
                Some(Command::Live) => {
                    log("phase Live");
                    let _ = events.send(VoiceEvent::Phase(Phase::Live)).await;
                    restarting = false;
                    out.away = detached;
                    // A voz V3 nunca fala primeiro: sem isto a pessoa não tem prova de que o alto-falante funciona.
                    if !greeted {
                        greeted = true;
                        let spoke = rpc.request("thread/realtime/appendSpeech",
                            json!({"threadId": thread, "text": organizer::greeting(onboarding)})).await;
                        log(format!("greeting appendSpeech ok={}", spoke.is_ok()));
                    }
                    // O que a voz ia falar durante a troca sai agora, na ordem; ela nunca fala primeiro, então se perderia.
                    if !out.away {
                        for text in std::mem::take(&mut out.parked) { speak(&rpc, &thread, &mut out, text, "parked").await; }
                    }
                }
                Some(Command::Level(input)) => if input >= MIC_VOICE_LEVEL {
                    gate.heard_voice(Instant::now()); plan_gate.heard_voice(Instant::now()); out.hold.heard_voice(Instant::now());
                },
                Some(Command::Detached) => { log("device detached"); detached = true; out.away = true; }
                Some(Command::Offer(seq, sdp, context)) if answering.is_some() => {
                    // A mais nova substitui a que esperava: a resposta desta é a única que o dono atual aplica.
                    log("handoff offer waits for the previous answer");
                    audio_lost = None;
                    waiting_offer = Some((seq, sdp, context));
                }
                Some(Command::Offer(seq, sdp, context)) => {
                    // Outro aparelho assumiu (ou o mesmo voltou): a conversa falada recomeça na mesma thread, com o histórico dela.
                    log(format!("handoff offer bytes={}", sdp.len()));
                    (last_offer, answering, audio_lost) = (seq, Some(Instant::now() + ANSWER_WAIT), None);
                    restarting = true;
                    detached = false;
                    out.away = true;
                    let mut again = realtime_start.clone();
                    again["transport"] = json!({"type": "webrtc", "sdp": sdp});
                    again["includeStartupContext"] = json!(true);
                    // A conversa falada recomeça com o prompt da abertura: as preferências gravadas depois vão junto.
                    let mut items = vec![json!({"role": "developer", "text": context})];
                    items.extend(new_prefs.iter().map(|text| json!({"role": "developer", "text": text})));
                    // A voz nova não sabe nada do que foi falado, e a fala que ela não repassou nunca chegou ao organizador.
                    let recap = transcript.recap();
                    log(format!("handoff recap lines={} unforwarded={}", recap.lines, recap.organizer.is_some()));
                    if let Some(text) = recap.voice { items.push(json!({"role": "developer", "text": text})); }
                    if let Some(note) = recap.organizer { carry.push(note); }
                    again["initialItems"] = json!(items);
                    if let Err(error) = rpc.request("thread/realtime/start", again).await {
                        log(format!("handoff realtime/start failed kind={}", rpc_error_kind(&error)));
                        break Err(VoiceFailure::Realtime(String::new()));
                    }
                }
                Some(Command::Reply(id, reply)) => {
                    log(format!("tool reply success={}", reply["success"]));
                    if let Some(turn) = acting_calls.remove(&id.to_string()) && reply["success"] != true
                        && let Some(count) = acted.get_mut(&turn) { *count = count.saturating_sub(1); }
                    let _ = rpc.respond(id, reply).await;
                }
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
                    for model in models.fit(available.as_deref()) { log(format!("model {model} not in the account; using the account default")); }
                    // A falha já aparece no cartão; o modo não mudou, não há o que dizer ao organizador.
                    let _ = apply_models(&rpc, &thread, &mut applied, &models, planner.mode, &defaults, &mut effective, events).await;
                }
                Some(Command::PlanDelivered) => {
                    planner.delivered();
                    if let Some((id, warn)) = plan_call.take() {
                        let reply = format!("Plano entregue à sessão; o resultado chega depois. {}", warn.unwrap_or_default());
                        let _ = rpc.respond(id, tool_reply(reply.trim_end(), true)).await;
                    }
                }
                Some(Command::PlanFailed(why)) => match plan_call.take() {
                    Some((id, _)) => {
                        let reply = format!("O plano não chegou à sessão: {why} Ele continua guardado; para reenviar, chame finish_plan de novo.");
                        let _ = rpc.respond(id, tool_reply(reply, false)).await;
                    }
                    None => if let Some(input) = results.push(String::new(), format!("O plano não chegou à sessão: {why}")) {
                        start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await;
                    },
                },
                Some(Command::Jev { turn, speech, note }) => {
                    log(format!("jev acted note={} speech={}", note.is_some(), speech.is_some()));
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
                    log(format!("session answer bytes={} quiet={}", text.len(), results.quiet()));
                    if let Some(input) = results.push(String::new(), text) { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                }
                Some(Command::Retarget(name, context, cwd)) => {
                    log("retarget");
                    // O envio retido sem destino sairia para a sessão que está na tela agora; o nomeado segue para a dele.
                    for ((old, ..), _) in gate.take_for("") {
                        let _ = rpc.respond(old, tool_reply("Cancelado: a sessão mudou; nada foi enviado.", false)).await;
                    }
                    target = name.clone();
                    results.set_screen(&name);
                    target_cwd = cwd;
                    // Sem anúncio falado: a pessoa vê a tela. O contexto da sessão só entra quando ela voltar a falar; a troca
                    // de sessão substitui a nota da anterior, mas não a do modo.
                    let note = format!("{}\n{context}", organizer::code_note(target_cwd.as_deref(), &own));
                    pending_context = Some(match pending_context.take().filter(|old| old.starts_with("Modo ")) { Some(old) => format!("{old}\n{note}"), None => note });
                    context_failures = 0;
                }
                Some(Command::Result(session, text)) => {
                    log(format!("session result bytes={} quiet={}", text.len(), results.quiet()));
                    if let Some(input) = results.push(session, text) { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                }
                Some(Command::Listen(session, on)) => results.listen(&session, on),
                Some(Command::Heard(session, read)) => results.heard(&session, &read),
                None => break Ok(()),
            },
            item = incoming.recv() => match item {
                Ok(Incoming::Request { id, method, params }) if method == "item/tool/call" => {
                    let tool = params["tool"].as_str().unwrap_or("?").to_owned();
                    let _ = events.send(VoiceEvent::Action(Some(organizer::OrganizerAction::Tool(tool.clone())))).await;
                    let parsed = parse_tool(&params);
                    // Turno de fala repetida só não age se a original agiu: a original que acabou sem fazer nada deixaria o
                    // pedido perdido (o modelo ouvia "já tratada" e desistia).
                    let repeat_blocked = params["turnId"].as_str().and_then(|t| repeated.get(t))
                        .is_some_and(|original| original.as_ref().is_none_or(|o| acted.get(o).is_some_and(|n| *n > 0)));
                    let acting = parsed.acts().then(|| (id.to_string(), params["turnId"].as_str().map(str::to_owned)));
                    let outcome = match parsed {
                        ToolCall::ReadSession { session, user_turns } => {
                            let _ = events.send(VoiceEvent::ReadSession { call: CallId(id), session, user_turns }).await;
                            "read"
                        }
                        ToolCall::Send { .. } | ToolCall::Hold(_) if !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Pedido recusado: só uma fala do usuário pode gerar envio.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::FinishPlan { .. } | ToolCall::AskSession(_) | ToolCall::SetMode(_) | ToolCall::SwitchSession(_)
                            | ToolCall::OpenSession(_) | ToolCall::CloseSession { .. } | ToolCall::PairSessions(..) | ToolCall::UnpairSession(_)
                            | ToolCall::HangarAction { .. } | ToolCall::Computer(_) | ToolCall::ClickScreen { .. }
                            | ToolCall::Remember { .. } | ToolCall::Forget(_) | ToolCall::Quiet(_)
                            if !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Só a pedido falado do usuário.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::Assist(assist) if assist.acts() && !spoken.allows(&params) => {
                            let _ = rpc.respond(id, tool_reply("Só a pedido falado do usuário.", false)).await;
                            "refused-not-spoken"
                        }
                        ToolCall::Assist(assist) if assist.acts() && repeat_blocked => {
                            let _ = rpc.respond(id, tool_reply("Fala repetida; já tratada.", false)).await;
                            "refused-repeated"
                        }
                        ToolCall::Send { .. } | ToolCall::FinishPlan { .. } | ToolCall::AskSession(_) | ToolCall::SwitchSession(_)
                            | ToolCall::OpenSession(_) | ToolCall::CloseSession { .. } | ToolCall::PairSessions(..) | ToolCall::UnpairSession(_)
                            | ToolCall::HangarAction { .. } | ToolCall::Computer(_) | ToolCall::ClickScreen { .. }
                            | ToolCall::Remember { .. } | ToolCall::Forget(_)
                            if repeat_blocked => {
                            let _ = rpc.respond(id, tool_reply("Fala repetida; já tratada.", false)).await;
                            "refused-repeated"
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
                        ToolCall::FinishPlan { action, session } => {
                            let destination = planner.destination(session.as_deref(), &target);
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
                            } else if planner.finish_step(action, params["turnId"].as_str().unwrap_or_default(), &destination) == FinishStep::Arm {
                                let _ = rpc.respond(id, tool_reply(organizer::finish_arm_note(&destination, &target), true)).await;
                                "finish-armed"
                            } else {
                                let path = planner.path().map(|p| p.to_path_buf()).unwrap_or_default();
                                // Sessão de outra máquina não lê o arquivo daqui, e um destino fora da tela pode ser de outra
                                // máquina: nos dois casos o conteúdo vai junto.
                                let away = target_cwd.is_none() || organizer::squash(&destination) != organizer::squash(&target);
                                let text = finish_request(&path, action, away.then_some(content.as_str()));
                                // Um plano por vez: o confirmado agora substitui o que ainda esperava, e esse recebe resposta.
                                for ((old, _), _) in plan_gate.user_spoke() {
                                    let _ = rpc.respond(old, tool_reply("Substituído por uma confirmação mais nova; este não foi enviado.", false)).await;
                                }
                                if let Some((old, _)) = plan_call.take() {
                                    let _ = rpc.respond(old, tool_reply("O envio anterior do plano segue em curso; o resultado dele não será confirmado aqui.", true)).await;
                                }
                                match plan_gate.offer((id.clone(), destination.clone()), &destination, text, Instant::now()) {
                                    Ok(_) => "plan-held",
                                    Err(((id, _), why)) => { let _ = rpc.respond(id, tool_reply(why, false)).await; "refused-plan" }
                                }
                            }
                        }
                        // A resposta vem da tela (`Voice::reply`), depois de resolver o nome.
                        ToolCall::SwitchSession(name) => { let _ = events.send(VoiceEvent::SwitchSession { call: CallId(id), name }).await; "switch" }
                        ToolCall::HangarActions => { let _ = events.send(VoiceEvent::HangarActions(CallId(id))).await; "hangar-actions" }
                        ToolCall::HangarAction { id: action, arg } => { let _ = events.send(VoiceEvent::HangarAction { call: CallId(id), id: action, arg }).await; "hangar-action" }
                        ToolCall::ReadScreen(area) => { let _ = events.send(VoiceEvent::ReadScreen(CallId(id), area)).await; "read-screen" }
                        ToolCall::ClickScreen { id: target, confirmed } => {
                            let turn = params["turnId"].as_str().unwrap_or_default().to_owned();
                            let _ = events.send(VoiceEvent::ClickScreen { call: CallId(id), id: target, confirmed, turn }).await;
                            "click-screen"
                        }
                        ToolCall::Observe(request) => { let _ = events.send(VoiceEvent::Observe(CallId(id), request)).await; "observe" }
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
                        ToolCall::Remember { kind, text, replaces } => {
                            let today = chrono::Local::now().date_naive();
                            // I/O de arquivo fora do worker da chamada (a trava é compartilhada com a rota DELETE).
                            let (file, said) = (memory_file.clone(), text.clone());
                            let done = tokio::task::spawn_blocking(move || super::memory::remember(&file, kind, &text, replaces.as_deref(), today)).await
                                .inspect_err(|e| log(format!("memory task failed op=remember panic={}", e.is_panic()))).ok();
                            match done {
                                None => { let _ = rpc.respond(id, tool_reply("Não consegui gravar a memória.", false)).await; "memory-io" }
                                Some(Ok(entry)) => {
                                    let _ = rpc.respond(id, tool_reply(super::memory::saved_reply(&entry, &said), true)).await;
                                    let _ = events.send(VoiceEvent::MemoryChanged).await;
                                    // O prompt da voz é fixo desde a abertura: a preferência nova entra como contexto, que não é falado.
                                    if entry.kind == super::memory::Kind::Preferencia {
                                        let text = format!("{} {}", organizer::VOICE_PREFERENCE_PREFIX, entry.text);
                                        let added = rpc.request("thread/realtime/appendText",
                                            json!({"threadId": thread, "text": text, "role": "developer"})).await;
                                        log(format!("preference appendText ok={}", added.is_ok()));
                                        new_prefs.push(text);
                                    }
                                    "remembered"
                                }
                                Some(Err(super::memory::Refusal::Full(list))) => {
                                    let ids: Vec<String> = list.iter().map(|e| format!("{} {}", e.id, e.text)).collect();
                                    let _ = rpc.respond(id, tool_reply(format!("Memória cheia ({}). Substitua uma com replaces ou apague com forget:\n{}",
                                        super::memory::MAX_ENTRIES, ids.join("\n")), false)).await;
                                    "memory-full"
                                }
                                Some(Err(super::memory::Refusal::Unknown(missing))) => { let _ = rpc.respond(id, tool_reply(format!("Não existe memória {missing}."), false)).await; "memory-unknown" }
                                Some(Err(super::memory::Refusal::Empty)) => { let _ = rpc.respond(id, tool_reply("Memória vazia não é guardada.", false)).await; "memory-empty" }
                                Some(Err(super::memory::Refusal::Io(why))) => { let _ = rpc.respond(id, tool_reply(format!("Não consegui gravar a memória: {why}"), false)).await; "memory-io" }
                            }
                        }
                        ToolCall::Forget(memory) => {
                            let file = memory_file.clone();
                            let done = tokio::task::spawn_blocking(move || super::memory::forget(&file, &memory)).await
                                .inspect_err(|e| log(format!("memory task failed op=forget panic={}", e.is_panic()))).ok();
                            match done {
                                Some(Ok(true)) => {
                                    let _ = rpc.respond(id, tool_reply("Apagado; já não vale nesta conversa.", true)).await;
                                    let _ = events.send(VoiceEvent::MemoryChanged).await;
                                    "forgot"
                                }
                                Some(Ok(false)) => { let _ = rpc.respond(id, tool_reply("Essa memória não existe.", false)).await; "forget-unknown" }
                                Some(Err(_)) | None => { let _ = rpc.respond(id, tool_reply("Não consegui ler a memória para apagar.", false)).await; "memory-io" }
                            }
                        }
                        ToolCall::FollowSession(name) => { let _ = events.send(VoiceEvent::FollowSession(CallId(id), name)).await; "follow" }
                        ToolCall::UnfollowSession(name) => { let _ = events.send(VoiceEvent::UnfollowSession(CallId(id), name)).await; "unfollow" }
                        ToolCall::Assist(assist) => {
                            let turn = params["turnId"].as_str().unwrap_or_default().to_owned();
                            let _ = events.send(VoiceEvent::Assist { call: CallId(id), assist, turn }).await;
                            "assist"
                        }
                        ToolCall::Quiet(on) => {
                            let held = results.held();
                            let next = results.set_quiet(on);
                            log(format!("quiet on={on} held={held}"));
                            let reply = match (on, held) {
                                (true, _) => "Silêncio ligado: os resultados das sessões ficam guardados e nada é falado até o usuário pedir.".to_owned(),
                                (false, 0) => "Silêncio desligado; nenhum resultado chegou enquanto isso.".to_owned(),
                                (false, n) => format!("Silêncio desligado; {n} resultado(s) guardado(s) chegam a seguir, um por vez, para você \
                                    resumir. Não resuma antes deles."),
                            };
                            let _ = rpc.respond(id, tool_reply(reply, true)).await;
                            if let Some(input) = next { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                            if on { "quiet-on" } else { "quiet-off" }
                        }
                        ToolCall::SetMode(_) if let Some(name) = organizer::mode_word_session(spoken.text(&params).unwrap_or_default(), &session_names) => {
                            let reply = format!("'{name}' é o nome de uma sessão, não o modo; use switch_session para ir até ela.");
                            let _ = rpc.respond(id, tool_reply(reply, false)).await;
                            "refused-session-name"
                        }
                        ToolCall::SetMode(mode) => {
                            for ((old, _), _) in plan_gate.user_spoke() {
                                let _ = rpc.respond(old, tool_reply("O modo mudou antes do envio; o plano não foi enviado e continua guardado.", false)).await;
                            }
                            let mut note = switch_mode(&mut planner, mode, &target, &mut gate, &rpc, events).await;
                            if let Some(warn) = apply_models(&rpc, &thread, &mut applied, &models, mode, &defaults, &mut effective, events).await { note = format!("{note} {warn}"); }
                            let _ = rpc.respond(id, tool_reply(note, true)).await;
                            "mode-set"
                        }
                        ToolCall::Send { .. } if !continuation.is_empty() && delegated.as_ref().map(|(t, _)| t.as_str()) == params["turnId"].as_str() => {
                            log(format!("send held: user kept talking parts={}", continuation.len()));
                            let note = organizer::continuation_note(&std::mem::take(&mut continuation), std::mem::take(&mut continuation_cut));
                            let replied = rpc.respond(id, tool_reply(note, false)).await;
                            if replied.is_err() { log("continuation reply failed"); }
                            "held-continuation"
                        }
                        ToolCall::Send { request, session } => {
                            let turn = params["turnId"].as_str().unwrap_or_default().to_owned();
                            let to = session.clone().unwrap_or_default();
                            // O organizador já entendeu a intenção e o destino pela conversa: sem trava de palavras. A repetição
                            // para a mesma sessão no turno o controlador recusa (`sent_turn`); a que ainda espera é correção.
                            match gate.offer((id, session, turn), &to, request, Instant::now()) {
                                Err(((id, ..), why)) => { let _ = rpc.respond(id, tool_reply(why, false)).await; "refused-short" }
                                Ok(Some(((old, ..), _))) => { let _ = rpc.respond(old, tool_reply("Substituído por um pedido mais recente; nada foi enviado.", false)).await; "offered-superseded" }
                                Ok(None) => "offered",
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
                        ToolCall::Unknown(name) => {
                            // Ferramenta do catálogo com argumento faltando não é "inexistente": o modelo deixaria de tentar.
                            let known = organizer::tools().as_array().is_some_and(|t| t.iter().any(|t| t["name"] == name.as_str()));
                            let text = if known { format!("Argumento faltando ou inválido em {name}; corrija e chame de novo.") }
                                else { format!("Ferramenta inexistente: {name}") };
                            let _ = rpc.respond(id, tool_reply(text, false)).await;
                            "unknown"
                        }
                    };
                    log(format!("tool call {tool} outcome={outcome}"));
                    if let Some((call, Some(turn))) = acting && !failed_outcome(outcome) {
                        if acting_calls.len() >= 64 { acting_calls.clear(); }
                        if acted.len() >= 64 { acted.clear(); }
                        *acted.entry(turn.clone()).or_default() += 1;
                        acting_calls.insert(call, turn);
                    }
                }
                // Perguntas do organizador não têm tela: a resposta é pela voz.
                Ok(Incoming::Request { id, method, .. }) if method == "item/tool/requestUserInput" => {
                    log("request item/tool/requestUserInput answered empty");
                    let _ = rpc.respond(id, json!({"answers": {}})).await;
                }
                // O organizador tem acesso completo; só o comando destrutivo espera o sim falado do usuário.
                Ok(Incoming::Request { id, method, params }) if method == "item/commandExecution/requestApproval" => {
                    let command = params["command"].as_str();
                    let destructive = command.is_none_or(organizer::destructive);
                    // Só fala certamente deste turno confirma: a final velha que abre o trecho seguinte não é um sim, e o turno
                    // de uma fala repetida não tem fala nova.
                    let said = spoken.sure_text(&params).filter(|_| !params["turnId"].as_str().is_some_and(|t| repeated.contains_key(t)));
                    let ok = organizer::approval_decision(command, said.as_deref(), &mut command_gate,
                        params["turnId"].as_str().unwrap_or_default(), Instant::now());
                    // Só o desfecho: o texto do comando é conversa e não vai ao diário.
                    log(format!("command approval destructive={destructive} accepted={ok}"));
                    let _ = rpc.respond(id, json!({"decision": if ok { "accept" } else { "decline" }})).await;
                }
                // Edição e permissão passam direto (acesso total), mas deixam rastro no diário: só o tipo, nunca o conteúdo.
                Ok(Incoming::Request { id, method, .. }) if method == "item/fileChange/requestApproval" => {
                    log("file change approval accepted=true");
                    let _ = rpc.respond(id, json!({"decision": "accept"})).await;
                }
                Ok(Incoming::Request { id, method, params }) if method == "item/permissions/requestApproval" => {
                    log("permissions approval accepted=true scope=turn");
                    let _ = rpc.respond(id, json!({"permissions": params["permissions"], "scope": "turn"})).await;
                }
                // Os pedidos antigos do Codex também têm resposta na hora: nenhum fica esperando um clique que não existe.
                Ok(Incoming::Request { id, method, params }) if method == "execCommandApproval" => {
                    let command = params["command"].as_array().and_then(|parts| argv_line(parts));
                    let said = spoken.sure_text(&params).filter(|_| !params["turnId"].as_str().is_some_and(|t| repeated.contains_key(t)));
                    let ok = organizer::approval_decision(command.as_deref(), said.as_deref(), &mut command_gate,
                        params["turnId"].as_str().unwrap_or_default(), Instant::now());
                    log(format!("legacy command approval accepted={ok}"));
                    let _ = rpc.respond(id, json!({"decision": if ok { "approved" } else { "denied" }})).await;
                }
                Ok(Incoming::Request { id, method, .. }) if method == "applyPatchApproval" => {
                    log("legacy patch approval accepted=true");
                    let _ = rpc.respond(id, json!({"decision": "approved"})).await;
                }
                // Só o MCP `hangar` fica ligado, e ele não pede formulário: o que vier dele é o Codex pedindo para usar a
                // ferramenta. Vale como as outras ações: só em turno com fala do usuário que não seja repetida. O resto não
                // tem quem preencha.
                Ok(Incoming::Request { id, method, params }) if method == "mcpServer/elicitation/request" => {
                    let accept = params["serverName"] == "hangar" && params["mode"] != "url" && spoken.allows(&params)
                        && !params["turnId"].as_str().is_some_and(|t| repeated.contains_key(t));
                    log(format!("mcp elicitation hangar={} accepted={accept}", params["serverName"] == "hangar"));
                    let reply = if accept { json!({"action": "accept", "content": {}, "_meta": null}) }
                        else { json!({"action": "decline", "content": null, "_meta": null}) };
                    let _ = rpc.respond(id, reply).await;
                }
                Ok(Incoming::Request { id, method, .. }) => { log(format!("request {method} answered empty")); let _ = rpc.respond(id, json!({})).await; }
                Ok(Incoming::Notification { method, params }) => {
                    let ours = params["threadId"].as_str() == Some(thread.as_str());
                    log_notification(&method, &params, ours, &mut last_delta);
                    // Limite da conta não leva threadId: tem de passar antes do filtro.
                    if method == "account/rateLimits/updated" { send_limits(events, usage::account_limits(&params["rateLimits"])).await; continue; }
                    if !ours { continue; }
                    if method == "thread/realtime/sdp" {
                        let _ = events.send(VoiceEvent::Answer(last_offer, params["sdp"].as_str().unwrap_or_default().to_owned())).await;
                        next_offer(&mut answering, &mut waiting_offer, &mut early);
                        continue;
                    }
                    // A passagem em curso falhou sem resposta: a oferta seguinte não fica presa atrás dela.
                    if method == "thread/realtime/error" && answering.is_some() {
                        log("handoff offer failed without answer");
                        next_offer(&mut answering, &mut waiting_offer, &mut early);
                    }
                    if method == "item/started" || method == "item/completed" { spoken.item_started(&params); }
                    // Cada fala uma vez: o started pode vir sem texto e o completed repete o item.
                    let text = organizer::user_message_text(&params).filter(|t| !t.trim().is_empty());
                    let item_key = params["item"]["id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned)
                        .or_else(|| text.as_ref().map(|t| format!("{}\n{t}", params["turnId"].as_str().unwrap_or_default()))).unwrap_or_default();
                    if (method == "item/started" || method == "item/completed")
                        && let Some(text) = text.clone()
                        && !organizer::from_hangar(&text) && !seen_items.contains(&item_key) {
                        if seen_items.len() >= 256 { seen_items.clear(); }
                        seen_items.insert(item_key);
                        let now = Instant::now();
                        let _ = events.send(VoiceEvent::Backstage(backstage_input(&text))).await;
                        let input = spoken_input(&text).to_owned();
                        // A voz encaminha a mesma fala de novo quando a transcrição final fecha: não é o usuário continuando.
                        let repeat = last_input.as_ref().is_some_and(|(prev, at, turn)| {
                            let running = turn.as_ref().is_none_or(|t| delegated.as_ref().is_some_and(|(d, _)| d == t));
                            let did = turn.as_ref().is_some_and(|t| acted.get(t).is_some_and(|n| *n > 0));
                            organizer::treat_as_repeat(prev, &input, now.saturating_duration_since(*at), running, did)
                        });
                        log(format!("handoff repeat={repeat}"));
                        if repeat {
                            // No mesmo turno da original não há o que marcar; noutro, nada que age sai dele se a original agiu.
                            if let Some(turn) = params["turnId"].as_str() && delegated.as_ref().is_none_or(|(d, _)| d != turn) {
                                if repeated.len() >= 64 { repeated.clear(); }
                                repeated.insert(turn.to_owned(), last_input.as_ref().and_then(|(_, _, t)| t.clone()));
                            }
                            let steer =json!({"threadId": thread, "expectedTurnId": params["turnId"], "input": [{"type": "text", "text": REPEAT_NOTE}]});
                            let steered = rpc.request("turn/steer", steer).await;
                            log(format!("repeat note steered ok={}", steered.is_ok()));
                        } else {
                            // Só fala nova vai ao Jev: a repetida já foi tratada. A fala do turno é a separada em SpokenTurns.
                            let speech = spoken.text(&params).unwrap_or_default().to_owned();
                            // O trecho desta delegação já traz o que veio antes dela.
                            delegated = params["turnId"].as_str().map(|t| (t.to_owned(), now));
                            transcript.forwarded();
                            if let Some(turn) = params["turnId"].as_str() { ack.delegated(turn, now); }
                            continuation.clear();
                            continuation_cut = false;
                            speaking = None;
                            if let Some(turn) = params["turnId"].as_str() {
                                let recent = organizer::transcript_delta(&text).to_owned();
                                let _ = events.send(VoiceEvent::Heard { turn: turn.to_owned(), speech, recent }).await;
                            }
                            last_input = Some((input, now, params["turnId"].as_str().map(str::to_owned)));
                            // Sim curto ao aviso: o primeiro resultado entra já neste turno, sem esperar o organizador chamar quiet,
                            // fechar a resposta dele e abrir outro turno para o resumo.
                            if offer_note.is_some() && results.running_summary().is_none()
                                && organizer::accepts_offer(organizer::transcript_delta(&text))
                                && let Some(turn) = params["turnId"].as_str()
                                && let Some(mut steer) = results.accept_offer() {
                                offer_note = None;
                                steer["threadId"] = json!(thread);
                                steer["expectedTurnId"] = json!(turn);
                                if let Some(parts) = steer["input"].as_array_mut() { parts.push(json!({"type": "text", "text": OFFER_ACCEPTED_NOTE})); }
                                match rpc.request("turn/steer", steer).await {
                                    Ok(_) => {
                                        log("offer accepted: result steered into the yes turn");
                                        results.mark_summary(turn.to_owned());
                                        spoken.revoke(turn);
                                        ack.answered(turn);
                                    }
                                    Err(error) => {
                                        log(format!("offer accept steer failed kind={}", rpc_error_kind(&error)));
                                        if let Some(input) = results.steer_failed(organizer_busy) {
                                            start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await;
                                        }
                                    }
                                }
                            }
                            // A troca de sessão ou de modo chega ao organizador no turno desta fala. Nunca pela voz: na v3
                            // o texto acrescentado a ela é falado, e o usuário ouvia o caminho da pasta e as regras.
                            for note in offer_note.take().into_iter().chain(carry.drain(..)) {
                                pending_context = Some(match pending_context.take() { Some(old) => format!("{old}\n{note}"), None => note });
                            }
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
                            for ((id, _), _) in plan_gate.user_spoke() {
                                log("plan held cancelled: user kept talking");
                                let _ = rpc.respond(id, tool_reply("O usuário continuou falando; o plano não foi enviado e continua guardado. Ouça; \
                                    se ele confirmar de novo, chame finish_plan de novo.", false)).await;
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
                    let item_id = params["item"]["id"].as_str().unwrap_or_default();
                    match method.as_str() {
                        "item/started" if kind == "agentMessage" => ack.answered(params["turnId"].as_str().unwrap_or_default()),
                        "thread/realtime/transcript/delta" if params["role"] == "assistant" => { ack.voice_spoke(); out.hold.voice_delta(Instant::now()); }
                        "item/started" if kind == "commandExecution" && !item_id.is_empty() => {
                            if let Some(turn) = params["turnId"].as_str().filter(|t| !t.is_empty()) {
                                running_commands.insert(item_id.to_owned(), (turn.to_owned(), Instant::now() + command_limit));
                            }
                        }
                        "item/completed" => { running_commands.remove(item_id); }
                        "turn/completed" => if let Some(done) = params["turn"]["id"].as_str() { running_commands.retain(|_, (t, _)| t != done); ack.answered(done); },
                        _ => {}
                    }
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
                        // A fala que seguiu depois da delegação: o envio desse turno espera a final (com teto).
                        "thread/realtime/transcript/delta" if params["role"] == "user" => {
                            last_user_voice = Some(Instant::now());
                            last_user_delta = last_user_voice;
                            out.hold.heard_speech(Instant::now());
                            plan_gate.heard_voice(Instant::now());
                            if speaking.is_none() && let Some((turn, at)) = &delegated && at.elapsed() >= organizer::CONTINUATION_AFTER {
                                log("user still speaking after the delegation");
                                speaking = Some((turn.clone(), Instant::now()));
                            }
                        }
                        // Fala que fechou depois da delegação sem ser só a final dela: o envio desse turno não sai sem ela.
                        "thread/realtime/transcript/done" if params["role"] == "assistant" => {
                            out.hold.voice_done();
                            transcript.line(super::transcript::Who::Voice, params["text"].as_str().unwrap_or_default(), chrono::Local::now());
                        }
                        "thread/realtime/transcript/done" if params["role"] == "user" => {
                            transcript.line(super::transcript::Who::User, params["text"].as_str().unwrap_or_default(), chrono::Local::now());
                            let heard = params["text"].as_str().unwrap_or_default().trim();
                            // A final costuma chegar segundos depois da fala, quando a voz responde: sem pedaço recente ela é
                            // fechamento da fala anterior e não pode segurar nem cortar o resumo.
                            if last_user_delta.is_some_and(|at| at.elapsed() < LATE_FINAL) {
                                last_user_voice = Some(Instant::now());
                                if let Some(rest) = out.hold.user_said(heard, Instant::now()) {
                                    log(format!("reading cut by user speech rest_bytes={}", rest.len()));
                                    carry.push(organizer::reading_cut_note(&rest));
                                }
                            }
                            else { log("user final without recent speech: not counted as talking"); }
                            speaking = None;
                            // Toda final ouvida: no trecho seguinte ela é fala velha, nunca a fala daquele turno.
                            if !heard.is_empty() { spoken.heard_late(heard, Instant::now()); }
                            if let Some((turn, at)) = delegated.clone() && !heard.is_empty() {
                                // A final da própria fala delegada chega depois da delegação: ela já está com o organizador.
                                if at.elapsed() < organizer::CONTINUATION_AFTER { log("late final ignored reason=early"); transcript.forwarded(); }
                                else if organizer::same_utterance(spoken.text_of(&turn).unwrap_or_default(), heard) {
                                    log("late final ignored reason=same");
                                    transcript.forwarded();
                                }
                                else {
                                    // Passou do teto: ficam os últimos trechos e o envio volta avisando do corte.
                                    if continuation.len() >= organizer::CONTINUATION_CAP {
                                        continuation.remove(0);
                                        continuation_cut = true;
                                        log("continuation truncated: cap");
                                    }
                                    continuation.push(heard.to_owned());
                                    let waiting = gate.take_where(|_, (_, _, t)| *t == turn);
                                    log(format!("user kept talking parts={} pending={}", continuation.len(), waiting.len()));
                                    if !waiting.is_empty() {
                                        let note = organizer::continuation_note(&std::mem::take(&mut continuation), std::mem::take(&mut continuation_cut));
                                    transcript.forwarded();
                                        for ((old, ..), _) in waiting {
                                            if rpc.respond(old, tool_reply(note.clone(), false)).await.is_err() { log("continuation reply failed"); }
                                        }
                                    }
                                }
                            }
                        }
                        "turn/completed" => {
                            if params["turn"]["id"].as_str().is_some_and(|t| delegated.as_ref().is_some_and(|(d, _)| d == t)) {
                                delegated = None; continuation.clear(); continuation_cut = false; speaking = None;
                            }
                            if let Some(turn) = params["turn"]["id"].as_str() {
                                repeated.remove(turn);
                                // Resumo que acabou sem mensagem: nada chegou à voz, o resultado volta como aviso.
                                if results.take_summary(turn) { log("summary turn ended without a message: result kept"); results.interrupted(); }
                            }
                            organizer_busy = false;
                            let _ = events.send(VoiceEvent::TurnDone).await;
                            spoken.turn_completed(&params);
                            // Turno interrompido ou falho não pode deixar um envio esperando a janela de 1,5 s. O resumo
                            // cortado pela fala dele não é turno de fala: o envio pendente de outro turno segue.
                            let was_cut = cut_summary.is_some() && cut_summary.as_deref() == params["turn"]["id"].as_str();
                            if was_cut { cut_summary = None; }
                            if params["turn"]["status"] != "completed" && !was_cut {
                                for ((id, ..), _) in gate.user_spoke() {
                                    log("gate cancelled: turn interrupted");
                                    let _ = rpc.respond(id, tool_reply("O turno foi interrompido; nada foi enviado.", false)).await;
                                }
                                for ((id, _), _) in plan_gate.user_spoke() {
                                    log("plan held cancelled: turn interrupted");
                                    let _ = rpc.respond(id, tool_reply("O turno foi interrompido; o plano não foi enviado e continua guardado.", false)).await;
                                }
                            }
                            if params["turn"]["status"] == "failed" {
                                log(format!("organizer failure: {:?}", VoiceFailure::Organizer));
                                let _ = events.send(VoiceEvent::Failed(VoiceFailure::Organizer)).await;
                            }
                            if let Some(input) = results.turn_completed() { start_summary(&rpc, &thread, input, &mut results, &mut out, organizer_busy).await; }
                        }
                        // Toda mensagem do organizador vira fala, inclusive as do meio do turno; quem a solta é o `SpeechHold`.
                        "item/completed" if params["item"]["type"] == "agentMessage" => {
                            let text = params["item"]["text"].as_str().unwrap_or_default();
                            let turn = params["turnId"].as_str().unwrap_or_default();
                            let commentary = params["item"]["phase"] == "commentary";
                            if !commentary { transcript.line(super::transcript::Who::Organizer, text, chrono::Local::now()); }
                            // O turno do sim ao aviso é o da fala dele e já é o resumo; sem aparelho, a fala fica guardada.
                            let yes_turn = delegated.as_ref().is_some_and(|(d, _)| d == turn);
                            let in_talk = !out.away && !yes_turn && (last_user_voice.is_some_and(|at| at.elapsed() < result_silence)
                                || delegated.as_ref().is_some_and(|(_, at)| at.elapsed() < command_limit * 2));
                            if !commentary && results.take_summary(turn) && in_talk {
                                // Resultado de outra sessão não entra na conversa dele: volta como aviso.
                                log("summary ready while the user talks: kept as an offer");
                                results.interrupted();
                            } else if organizer::speakable(text) {
                                speak(&rpc, &thread, &mut out, text.to_owned(), "organizer").await;
                            }
                        }
                        "thread/tokenUsage/updated" => if let Some((used, window)) = usage::context_usage(&params) {
                            log(format!("organizer context used={used} window={window:?}"));
                            let _ = events.send(VoiceEvent::OrganizerContext { used, window }).await;
                        },
                        "thread/realtime/error" | "thread/realtime/closed" if restarting || detached => log("realtime ended while switching device"),
                        // Fim que não pedimos: caiu só a perna de áudio (o celular dorme antes de o WebSocket perceber). A thread
                        // do organizador espera a próxima oferta; quem encerra a chamada é o stop explícito ou o prazo do hub.
                        "thread/realtime/error" | "thread/realtime/closed" => {
                            if stopped.load(Ordering::Relaxed) { break Ok(()); }
                            // Só um código curto vai ao diário; o texto livre pode trazer conteúdo.
                            let reason = params["reason"].as_str().filter(|r| r.len() <= 40 && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or("?");
                            log(format!("realtime audio lost method={method} reason={reason}: waiting for a device"));
                            detached = true;
                            out.away = true;
                            audio_lost = Some(Instant::now() + audio_grace);
                            let _ = events.send(VoiceEvent::Phase(Phase::Connecting)).await;
                            let _ = events.send(VoiceEvent::Failed(VoiceFailure::AudioLost)).await;
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
    if results.held() > 0 { log(format!("session results unspoken at call end count={} quiet={}", results.held(), results.quiet())); }
    let _ = rpc.request("thread/realtime/stop", json!({"threadId": thread})).await;
    outcome
}

/// argv do pedido antigo de aprovação → linha que `destructive` lê: argumento com espaço ou aspas vai entre aspas, senão
/// Rótulo de ferramenta que já voltou falha na hora: não conta como "agiu" para a fala repetida.
fn failed_outcome(outcome: &str) -> bool {
    outcome.starts_with("refused") || outcome.starts_with("memory-")
        || matches!(outcome, "unknown" | "forget-unknown" | "held-continuation" | "plan-failed" | "plan-read-failed")
}

/// `bash -lc "git push"` vira palavras soltas. Com os dois tipos de aspas não há como citar sem escape: sem linha, o comando
/// conta como destrutivo.
fn argv_line(parts: &[Value]) -> Option<String> {
    parts.iter().map(|part| {
        let arg = part.as_str()?;
        if !arg.is_empty() && !arg.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') { return Some(arg.to_owned()); }
        if !arg.contains('"') { Some(format!("\"{arg}\"")) } else if !arg.contains('\'') { Some(format!("'{arg}'")) } else { None }
    }).collect::<Option<Vec<_>>>().map(|words| words.join(" "))
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

/// Fala que a voz puxa sozinha passa por aqui: com o usuário ou a voz falando, fica guardada até a vez dela.
async fn speak(rpc: &Rpc, thread: &str, out: &mut Speech, text: String, tag: &str) {
    if out.away { out.park(text, tag); return; }
    out.hold.push(&text, Instant::now());
    match out.hold.due(Instant::now()) {
        Some(text) => append_speech(rpc, thread, &text, tag).await,
        None => log(format!("{tag} speech held bytes={}", text.len())),
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
                if let Some((session, text)) = results.turn_start_failed(organizer_busy) {
                    let short: String = text.chars().take(400).collect();
                    let from = if session.is_empty() { "A sessão respondeu à pergunta".to_owned() } else { format!("Chegou um resultado da {session}") };
                    speak(rpc, thread, out, format!("{from}: {short}"), "summary fallback").await;
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
        CallOptions { voice: None, context: "ctx".into(), cwd: None, target: "hangar".into(), organizer: ModeModels::default(), tools, handoff_same_thread: true, voice_dir,
            audio_grace: Duration::from_secs(600), hangar_mcp: None, command_limit: COMMAND_LIMIT, result_silence: Duration::from_millis(600),
            style: Default::default() }
    }

    async fn next_answer(events: &async_channel::Receiver<VoiceEvent>) -> (u64, String) {
        loop { if let VoiceEvent::Answer(seq, sdp) = events.recv().await.unwrap() { return (seq, sdp); } }
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
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        assert_eq!(next_answer(&events).await, (1, "v=0 answer 1".into()));
        let mut start = None;
        while let Some(m) = seen.recv().await { if m["method"] == "thread/realtime/start" { start = Some(m); break; } }
        let start = start.unwrap();
        assert_eq!(start["params"]["transport"]["sdp"], "v=0 offer A");
        assert_eq!(start["params"]["includeStartupContext"], false);
        // O ORGANIZER_PROMPT já é a base do organizador: aqui vai só a nota curta da passagem pela voz.
        assert_eq!(start["params"]["realtimeStartInstructions"], organizer::REALTIME_START);
        assert_eq!(start["params"]["delegationAckFiller"], false, "sem 'um segundo' a cada delegação");
        assert_ne!(start["params"]["realtimeStartInstructions"], organizer::ORGANIZER_PROMPT);
        assert!(start["params"]["prompt"].as_str().unwrap().contains("O que o organizador consegue nesta conversa"));
    }

    #[tokio::test]
    async fn second_offer_restarts_realtime_on_the_same_thread() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.offer(2, "v=0 offer B".into(), "A sessão na tela agora é web.".into());
        // O fechamento da conversa anterior não encerra a chamada.
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "replaced"}})).unwrap();
        assert_eq!(next_answer(&events).await, (2, "v=0 answer 2".into()));
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
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.detached();
        // Aparelho caído: a OpenAI fecha a conversa falada; a chamada continua esperando outro aparelho.
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "peer_gone"}})).unwrap();
        push.send(json!({"method": "thread/realtime/error", "params": {"threadId": "t1", "message": "ice failed"}})).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        voice.offer(2, "v=0 offer C".into(), "ctx".into());
        assert_eq!(next_answer(&events).await, (2, "v=0 answer 2".into()));
        while let Ok(e) = events.try_recv() { assert!(!matches!(e, VoiceEvent::Phase(Phase::Closed)), "não encerrou"); }
        let all: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        assert!(!all.iter().any(|m| m["method"] == "thread/realtime/stop"));
    }

    #[tokio::test]
    async fn offer_that_failed_without_answer_does_not_hold_the_next_one() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.offer(2, "v=0 no-answer".into(), "ctx".into());
        loop { let m = seen.recv().await.unwrap(); if m["method"] == "thread/realtime/start" && m["params"]["transport"]["sdp"] == "v=0 no-answer" { break; } }
        push.send(json!({"method": "thread/realtime/error", "params": {"threadId": "t1", "message": "ice failed"}})).unwrap();
        // O aparelho reconecta e oferece de novo: recebe a resposta da oferta dele.
        voice.offer(3, "v=0 offer C".into(), "ctx".into());
        assert_eq!(next_answer(&events).await, (3, "v=0 answer 3".into()));
    }

    #[tokio::test]
    async fn thread_start_carries_the_owner_tools() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, _events) = async_channel::unbounded();
        let tools = crate::voice::organizer::tools_for(&["switch_session".into()]);
        let voice = Voice::start(options(tools.clone()), spawn, tx);
        voice.offer(1, "v=0".into(), "ctx".into());
        loop { let m = seen.recv().await.unwrap(); if m["method"] == "thread/start" { assert_eq!(m["params"]["dynamicTools"], tools); break; } }
    }

    #[tokio::test]
    async fn speech_while_detached_waits_for_the_next_device() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.detached();
        voice.jev_verdict("turn-1".into(), Some("resposta guardada".into()), None);
        let spoke = |m: &Value| m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == "resposta guardada";
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| spoke(&m)), "sem aparelho, nada vai à voz");
        voice.offer(2, "v=0 offer B".into(), "ctx".into());
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
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.detached();
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        // Com o aparelho presente, a fala sai na hora em vez de ficar guardada para o próximo.
        voice.jev_verdict("turn-1".into(), Some("falada já".into()), None);
        loop { let m = seen.recv().await.unwrap(); if m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == "falada já" { break; } }
    }

    /// O app-server fecha a conversa falada antes de o hub perceber a queda do aparelho: a chamada espera, não acaba.
    #[tokio::test]
    async fn audio_lost_before_detached_keeps_the_call_for_the_next_offer() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(CallOptions { audio_grace: Duration::from_millis(800), ..options(json!([])) }, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        tokio::time::sleep(Duration::from_millis(100)).await;
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "peer_gone"}})).unwrap();
        audio_lost(&events).await;
        // Sem aparelho, a fala espera o próximo.
        voice.jev_verdict("turn-1".into(), Some("guardada".into()), None);
        voice.detached();
        voice.offer(2, "v=0 offer B".into(), "ctx".into());
        assert_eq!(next_answer(&events).await, (2, "v=0 answer 2".into()));
        voice.live();
        let spoke = |m: &Value| m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == "guardada";
        let mut threads = 0;
        loop {
            let m = seen.recv().await.unwrap();
            if m["method"] == "thread/start" { threads += 1; }
            if spoke(&m) { break; }
        }
        assert_eq!(threads, 1, "mesma thread do organizador");
        // A oferta nova desarmou o prazo da queda: passado ele, a chamada segue.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        while let Ok(e) = events.try_recv() { assert!(!matches!(e, VoiceEvent::Phase(Phase::Closed)), "não encerrou"); }
    }

    async fn audio_lost(events: &async_channel::Receiver<VoiceEvent>) {
        loop {
            match events.recv().await.unwrap() {
                VoiceEvent::Failed(VoiceFailure::AudioLost) => return,
                VoiceEvent::Phase(Phase::Closed) => panic!("a chamada acabou"),
                _ => {}
            }
        }
    }

    /// Erro do realtime com o aparelho ainda ligado e nenhuma oferta nova: a chamada acaba no prazo, com o código.
    #[tokio::test]
    async fn audio_lost_without_a_new_offer_ends_after_the_grace() {
        let (spawn, _seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(CallOptions { audio_grace: Duration::from_millis(300), ..options(json!([])) }, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        tokio::time::sleep(Duration::from_millis(100)).await;
        push.send(json!({"method": "thread/realtime/error", "params": {"threadId": "t1", "message": "ice failed"}})).unwrap();
        audio_lost(&events).await;
        let started = Instant::now();
        let mut ended = false;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), events.recv()).await.expect("prazo").unwrap() {
                VoiceEvent::Failed(VoiceFailure::AudioLostEnded) => ended = true,
                VoiceEvent::Phase(Phase::Closed) => break,
                _ => {}
            }
        }
        assert!(ended, "acabou com o código da queda");
        assert!(started.elapsed() >= Duration::from_millis(250), "esperou o prazo");
    }

    /// Parada nossa: o fechamento que vem depois não vira falha.
    #[tokio::test]
    async fn own_stop_then_late_closed_ends_cleanly() {
        let (spawn, _seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.stop();
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "stopped"}})).unwrap();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), events.recv()).await.expect("prazo").unwrap() {
                VoiceEvent::Failed(failure) => panic!("falha numa parada pedida: {}", failure_kind(&failure)),
                VoiceEvent::Phase(Phase::Closed) => break,
                _ => {}
            }
        }
    }

    fn command(push: &tokio::sync::mpsc::UnboundedSender<Value>, method: &str, id: &str) {
        push.send(json!({"method": method, "params": {"threadId": "t1", "turnId": "turn-1", "item": {"type": "commandExecution", "id": id}}})).unwrap();
    }

    /// `gh pr checks --watch` prendia o turno: passado o limite, o turno cai, a voz avisa e a próxima fala leva a nota.
    #[tokio::test]
    async fn command_past_the_limit_interrupts_the_turn_and_explains() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(CallOptions { command_limit: Duration::from_millis(300), ..options(json!([])) }, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        command(&push, "item/started", "cmd-1");
        let (mut interrupted, mut spoke) = (false, false);
        while !(interrupted && spoke) {
            let m = tokio::time::timeout(Duration::from_secs(5), seen.recv()).await.expect("prazo").unwrap();
            if m["method"] == "turn/interrupt" { assert_eq!(m["params"], json!({"threadId": "t1", "turnId": "turn-1"})); interrupted = true; }
            if m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == COMMAND_LIMIT_SPEECH { spoke = true; }
        }
        push.send(json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "turn-1", "status": "interrupted"}}})).unwrap();
        crate::voice::test_support::push_speech(&push, "turn-2", "e o PR?");
        loop {
            let m = tokio::time::timeout(Duration::from_secs(5), seen.recv()).await.expect("prazo").unwrap();
            if m["method"] == "turn/steer" && m["params"]["expectedTurnId"] == "turn-2" {
                assert!(m["params"]["input"][0]["text"].as_str().unwrap().contains(COMMAND_LIMIT_NOTE));
                break;
            }
        }
    }

    async fn until(seen: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, seen.recv()).await {
                Ok(Some(m)) if pred(&m) => return m,
                Ok(Some(_)) => {}
                _ => panic!("não chegou: {what}"),
            }
        }
    }

    /// O fim de outro turno não tira o comando da vigia; interrupção recusada não vira "parei o comando".
    #[tokio::test]
    async fn other_turn_ending_keeps_watching_and_a_refused_interrupt_says_nothing() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(CallOptions { command_limit: Duration::from_millis(300), ..options(json!([])) }, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        command(&push, "item/started", "cmd-1");
        push.send(json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "turn-0", "status": "completed"}}})).unwrap();
        let (mut interrupted, mut spoke) = (false, false);
        while !(interrupted && spoke) {
            let m = tokio::time::timeout(Duration::from_secs(5), seen.recv()).await.expect("prazo").unwrap();
            if m["method"] == "turn/interrupt" { assert_eq!(m["params"]["turnId"], "turn-1"); interrupted = true; }
            if m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == COMMAND_LIMIT_SPEECH { spoke = true; }
        }
        push.send(json!({"method": "item/started", "params": {"threadId": "t1", "turnId": "turn-gone",
            "item": {"type": "commandExecution", "id": "cmd-2"}}})).unwrap();
        tokio::time::sleep(Duration::from_millis(900)).await;
        let spoken = std::iter::from_fn(|| seen.try_recv().ok())
            .filter(|m| m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"] == COMMAND_LIMIT_SPEECH).count();
        assert_eq!(spoken, 0, "o aviso do turn-1 já saiu antes; o do turn-gone recusado não sai");
    }

    #[tokio::test]
    async fn command_that_completes_in_time_is_not_interrupted() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(CallOptions { command_limit: Duration::from_millis(300), ..options(json!([])) }, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        command(&push, "item/started", "cmd-1");
        tokio::time::sleep(Duration::from_millis(100)).await;
        command(&push, "item/completed", "cmd-1");
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| m["method"] == "turn/interrupt"));
    }

    /// Fala do usuário diferente do `input` da voz: a fala separada é a fala certa do turno.
    fn delegation(push: &tokio::sync::mpsc::UnboundedSender<Value>, turn: &str, said: &str) {
        let text = format!("<realtime_delegation>\n<input>pedido {turn}</input>\n<transcript_delta>user: {said}\nuser: pedido {turn}</transcript_delta>\n</realtime_delegation>");
        let _ = push.send(json!({"method": "item/started", "params": {"threadId": "t1", "turnId": turn,
            "item": {"type": "userMessage", "id": format!("item-{turn}"), "content": [{"type": "text", "text": text}]}}}));
    }

    fn approval(push: &tokio::sync::mpsc::UnboundedSender<Value>, id: &str, turn: &str, command: &str) {
        let _ = push.send(json!({"id": id, "method": "item/commandExecution/requestApproval", "params": {"threadId": "t1", "turnId": turn,
            "itemId": format!("cmd-{id}"), "startedAtMs": 0, "command": command}}));
    }

    /// O caso real: `hangar-send --help` pediu aprovação e a voz mandou o usuário aprovar no app. O organizador nasce sem
    /// pedido de aprovação para o comum; o que ainda vier é respondido na hora, e o destrutivo só passa com o sim falado.
    #[tokio::test]
    async fn approvals_are_answered_at_once_and_destructive_waits_for_the_spoken_yes() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        let start = until(&mut seen, "thread/start", |m| m["method"] == "thread/start").await;
        assert_eq!(start["params"]["approvalPolicy"], "never");
        assert_eq!(start["params"]["sandbox"], "danger-full-access");
        next_answer(&events).await;
        voice.live();
        let decision = |id: &'static str| move |m: &Value| m["id"] == id && m.get("result").is_some();
        approval(&push, "a1", "turn-0", "hangar-send --help");
        assert_eq!(until(&mut seen, "a1", decision("a1")).await["result"]["decision"], "accept", "comando comum passa sem fala");
        let _ = push.send(json!({"id": "f1", "method": "item/fileChange/requestApproval", "params": {"threadId": "t1", "turnId": "turn-0", "itemId": "p1"}}));
        assert_eq!(until(&mut seen, "f1", decision("f1")).await["result"]["decision"], "accept");
        delegation(&push, "turn-1", "apaga o arquivo a");
        approval(&push, "a2", "turn-1", "rm a");
        assert_eq!(until(&mut seen, "a2", decision("a2")).await["result"]["decision"], "decline", "o destrutivo espera o sim");
        delegation(&push, "turn-2", "sim, pode apagar");
        approval(&push, "a3", "turn-2", "rm a");
        assert_eq!(until(&mut seen, "a3", decision("a3")).await["result"]["decision"], "accept", "o sim falado libera o mesmo comando");
    }

    #[test]
    fn legacy_argv_keeps_quoted_arguments_whole() {
        let line = argv_line(&[json!("bash"), json!("-lc"), json!("git push")]).unwrap();
        assert_eq!(line, "bash -lc \"git push\"");
        assert!(organizer::destructive(&line));
        assert_eq!(argv_line(&[json!("echo"), json!("diz \"oi\"")]).as_deref(), Some("echo 'diz \"oi\"'"));
        assert!(!organizer::destructive("echo 'diz \"oi\"'"));
        assert_eq!(argv_line(&[json!("echo"), json!("it's \"x\"")]), None, "sem como citar: conta como destrutivo");
        assert_eq!(argv_line(&[json!("ls"), json!(1)]), None);
    }

    fn quiet(push: &tokio::sync::mpsc::UnboundedSender<Value>, id: &str, turn: &str, on: bool) {
        let _ = push.send(json!({"id": id, "method": "item/tool/call", "params": {"tool": "quiet", "arguments": {"on": on}, "turnId": turn, "threadId": "t1"}}));
    }

    /// O caso real: ele pediu silêncio até voltar e dois resultados foram anunciados mesmo assim. Em silêncio o resultado fica
    /// guardado; só a volta pedida por ele o solta para o resumo.
    #[tokio::test]
    async fn quiet_holds_session_results_until_the_user_comes_back() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        let reply = |id: &'static str| move |m: &Value| m["id"] == id && m.get("result").is_some();
        quiet(&push, "q0", "turn-sem-fala", true);
        assert_eq!(until(&mut seen, "q0", reply("q0")).await["result"]["success"], false, "sem fala do usuário não silencia");
        delegation(&push, "turn-1", "vou sair dez minutos, não me avisa nada até eu voltar");
        quiet(&push, "q1", "turn-1", true);
        assert_eq!(until(&mut seen, "q1", reply("q1")).await["result"]["success"], true);
        voice.session_result("auditoria".into(), "migração conferida".into());
        tokio::time::sleep(Duration::from_millis(400)).await;
        let summary = |m: &Value| m["method"] == "turn/start"
            && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.starts_with("[RESULTADO DA SESSÃO auditoria]"));
        let quiet_log: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        assert!(!quiet_log.iter().any(summary), "em silêncio nada vira resumo");
        assert!(!quiet_log.iter().any(|m| m["method"] == "thread/realtime/appendSpeech"
            && m["params"]["text"].as_str().is_some_and(|t| t.contains("migração"))), "nem fala");
        delegation(&push, "turn-2", "voltei, me dá o resumo");
        quiet(&push, "q2", "turn-2", false);
        let off = until(&mut seen, "q2", reply("q2")).await;
        assert!(off["result"]["contentItems"][0]["text"].as_str().unwrap().contains("1 resultado"), "{off}");
        until(&mut seen, "resumo do guardado", summary).await;
    }

    fn summary_of(session: &'static str) -> impl Fn(&Value) -> bool {
        move |m: &Value| m["method"] == "turn/start"
            && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.starts_with(&format!("[RESULTADO DA SESSÃO {session}]")))
    }

    fn spoke(m: &Value, part: &str) -> bool {
        m["method"] == "thread/realtime/appendSpeech" && m["params"]["text"].as_str().is_some_and(|t| t.contains(part))
    }

    /// O caso real (14/10, 19:41): resultados da deploy chegaram enquanto ele falava da loja e a voz tomou a vez.
    /// Fala longa com pausa menor que a janela: nada sai; só depois do silêncio inteiro vem o aviso, e o resumo só quando ele
    /// pedir para ouvir.
    #[tokio::test]
    async fn results_during_a_long_speech_wait_for_real_silence_and_become_an_offer() {
        use crate::voice::test_support::{push_speech, push_user_audio};
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        push_user_audio(&push, "eu tava lendo a loja e");
        tokio::time::sleep(Duration::from_millis(100)).await;
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        tokio::time::sleep(Duration::from_millis(350)).await;
        push_user_audio(&push, "já fez, não era pra ter feito assim");
        let last_speech = Instant::now();
        voice.session_result("hangar".into(), "Build passou.".into());
        tokio::time::sleep(Duration::from_millis(400)).await;
        let during: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        assert!(!during.iter().any(|m| summary_of("deploy")(m) || summary_of("hangar")(m)), "pausa curta não solta resumo");
        assert!(!during.iter().any(|m| spoke(m, "resultado")), "nem o aviso");
        let offer = until(&mut seen, "aviso", |m| spoke(m, "resultado")).await;
        assert!(last_speech.elapsed() >= Duration::from_millis(600), "o aviso esperou a janela inteira");
        assert_eq!(offer["params"]["text"], "Chegaram dois resultados, da deploy e da hangar. Quer ouvir?");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| summary_of("deploy")(&m)), "aviso não é despejo");
        push_speech(&push, "turn-9", "pode falar os resultados");
        quiet(&push, "q9", "turn-9", false);
        until(&mut seen, "resumo depois do sim", summary_of("deploy")).await;
    }

    /// A conversa fica em disco: a fala dele, a da voz e a resposta do organizador, na ordem em que fecharam.
    #[tokio::test]
    async fn the_call_conversation_is_saved_to_a_file() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(json!([]));
        opts.voice_dir = dir.path().to_path_buf();
        let voice = Voice::start(opts, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        crate::voice::test_support::push_user_audio(&push, "me fala");
        push.send(json!({"method": "thread/realtime/transcript/done", "params": {"threadId": "t1", "role": "user", "text": "me fala o status"}})).unwrap();
        push.send(json!({"method": "item/completed", "params": {"threadId": "t1", "turnId": "turn-1",
            "item": {"type": "agentMessage", "id": "m1", "text": "Tudo verde na hangar."}}})).unwrap();
        push.send(json!({"method": "thread/realtime/transcript/done", "params": {"threadId": "t1", "role": "assistant", "text": "Tudo verde na hangar."}})).unwrap();
        voice.session_result("x".into(), "y".into());
        until(&mut seen, "o laço andou", summary_of_any).await;
        let file = std::fs::read_dir(dir.path().join("conversas")).unwrap().next().unwrap().unwrap().path();
        let text = std::fs::read_to_string(file).unwrap();
        let order: Vec<&str> = ["**Você:** me fala o status", "**Organizador:** Tudo verde na hangar.", "**Voz:** Tudo verde na hangar."]
            .into_iter().filter(|part| text.contains(part)).collect();
        assert_eq!(order.len(), 3, "{text}");
    }

    fn summary_of_any(m: &Value) -> bool {
        m["method"] == "turn/start" && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.starts_with("[RESULTADO"))
    }

    /// O caso de 15/10 (18:09 a 18:22, seis vezes): a final atrasada do "sim", sem fala nova, cortava o resumo que acabava de
    /// começar, e o resultado voltava como aviso repetido. Final sem pedaço de fala recente não é ele falando.
    #[tokio::test]
    async fn late_final_of_earlier_speech_does_not_cut_the_summary() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        until(&mut seen, "resumo em silêncio", summary_of("deploy")).await;
        push.send(json!({"method": "turn/started", "params": {"threadId": "t1", "turn": {"id": "turn-s1"}}})).unwrap();
        push.send(json!({"method": "thread/realtime/transcript/done", "params": {"threadId": "t1", "role": "user", "text": "sim"}})).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| m["method"] == "turn/interrupt"), "final atrasada não corta");
    }

    /// O sim ao aviso já leva o resultado ao turno dele: nada de esperar o organizador chamar quiet e abrir outro turno.
    #[tokio::test]
    async fn yes_to_the_offer_steers_the_result_into_the_same_turn() {
        use crate::voice::test_support::push_user_audio;
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        push_user_audio(&push, "tava falando da loja");
        tokio::time::sleep(Duration::from_millis(100)).await;
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        until(&mut seen, "aviso", |m| spoke(m, "Quer ouvir?")).await;
        push.send(json!({"method": "turn/started", "params": {"threadId": "t1", "turn": {"id": "turn-9"}}})).unwrap();
        let delegation = "<realtime_delegation>\n<input>\"sim\" — quer ouvir</input>\n<transcript_delta>assistant: Chegou um resultado da deploy. Quer ouvir?\nuser: sim</transcript_delta>\n</realtime_delegation>";
        push.send(json!({"method": "item/started", "params": {"threadId": "t1", "turnId": "turn-9",
            "item": {"type": "userMessage", "id": "u9", "content": [{"type": "text", "text": delegation}]}}})).unwrap();
        let steer = until(&mut seen, "resultado no turno do sim", |m| m["method"] == "turn/steer"
            && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.starts_with("[RESULTADO DA SESSÃO deploy]"))).await;
        assert_eq!(steer["params"]["expectedTurnId"], "turn-9");
        assert!(steer["params"]["input"][1]["text"].as_str().unwrap().contains("não chame quiet"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| summary_of("deploy")(&m)), "sem turno de resumo à parte");
    }

    /// Pedido respondido logo não ganha confirmação; o que fica pensando calado ganha uma frase curta do Hangar.
    #[tokio::test]
    async fn slow_spoken_request_gets_a_wait_acknowledgment_and_a_fast_one_does_not() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        let ack = |m: &Value| organizer::ACKS.iter().any(|a| spoke(m, a));
        crate::voice::test_support::push_speech(&push, "turn-1", "qual sessão está aberta agora");
        push.send(json!({"method": "item/started", "params": {"threadId": "t1", "turnId": "turn-1",
            "item": {"type": "agentMessage", "id": "m1"}}})).unwrap();
        tokio::time::sleep(organizer::ACK_AFTER + Duration::from_millis(300)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| ack(&m)), "resposta começou: sem confirmação");
        let asked = Instant::now();
        crate::voice::test_support::push_speech(&push, "turn-2", "revisa o plano inteiro da migração e me diz o que falta");
        until(&mut seen, "confirmação da espera", ack).await;
        assert!(asked.elapsed() >= organizer::ACK_AFTER, "só depois da espera");
    }

    /// Ele voltou a falar enquanto o organizador montava o resumo: o turno cai antes de a mensagem (que o Codex repassa
    /// sozinho à voz) ficar pronta, e o resultado vira aviso depois da fala.
    #[tokio::test]
    async fn speech_while_the_summary_is_written_cuts_it_and_offers_it_later() {
        use crate::voice::test_support::push_user_audio;
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        until(&mut seen, "resumo em silêncio", summary_of("deploy")).await;
        push.send(json!({"method": "turn/started", "params": {"threadId": "t1", "turn": {"id": "turn-s1"}}})).unwrap();
        push_user_audio(&push, "espera, deixa eu terminar");
        let cut = until(&mut seen, "corte do resumo", |m| m["method"] == "turn/interrupt").await;
        assert_eq!(cut["params"]["turnId"], "turn-s1");
        push.send(json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "turn-s1", "status": "interrupted"}}})).unwrap();
        let offer = until(&mut seen, "aviso", |m| spoke(m, "resultado")).await;
        assert_eq!(offer["params"]["text"], "Chegou um resultado da deploy. Quer ouvir?");
    }

    /// Turno de resumo que acaba sem mensagem (falhou, vazio): nada foi à voz, então o resultado volta como aviso.
    #[tokio::test]
    async fn summary_turn_without_a_message_keeps_the_result() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        until(&mut seen, "resumo", summary_of("deploy")).await;
        push.send(json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "turn-s1", "status": "failed"}}})).unwrap();
        let offer = until(&mut seen, "aviso", |m| spoke(m, "resultado")).await;
        assert_eq!(offer["params"]["text"], "Chegou um resultado da deploy. Quer ouvir?");
    }

    /// Com `clientManagedHandoffs` o Codex não repassa a mensagem: o Hangar a fala, uma vez só.
    #[tokio::test]
    async fn summary_reaches_the_voice_once() {
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        until(&mut seen, "resumo", summary_of("deploy")).await;
        push.send(json!({"method": "item/completed", "params": {"threadId": "t1", "turnId": "turn-s1",
            "item": {"type": "agentMessage", "id": "m1", "text": "Chegou um resultado da deploy: criou a TK-2."}}})).unwrap();
        push.send(json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "turn-s1", "status": "completed"}}})).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(std::iter::from_fn(|| seen.try_recv().ok()).filter(|m| spoke(m, "TK-2")).count(), 1, "falado uma vez, pelo Hangar");
    }

    /// Preferência gravada no meio da chamada chega também a quem fala, sem virar fala.
    #[tokio::test]
    async fn preference_saved_mid_call_reaches_the_voice_as_context() {
        use crate::voice::test_support::{push_speech, push_tool};
        let (spawn, mut seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let dir = tempfile::tempdir().unwrap();
        let voice = Voice::start(CallOptions { voice_dir: dir.path().to_owned(), ..options(json!([])) }, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        push_speech(&push, "turn-1", "guarda que eu não quero frase de espera");
        push_tool(&push, "r1", "remember", json!({"kind": "preferencia", "text": "Sem frases de espera como 'um segundo'."}), "turn-1");
        let added = until(&mut seen, "preferência para a voz", |m| m["method"] == "thread/realtime/appendText").await;
        assert_eq!(added["params"]["role"], "developer");
        assert!(added["params"]["text"].as_str().unwrap().contains("Sem frases de espera"));
    }

    #[tokio::test]
    async fn commands_before_the_offer_are_kept() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.set_mode(Mode::Plan);
        voice.offer(1, "v=0".into(), "ctx".into());
        next_answer(&events).await;
        loop { if let VoiceEvent::Mode(Mode::Plan) = events.recv().await.unwrap() { break; } }
        loop { let m = seen.recv().await.unwrap(); if m["method"] == "thread/realtime/appendSpeech" { assert_eq!(m["params"]["text"], "Modo planejar."); break; } }
    }

    /// Conecta com a memória que `prepare` deixa na pasta e devolve (instruções do organizador, prompt da voz, primeira fala).
    async fn connect_with_memory(prepare: impl FnOnce(&std::path::Path)) -> (String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        prepare(&crate::voice::memory::memory_path(dir.path()));
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let mut opts = options(json!([]));
        opts.voice_dir = dir.path().to_path_buf();
        let voice = Voice::start(opts, spawn, tx);
        voice.offer(1, "v=0".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        let (mut developer, mut prompt) = (String::new(), String::new());
        loop {
            let m = seen.recv().await.unwrap();
            match m["method"].as_str() {
                Some("thread/start") => developer = m["params"]["developerInstructions"].as_str().unwrap().to_owned(),
                Some("thread/realtime/start") => prompt = m["params"]["prompt"].as_str().unwrap().to_owned(),
                Some("thread/realtime/appendSpeech") => return (developer, prompt, m["params"]["text"].as_str().unwrap().to_owned()),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn empty_memory_invites_at_connect() {
        let (developer, prompt, first) = connect_with_memory(|_| {}).await;
        assert_eq!(first, organizer::ONBOARDING_GREETING, "a voz convida antes de a pessoa falar");
        assert!(prompt.contains("Primeira conversa"), "a resposta ao convite vai ao organizador");
        assert!(developer.contains("Passo 1") && developer.contains("Passo 2") && developer.contains("só leitura")
            && developer.contains("credencial") && developer.contains("MEMORY_SUMMARY") && !developer.contains("pausa natural"), "{developer}");
    }

    #[tokio::test]
    async fn stored_memory_connects_without_invite() {
        let (developer, prompt, first) = connect_with_memory(|file| {
            crate::voice::memory::remember(file, crate::voice::memory::Kind::Pessoa, "Desenvolvedor", None,
                chrono::Local::now().date_naive()).unwrap();
        }).await;
        assert_eq!(first, "Conectado. Pode falar.");
        assert!(!prompt.contains("Primeira conversa") && !developer.contains("Passo 1"));
        assert!(developer.contains("Desenvolvedor"), "{developer}");
    }

    /// As preferências iam só ao organizador; quem fala (e parafraseia) nunca as via.
    #[tokio::test]
    async fn stored_preferences_reach_the_voice_that_speaks() {
        let (_, prompt, _) = connect_with_memory(|file| {
            let today = chrono::Local::now().date_naive();
            crate::voice::memory::remember(file, crate::voice::memory::Kind::Preferencia,
                "Preservar as opções A e B com consequências e recomendação.", None, today).unwrap();
            crate::voice::memory::remember(file, crate::voice::memory::Kind::Projeto, "Prazo da TK-1 sexta.", None, today).unwrap();
        }).await;
        assert!(prompt.contains("Preservar as opções A e B"), "{prompt}");
        assert!(!prompt.contains("TK-1"), "só o jeito de falar vai à voz; fato de projeto fica com o organizador");
    }

    #[tokio::test]
    async fn unreadable_memory_connects_without_invite() {
        let (developer, prompt, first) = connect_with_memory(|file| std::fs::write(file, b"{not json").unwrap()).await;
        assert_eq!(first, "Conectado. Pode falar.");
        assert!(!prompt.contains("Primeira conversa") && !developer.contains("Passo 1"));
    }

    fn user_said(push: &tokio::sync::mpsc::UnboundedSender<Value>, text: &str) {
        crate::voice::test_support::push_user_audio(push, text);
        push.send(json!({"method": "thread/realtime/transcript/done", "params": {"threadId": "t1", "role": "user", "text": text}})).unwrap();
    }

    fn voice_speaking(push: &tokio::sync::mpsc::UnboundedSender<Value>) {
        push.send(json!({"method": "thread/realtime/transcript/delta", "params": {"threadId": "t1", "role": "assistant", "delta": "Fa"}})).unwrap();
    }

    fn voice_done(push: &tokio::sync::mpsc::UnboundedSender<Value>, text: &str) {
        push.send(json!({"method": "thread/realtime/transcript/done", "params": {"threadId": "t1", "role": "assistant", "text": text}})).unwrap();
    }

    fn organizer_message(push: &tokio::sync::mpsc::UnboundedSender<Value>, turn: &str, text: &str) {
        push.send(json!({"method": "item/completed", "params": {"threadId": "t1", "turnId": turn,
            "item": {"type": "agentMessage", "id": format!("m-{turn}"), "text": text}}})).unwrap();
    }

    async fn live_call(opts: CallOptions) -> (Voice, tokio::sync::mpsc::UnboundedReceiver<Value>, tokio::sync::mpsc::UnboundedSender<Value>,
        async_channel::Receiver<VoiceEvent>) {
        let (spawn, seen, push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(opts, spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        next_answer(&events).await;
        voice.live();
        (voice, seen, push, events)
    }

    /// O caso real (19/10, 17:05): ele explicava um problema, a voz fechou a vez dele numa pausa e a resposta do organizador
    /// saiu por cima ("calma"). O Codex não repassa mais a resposta sozinho; o Hangar só a fala com ele calado.
    #[tokio::test]
    async fn organizer_reply_waits_for_the_user_to_stop_talking() {
        let (_voice, mut seen, push, _events) = live_call(options(json!([]))).await;
        user_said(&push, "por que a bolha estourou se tinha um limite");
        organizer_message(&push, "turn-1", "A bolha tinha limite, mas o nome comprido passou dele.");
        tokio::time::sleep(Duration::from_millis(600)).await;
        crate::voice::test_support::push_user_audio(&push, "eu tô falando, calma");
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let during: Vec<Value> = std::iter::from_fn(|| seen.try_recv().ok()).collect();
        assert!(!during.iter().any(|m| spoke(m, "nome comprido")), "pausa curta não abre a vez para a resposta");
        let quiet_since = Instant::now();
        until(&mut seen, "resposta depois do silêncio", |m| spoke(m, "nome comprido")).await;
        assert!(quiet_since.elapsed() >= Duration::from_millis(500), "esperou o silêncio dele");
    }

    #[tokio::test]
    async fn the_hangar_speaks_the_organizer_itself() {
        let (spawn, mut seen, _push) = fake_app_server();
        let (tx, events) = async_channel::unbounded();
        let voice = Voice::start(options(json!([])), spawn, tx);
        voice.offer(1, "v=0 offer A".into(), "ctx".into());
        let start = until(&mut seen, "realtime/start", |m| m["method"] == "thread/realtime/start").await;
        assert_eq!(start["params"]["clientManagedHandoffs"], true, "o Codex não fala a resposta por conta própria");
        next_answer(&events).await;
    }

    /// O caso real (19/10, 16:58 e 17:04): o resultado de outra sessão saiu emendado na resposta que a voz falava.
    #[tokio::test]
    async fn session_result_waits_for_the_voice_to_finish_its_answer() {
        let (voice, mut seen, push, _events) = live_call(options(json!([]))).await;
        voice_speaking(&push);
        voice.session_result("hangar".into(), "A tarefa 4 terminou.".into());
        until(&mut seen, "resumo", summary_of("hangar")).await;
        organizer_message(&push, "turn-s1", "Chegou um resultado da hangar: a tarefa 4 terminou.");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| spoke(&m, "tarefa 4")), "a voz ainda fala a resposta dela");
        voice_done(&push, "No Hangar, a sessão do plano já está com o A enviado.");
        until(&mut seen, "resumo à parte", |m| spoke(m, "tarefa 4")).await;
    }

    /// O sim ao aviso abre o resumo no próprio turno do sim: a fala dele nesse turno não pode devolver o resumo ao aviso.
    #[tokio::test]
    async fn the_yes_to_the_offer_gets_the_summary_spoken() {
        use crate::voice::test_support::{push_speech, push_user_audio};
        let (voice, mut seen, push, _events) = live_call(options(json!([]))).await;
        push_user_audio(&push, "eu tava falando da loja");
        tokio::time::sleep(Duration::from_millis(100)).await;
        voice.session_result("deploy".into(), "Criei a TK-2 na VM.".into());
        until(&mut seen, "aviso", |m| spoke(m, "Quer ouvir?")).await;
        voice_speaking(&push);
        voice_done(&push, "Chegou um resultado da deploy. Quer ouvir?");
        push_user_audio(&push, "sim");
        push_speech(&push, "turn-9", "sim");
        until(&mut seen, "resultado no turno do sim", |m| m["method"] == "turn/steer" && m["params"]["expectedTurnId"] == "turn-9").await;
        organizer_message(&push, "turn-9", "Chegou um resultado da deploy: criou a TK-2 na VM.");
        until(&mut seen, "resumo falado", |m| spoke(m, "TK-2")).await;
    }

    /// O caso real (19/10, 16:19): o organizador escreveu 12 mil caracteres e a voz falou 1,8 mil, porque o Codex corta em
    /// ~1.000 tokens o que entrega de uma vez. A leitura vai em partes, cada uma depois de a voz terminar a anterior.
    #[tokio::test]
    async fn a_long_reading_is_spoken_whole_in_parts() {
        let (_voice, mut seen, push, _events) = live_call(options(json!([]))).await;
        let paragraphs: Vec<String> = (1..=8).map(|n| format!("Achado {n}: {}", "detalhe importante ".repeat(70))).collect();
        organizer_message(&push, "turn-1", &paragraphs.join("\n\n"));
        let first = until(&mut seen, "primeira parte", |m| spoke(m, "Achado 1:")).await;
        let first = first["params"]["text"].as_str().unwrap().to_owned();
        assert!(first.len() <= organizer::SPEECH_CHUNK, "parte de {} bytes", first.len());
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| spoke(&m, "Achado 8:")), "a próxima espera a voz");
        let mut spoken = first;
        while !spoken.contains("Achado 8:") {
            voice_speaking(&push);
            voice_done(&push, "parte");
            let next = until(&mut seen, "parte seguinte", |m| m["method"] == "thread/realtime/appendSpeech").await;
            let next = next["params"]["text"].as_str().unwrap();
            assert!(next.len() <= organizer::SPEECH_CHUNK);
            spoken.push_str(next);
        }
        for p in &paragraphs { assert!(spoken.contains(p.trim()), "nada da leitura se perde"); }
    }

    /// Falar por cima da leitura corta o resto dela; o que faltou vai ao organizador para continuar se ele pedir.
    #[tokio::test]
    async fn talking_over_a_reading_drops_the_rest_and_tells_the_organizer() {
        let (_voice, mut seen, push, _events) = live_call(options(json!([]))).await;
        let paragraphs: Vec<String> = (1..=4).map(|n| format!("Parte {n}: {}", "texto da sessão ".repeat(150))).collect();
        organizer_message(&push, "turn-1", &paragraphs.join("\n\n"));
        until(&mut seen, "primeira parte", |m| spoke(m, "Parte 1:")).await;
        voice_speaking(&push);
        user_said(&push, "espera, isso eu já sei");
        voice_done(&push, "parte");
        tokio::time::sleep(Duration::from_millis(2600)).await;
        assert!(!std::iter::from_fn(|| seen.try_recv().ok()).any(|m| spoke(&m, "Parte 4:")), "o resto não volta sozinho");
        delegation(&push, "turn-2", "continua de onde parou");
        let steer = until(&mut seen, "nota do resto", |m| m["method"] == "turn/steer"
            && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.contains("Parte 4:"))).await;
        assert!(steer["params"]["input"][0]["text"].as_str().unwrap().starts_with(organizer::NOTE_PREFIX));
    }

    /// O caso real (19/10, 17:02): ele escolheu a opção A, a conexão caiu 8 s depois, antes de a voz repassar a fala, e na
    /// volta o organizador disse que ele ainda não tinha escolhido. A voz nova e o organizador recebem o que foi dito.
    #[tokio::test]
    async fn reconnect_brings_back_what_was_said_and_the_unforwarded_answer() {
        let (voice, mut seen, push, events) = live_call(options(json!([]))).await;
        delegation(&push, "turn-1", "vai pra sessão do mobile e vê o que ela respondeu");
        voice_speaking(&push);
        voice_done(&push, "Ela abriu duas opções: A, conferir no emulador; B, publicar direto. Qual você prefere?");
        tokio::time::sleep(Duration::from_millis(1700)).await;
        user_said(&push, "confere no Android, nem precisa enviar outro");
        tokio::time::sleep(Duration::from_millis(100)).await;
        push.send(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "transport_closed"}})).unwrap();
        audio_lost(&events).await;
        voice.detached();
        voice.offer(2, "v=0 offer B".into(), "ctx".into());
        let again = until(&mut seen, "realtime/start da volta", |m| m["method"] == "thread/realtime/start"
            && m["params"]["transport"]["sdp"] == "v=0 offer B").await;
        let items = again["params"]["initialItems"].to_string();
        assert!(items.contains("confere no Android") && items.contains("Qual você prefere?"), "{items}");
        next_answer(&events).await;
        voice.live();
        delegation(&push, "turn-2", "voltou? cê perdeu o contexto?");
        let steer = until(&mut seen, "fala que não chegou", |m| m["method"] == "turn/steer"
            && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.contains("confere no Android"))).await;
        assert!(steer["params"]["expectedTurnId"] == "turn-2");
    }
}
