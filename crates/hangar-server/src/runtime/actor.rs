use super::{cano::{CanoConnection,IoEvent,WireFrame},websocket::WebSocketConnection,claude::ClaudeEngine,codex::Engine as CodexEngine,local_policy,
    protocol::*,queue::{Action,QueueActor,Status},receipt::ReceiptIndex};
use crate::mods::model::{ModsCall,ModsError,SurfaceEffect};
use serde_json::{Value,json};
use std::collections::{BTreeMap,BTreeSet,VecDeque};
use std::sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}};
use std::time::{Duration,Instant,SystemTime,UNIX_EPOCH};
use tokio::sync::{broadcast,mpsc,oneshot,Mutex,Notify};
use tokio::task::{JoinHandle,JoinSet};

enum Core { Claude(ClaudeEngine),Codex(CodexEngine) }

const CONFIRM_POLL:Duration=Duration::from_millis(50);
const CONFIRM_WINDOW:Duration=Duration::from_secs(30);
const CONFIRM_RETRY:Duration=Duration::from_secs(30);
/// Teto de um pedido de app aos mods: o mais longo da superfície (desenho de novo e clique, 6 s) mais
/// 1 s de folga para o relógio do ator, abaixo dos 8 s em que o app desiste. O registro serializa os
/// pedidos da sessão, então um pedido sem teto prenderia os seguintes.
const MODS_CALL_LIMIT:Duration=Duration::from_secs(crate::mods::surface::APP_CALL_MAX_S as u64 + 1);

/// O que os serviços locais do ator precisam do servidor: diário, cotas e o ambiente da subida do Codex.
/// Nenhum serviço vai mais ao Python (a rota `/internal/runtime/policy` saiu na #145).
#[derive(Clone)]
pub struct PolicyClient {
    /// Dono das cotas; sem ele a linha de status sai sem janelas.
    accounts:Option<crate::accounts::AccountService>,
    diag:crate::diag::DiagClient,
    /// Só testes: ambiente-base da subida do Codex por registro. `None` = o do processo, lido a cada subida.
    codex_env:Option<Arc<crate::accounts::environment::AccountEnvironment>>,
    /// Ponte do plugin que o Claude subido aqui recebe no ambiente; sem ela, sobe sem a ponte.
    plugin_bridge:Option<crate::session_open::claude::PluginBridge>,
}

impl PolicyClient {
    pub fn new(upstream:std::net::SocketAddr,secret:String) -> Self {
        Self { diag:crate::diag::DiagClient::new(upstream,secret),accounts:None,codex_env:None,plugin_bridge:None }
    }
    pub fn with_plugin_bridge(mut self,bridge:crate::session_open::claude::PluginBridge) -> Self { self.plugin_bridge = Some(bridge); self }
    pub(super) fn plugin_bridge(&self) -> Option<crate::session_open::claude::PluginBridge> { self.plugin_bridge.clone() }
    pub(super) fn diag(&self) -> crate::diag::DiagClient { self.diag.clone() }
    pub fn with_accounts(mut self,accounts:crate::accounts::AccountService) -> Self { self.accounts = Some(accounts); self }
    #[doc(hidden)]
    pub fn with_codex_environment(mut self,env:crate::accounts::environment::AccountEnvironment) -> Self { self.codex_env = Some(Arc::new(env)); self }
    pub(super) fn codex_environment(&self) -> crate::accounts::environment::AccountEnvironment {
        self.codex_env.as_deref().cloned().unwrap_or_else(crate::accounts::environment::AccountEnvironment::capture)
    }
    /// Janelas de cota da conta, só quando vai formatar, lidas do módulo de contas (cache dele). Falha
    /// formata sem janelas; o log leva o código, nunca o dado.
    pub async fn quota_windows(&self,key:&str,config_dir:&str) -> Option<Value> {
        let accounts = self.accounts.as_ref()?;
        match accounts.claude_windows(config_dir).await {
            Ok(windows) => Some(json!({"windows":windows})),
            Err(code) => {
                if crate::warn_limit::allow(Some(key),code) { tracing::warn!(key=%key,code=%code,"cota do Claude indisponível; a linha sai sem janelas"); }
                None
            }
        }
    }
}

/// Sessão cujo processo o Rust sobe: pasta do arquivo dela e a primeira espera da religação.
#[derive(Clone)]
pub struct LaunchConfig { pub sidecar_dir:std::path::PathBuf, pub backoff:Duration }

/// Religação depois da queda do cano: um timer por sessão (`next_at`, no relógio do ator), até 3
/// subidas seguidas sem ficar pronta, cada espera o dobro da anterior.
#[derive(Default)]
struct Respawn {
    failures:u8,
    next_at:Option<f64>,
    task:Option<JoinHandle<Result<Relaunched,(RuntimeError,Value)>>>,
    /// O que uma subida que falhou já gravou no arquivo da sessão: a próxima vida parte dele.
    carried:Value,
    /// Operação da pessoa (reiniciar, trocar o sandbox) e o que ela recebe quando o processo novo conecta.
    user:Option<(String,Value)>,
    /// Reiniciar pedido durante uma religação automática: o processo novo dela é o pedido, e quem pediu
    /// recebe o desfecho dela (o Python serializava pela trava em vez de recusar).
    waiting:Vec<(String,Value)>,
    /// A queda entregou `cano_saiu`: o processo filho morreu e o cano que ficou pode ser encerrado.
    kill:bool,
    /// O cano caiu durante uma subida pedida: a queda é dela, não agenda outra.
    swallowed:bool,
    was_working:bool,
    last:Option<RuntimeError>,
}
const RESPAWN_MAX:u8 = 3;

struct Relaunched { target:RuntimeTarget, spawned:bool, connection:CanoConnection, patch:Value }

/// Transporte do ator. Só o cano é de processo que o Rust sobe, religa e mata; o WebSocket fala com
/// o app-server adotado, que sobrevive ao ator.
pub enum RuntimeConnection { Cano(CanoConnection), WebSocket(WebSocketConnection) }

impl From<CanoConnection> for RuntimeConnection { fn from(connection:CanoConnection) -> Self { Self::Cano(connection) } }
impl From<WebSocketConnection> for RuntimeConnection { fn from(connection:WebSocketConnection) -> Self { Self::WebSocket(connection) } }

type WriteSources = BTreeMap<String,(crate::interactions::ExpectedInteraction,Option<crate::interactions::ExpectedInteraction>,Arc<futures_util::task::AtomicWaker>)>;
/// Resposta reservada em escrita: operação → (pergunta, ainda aberta, quem acordar).
type AsyncGuards = BTreeMap<String,(String,Arc<AtomicBool>,Arc<futures_util::task::AtomicWaker>)>;

pub struct RuntimeEngine {
    interactions:Option<(String,Arc<crate::interactions::InteractionStore>)>,
    write_sources:Arc<std::sync::Mutex<WriteSources>>,
    async_guards:Arc<std::sync::Mutex<AsyncGuards>>,
    core:Core,
    launch:Option<LaunchConfig>,
    policy:Option<PolicyClient>,
    publisher:Option<broadcast::Sender<RuntimeEvent>>,
    revision:Arc<AtomicU64>,
    mods:Option<crate::mods::state::Mods>,
    /// A vida deste ator no `Mods` (`Mods::new_life`), com que ele publica e é esquecido.
    mods_life:u64,
    /// Canal em processo do hub: estado, erro e prévia saem por ele.
    live:Option<LiveSender>,
    lifecycle:Option<std::sync::Weak<super::gateway::RuntimeRegistry>>,
    /// Esperas da religação ao mesmo cano (Claude e Codex sem terminal).
    reconnect_waits:super::cano_reconnect::ReconnectWaits,
    /// Nascimento do cano desta vida: sem ele, a religação não prova que o pid ainda é o mesmo processo.
    cano_birth:Option<f64>,
}

impl RuntimeEngine {
    pub fn new(provider:&str,metadata:Value,generation:u64,clock:ClockSample) -> Result<Self,RuntimeError> {
        let core = match provider { "claude"=>Core::Claude(ClaudeEngine::new(metadata,generation,clock)),
            "codex"=>Core::Codex(CodexEngine::new_checked(metadata,generation,clock)?),_=>return Err(failure("provider")) };
        Ok(Self { interactions:None,write_sources:Default::default(),async_guards:Default::default(),core,launch:None,policy:None,publisher:None,revision:Arc::new(AtomicU64::new(0)),mods:None,mods_life:0,live:None,lifecycle:None,reconnect_waits:Default::default(),cano_birth:None })
    }
    /// O ator sobe o processo de novo quando ele cai, e reinicia/troca o sandbox (só Codex).
    pub fn with_launch(mut self,launch:LaunchConfig) -> Self { if matches!(self.core,Core::Codex(_)) { self.launch = Some(launch); } self }
    /// Motor da vida nova do processo, com as ligações desta.
    fn renewed(&self,metadata:Value,generation:u64,clock:ClockSample) -> Result<Self,RuntimeError> {
        let provider = match self.core { Core::Claude(_)=>"claude",Core::Codex(_)=>"codex" };
        let mut next = Self::new(provider,metadata,generation,clock)?;
        if let Some((key,store)) = &self.interactions {
            store.forget(key).map_err(crate::interactions::runtime_error)?;
            next = next.with_interactions(key.clone(),store.clone())?;
        }
        (next.launch,next.policy,next.publisher,next.revision,next.live,next.lifecycle) = (self.launch.clone(),self.policy.clone(),self.publisher.clone(),self.revision.clone(),self.live.clone(),self.lifecycle.clone());
        (next.reconnect_waits,next.cano_birth) = (self.reconnect_waits.clone(),self.cano_birth);
        Ok(next)
    }
    fn problem(&self) -> Option<String> { match &self.core { Core::Codex(core)=>core.problem().map(str::to_owned),Core::Claude(_)=>None } }
    fn queue_problem(&mut self,detail:Option<String>) -> Vec<Effect> {
        match &mut self.core { Core::Codex(core)=>core.queue_problem(detail),Core::Claude(core)=>core.queue_problem(detail) }
    }
    fn batch_validator(&self) -> super::queue::BatchValidator {
        let template=match &self.core {Core::Claude(_)=>ClaudeEngine::input_frame("batch",&json!({"text":""})),Core::Codex(core)=>core.input_frame_template()};
        let provider=match self.core {Core::Claude(_)=>"claude",Core::Codex(_)=>"codex"};
        local_policy::batch_validator(provider.into(),template)
    }
    fn set_problem(&mut self,code:&str,detail:Option<String>) -> Vec<Effect> {
        match &mut self.core { Core::Codex(core)=>core.set_problem(code,detail),Core::Claude(_)=>Vec::new() }
    }
    /// O problema que vem de fora do ator (`Message::Problem`), nos dois provedores.
    fn show_problem(&mut self,code:&str,detail:Option<String>) -> Vec<Effect> {
        match &mut self.core { Core::Codex(core)=>core.set_problem(code,detail),Core::Claude(core)=>core.set_problem(code,detail) }
    }
    pub fn with_interactions(mut self,key:String,store:Arc<crate::interactions::InteractionStore>) -> Result<Self,RuntimeError> {
        match &mut self.core {
            Core::Claude(core)=>core.set_interactions(key.clone(),store.clone()),
            Core::Codex(core)=>core.set_interactions(key.clone(),store.clone()),
        }?;
        self.interactions = Some((key,store));
        Ok(self)
    }
    pub fn with_policy(mut self,policy:PolicyClient) -> Self { self.policy = Some(policy); self }
    pub fn with_publisher(mut self,publisher:broadcast::Sender<RuntimeEvent>) -> Self { self.publisher = Some(publisher); self }
    pub fn with_revision(mut self,revision:Arc<AtomicU64>) -> Self { self.revision = revision; self }
    /// Canal em processo do hub: estado, erro e prévia da sessão sem terminal saem por ele, fora do `events`.
    pub fn with_live(mut self,live:LiveSender) -> Self { self.live = Some(live); self }
    /// Esperas da religação ao mesmo cano; por padrão `RECONNECT_WAITS`, `RECONNECT_WAITS_PENDING` e `DELIVERY_STALL`.
    pub fn with_reconnect_waits(mut self,normal:Vec<Duration>,pending:Vec<Duration>,stall:Duration) -> Self {
        self.reconnect_waits = super::cano_reconnect::ReconnectWaits { normal,pending,stall,..self.reconnect_waits }; self
    }
    /// Intervalo da conferência da vida do cano durante a religação (`CANO_CHECK` por padrão).
    pub fn with_cano_check(mut self,every:Duration) -> Self { self.reconnect_waits.check = every; self }
    pub fn with_cano_birth(mut self,birth:Option<f64>) -> Self { self.cano_birth = birth; self }
    /// Silêncio do cano, com o transcript já no fim do turno, que dá a saída dele como perdida (`OUTPUT_LOST` por padrão).
    pub fn with_output_lost(mut self,after:Duration) -> Self { self.reconnect_waits.output_lost = after; self }
    /// Claude sem terminal com turno aberto: o transcript é conferido contra o fim dele.
    fn turn_watch(&self) -> bool { matches!(&self.core,Core::Claude(core) if core.turn_watch()) }
    fn output_lost(&self) -> bool { matches!(&self.core,Core::Claude(core) if core.deaf()) }
    fn active_input(&self) -> Option<String> { match &self.core { Core::Claude(core)=>core.active_input().map(str::to_owned),Core::Codex(_)=>None } }
    fn heard(&mut self) -> Vec<Effect> { match &mut self.core { Core::Claude(core)=>core.heard(),Core::Codex(_)=>Vec::new() } }
    fn transcript_turn_end(&mut self) -> Vec<Effect> { match &mut self.core { Core::Claude(core)=>core.transcript_turn_end(),Core::Codex(_)=>Vec::new() } }
    pub(super) fn with_lifecycle(mut self,lifecycle:Option<std::sync::Weak<super::gateway::RuntimeRegistry>>)->Self {self.lifecycle=lifecycle;self}
    fn advance_claude_generation(&mut self,generation:u64)->Result<(),RuntimeError> {
        match &mut self.core {Core::Claude(core)=>core.advance_generation(generation),Core::Codex(_)=>Err(failure("lifecycle_provider"))}
    }
    /// Processo subido agora pelo Rust: a abertura do Codex repete a política (ver `codex::Engine`).
    pub fn set_fresh_process(&mut self,fresh:bool) { if let Core::Codex(core) = &mut self.core { core.set_fresh_process(fresh); } }
    /// Liga a interface dos mods: o Claude sem terminal vira superfície `desktop` e publica no `Mods`.
    /// O prefixo dos pedidos é único por ator, para a resposta de uma vida anterior não casar.
    pub fn with_mods(mut self,mods:crate::mods::state::Mods) -> Self {
        static ACTORS:AtomicU64 = AtomicU64::new(0);
        if let Core::Claude(core) = &mut self.core {
            core.enable_surface(format!("ui:{}.{}",std::process::id(),ACTORS.fetch_add(1,Ordering::Relaxed)));
            self.mods_life = mods.new_life();
            self.mods = Some(mods);
        }
        self
    }
    pub fn mods_life(&self) -> u64 { self.mods_life }
    fn mods_call(&mut self,token:u64,call:ModsCall,left_s:f64,clock:ClockSample) -> Result<Vec<Effect>,ModsError> {
        match &mut self.core { Core::Claude(core)=>core.mods_call(token,call,left_s,clock),Core::Codex(_)=>Err(crate::mods::model::missing()) }
    }
    fn park_idle(&self,now:f64) -> Option<f64> {
        match &self.core { Core::Claude(core)=>core.park_idle(now),Core::Codex(_)=>None }
    }
    fn view(&self) -> Value {
        match &self.core { Core::Claude(core)=>core.view(),Core::Codex(core)=> {
            let mut view = core.control_view(); view["public_state"] = core.view(); view["conversation"] = view["thread_id"].clone(); view
        } }
    }
    fn refresh_write_sources(core:&Core,sources:&mut WriteSources,guards:&AsyncGuards) {
        for (expected,current,waker) in sources.values_mut() {
            let next = match core { Core::Claude(core)=>core.interaction_source(expected),Core::Codex(core)=>core.interaction_source(expected) };
            if *current != next { *current = next; waker.wake(); }
        }
        if let Core::Codex(core) = core {
            for (request_id,open,waker) in guards.values() {
                let now = core.async_question_pending(request_id);
                if open.swap(now,Ordering::AcqRel) != now { waker.wake(); }
            }
        }
    }
    fn apply(&mut self,input:EngineInput,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        let mut sources = self.write_sources.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let guards = self.async_guards.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let result = match &mut self.core { Core::Claude(core)=>core.apply(input,clock),Core::Codex(core)=>core.apply(input,clock) };
        Self::refresh_write_sources(&self.core,&mut sources,&guards);
        result
    }
    fn command(&mut self,command:RuntimeCommand,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        let mut sources = self.write_sources.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let guards = self.async_guards.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let result = match &mut self.core { Core::Claude(core)=>core.command(command,clock),Core::Codex(core)=>core.command(command,clock) };
        Self::refresh_write_sources(&self.core,&mut sources,&guards);
        result
    }
    fn hydrate(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        let mut sources = self.write_sources.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let guards = self.async_guards.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let result = match &mut self.core { Core::Claude(core)=>core.hydrate(snapshot),Core::Codex(core)=>core.hydrate(snapshot) };
        Self::refresh_write_sources(&self.core,&mut sources,&guards);
        result
    }
    /// Só o socket caiu (Codex com terminal adotado, Claude e Codex sem terminal): a fila segura até reconectar.
    fn transport_lost(&mut self,detail:&str,handed:&BTreeSet<String>) -> Result<Vec<Effect>,RuntimeError> {
        let mut sources = self.write_sources.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let guards = self.async_guards.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let effects = match &mut self.core { Core::Codex(core)=>core.transport_lost(detail,handed),Core::Claude(core)=>core.transport_lost(handed) };
        Self::refresh_write_sources(&self.core,&mut sources,&guards);
        Ok(effects)
    }
    /// Socket novo ao mesmo cano sem terminal: o retrato dele repõe o estado; o motor (e as identidades) segue.
    fn reattach(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        let mut sources = self.write_sources.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let guards = self.async_guards.lock().map_err(|_|crate::interactions::runtime_error(crate::interactions::InteractionError::Unavailable))?;
        let result = match &mut self.core { Core::Claude(core)=>core.reattach(snapshot),Core::Codex(core)=>core.reattach(snapshot) };
        Self::refresh_write_sources(&self.core,&mut sources,&guards);
        result
    }
    fn clear_problem(&mut self,code:&str) -> Vec<Effect> {
        match &mut self.core { Core::Claude(core)=>core.clear_problem(code),Core::Codex(core)=>core.clear_problem(code) }
    }
    fn interaction_guard(&self,id:&str) -> Result<Option<hangar_codex::client::RawWriteGuard>,RuntimeError> {
        use crate::interactions::{InteractionError,runtime_error};
        // Resposta reservada: o id gravado pode ser de outra vida, então a guarda confere só que a pergunta segue aberta.
        if let Core::Codex(core) = &self.core && let Some(request_id) = core.reserved_question(id) {
            let open = Arc::new(AtomicBool::new(core.async_question_pending(&request_id)));
            let waker = Arc::new(futures_util::task::AtomicWaker::new());
            self.async_guards.lock().map_err(|_|runtime_error(InteractionError::Unavailable))?.insert(id.into(),(request_id,open.clone(),waker.clone()));
            return Ok(Some(Arc::new(move |waiting| {
                waker.register(waiting);
                if open.load(Ordering::Acquire) { Ok(()) } else { Err(InteractionError::Changed.code().to_owned()) }
            })));
        }
        let expected = match &self.core { Core::Claude(core)=>core.write_interaction(id),Core::Codex(core)=>core.write_interaction(id) };
        let Some(expected) = expected.cloned() else { return Ok(None); };
        let (_,store) = self.interactions.as_ref().ok_or_else(||runtime_error(InteractionError::Unavailable))?;
        let current = match &self.core { Core::Claude(core)=>core.interaction_source(&expected),Core::Codex(core)=>core.interaction_source(&expected) };
        let waker = Arc::new(futures_util::task::AtomicWaker::new());
        self.write_sources.lock().map_err(|_|runtime_error(InteractionError::Unavailable))?.insert(id.into(),(expected.clone(),current,waker.clone()));
        let (sources,store,id) = (self.write_sources.clone(),store.clone(),id.to_owned());
        Ok(Some(Arc::new(move |waiting| {
            waker.register(waiting);
            let check = || {
                let sources = sources.lock().map_err(|_|InteractionError::Unavailable)?;
                let current = sources.get(&id).and_then(|(_,current,_)|current.as_ref()).ok_or(InteractionError::Changed)?;
                if current != &expected || store.expected(&expected.binding,Some(&expected.interaction_id))? != expected { return Err(InteractionError::Changed); }
                Ok(())
            };
            check().map_err(|error:InteractionError|error.code().to_owned())
        })))
    }
    fn deadline(&self) -> Option<f64> { match &self.core { Core::Claude(core)=>core.next_deadline(),Core::Codex(core)=>core.next_deadline() } }
    fn initialize(&mut self,id:String) -> Result<Vec<Effect>,RuntimeError> {
        match &mut self.core { Core::Claude(core)=>core.start_initialize(id),Core::Codex(core)=>core.bootstrap(true,id) }
    }
    fn interaction_write_error(&self,id:&str) -> Option<RuntimeError> {
        match &self.core { Core::Claude(core)=>core.interaction_write_error(id),Core::Codex(core)=>core.interaction_write_error(id) }
    }
    fn write_is_current(&self,id:&str) -> bool { match &self.core { Core::Claude(core)=>core.write_is_current(id),Core::Codex(core)=>core.write_is_current(id) } }
    fn volatile_write(&self,id:&str) -> bool { match &self.core { Core::Claude(_)=>false,Core::Codex(core)=>core.volatile_write(id) } }
    fn forget_policy(&mut self,id:&RequestId) { match &mut self.core { Core::Claude(core)=>core.forget_policy(id),Core::Codex(core)=>core.forget_policy(id) } }
    fn confirm_input(&mut self,id:&str) -> Vec<Effect> {
        match &mut self.core { Core::Claude(_)=>vec![Effect::Reply { operation_id:id.into(),disposition:Disposition::Accepted,payload:json!({"confirmed":true}) }],
            Core::Codex(core)=>core.confirm_input(id) }
    }
    /// Entrada reservada que o drain pode reivindicar sozinha (só Codex com terminal).
    fn ready_async_answer_entry(&self) -> Option<String> {
        match &self.core { Core::Codex(core)=>core.ready_async_answer_entry(),Core::Claude(_)=>None }
    }
    fn settle_async_answer(&mut self,entry_id:&str,reserved:bool) -> Vec<Effect> {
        match &mut self.core { Core::Codex(core)=>core.settle_async_answer(entry_id,reserved),Core::Claude(_)=>Vec::new() }
    }
    fn uncertain_async_answer(&mut self,entry_id:&str) {
        if let Core::Codex(core) = &mut self.core { core.uncertain_async_answer(entry_id); }
    }
    fn restore(&mut self,state:&super::queue::State) {
        // Reservas antes das fases: a resposta tardia de um `turn/start` restaurado precisa achar a dela.
        if let Core::Codex(core) = &mut self.core {
            for op in state.rows.iter().filter_map(|row|row["id"].as_str().and_then(|id|state.operations.get(id)))
                .filter(|op|op.payload["kind"] == "dispatch_async_answer") {
                if let Ok(reservation) = serde_json::from_value::<ReservedAsyncAnswer>(op.payload["payload"].clone()) {
                    // Gravada em outra geração (ou sem ela): a vida não se prova, então fica incerta e nunca sai.
                    let proven = op.payload["payload"]["generation"].as_u64() == Some(state.generation);
                    let status = if proven || matches!(op.status,Status::Accepted | Status::Confirmed | Status::Rejected) { op.status } else { Status::Unknown };
                    core.restore_async_answer(reservation,op.payload["payload"]["thread_id"].as_str().unwrap_or(""),status,maybe_written(state,op) || !proven);
                }
            }
        }
        for phase in state.operations.values().filter(|phase|recover_phase(state,phase)) {
            if let Some(id) = phase.payload["logical_id"].as_str() {
                match &mut self.core {
                    Core::Codex(core)=>{
                        core.restore_rpc(id.into(),&phase.payload["frame"],phase.payload["state_revision"].as_u64().unwrap_or(0),
                            phase.payload["settings_revision"].as_u64().unwrap_or(0));
                    },
                    Core::Claude(core)=>{
                        let mut frame = phase.payload["frame"].clone();
                        if frame["request"]["subtype"] == "set_model" {
                            if let Some(effort) = state.operations.get(id).and_then(|root|root.payload["payload"].get("effort")).filter(|effort|effort.is_string()) {
                                frame["request"]["effort"] = effort.clone();
                            }
                        }
                        core.restore_control(id.into(),&frame);
                    },
                }
            }
        }
    }
}

fn failure(code:&str) -> RuntimeError { RuntimeError::new(code,"runtime indisponível; operação conservada no diário") }
fn io_failure(error:std::io::Error) -> RuntimeError {
    if super::queue::refusal(&error) == Some(super::queue::BATCH_SIZE_MESSAGE) {
        return RuntimeError::new(super::queue::BATCH_TOO_LARGE,"O lote é grande demais. Edite ou remova mensagens da fila para enviar.");
    }
    // Recusa da fila tem frase fixa e vai inteira (log e diário do Python); de resto só o tipo, porque
    // a mensagem do io::Error ou do serde pode trazer caminho ou texto.
    let reason = super::queue::refusal(&error).map_or_else(||format!("{:?}",error.kind()),str::to_owned);
    if crate::warn_limit::allow(None,&format!("queue_io:{reason}")) { tracing::warn!(reason=%reason,"runtime falhou em E/S (diário ou transcript)"); }
    RuntimeError::new("queue_io",&format!("fila recusou: {reason}"))
}

/// Loga só na entrada em erro ou na troca de código: o mesmo erro repetido não enche o log.
fn enter_error(error:&mut Option<RuntimeError>,target:&RuntimeTarget,failure:RuntimeError) {
    if error.as_ref().is_none_or(|current|current.code != failure.code) {
        tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,reason=%failure.message,"runtime entrou em erro");
    }
    *error = Some(failure);
}
fn clock(start:Instant) -> ClockSample {
    let epoch_s = match SystemTime::now().duration_since(UNIX_EPOCH) { Ok(time)=>time.as_secs_f64(),Err(error)=>-error.duration().as_secs_f64() };
    ClockSample { monotonic_s:start.elapsed().as_secs_f64(),epoch_s }
}

#[derive(Clone)]
struct ModeProof {binding:crate::interactions::InteractionBinding,operation_id:String,mode:String}
struct ModeCommit {proof:ModeProof,digest:String}
fn mode_ready_proof(target:&RuntimeTarget,view:&Value,commit:Option<&ModeCommit>,proof:&ModeProof,digest:&str,error:Option<&RuntimeError>)->Result<(),RuntimeError> {
    if let Some(error)=error {return Err(error.clone());}
    let commit=commit.ok_or_else(||failure("lifecycle_mode_not_persisted"))?;
    if target.metadata["lifecycle_pending"]!=true || proof.binding.key!=target.key || proof.binding.generation!=target.generation
        || target.metadata["session_id"]!=proof.binding.conversation || view["conversation"]!=proof.binding.conversation
        || view["permission_mode"]!=proof.mode || commit.proof.binding!=proof.binding
        || commit.proof.operation_id!=proof.operation_id || commit.proof.mode!=proof.mode || commit.digest!=digest {return Err(failure("lifecycle_commit_changed"));}
    Ok(())
}
/// Por que não há o que orientar agora (as recusas do adapter Python): sem turno, ou o Claude parado numa
/// permissão ou pergunta, que o stdin não lê. O código deixa a rota separar "o turno acabou" de "responda antes".
fn steer_refusal(view:&Value,provider:&str)->Option<(&'static str,&'static str)> {
    if view["alive"] != true || view["in_progress"] != true { return Some(("steer_no_turn","Não há turno em andamento para orientar")); }
    if provider == "claude" && (view["pending"].as_array().is_some_and(|p|!p.is_empty()) || !view["question"].is_null()) {
        return Some(("steer_question_open","Responda a permissão ou pergunta pendente antes de orientar"));
    }
    None
}
fn claude_delivery_allowed(target:&RuntimeTarget,view:&Value,error:bool)->bool {view["deliverable"]==true && !error && target.metadata["lifecycle_pending"]!=true}
fn block_claude_sid(target:&mut RuntimeTarget,view:&Value)->bool {
    let changed=target.provider=="claude" && view["conversation"].as_str().is_some_and(|sid|!sid.is_empty() && target.metadata["session_id"]!=sid);
    if changed {target.metadata["lifecycle_pending"]=json!(true);}
    changed
}

type Response = oneshot::Sender<Result<RuntimeReply,RuntimeError>>;
fn join_active_pending(pending:&mut Pending,command:&RuntimeCommand,response:Response)->Result<(),Response> {
    if pending.result.is_some() || pending.error.is_some() {return Err(response);}
    if pending.original!=serde_json::to_value(command).unwrap() {let _=response.send(Err(failure("operation_reused")));}
    else {pending.responses.push(response);}
    Ok(())
}
enum Message {
    Command { command:RuntimeCommand,response:Response,from_queue:bool,generation:u64 },
    Queue { call_id:String,action:Action,response:oneshot::Sender<Result<Value,RuntimeError>>,generation:u64 },
    Snapshot(oneshot::Sender<Result<Value,RuntimeError>>),
    /// Vista do motor agora, sem esperar a gravação: o que uma troca já respondida deixou valendo.
    View(oneshot::Sender<Value>),
    Drain(oneshot::Sender<Result<Value,RuntimeError>>),
    Confirm(oneshot::Sender<Result<Value,RuntimeError>>),
    /// `deadline`: quando quem pediu deixa de esperar (o prazo da rota, limitado ao teto do ator). Pedido
    /// que chega à vez depois disso não roda, e a superfície recebe o que sobra dele.
    Mods { call:ModsCall,deadline:Instant,response:oneshot::Sender<Result<Value,ModsError>> },
    /// `kill`: o processo da sessão morre junto (encerrar a sessão).
    Stop { response:oneshot::Sender<Result<(),RuntimeError>>,kill:bool },
    LifecycleReady {proof:ModeProof,response:oneshot::Sender<Result<(),RuntimeError>>},
    HoldTurns(oneshot::Sender<Result<TurnHold,RuntimeError>>),
    /// Problema que o motor não vê (a troca por limite que não aconteceu); sai no próximo turno bom.
    Problem { code:String,detail:Option<String> },
    /// Gatilho (conversa aberta no app): antecipa a religação ao cano já marcada; sem ela, nada.
    WakeReconnect,
}

/// Problema no estado: mensagem esperando além de `DELIVERY_STALL` com o cano vivo e o socket fora.
pub const DELIVERY_STALLED:&str = "headless_delivery_stalled";

/// Quando entrou na fila a mensagem mais antiga ainda por entregar (relógio de parede).
fn oldest_waiting(state:&super::queue::State,created:f64) -> Option<f64> {
    let waiting = super::queue::waiting_ids(state,created);
    state.rows.iter().filter(|row|row["id"].as_str().is_some_and(|id|waiting.iter().any(|w|w == id)))
        .map(|row|row["ts"].as_f64().unwrap_or(0.0)).min_by(f64::total_cmp)
}

/// Recusa da `hold_turns`: turno em voo, pedido do app-server pendente ou fila já despachando.
pub const TURN_HOLD_BUSY:&str = "turn_hold_busy";

/// Enquanto viva, o ator desta vida não abre turno (`turn/start` de entrada comum ou de resposta
/// reservada): elas adiam e saem uma vez quando a última cópia cai. Soltar é só largar o valor, então
/// erro, cancelamento e pânico de quem segura também soltam; vida nova do ator não a herda.
pub struct TurnHold { token:Option<Arc<()>>,released:Arc<Notify> }

impl Drop for TurnHold {
    fn drop(&mut self) {
        // O token cai antes do aviso: acordado, o ator já vê a contagem sem esta cópia.
        drop(self.token.take());
        self.released.notify_one();
    }
}

#[derive(Clone)]
pub struct RuntimeHandle {
    sender:mpsc::Sender<Message>,
    task:Arc<Mutex<Option<JoinHandle<Result<(),RuntimeError>>>>>,
    closed:Arc<AtomicBool>,
    events:broadcast::Sender<RuntimeEvent>,
    stopped:Arc<Mutex<Option<Result<(),RuntimeError>>>>,
    key:String,
    generation:u64,
}

impl RuntimeHandle {
    pub(super) fn with_generation(mut self,generation:u64)->Self {self.generation=generation;self}
    pub async fn command(&self,command:RuntimeCommand) -> Result<RuntimeReply,RuntimeError> {
        if self.closed.load(Ordering::Acquire) { return Err(failure("runtime_stopping")); }
        let (response,receive) = oneshot::channel();
        self.sender.send(Message::Command { command,response,from_queue:false,generation:self.generation }).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    pub async fn queue(&self,call_id:String,action:Action) -> Result<Value,RuntimeError> {
        let (response,receive) = oneshot::channel();
        self.sender.send(Message::Queue { call_id,action,response,generation:self.generation }).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    pub async fn snapshot(&self) -> Result<Value,RuntimeError> {
        let (send,receive) = oneshot::channel();
        self.sender.send(Message::Snapshot(send)).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    pub async fn view(&self) -> Result<Value,RuntimeError> {
        let (send,receive) = oneshot::channel();
        self.sender.send(Message::View(send)).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))
    }
    /// Antecipa a religação ao cano que está esperando; caixa cheia ou ator fechado não importam.
    pub fn wake_reconnect(&self) { let _ = self.sender.try_send(Message::WakeReconnect); }
    /// Ator de mentira que só conta os pedidos de religação; o resto da caixa cai sem resposta.
    #[cfg(test)]
    pub(super) fn counting_wakes(key:&str,generation:u64) -> (Self,Arc<std::sync::atomic::AtomicUsize>) {
        let (sender,mut inbox) = mpsc::channel(8);
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = wakes.clone();
        tokio::spawn(async move { while let Some(message) = inbox.recv().await {
            if matches!(message,Message::WakeReconnect) { counted.fetch_add(1,Ordering::SeqCst); }
        } });
        (Self { sender,task:Arc::new(Mutex::new(None)),closed:Arc::new(AtomicBool::new(false)),events:broadcast::channel(1).0,
            stopped:Arc::new(Mutex::new(None)),key:key.into(),generation },wakes)
    }
    pub async fn drain(&self) -> Result<Value,RuntimeError> {
        let (send,receive) = oneshot::channel(); self.sender.send(Message::Drain(send)).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    pub async fn show_problem(&self,code:&str,detail:Option<String>) -> Result<(),RuntimeError> {
        self.sender.send(Message::Problem { code:code.to_owned(),detail }).await.map_err(|_|self.gone("runtime_closed"))
    }
    pub async fn confirm(&self) -> Result<Value,RuntimeError> {
        let (send,receive) = oneshot::channel(); self.sender.send(Message::Confirm(send)).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    /// Pedido de um app à interface dos mods desta sessão, com o prazo de quem pediu (a rota), limitado ao
    /// teto do ator. O prazo vai até a superfície, que não leva ação ao mod sem tempo para a resposta
    /// voltar. Ator parado ou sumido responde com código, nunca pendura o app.
    pub async fn mods(&self,call:ModsCall,deadline:Instant) -> Result<Value,ModsError> {
        if self.closed.load(Ordering::Acquire) { return Err(crate::mods::model::no_answer()); }
        let (response,receive) = oneshot::channel();
        let deadline = deadline.min(Instant::now() + MODS_CALL_LIMIT);
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline),async {
            self.sender.send(Message::Mods { call,deadline,response }).await.map_err(|_|crate::mods::model::no_answer())?;
            receive.await.map_err(|_|crate::mods::model::no_answer())?
        }).await.unwrap_or_else(|_|Err(crate::mods::model::no_answer()))
    }
    pub(super) async fn lifecycle_ready(&self,binding:crate::interactions::InteractionBinding,operation_id:String,mode:String)->Result<(),RuntimeError> {
        let (response,receive)=oneshot::channel();
        self.sender.send(Message::LifecycleReady {proof:ModeProof {binding,operation_id,mode},response}).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    pub async fn ensure_projection(&self) -> Result<Value,RuntimeError> {
        self.queue(format!("projection:{}",unique()),Action::EnsureProjection).await
    }
    /// Segura os turnos novos desta vida (seletor da TUI). A checagem de ocupado e a retenção acontecem
    /// no mesmo passo do ator, então nenhum `turn/start` passa entre as duas. Recusa: `TURN_HOLD_BUSY`.
    pub async fn hold_turns(&self) -> Result<TurnHold,RuntimeError> {
        if self.closed.load(Ordering::Acquire) { return Err(failure("runtime_stopping")); }
        let (send,receive) = oneshot::channel();
        self.sender.send(Message::HoldTurns(send)).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    /// O motivo real já saiu na linha de saída do ator; aqui fica qual chave o perdeu.
    fn gone(&self,code:&str) -> RuntimeError {
        if crate::warn_limit::allow(Some(&self.key),code) { tracing::warn!(key=%self.key,code,"runtime sem ator"); }
        failure(code)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> { self.events.subscribe() }
    pub async fn stop(&self) -> Result<(),RuntimeError> { self.stop_with(false).await }
    /// Para o ator e mata o processo da sessão, com os arquivos do cano.
    pub async fn stop_killing(&self) -> Result<(),RuntimeError> { self.stop_with(true).await }
    async fn stop_with(&self,kill:bool) -> Result<(),RuntimeError> {
        let mut stopped = self.stopped.lock().await;
        if let Some(result) = &*stopped { return result.clone(); }
        self.closed.store(true,Ordering::Release);
        let (send,receive) = oneshot::channel();
        let mut result = match self.sender.send(Message::Stop { response:send,kill }).await {
            Ok(())=>receive.await.map_err(|_|self.gone("runtime_closed")).and_then(|result|result),
            Err(_)=>Err(self.gone("runtime_closed")),
        };
        if let Some(task) = self.task.lock().await.take() {
            let joined = task.await.map_err(|_|self.gone("runtime_panic")).and_then(|result|result);
            if joined.is_err() { result = joined; }
        }
        *stopped = Some(result.clone());
        result
    }
}

impl crate::mods::state::SurfaceLink for RuntimeHandle {
    fn call(&self,call:ModsCall,deadline:Instant) -> crate::mods::state::CallFuture {
        let handle = self.clone();
        Box::pin(async move { handle.mods(call,deadline).await })
    }
}

struct Pending {
    from_queue:bool,
    defer_for_batch:bool,
    command:RuntimeCommand,
    original:Value,
    responses:Vec<Response>,
    result:Option<RuntimeReply>,
    preparing:bool,
    cancelled:bool,
    error:Option<RuntimeError>,
    deadline:f64,
    timed_out:bool,
    ready_to_run:bool,
    arrival:u64,
}

impl Pending {
    fn stored(command:RuntimeCommand,reply:RuntimeReply) -> Self {
        Self { from_queue:false,defer_for_batch:false,original:serde_json::to_value(&command).unwrap(),command,responses:Vec::new(),result:Some(reply),preparing:false,
            cancelled:false,error:None,deadline:0.0,timed_out:false,ready_to_run:false,arrival:0 }
    }
}

#[derive(Clone)]
struct SidCommitAttempt {generation:u64,previous:String,patch:Value}
impl SidCommitAttempt {
    fn current(&self,target:&RuntimeTarget,view:&Value,closed:bool)->bool {
        !closed && self.generation==target.generation && target.metadata["session_id"]==self.previous && self.patch["session_id"]==view["conversation"]
    }
    fn retry_at(&self,target:&RuntimeTarget,view:&Value,closed:bool,failed:&RuntimeError,now:f64)->Option<f64> {
        (failed.code=="lifecycle_sid_in_transfer" && self.current(target,view,closed)).then_some(now+0.05)
    }
}
fn spawn_sid_commit(jobs:&mut JoinSet<Job>,registry:Option<std::sync::Weak<super::gateway::RuntimeRegistry>>,target:RuntimeTarget,queue:Arc<QueueActor>,context:SidCommitAttempt) {
    jobs.spawn(async move {
        let result=match registry.and_then(|registry|registry.upgrade()) {Some(registry)=>registry.commit_claude_sid(&target,&queue,&context.patch).await,None=>Err(failure("state_lifecycle_unbound"))};
        Job::ClaudeSid {context,result}
    });
}

#[derive(Clone)]
/// `volatile`: escrita fora do diário (assinatura da TUI); preparo, recibo e desfecho não tocam a fila.
struct Attempt { logical_id:String,phase_id:String,frame:Value,order:u64,epoch:u64,generation:u64,volatile:bool }
fn next_current_write<'a>(ready:&mut BTreeMap<u64,(String,u64,Result<(),RuntimeError>)>,next:&mut u64,attempts:&'a BTreeMap<String,Attempt>,generation:u64)->Option<(&'a Attempt,Result<(),RuntimeError>)> {
    while let Some((wire,source,result))=ready.remove(next) {
        *next+=1;
        let Some(attempt)=attempts.get(&wire) else {continue};
        if source!=generation || attempt.generation!=generation {continue;}
        return Some((attempt,result));
    }
    None
}

enum Job {
    Root { id:String,result:Result<Option<RuntimeReply>,RuntimeError> },
    Write { wire:String,generation:u64,result:Result<(),RuntimeError> },
    Ack { logical_id:String,outcome:WriteOutcome,guard_error:Option<RuntimeError>,result:Result<(),RuntimeError> },
    Finished {generation:u64,reply:RuntimeReply,result:Result<(),RuntimeError> },
    Policy { request_id:RequestId,kind:String,phase_id:String,conversation:Option<Value>,result:Result<Value,RuntimeError> },
    PreparedInput { id:String,result:Result<Value,RuntimeError> },
    Saved(Result<(),RuntimeError>),
    ClaudeSid {context:SidCommitAttempt,result:Result<RuntimeTarget,RuntimeError>},
    Queued { wake:bool,result:Result<(),RuntimeError> },
    View { generation:u64,version:u64,view:Value,result:Result<(),RuntimeError> },
    Drained {generation:u64,result:Result<Vec<RuntimeCommand>,RuntimeError>},
    DrainFinished(Result<RuntimeReply,RuntimeError>),
    Confirmed { response:Option<oneshot::Sender<Result<Value,RuntimeError>>>,result:Result<(Vec<String>,usize,Option<f64>),RuntimeError> },
    Steered { id:String,result:Result<Vec<String>,RuntimeError> },
    /// Conferência do fim do turno no transcript. `since`: a última saída do cano quando ela começou.
    TurnCheck { since:f64,result:Result<Option<(u64,Option<f64>)>,RuntimeError> },
    NativeInput { id:String,result:Result<Value,RuntimeError> },
    /// `persisted`: o que o disco tem depois da gravação (None = nem a leitura respondeu).
    Reserved { operation_id:String,reservation:ReservedAsyncAnswer,result:Result<(),RuntimeError>,persisted:Option<bool> },
}

/// Identificador de operação da fila. O diário guarda recibos de servidores anteriores e o pid se
/// repete entre reinícios: sem a marca de subida, o contador zerado colide com um recibo antigo.
fn unique() -> String {
    static COUNTER:std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static BOOT:std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let boot = BOOT.get_or_init(||boot_id(std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).map_or(0,|d|d.as_nanos())));
    format!("{boot}:{}",COUNTER.fetch_add(1,Ordering::Relaxed))
}

fn boot_id(pid:u32,nanos:u128) -> String { format!("{pid}-{nanos}") }

pub struct RuntimeActor;

impl RuntimeActor {
    pub fn spawn(target:RuntimeTarget,queue:QueueActor,connection:impl Into<RuntimeConnection>,engine:RuntimeEngine) -> RuntimeHandle {
        Self::spawn_with(target,queue,connection,engine,|_|{})
    }

    /// Como `spawn`, mas `before` recebe o handle antes de a tarefa do ator existir: o que ele registrar
    /// (o dono no `Mods`) já está lá quando o ator dá o primeiro passo. Pedidos que chegarem nesse meio
    /// esperam na caixa.
    pub fn spawn_with(target:RuntimeTarget,queue:QueueActor,connection:impl Into<RuntimeConnection>,engine:RuntimeEngine,
        before:impl FnOnce(&RuntimeHandle)) -> RuntimeHandle {
        Self::start(target,queue,Some(connection.into()),engine,before,None)
    }

    /// Claude sem terminal cujo cano vivo não atendeu a abertura: o ator nasce como depois de uma queda do
    /// socket (fila segurando, religação marcada) e hidrata e inicializa na primeira conexão.
    pub fn spawn_unconnected_with(target:RuntimeTarget,queue:QueueActor,engine:RuntimeEngine,before:impl FnOnce(&RuntimeHandle)) -> RuntimeHandle {
        Self::start(target,queue,None,engine,before,None)
    }

    /// Codex com terminal: o WebSocket do app-server adotado. A queda do socket reconecta ao mesmo
    /// processo (revalidado em `adoption`), sem religar, subir nem matar nada.
    pub fn spawn_adopted(target:RuntimeTarget,queue:QueueActor,connection:WebSocketConnection,engine:RuntimeEngine,
        adoption:super::codex_terminal::Adoption) -> RuntimeHandle {
        Self::start(target,queue,Some(RuntimeConnection::WebSocket(connection)),engine,|_|{},Some(adoption))
    }

    fn start(target:RuntimeTarget,queue:QueueActor,connection:Option<RuntimeConnection>,engine:RuntimeEngine,
        before:impl FnOnce(&RuntimeHandle),adoption:Option<super::codex_terminal::Adoption>) -> RuntimeHandle {
        let (sender,receiver) = mpsc::channel(64);
        let events = engine.publisher.clone().unwrap_or_else(||broadcast::channel(256).0);
        let closed = Arc::new(AtomicBool::new(false));
        let (key,name,life) = (target.key.clone(),target.name.clone(),engine.mods_life);
        let mods = engine.mods.clone();
        let live = engine.live.clone();
        let handle = RuntimeHandle { sender:sender.clone(),task:Arc::new(Mutex::new(None)),closed:closed.clone(),events:events.clone(),
            stopped:Arc::new(Mutex::new(None)),key:key.clone(),generation:target.generation };
        // O slot fica preso até receber a tarefa: um `stop` que chegue antes espera por ele e junta a tarefa.
        let slot = handle.task.clone();
        let mut slot = slot.try_lock().expect("slot da tarefa recém-criado");
        before(&handle);
        let run = run(target,queue,connection,engine,receiver,sender,closed,events,adoption);
        *slot = Some(tokio::spawn(async move {
            let mut guard = ClearOnDrop { mods,name,life };
            let mut panic = LivePanicMark(live);
            let result = run.await;
            let live = panic.0.take();
            // Saída por `?` deixava o ator mudo: só sobrava o runtime_closed de quem chamasse depois.
            if let Err(error) = &result {
                tracing::warn!(key=%key,session=%guard.name,code=%error.code,reason=%error.message,"ator do runtime terminou com erro");
                // O hub mostra a saída com erro, inclusive depois do `close`.
                if let Some(live) = &live { mark_live_error(live,&error.code,&error.message); }
            } else {
                // Saída normal (`stop` ou caixa fechada): quem limpa a faixa é o `close`, com o `forget`.
                guard.mods = None;
            }
            result
        }));
        drop(slot);
        handle
    }
}

/// Erro durável no canal do hub, sobre o último valor.
pub(crate) fn mark_live_error(live:&LiveSender,code:&str,message:&str) {
    live.send_modify(|value| {
        let mut next = value.as_deref().cloned().unwrap_or_default();
        next.error = Some((code.to_owned(),message.to_owned()));
        *value = Some(Arc::new(next));
    });
}

/// Armada durante a vida do ator: em pânico ela é solta sem desarmar, e o hub passa a mostrar o
/// problema em vez do último estado (o canal não fecha, o registro ainda segura o emissor).
struct LivePanicMark(Option<LiveSender>);

impl Drop for LivePanicMark {
    fn drop(&mut self) {
        if let Some(live) = &self.0 { mark_live_error(live,"runtime_panic","o ator do runtime caiu"); }
    }
}

/// Armada durante a vida do ator: se ele sair com erro ou em pânico, sem `cano_saiu`, a superfície não
/// limpou e os apps ficariam com botões mortos até o `close`. Ao ser solta, publica a interface vazia;
/// a posse fica, quem a solta é o `close`. Desarmada com `mods = None`.
struct ClearOnDrop { mods:Option<crate::mods::state::Mods>,name:String,life:u64 }

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        if let Some(mods) = &self.mods { mods.clear_ui(&self.name,self.life); }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(mut target:RuntimeTarget,queue:QueueActor,connection:Option<RuntimeConnection>,mut engine:RuntimeEngine,
    mut receiver:mpsc::Receiver<Message>,internal:mpsc::Sender<Message>,closed:Arc<AtomicBool>,events:broadcast::Sender<RuntimeEvent>,
    adoption:Option<super::codex_terminal::Adoption>) -> Result<(),RuntimeError> {
    let start = Instant::now();
    let initial = queue.initial_state().clone();
    engine.restore(&initial);
    // Socket novo nunca herda `connected`/`subscribed` da vista salva: inicializa e assina de novo.
    let adopted = matches!(connection,Some(RuntimeConnection::WebSocket(_)));
    let unconnected = connection.is_none();
    let (snapshot,io) = match connection {
        Some(RuntimeConnection::Cano(connection)) => (Some(connection.snapshot.clone()),connection.start(target.generation,128)),
        Some(RuntimeConnection::WebSocket(connection)) => {
            // Sem `launch`, religar e matar recusam: o app-server adotado não é do ator.
            engine.launch = None;
            (None,connection.start(target.generation,128))
        }
        None => (None,super::codex_terminal::closed_io()),
    };
    let mut io = io.hold_lease(queue.lease());
    let queue = Arc::new(queue);
    let mut jobs = JoinSet::new();
    let mut roots:BTreeMap<String,Pending> = BTreeMap::new();
    let preparation = Arc::new((Mutex::new(1u64),Notify::new()));
    let mut arrival = 0u64;
    let mut attempts:BTreeMap<String,Attempt> = initial.operations.values().filter(|phase|recover_phase(&initial,phase))
        .filter_map(|phase|Some((phase.id.clone(),Attempt { logical_id:phase.payload["logical_id"].as_str()?.into(),
            phase_id:phase.id.clone(),frame:phase.payload["frame"].clone(),order:0,epoch:0,generation:phase.payload["generation"].as_u64().unwrap_or(target.generation),volatile:false }))).collect();
    // Época local do transporte adotado: sobe na queda e na reconexão, e escrita preparada em outra época
    // não sai no socket novo.
    let mut transport_epoch = 0u64;
    // Uma vaga de religação por ator: o app-server adotado do Codex com terminal ou o cano do Claude sem terminal.
    let mut reconnect = super::cano_reconnect::CanoReconnect::new(if adoption.is_some() { super::codex_terminal::reconnect_waits() } else { engine.reconnect_waits.clone() });
    // Claude sem terminal com o socket fora e o cano vivo: a fila espera a religação, sem estado novo.
    let mut cano_lost = false;
    // Próxima conferência da vida do cano, fora das tentativas.
    let mut cano_check_at:Option<f64> = None;
    // O cano morreu (código de saída, quando o retrato o trouxe): queda real, tratada no topo do laço.
    let mut cano_gone:Option<Option<i64>> = None;
    let mut lost_at = 0.0_f64;
    // Aviso de entrega parada: quando sai (relógio do ator), se já saiu e se a fila mudou desde a conta.
    let mut stall_due:Option<f64> = None;
    let mut stall_shown = false;
    let mut stall_dirty = false;
    // Claude sem terminal: última linha da CLI pelo cano (relógio do ator) e a conferência do fim do turno no
    // transcript, para quando o cano para de repassar a saída e o `result` nunca chega.
    let mut last_output = clock(start).monotonic_s;
    let mut turn_checking = false;
    let mut turn_check_after = 0.0_f64;
    let mut output_lost_reported = false;
    // Fim de turno sem carimbo (início da linha) e quando ele foi visto pela primeira vez, no relógio de parede.
    let mut end_seen:Option<(u64,f64)> = None;
    // Operações cujo quadro foi entregue ao transporte desta época: na queda, só estas podem ter saído.
    let mut handed:BTreeSet<String> = BTreeSet::new();
    let mut effects:VecDeque<Effect> = VecDeque::new();
    let mut revision = Revision { value:engine.revision.load(Ordering::Acquire),counter:engine.revision.clone() };
    let mut state_version = 0u64;
    let mut published_state_version = 0u64;
    let state_gate = Arc::new(Mutex::new(SavedView::default()));
    let mut durable_view = json!({"alive":true,"initialized":false,"ready":false});
    let mut last_state = String::new();
    let mut last_marker = String::new();
    let mut confirming = false;
    // Codex: o ocioso chegou com uma conferência em voo, que pode ter lido o rollout antes da linha da entrada.
    let mut confirm_again = false;
    // Entrada enviada: a confirmação segue o crescimento do transcript (só `stat`) em vez de esperar o idle;
    // a prova é a mesma do `confirm_inputs`. Prazo para não vigiar uma entrada que nunca chega.
    let mut confirm_until: Option<Instant> = None;
    let mut confirm_seen: Option<u64> = None;
    let prompt_check = Arc::new(AtomicBool::new(false));
    let prompt_last = Arc::new(std::sync::Mutex::new(0.0_f64));
    let prompt_env = crate::accounts::environment::AccountEnvironment::capture();
    // Claude sem terminal: próxima conferência da fila enquanto houver entrada entregue sem prova. Nasce
    // vencida para a subida acertar o que ficou da vida anterior.
    let mut confirm_recheck: Option<Instant> = (target.provider == "claude").then(Instant::now);
    let mut channels:BTreeMap<String,Value> = ["preview","thinking","tool"].into_iter().map(|channel|
        (channel.into(),json!({"session":target.name,"text":"","md":true,"full":true,"vivo":true}))).collect();
    let mut live = LiveOut { sender:engine.live.clone(),state:LiveState {
        binding:engine.view()["conversation"].as_str().filter(|conversation|!conversation.is_empty()).map(|conversation|crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:conversation.into()}),
        public_state:engine.view()["public_state"].clone(),..Default::default() } };
    live.send();
    let receipt = Arc::new(std::sync::Mutex::new(ReceiptIndex::new(&target.provider,engine.view()["conversation"].as_str().unwrap_or(""))));
    // Entrada de uma sessão anterior com a mesma chave nunca drena nem confirma: só ocupava a fila.
    if target.provider == "claude" && target.created > 0.0 {
        match queue.exec(target.generation,&format!("prune:{}",unique()),clock(start),Action::Prune { min_ts:target.created }).await {
            Ok(pruned) if pruned.as_u64().unwrap_or(0) > 0 => tracing::info!(key=%target.key,session=%target.name,pruned=%pruned,"fila: entradas de sessão anterior removidas"),
            Ok(_) => {},
            Err(failure) => { let _ = io_failure(failure); }
        }
    }
    let mut sequence = initial.operations.keys().filter_map(|id|id.rsplit(':').next()?.parse::<u64>().ok()).max().unwrap_or(0);
    let mut write_order = 0u64;
    let mut next_write = 1u64;
    let mut native:BTreeSet<String> = BTreeSet::new();
    let mut prepared_writes:BTreeMap<u64,(String,u64,Result<(),RuntimeError>)> = BTreeMap::new();
    let mut error:Option<RuntimeError> = None;
    let mut io_open = true;
    let mut drain_requested = true;
    let mut drain_claim:Option<String> = None;
    let mut drain_active = false;
    let mut drain_waiters = Vec::new();
    let mut mods_waiters:ModsWaiters = BTreeMap::new();
    let mut mods_token = 0u64;
    let mut ui_writes = 0u64;
    let mut respawn = Respawn::default();
    let mut sid_transition=false;
    let mut sid_retry:Option<(SidCommitAttempt,f64)>=None;
    let mut sid_commit_inflight=false;
    let mut mode_commit:Option<ModeCommit>=None;
    let mut mode_failures:BTreeMap<String,RuntimeError>=BTreeMap::new();
    // Retenção de turnos (`hold_turns`): só a referência fraca fica aqui; quem segura é dono da forte.
    let mut turn_hold:Option<std::sync::Weak<()>> = None;
    let turn_hold_released = Arc::new(Notify::new());
    // Pedidos de retenção que chegaram com uma drenagem em curso: respondem quando ela termina.
    let mut hold_waiters:Vec<oneshot::Sender<Result<TurnHold,RuntimeError>>> = Vec::new();
    // O snapshot é do cano; no WebSocket o estado vem do próprio app-server depois de conectar.
    if let Some(snapshot) = snapshot { effects.extend(engine.hydrate(snapshot)?); }
    // Sem conexão na abertura: o mesmo estado da queda do socket, e a inicialização que faltar sai ao conectar.
    let mut boot_pending = false;
    if unconnected {
        io_open = false;
        let sample = clock(start);
        (cano_lost,lost_at,stall_dirty) = (true,sample.epoch_s,true);
        cano_check_at = Some(sample.monotonic_s + engine.reconnect_waits.check.as_secs_f64());
        effects.extend(engine.transport_lost("",&BTreeSet::new())?);
        reconnect.schedule(sample.monotonic_s,false);
        boot_pending = engine.view()["initialized"] != true;
    } else if adopted || engine.view()["initialized"] != true || target.provider == "codex" && engine.view()["ready"] != true {
        let mut id = if target.provider=="claude" {format!("bootstrap:{}:{}:{}:{}",target.key,target.generation,target.binding.pid,target.metadata["cano"]["process_birth"])} else {format!("bootstrap:{}:{}",target.key,target.generation)};
        // Vida nova do ator na mesma geração (reabertura): a subida da vida anterior já fechou e não pode ser
        // reenviada; esta sobe com outra operação, senão o `initialize` nunca sai.
        if initial.operations.get(&id).is_some_and(|op|matches!(op.status,Status::Accepted|Status::Rejected|Status::Confirmed)) { id = format!("{id}:{}",unique()); }
        queue.exec(target.generation,&format!("prepare:{id}"),clock(start),Action::Prepare { id:id.clone(),payload:json!({"kind":"bootstrap"}),entry_id:None }).await.map_err(io_failure)?;
        effects.extend(engine.initialize(id)?);
    }
    loop {
        if let Some(rc) = cano_gone.take() {
            reconnect.cancel();
            (cano_lost,cano_check_at,stall_due,stall_shown) = (false,None,None,false);
            tracing::warn!(key=%target.key,session=%target.name,provider=%target.provider,"cano morreu com o socket fora; é queda");
            // Codex: a queda de sempre, com o teto de 3 subidas (o cano que guardou o código ainda pode estar de pé).
            if target.provider == "codex" {
                if rc.is_some() { respawn.kill = true; } else { enter_error(&mut error,&target,failure("cano_closed")); }
                effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":rc})),clock(start))?);
                effects.extend(after_exit(&mut respawn,&mut engine,&target,clock(start).monotonic_s,closed.load(Ordering::Acquire)));
            } else {
                effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":rc})),clock(start))?);
                publish(&events,&target,&mut revision,"claude_exit",json!({"conversation":engine.view()["conversation"]}));
            }
        }
        if cano_lost && std::mem::take(&mut stall_dirty) {
            match queue.snapshot().await {
                Ok(state) => match oldest_waiting(&state,target.created) {
                    None => {
                        stall_due = None;
                        if std::mem::take(&mut stall_shown) { effects.extend(engine.clear_problem(DELIVERY_STALLED)); }
                    }
                    Some(queued) if !stall_shown => {
                        // Conta do mais tardio entre a queda e a entrada mais antiga ainda por entregar.
                        let sample = clock(start);
                        stall_due = Some(sample.monotonic_s + queued.max(lost_at) + engine.reconnect_waits.stall.as_secs_f64() - sample.epoch_s);
                    }
                    Some(_) => {},
                },
                Err(failure) => {
                    // Sem rearmar, a mensagem presa nunca acende o aviso (o único caminho para o Reabrir).
                    stall_dirty = true;
                    if crate::warn_limit::allow(Some(&target.key),"stall_queue") {
                        tracing::warn!(key=%target.key,session=%target.name,reason=?failure.kind(),"fila ilegível para o aviso de entrega parada");
                    }
                },
            }
        }
        live.error(&error);
        if !drain_active && !hold_waiters.is_empty() {
            let view = engine.view();
            for response in hold_waiters.drain(..) { let _ = response.send(turn_hold_reply(&view,&mut turn_hold,&turn_hold_released)); }
        }
        if block_claude_sid(&mut target,&engine.view()) {
            sid_transition=true;
            for (id,pending) in roots.iter_mut().filter(|(_,pending)|pending.result.is_none()) {
                pending.cancelled=true;
                let reply=RuntimeReply {operation_id:id.clone(),disposition:Disposition::Unknown,payload:json!({"reason":"conversation_changed"})};
                for response in pending.responses.drain(..) {let _=response.send(Ok(reply.clone()));}
            }
        }
        if turn_hold.as_ref().is_some_and(|hold|hold.strong_count() == 0) {
            // O que adiou durante a retenção sai agora, pela fila.
            turn_hold = None;
            drain_requested = true;
        }
        loop {
            let first_input = roots.values().filter(|root|root.preparing && matches!(root.command.kind,OperationKind::Input | OperationKind::Steer))
                .map(|root|root.arrival).min();
            let ready = roots.iter().filter(|(_,root)|root.ready_to_run && root.preparing
                && (!matches!(root.command.kind,OperationKind::Input | OperationKind::Steer) || Some(root.arrival) == first_input))
                .min_by_key(|(_,root)|root.arrival).map(|(id,_)|id.clone());
            let Some(id) = ready else { break };
            if sid_transition {break;}
            let pending = roots.get_mut(&id).unwrap();
            pending.preparing = false;
            if pending.cancelled || pending.timed_out { continue; }
            if matches!(pending.command.kind,OperationKind::SteerQueue | OperationKind::Steer)
                && pending.command.payload.get("expected_turn").is_some_and(|expected|turn_marker(&engine.view()).as_ref()!=Some(expected)) {
                effects.push_back(Effect::Reply { operation_id:id,disposition:Disposition::Rejected,payload:json!({"code":"queue_steer_work_changed"}) });
                continue;
            }
            if pending.command.kind == OperationKind::SteerQueue {
                if let Some((code,text)) = steer_refusal(&engine.view(),&target.provider) {
                    effects.push_back(Effect::Reply { operation_id:id,disposition:Disposition::Rejected,payload:json!({"error":text,"code":code}) });
                    continue;
                }
                let queue = queue.clone(); let target = target.clone(); let sender = internal.clone(); let sample = clock(start);
                let entry_id = pending.command.payload["entry_id"].as_str().map(str::to_owned);
                let expected_turn = pending.command.payload.get("expected_turn").cloned();
                jobs.spawn(async move {
                    let result = async {
                        let claimed = queue.exec(target.generation,&format!("steer-claim:{id}"),sample,
                            Action::Claim { min_ts:target.created,limit:None,entry_id }).await.map_err(io_failure)?;
                        let mut replies = Vec::new();
                        let state = queue.snapshot().await.map_err(io_failure)?;
                        for row in claimed.as_array().ok_or_else(||failure("queue_shape"))? {
                            let entry = row["id"].as_str().ok_or_else(||failure("queue_entry"))?.to_owned();
                            // A resposta reservada não é orientação: sai pelo despacho dela, senão iria duas vezes.
                            if state.operations.get(&entry).is_some_and(|op|op.payload["kind"] == "dispatch_async_answer") {
                                queue.exec(target.generation,&format!("steer-skip:{id}:{entry}"),sample,
                                    Action::SetDelivered { entry_id:entry,value:false,steered:false }).await.map_err(io_failure)?;
                                continue;
                            }
                            let mut payload=json!({"text":row["text"],"entry_id":entry,"pre_transcript":row["pre_transcript"].as_bool().unwrap_or(false)});
                            if let Some(expected)=&expected_turn {
                                payload["expected_turn"]=expected.clone();
                                if expected["turn_id"].is_string() {payload["turn_id"]=expected["turn_id"].clone();}
                            }
                            let command = RuntimeCommand { operation_id:format!("{id}:{entry}"),kind:OperationKind::Steer,payload };
                            let (response,receive) = oneshot::channel();
                            sender.send(Message::Command { command,response,from_queue:false,generation:target.generation }).await.map_err(|_|failure("runtime_closed"))?;
                            replies.push((entry,receive));
                        }
                        let mut accepted = Vec::new();
                        let mut uncertain = false;
                        for (entry,receive) in replies {
                            let result = receive.await.map_err(|_|failure("runtime_closed"))?;
                            match result {
                                Ok(reply) if reply.disposition == Disposition::Accepted => {
                                    queue.exec(target.generation,&format!("steered:{id}:{entry}"),sample,
                                        Action::SetDelivered { entry_id:entry.clone(),value:true,steered:true }).await.map_err(io_failure)?;
                                    accepted.push(entry);
                                }
                                Ok(reply) if reply.disposition == Disposition::Unknown => { uncertain = true; },
                                _=>{
                                    if queue.exec(target.generation,&format!("steer-unclaim:{id}:{entry}"),sample,
                                        Action::SetDelivered { entry_id:entry,value:false,steered:false }).await.is_err() { uncertain = true; }
                                }
                            }
                        }
                        if uncertain { Err(failure("steer_unknown")) } else { Ok(accepted) }
                    }.await;
                    Job::Steered { id,result }
                });
                continue;
            }
            // Mesma resposta do motor sem `deliverable`: a entrada fica na fila e o drain a leva depois.
            if pending.defer_for_batch || turn_hold.is_some() && matches!(pending.command.kind,OperationKind::Input | OperationKind::DispatchAsyncAnswer) {
                if pending.defer_for_batch {drain_requested=true;}
                effects.push_back(Effect::Reply { operation_id:id,disposition:Disposition::Deferred,payload:json!({}) });
                continue;
            }
            // Compactar também abre trabalho no app-server: recusa como o motor recusa com a sessão ocupada.
            if turn_hold.is_some() && pending.command.kind == OperationKind::Compact {
                fail_root(&mut roots,&id,RuntimeError::new(TURN_HOLD_BUSY,"espere o Codex terminar e responda às perguntas antes de compactar"));
                continue;
            }
            let message = matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer);
            match engine.command(pending.command.clone(),clock(start)) {
                Ok(next)=>effects.extend(next),
                Err(error)=>{
                    // Como o "próximo prompt tenta de novo" do Python: a mensagem espera na fila e pede outra subida.
                    if message && target.provider == "claude" && engine.view()["alive"] == false {
                        publish(&events,&target,&mut revision,"claude_wake",json!({"conversation":engine.view()["conversation"]}));
                    }
                    defer_unwritten(&target.provider,&mut roots,&attempts,&native,&id,error,&mut effects)
                }
            }
        }
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::Write { frame,operation_id } => {
                    sequence += 1;
                    write_order += 1;
                    let logical_id = operation_id.unwrap_or_else(||format!("system:{}:{sequence}",target.generation));
                    let wire = format!("wire:{logical_id}:{sequence}");
                    let volatile = engine.volatile_write(&logical_id);
                    let attempt = Attempt { logical_id:logical_id.clone(),phase_id:wire.clone(),frame:frame.clone(),order:write_order,epoch:transport_epoch,generation:target.generation,volatile };
                    attempts.insert(wire.clone(),attempt.clone());
                    // Sem preparo nem vista forçada: os IDs dela nunca entram no diário, então o contador salvo
                    // continua cobrindo todos os que entram. Mantém a ordem das escritas.
                    if volatile { prepared_writes.insert(write_order,(wire,target.generation,Ok(()))); continue; }
                    let queue = queue.clone(); let target = target.clone(); let view = engine.view(); let sample = clock(start);
                    let state_gate = state_gate.clone(); let state_version = state_version;
                    jobs.spawn(async move {
                        let result = async {
                            let authoritative = queue.snapshot().await.map_err(io_failure)?;
                            let entry_id = authoritative.operations.get(&logical_id).and_then(|op|op.entry_id.clone());
                            if !authoritative.operations.contains_key(&logical_id) {
                                queue.exec(target.generation,&format!("prepare-logical:{wire}"),sample,Action::Prepare { id:logical_id.clone(),
                                    payload:json!({"kind":"phase","frame":frame}),entry_id:None }).await.map_err(io_failure)?;
                            }
                            queue.exec(target.generation,&format!("prepare:{wire}"),sample,Action::Prepare {
                                id:wire.clone(),payload:json!({"logical_id":logical_id,"frame":frame,"request_id":frame.get("id").or_else(||frame.get("request_id")),
                                    "generation":target.generation,"conversation":view["conversation"],
                                    "state_revision":view["state_revision"],"settings_revision":view["settings_revision"]}),entry_id }).await.map_err(io_failure)?;
                            // Antes de cada escrita no fio a vista vai inteira: o contador dos IDs tem que estar salvo.
                            save_view(&queue,target.generation,sample,&state_gate,state_version,&view,true).await?;
                            let cursor = capture_cursor(&target,&view).await?;
                            queue.exec(target.generation,&format!("cursor:{wire}"),sample,Action::BindDispatch { id:wire.clone(),cursor:cursor.clone() }).await.map_err(io_failure)?;
                            queue.exec(target.generation,&format!("logical-cursor:{wire}"),sample,Action::BindDispatchIfPrepared { id:logical_id.clone(),cursor }).await.map_err(io_failure)?;
                            queue.exec(target.generation,&format!("dispatch:{wire}"),sample,Action::BeginDispatch { id:wire.clone(),wire_id:wire.clone(),staged:false }).await.map_err(io_failure)?;
                            queue.exec(target.generation,&format!("logical-dispatch:{wire}"),sample,Action::BeginDispatch { id:logical_id.clone(),wire_id:wire.clone(),staged:false }).await.map_err(io_failure)?;
                            Ok(())
                        }.await;
                        Job::Write { wire,generation:target.generation,result }
                    });
                }
                Effect::Publish { channel,data } => {
                    if ["preview","thinking","tool"].contains(&channel.as_str()) {
                        channels.insert(channel.clone(),data.clone());
                        // Prévia só pelo canal do hub: no `events` ela subiria a
                        // revisão e o Python a decodificaria a cada delta.
                        if !live.channel(&channel,&data) { publish(&events,&target,&mut revision,&channel,data); }
                    } else if channel == "rate" {
                        if let Some(registry) = engine.lifecycle.as_ref().and_then(std::sync::Weak::upgrade) {
                            registry.record_rate(&target.name,&target.key,target.generation,&data);
                        }
                        // O `/events` que o Python ainda serve (Connect, modo `python`) mede pela cópia dele.
                        publish(&events,&target,&mut revision,&channel,data);
                    }
                }
                Effect::StateChanged => {
                    if sid_transition {continue;}
                    let view = engine.view();
                    // Subida boa zera o teto: só a que nunca fica pronta conta como seguida.
                    if view["ready"] == true && respawn.failures > 0 { respawn.failures = 0; respawn.last = None; }
                    state_version += 1;
                    let version = state_version;
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    let gate = state_gate.clone();
                    jobs.spawn(async move {
                        let result = save_view(&queue,generation,sample,&gate,version,&view,false).await;
                        Job::View { generation,version,view,result }
                    });
                }
                Effect::Reply { operation_id,mut disposition,mut payload } => {
                    if let Some(failed)=mode_failures.get(&operation_id) {disposition=Disposition::Unknown;payload=json!({"error_code":failed.code,"persistence_confirmed":false});}
                    // Operação com desfecho deixa de contar como "pode ter saído" numa queda futura.
                    handed.remove(&operation_id);
                    let reply = RuntimeReply { operation_id:operation_id.clone(),disposition,payload };
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    let related:Vec<_> = attempts.values().filter(|attempt|attempt.logical_id == operation_id).cloned().collect();
                    let volatile = !related.is_empty() && related.iter().all(|attempt|attempt.volatile);
                    jobs.spawn(async move {
                        let result = async {
                            if volatile { return Ok(()); }
                            for attempt in related {
                                queue.exec(generation,&format!("finish:{}:{}",attempt.phase_id,unique()),sample,Action::Finish {
                                    id:attempt.phase_id,status:status(disposition),result:serde_json::to_value(&reply).unwrap() }).await.map_err(io_failure)?;
                            }
                            queue.exec(generation,&format!("finish:{operation_id}:{}",unique()),sample,Action::Finish {
                                id:operation_id,status:status(disposition),result:serde_json::to_value(&reply).unwrap() }).await.map_err(io_failure)?;
                            Ok(())
                        }.await;
                        Job::Finished { generation,reply,result }
                    });
                }
                Effect::Policy { kind,request_id,payload } => {
                    if kind == "local_output" {
                        let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                        let text = payload["text"].as_str().unwrap_or("").to_owned();
                        let confirms = payload["source"].as_str().map(str::to_owned);
                        jobs.spawn(async move { Job::Saved(queue.exec(generation,&format!("local:{}",unique()),sample,
                            Action::AppendLocal { text,entry_id:None,confirms }).await.map(|_|()).map_err(io_failure)) });
                        continue;
                    }
                    sequence += 1;
                    let phase_id = format!("policy:{}:{sequence}",target.generation);
                    let target = target.clone(); let policy = engine.policy.clone();
                    // O Python recusa como velho o campo que difere da vista salva (thread nova, modo, Fast,
                    // modelo): ela vai antes de todo patch. Sem mudança durável a gravação não toca o disco.
                    let conversation = (kind == "session.patch_meta").then(||payload.get("thread_id").or_else(||payload.get("session_id")).cloned()).flatten();
                    let save = if kind == "session.patch_meta" {
                        state_version += 1;
                        Some((state_version,engine.view()))
                    } else { None };
                    let queue = queue.clone(); let gate = state_gate.clone(); let sample = clock(start);
                    // Codex com terminal adotado: o sidecar é gravado aqui, nunca pela política do Python.
                    let sidecar = adoption.as_ref().filter(|_|kind == "session.patch_meta").map(|adoption|adoption.cell.sidecar().cloned());
                    // Sem terminal (Codex que o Rust sobe, Claude): o mesmo, com a vida pela chave e sob a trava
                    // que o Python também segura ao gravar o arquivo.
                    let headless_dir = matches!(kind.as_str(),"session.patch_meta" | "session.clear_cano").then(|| match target.provider.as_str() {
                        "claude" => Some(super::lifecycle::claude_sidecar_dir()),
                        _ => engine.launch.as_ref().filter(|_|adoption.is_none()).map(|launch|Ok(launch.sidecar_dir.clone())),
                    }).flatten();
                    // Estes serviços não escrevem na CLI (formatar status, carimbo, sidecar, log): repetir é
                    // inofensivo, então não passam pelo diário. Quatro gravações por chamada, a cada mudança de
                    // estado, eram a maior parte do disco gasto por mensagem.
                    jobs.spawn(async move {
                        let result = async {
                            if let Some((version,view)) = save {
                                save_view(&queue,target.generation,sample,&gate,version,&view,false).await?;
                            }
                            if let Some(sidecar) = sidecar {
                                let life = sidecar.ok_or_else(||RuntimeError::new("codex_sidecar_unknown","sessão adotada sem a pasta do sidecar"))?;
                                let saved = queue.snapshot().await.map_err(io_failure)?.runtime_state["view"].clone();
                                return tokio::task::spawn_blocking(move||super::codex_sidecar::patch(&life,&payload,&saved)).await
                                    .map_err(|_|failure("sidecar_job"))?;
                            }
                            if let Some(dir) = headless_dir {
                                let saved = if kind == "session.clear_cano" { Value::Null } else { queue.snapshot().await.map_err(io_failure)?.runtime_state["view"].clone() };
                                let (dir,provider,name,key,service) = (dir?,target.provider.clone(),target.name.clone(),target.key.clone(),kind.clone());
                                return tokio::task::spawn_blocking(move||super::gateway::sidecar_service(&provider,&dir,&name,&key,&service,&payload,&saved)).await
                                    .map_err(|_|failure("sidecar_job"))?;
                            }
                            if local_policy::is_local(&kind) {
                                let quota = match &policy {
                                    Some(policy) if kind == "format_status" && target.provider == "claude" =>
                                        policy.quota_windows(&target.key,target.metadata["config_dir"].as_str().unwrap_or("")).await,
                                    _ => None,
                                };
                                return run_local(kind.clone(),payload,&target,quota).await;
                            }
                            let _ = (payload,&request_id,&phase_id);
                            Err(RuntimeError::new("policy_unknown","serviço do ator sem atendimento no Rust"))
                        }.await;
                        Job::Policy { request_id,kind,phase_id,conversation,result }
                    });
                }
                Effect::PersistClaude {operation_id,patch}=> {
                    if target.provider!="claude" {return Err(failure("lifecycle_provider"));}
                    let previous=target.metadata["session_id"].as_str().unwrap_or("").to_owned();
                    if patch["session_id"].as_str().is_some_and(|sid|sid!=previous) {
                        sid_transition=true;target.metadata["lifecycle_pending"]=json!(true);
                        if sid_commit_inflight {continue;}
                        let context=SidCommitAttempt {generation:target.generation,previous,patch};
                        sid_retry=None;sid_commit_inflight=true;
                        spawn_sid_commit(&mut jobs,engine.lifecycle.clone(),target.clone(),queue.clone(),context);
                        continue;
                    }
                    if sid_transition {continue;}
                    state_version+=1;
                    let version=state_version;let view=engine.view();let sample=clock(start);
                    let saved=async {
                        save_view(&queue,target.generation,sample,&state_gate,version,&view,false).await?;
                        let _guard=state_gate.lock().await;
                        let state=queue.snapshot().await.map_err(io_failure)?;
                        if state.owner_key!=target.key || state.generation!=target.generation || view["conversation"]!=target.metadata["session_id"] {return Err(failure("runtime_generation"));}
                        let mut captured=target.clone();
                        if let Some(metadata)=state.runtime_state.get("metadata").filter(|metadata|metadata["key"]==target.key && metadata["cano"]["pid"]==target.binding.pid) {captured.metadata=metadata.clone();}
                        let lease=queue.lease();let values=patch.clone();
                        let result=tokio::task::spawn_blocking(move || {let _lease=lease;super::lifecycle::patch_sidecar(&captured,&values)}).await.map_err(|_|failure("lifecycle_worker"))??;
                        if result["updated"]!=true {return Err(failure("lifecycle_sidecar_stale"));}
                        let mut metadata=result["metadata"].clone();metadata["lifecycle_pending"]=target.metadata["lifecycle_pending"].clone();
                        queue.exec(target.generation,&format!("claude-metadata:{}",unique()),sample,Action::AdvanceLifecycle {generation:target.generation,state:json!({"metadata":metadata})}).await.map_err(io_failure)?;
                        Ok::<_,RuntimeError>((metadata,result["digest"].as_str().ok_or_else(||failure("lifecycle_commit_digest"))?.to_owned()))
                    }.await;
                    match saved {
                        Ok((metadata,digest))=> {
                            target.metadata=metadata;
                            if let (Some(id),Some(mode))=(operation_id,patch["permission_mode"].as_str()) {
                                mode_commit=Some(ModeCommit {proof:ModeProof {binding:crate::interactions::InteractionBinding {key:target.key.clone(),conversation:previous,generation:target.generation},operation_id:id,mode:mode.into()},digest});
                            }
                        },
                        Err(failed)=>{
                            if let Some(id)=operation_id {mode_failures.insert(id,failed.clone());}
                            // Sidecar apagado: a sessão foi fechada pelo nome e não há onde gravar; a religação a solta.
                            if failed.code!="lifecycle_sidecar_read" || !super::lifecycle::claude_sidecar_absent(&target.name).await {enter_error(&mut error,&target,failed);}
                            else {
                                if crate::warn_limit::allow(Some(&target.key),"claude_sidecar_absent") {
                                    tracing::warn!(key=%target.key,session=%target.name,"sidecar da sessão sumiu; a gravação do estado não foi feita");
                                }
                                effects.push_back(Effect::Diag { event:DiagEvent::ClaudeSidecarAbsent,code:"claude_sidecar_absent".into() });
                            }
                        },
                    }
                },
                Effect::WakeQueue => { drain_requested = true; },
                Effect::InputEchoed { operation_id } => {
                    // Prova direta da CLI: confirma a entrada da operação sem esperar o transcript (que fica de reserva).
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    jobs.spawn(async move { Job::Saved(async {
                        let state = queue.snapshot().await.map_err(io_failure)?;
                        let Some(entry) = state.operations.get(&operation_id).and_then(|op|op.entry_id.clone()) else { return Ok(()) };
                        if !state.rows.iter().any(|row|row["id"] == entry.as_str() && row["confirmed"] != true) { return Ok(()); }
                        queue.exec(generation,&format!("echo-confirm:{}",unique()),sample,Action::Confirm { entry_ids:vec![entry] }).await.map(|_|()).map_err(io_failure)
                    }.await) });
                },
                Effect::ConfirmLocalCommands => {
                    // Mesma regra do adapter Python: comando local não aparece no transcript, então o
                    // reconcile nunca o confirmaria; a CLI já o consumiu. Só as entradas de barra.
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    jobs.spawn(async move { Job::Saved(async {
                        let state = queue.snapshot().await.map_err(io_failure)?;
                        // Só a que já foi ao fio: a próxima barra, reivindicada pelo drain ao mesmo tempo e ainda
                        // não escrita, confirmada aqui nunca seria enviada.
                        let entry_ids:Vec<String> = state.rows.iter().filter(|row|row["delivered"] == true && row["confirmed"] != true
                            && row["text"].as_str().is_some_and(|text|text.trim_start().starts_with('/'))
                            && row["id"].as_str().and_then(|id|state.operations.get(id)).is_some_and(|op|matches!(op.status,Status::Dispatching | Status::Accepted)))
                            .filter_map(|row|row["id"].as_str().map(str::to_owned)).collect();
                        if entry_ids.is_empty() { return Ok(()); }
                        queue.exec(generation,&format!("local-confirm:{}",unique()),sample,Action::Confirm { entry_ids }).await.map(|_|()).map_err(io_failure)
                    }.await) });
                },
                Effect::Surface { effect } => match effect {
                    SurfaceEffect::Write { frame,until } => {
                        // `ui_*` não muda a conversa nem precisa sobreviver a uma queda: sai direto, fora do
                        // diário, que gravaria no disco a cada desenho.
                        ui_writes += 1;
                        // Canal próprio e pequeno: o que não cabe é descartado e a superfície pede de novo no prazo.
                        let until = until.map(|until|tokio::time::Instant::from_std(start + Duration::from_secs_f64(until.max(0.0))));
                        let frame = WireFrame { operation_id:format!("ui:{}:{ui_writes}",target.generation),frame,ephemeral:true,until,guard:None };
                        if io.try_send(frame).is_err() && crate::warn_limit::allow(Some(&target.key),"ui_write") {
                            tracing::warn!(key=%target.key,session=%target.name,"pedido da interface dos mods descartado com o canal do cano cheio");
                        }
                    }
                    SurfaceEffect::Publish { data } => { if let Some(mods) = &engine.mods { mods.publish_ui(&target.name,engine.mods_life,data); } }
                    SurfaceEffect::Toast { plugin,text,timeout_ms } => { if let Some(mods) = &engine.mods { mods.toast(&target.name,engine.mods_life,&plugin,&text,timeout_ms); } }
                    SurfaceEffect::Copied { plugin,text } => { if let Some(mods) = &engine.mods { mods.copied(&target.name,engine.mods_life,&plugin,&text); } }
                    SurfaceEffect::Reply { token,result } => { if let Some(waiter) = mods_waiters.remove(&token) { let _ = waiter.send(result); } }
                },
                Effect::Diag { event,code } => {
                    // Versão é do servidor, não da sessão: sem nome, o limite de 1/min vale para todos.
                    let session = if event == DiagEvent::CodexVersion { "" } else { target.name.as_str() };
                    if let Some(policy) = &engine.policy { policy.diag.report(event.event(),session,&code,event.reason()); }
                }
                Effect::Stop { .. } => { closed.store(true,Ordering::Release); },
                Effect::UsageLimit { limit } => {
                    if let Some(registry) = engine.lifecycle.as_ref().and_then(std::sync::Weak::upgrade) { 
                        registry.usage_limit(super::gateway::LimitHit { name:target.name.clone(),provider:target.provider.clone(),
                            account:target.metadata["config_dir"].as_str().filter(|dir|!dir.is_empty()).map(str::to_owned),limit });
                    }
                },
                Effect::ReserveAsyncAnswer { operation_id,reservation,command } => {
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    jobs.spawn(async move {
                        // Entrada e operação na mesma gravação; o id da chamada é o da entrada, então repetir não abre outra.
                        let action = Action::ReserveAsyncAnswer { entry_id:reservation.entry_id.clone(),text:reservation.text.clone(),
                            payload:serde_json::to_value(&command).unwrap_or_default() };
                        let result = queue.exec(generation,&format!("reserve:{}",reservation.entry_id),sample,action).await.map(|_|()).map_err(io_failure);
                        // Erro da fila não prova que nada foi gravado: o desfecho segue o que o disco tem.
                        let persisted = match &result {
                            Ok(()) => Some(true),
                            Err(_) => queue.snapshot().await.ok().map(|state|state.operations.contains_key(&reservation.entry_id)),
                        };
                        Job::Reserved { operation_id,reservation,result,persisted }
                    });
                }
                Effect::Respawn { operation_id,reason,patch,reply } => {
                    if respawn.task.is_some() {
                        if reason == "restart" && patch.is_null() && respawn.user.is_none() { respawn.waiting.push((operation_id,reply)); continue; }
                        fail_root(&mut roots,&operation_id,RuntimeError::new("erro_codex_reiniciando","o Codex já está subindo de novo; tente em instantes"));
                        continue;
                    }
                    // Ação da pessoa: nova rodada de tentativas.
                    (respawn.failures,respawn.next_at,respawn.user) = (0,None,Some((operation_id.clone(),reply)));
                    // Socket fora: a subida passa a ser dona do processo; a tentativa em voo sai e volta depois dela.
                    if cano_lost { reconnect.cancel(); }
                    // O Python recusa como velho o campo que difere da vista salva: ela vai com o valor novo.
                    let saved = patch.as_object().map(|fields|{
                        let mut view = engine.view();
                        for (key,value) in fields { view[key] = value.clone(); }
                        state_version += 1;
                        (queue.clone(),clock(start),state_gate.clone(),state_version,view)
                    });
                    tracing::info!(key=%target.key,session=%target.name,reason=%reason,"processo da sessão sobe de novo a pedido");
                    if let Err(failure) = start_respawn(&mut respawn,&engine,&target,true,patch,saved) {
                        respawn.user = None;
                        fail_root(&mut roots,&operation_id,failure);
                    }
                }
            }
        }
        while let Some((attempt,result))=next_current_write(&mut prepared_writes,&mut next_write,&attempts,target.generation) {
            let wire=attempt.phase_id.clone();
            let ended = roots.iter().any(|(id,root)|(attempt.logical_id == *id || attempt.logical_id.starts_with(&format!("{id}:")))
                && (root.cancelled || root.timed_out));
            let interaction_error = engine.interaction_write_error(&attempt.logical_id);
            let stale_transport = attempt.epoch != transport_epoch;
            let stale_work = roots.iter().any(|(id, root)|(attempt.logical_id == *id || attempt.logical_id.starts_with(&format!("{id}:")))
                && root.command.payload.get("expected_turn").is_some_and(|expected|turn_marker(&engine.view()).as_ref()!=Some(expected)));
            if result.is_err() || ended || stale_transport || stale_work || interaction_error.is_some() || !engine.write_is_current(&attempt.logical_id) {
                let mut acknowledgements = engine.apply(EngineInput::WriteAck { operation_id:attempt.logical_id.clone(),outcome:WriteOutcome::NotWritten },clock(start))?;
                if let Some(failure) = interaction_error {
                    for effect in &mut acknowledgements {
                        if let Effect::Reply { operation_id,payload,.. } = effect && operation_id == &attempt.logical_id {
                            // A resposta terminal continua dizendo que a decisão não foi confirmada.
                            let confirmed = payload.get("decision_confirmed").cloned();
                            *payload = json!({"code":failure.code,"error":failure.message,"write_outcome":"not_written"});
                            if let Some(confirmed) = confirmed { payload["decision_confirmed"] = confirmed; }
                        }
                    }
                }
                effects.extend(acknowledgements);
                if let Err(failure) = result { enter_error(&mut error,&target,failure); }
            } else if sid_transition || io.try_send(WireFrame { operation_id:wire,frame:attempt.frame.clone(),ephemeral:false,until:None,guard:engine.interaction_guard(&attempt.logical_id)? }).is_err() {
                effects.extend(engine.apply(EngineInput::WriteAck { operation_id:attempt.logical_id.clone(),outcome:WriteOutcome::NotWritten },clock(start))?);
            } else {
                handed.insert(attempt.logical_id.clone());
            }
        }
        // Resposta reservada apta sai antes da mensagem comum: ela responde a uma pergunta já feita.
        let limit_held = || engine.lifecycle.as_ref().and_then(std::sync::Weak::upgrade).is_some_and(|registry|registry.limit_held(&target.name));
        let deliverable = if target.provider=="claude" {claude_delivery_allowed(&target,&engine.view(),error.is_some()) && !limit_held()} else {engine.view()["deliverable"]==true};
        let reserved = if target.metadata["lifecycle_pending"]==true { None } else { engine.ready_async_answer_entry() };
        if drain_requested && !drain_active && turn_hold.is_none() && !closed.load(Ordering::Acquire) && (deliverable || reserved.is_some())
            && !roots.values().any(|root|(root.preparing || root.defer_for_batch && root.result.is_none() && root.error.is_none()) && matches!(root.command.kind,OperationKind::Input | OperationKind::Steer)) {
            drain_requested = false; drain_active = true;
            let validate=reserved.is_none().then(||engine.batch_validator());
            let claim_id=format!("claim:{}",unique());drain_claim=Some(format!("call::{claim_id}"));
            let queue = queue.clone(); let target = target.clone(); let sample = clock(start);
            jobs.spawn(async move {
                let result = async {
                    let action=match reserved.clone() {
                        Some(entry_id)=>Action::Claim {min_ts:target.created,limit:Some(1),entry_id:Some(entry_id)},
                        None=>Action::ClaimBatch {min_ts:target.created},
                    };
                    let claimed = queue.exec_validated(target.generation,&claim_id,sample,action,false,validate).await.map_err(io_failure)?;
                    let state = queue.snapshot().await.map_err(io_failure)?;
                    let mut claimed = claimed.as_array().ok_or_else(||failure("queue_shape"))?.clone();
                    // Reivindicada numa vida que parou antes de despachar: operação preparada e sem tentativa
                    // prova que nada foi escrito, então a mesma entrada segue.
                    if let Some(entry) = reserved.as_deref().filter(|_|claimed.is_empty()) {
                        let row = state.rows.iter().find(|row|row["id"] == entry);
                        let unsent = state.operations.get(entry).is_some_and(|op|op.status == Status::Prepared && !maybe_written(&state,op));
                        if let Some(row) = row.filter(|row|unsent && row["delivered"] == true && row["confirmed"] != true && row["desistiu"] != true) {
                            claimed.push(row.clone());
                        }
                    }
                    let mut commands = Vec::new();
                    for row in &claimed {
                        let id = row["id"].as_str().ok_or_else(||failure("queue_entry"))?;
                        // Controle da voz antiga que sobrou da versão anterior: não tem mais dono e não vira mensagem.
                        if state.operations.get(id).is_some_and(|op|op.payload["kind"].as_str().is_some_and(|kind|kind.starts_with("voice_"))) { continue; }
                        let command = state.operations.get(id).and_then(|op|serde_json::from_value::<RuntimeCommand>(op.payload.clone()).ok())
                            .unwrap_or_else(||RuntimeCommand { operation_id:id.into(),kind:OperationKind::Input,
                                payload:json!({"text":row["text"],"pre_transcript":row["pre_transcript"].as_bool().unwrap_or(false)}) });
                        commands.push(command);
                    }
                    Ok(commands)
                }.await;
                Job::Drained {generation:target.generation,result}
            });
        }
        // Antes de a saída do cano ser dada como perdida, só depois do silêncio dele; depois, a cada `TURN_POLL`.
        let turn_check_due = (adoption.is_none() && io_open && !turn_checking && engine.turn_watch()).then(|| if engine.output_lost() { turn_check_after }
            else { (last_output + engine.reconnect_waits.output_lost.as_secs_f64()).max(turn_check_after) });
        let deadline = engine.deadline().into_iter().chain(turn_check_due).chain(respawn.next_at).chain(reconnect.next_at.filter(|_|respawn.task.is_none())).chain(cano_check_at.filter(|_|respawn.task.is_none())).chain(stall_due.filter(|_|!stall_shown)).chain(sid_retry.as_ref().map(|(_,at)|*at)).chain(roots.values().filter(|root|root.result.is_none() && !root.timed_out).map(|root|root.deadline))
            .min_by(f64::total_cmp).map(|seconds|start + Duration::from_secs_f64(seconds.max(0.0)))
            .unwrap_or_else(||Instant::now()+Duration::from_secs(3600));
        tokio::select! {
            message = receiver.recv() => {
                let Some(message) = message else { break };
                match message {
                    Message::Command { command,response,from_queue,generation } => {
                        if generation!=target.generation || sid_transition {
                            let _=response.send(if from_queue {Ok(RuntimeReply {operation_id:command.operation_id.clone(),disposition:Disposition::Deferred,payload:json!({"code":"runtime_sid_transition"})})} else {Err(failure("runtime_sid_transition"))});continue;
                        }
                        let response=match roots.get_mut(&command.operation_id) {
                            Some(pending)=>match join_active_pending(pending,&command,response) {Ok(())=>continue,Err(response)=>response},
                            None=>response,
                        };
                        let authoritative=queue.snapshot().await.map_err(io_failure)?;
                        match current_stored_reply(&command,&authoritative,from_queue) {
                            Ok(Some(reply))=>{roots.insert(command.operation_id.clone(),Pending::stored(command,reply.clone()));let _=response.send(Ok(reply));continue;},
                            Ok(None)=>{},
                            Err(error)=>{let _=response.send(Err(error));continue;},
                        }
                        let id = command.operation_id.clone();
                        // Só a entrada reservada que a própria fila devolveu vira esse despacho.
                        if command.kind == OperationKind::DispatchAsyncAnswer && !from_queue {
                            let _ = response.send(Err(RuntimeError::new("operation_internal","operação interna do runtime")));
                            continue;
                        }
                        if matches!(command.kind,OperationKind::Input | OperationKind::Steer) { confirm_until = Some(Instant::now() + CONFIRM_WINDOW); confirm_seen = None; }
                        if from_queue && roots.get(&id).is_some_and(|pending|pending.result.as_ref().is_some_and(|reply|reply.disposition == Disposition::Deferred)) {
                            roots.remove(&id);
                        }
                        if let Some(pending) = roots.get_mut(&id) {
                            if pending.original != serde_json::to_value(&command).unwrap() {
                                let _ = response.send(Err(failure("operation_reused")));
                            } else if let Some(error) = &pending.error { let _ = response.send(Err(error.clone()));
                            } else if let Some(reply) = &pending.result { let _ = response.send(Ok(reply.clone())); }
                            else { pending.responses.push(response); }
                            continue;
                        }
                        if command.kind == OperationKind::Interrupt {
                            for pending in roots.values_mut().filter(|root|root.preparing && matches!(root.command.kind,OperationKind::Input | OperationKind::Steer)) { pending.cancelled = true; }
                        }
                        arrival += 1;
                        let ticket = arrival;
                        let claimed=drain_active && drain_claim.as_ref().and_then(|id|authoritative.operations.get(id))
                            .and_then(|op|op.result.as_array()).is_some_and(|rows|!rows.is_empty());
                        let waiting=super::queue::waiting_ids(&authoritative,target.created).iter().any(|id|id!=&command.operation_id);
                        let defer_for_batch=(claimed || waiting) && !from_queue && command.kind==OperationKind::Input && !rowless(&target.provider,&command);
                        roots.insert(id.clone(),Pending { from_queue,defer_for_batch,original:serde_json::to_value(&command).unwrap(),command:command.clone(),responses:vec![response],
                            result:None,preparing:true,cancelled:false,error:None,deadline:clock(start).monotonic_s+30.0,timed_out:false,ready_to_run:false,arrival });
                        let queue = queue.clone(); let target = target.clone(); let sample = clock(start);
                        let preparation = preparation.clone();
                        jobs.spawn(async move {
                            loop {
                                let next = preparation.1.notified();
                                if *preparation.0.lock().await == ticket { break; }
                                next.await;
                            }
                            let result = async {
                                if matches!(command.kind,OperationKind::Input | OperationKind::Steer) && !rowless(&target.provider,&command) {
                                    let entry_id = command.payload["entry_id"].as_str().unwrap_or(&id);
                                    let rows = queue.exec(target.generation,&format!("load:{id}"),sample,Action::Load).await.map_err(io_failure)?;
                                    if !rows.as_array().is_some_and(|rows|rows.iter().any(|row|row["id"] == entry_id)) {
                                        queue.exec(target.generation,&format!("append:{id}"),sample,Action::Append { text:command.payload["text"].as_str().ok_or_else(||failure("input_text"))?.into(),
                                            delivered:false,ts:None,pre_transcript:command.payload["pre_transcript"] == true,entry_id:Some(entry_id.into()) }).await.map_err(io_failure)?;
                                    }
                                }
                                // A reservada já existe com a própria entrada: a mesma intenção, nunca outra.
                                let entry_id = match command.kind {
                                    OperationKind::Input | OperationKind::Steer if !rowless(&target.provider,&command) => Some(command.payload["entry_id"].as_str().unwrap_or(&id).to_owned()),
                                    OperationKind::DispatchAsyncAnswer => command.payload["entry_id"].as_str().map(str::to_owned),
                                    _ => None,
                                };
                                let prepared = queue.exec(target.generation,&format!("prepare:{id}:{}",unique()),sample,Action::Prepare { id:id.clone(),payload:serde_json::to_value(&command).unwrap(),
                                    entry_id }).await.map_err(io_failure)?;
                                Ok(stored_reply(&id,&prepared))
                            }.await;
                            *preparation.0.lock().await += 1;
                            preparation.1.notify_waiters();
                            Job::Root { id,result }
                        });
                    }
                    Message::Problem { code,detail } => { effects.extend(engine.show_problem(&code,detail)); }
                    Message::WakeReconnect => if cano_lost { reconnect.wake(clock(start).monotonic_s); },
                    Message::Mods { call,deadline,response } => {
                        effects.extend(take_mods(&mut engine,&mut mods_waiters,&mut mods_token,call,deadline,response,clock(start)));
                    }
                    Message::Queue { call_id,action,response,generation } => {
                        if generation!=target.generation {let _=response.send(Err(failure("runtime_generation")));continue;}
                        // A reserva nasce só da resposta conferida pelo motor, nunca de fora.
                        if matches!(action,Action::ReserveAsyncAnswer { .. } | Action::ClaimBatch { .. }) {
                            let _ = response.send(Err(RuntimeError::new("operation_internal","operação interna do runtime")));
                            continue;
                        }
                        let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                        let wake = matches!(&action,Action::Append { delivered:false,.. } | Action::Edit { .. } | Action::Cancel { .. } | Action::Remove { .. });
                        jobs.spawn(async move {
                            let result = queue.exec(generation,&call_id,sample,action).await.map_err(io_failure);
                            let saved = result.as_ref().map(|_|()).map_err(Clone::clone);
                            let _ = response.send(result);
                            Job::Queued { wake,result:saved }
                        });
                    }
                    Message::Snapshot(response) => {
                        // Antes da primeira gravação a vista durável não tem retrato; o do motor é o mesmo que o `live` já publicou.
                        let mut view=durable_view.clone();
                        if view.get("public_state").is_none() { view["public_state"] = engine.view()["public_state"].clone(); }
                        if sid_transition || target.metadata["lifecycle_pending"]==true {view["deliverable"]=json!(false);view["lifecycle_pending"]=json!(true);}
                        let _ = response.send(Ok(json!({"key":target.key,"generation":target.generation,"revision":revision.value,
                            "view":view,"channels":channels,"error":error.as_ref().map(|e|e.code.clone())})));
                    }
                    Message::View(response) => {
                        let mut view=engine.view();
                        view["park_idle_s"]=json!(engine.park_idle(clock(start).monotonic_s));
                        if sid_transition || target.metadata["lifecycle_pending"]==true {view["deliverable"]=json!(false);view["lifecycle_pending"]=json!(true);}
                        let _=response.send(view);
                    }
                    Message::LifecycleReady {proof,response}=> {
                        let result=async {
                            if target.provider!="claude" || sid_transition {return Err(failure("lifecycle_commit"));}
                            let captured=target.clone();
                            let digest=tokio::task::spawn_blocking(move ||super::lifecycle::sidecar_digest(&captured)).await.map_err(|_|failure("lifecycle_worker"))??;
                            mode_ready_proof(&target,&engine.view(),mode_commit.as_ref(),&proof,&digest,error.as_ref())?;
                            let mut metadata=target.metadata.clone();metadata["lifecycle_pending"]=json!(false);
                            queue.exec(target.generation,&format!("lifecycle-ready:{}:{}:{}",target.generation,target.binding.pid,proof.operation_id),clock(start),Action::AdvanceLifecycle {generation:target.generation,state:json!({"metadata":metadata})}).await.map_err(io_failure)?;
                            target.metadata=metadata;
                            Ok(())
                        }.await;
                        if result.is_ok() {drain_requested=true;}
                        let _=response.send(result);
                    },
                    Message::HoldTurns(response) => {
                        // Socket fora não recusa (sem fonte de turno, como o `_guard_permissao_codex`); fila
                        // reivindicando ou despachando já é um turno a caminho. A checagem vale também para quem
                        // reaproveita a retenção viva: a TUI pode ter aberto turno durante o primeiro seletor.
                        // Drenagem em curso pode não achar nada: a resposta espera o desfecho dela em vez de
                        // recusar por uma reivindicação que talvez não vire turno.
                        if drain_active { hold_waiters.push(response); continue; }
                        // Quem pediu desistiu: o valor cai aqui e a retenção some no próximo passo.
                        let _ = response.send(turn_hold_reply(&engine.view(),&mut turn_hold,&turn_hold_released));
                    }
                    Message::Drain(response) => {
                        if target.metadata["lifecycle_pending"]==true {let _=response.send(Ok(json!({"sent":0})));continue;}
                        // Resposta reservada apta sai mesmo com a fila comum barrada pela pergunta.
                        if !drain_has_work(engine.view()["deliverable"] == true,engine.ready_async_answer_entry().is_some(),drain_active) {
                            let _ = response.send(Ok(json!({"sent":0})));
                        } else {
                            drain_requested = true;
                            drain_waiters.push(response);
                        }
                    }
                    Message::Confirm(response) => {
                        let view = engine.view();
                        let job = confirm_inputs(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                        jobs.spawn(async move { Job::Confirmed { response:Some(response),result:job.await } });
                    }
                    Message::Stop { response,kill } => {
                        closed.store(true,Ordering::Release);
                        reconnect.cancel();
                        finish_respawn(&mut respawn,&mut target).await;
                        jobs.abort_all();
                        while jobs.join_next().await.is_some() {}
                        io.stop().await;
                        let killed = if kill { kill_process(&engine,&target).await } else { Ok(()) };
                        let result = queue.exec(target.generation,&format!("stop-repair:{}",unique()),clock(start),Action::EnsureProjection).await
                            .and_then(|_|Ok(())).map_err(io_failure);
                        if let Err(error) = result { let _ = response.send(Err(error.clone())); return Err(error); }
                        queue.exec(target.generation,&format!("stop-recover:{}",unique()),clock(start),Action::Recover).await.map_err(io_failure)?;
                        for pending in roots.values_mut() { for waiter in pending.responses.drain(..) { let _ = waiter.send(Err(failure("runtime_stopped"))); } }
                        let queue = Arc::try_unwrap(queue).map_err(|_|failure("queue_busy"))?;
                        queue.shutdown().await.map_err(io_failure)?;
                        let _ = response.send(killed);
                        return Ok(());
                    }
                }
            }
            event = io.events.recv(), if io_open => {
                // App-server adotado: a queda é só do socket de controle, nunca do processo. Sem religar
                // nem matar, uma reconexão por vez ao mesmo processo, com a fila segura até lá.
                if adoption.is_some() && matches!(event, None | Some(IoEvent::End { .. })) {
                    let code = match &event { Some(IoEvent::End { code:Some(code) }) => format!("close_{code}"), _ => "close_unknown".into() };
                    effects.push_back(Effect::Diag { event:DiagEvent::CodexConnectionClosed,code });
                    io_open = false;
                    std::mem::replace(&mut io,super::codex_terminal::closed_io()).stop().await;
                    transport_epoch += 1;
                    reconnect.cancel();
                    tracing::warn!(key=%target.key,session=%target.name,"conexão de controle do Codex com terminal caiu; reconectando ao mesmo app-server");
                    effects.extend(engine.transport_lost("a conexão de controle caiu; reconectando ao mesmo app-server",&std::mem::take(&mut handed))?);
                    reconnect.schedule(clock(start).monotonic_s,false);
                    continue;
                }
                // Codex sem terminal: socket fora com o cano ainda vivo é religação, não queda. O nascimento lido
                // agora prova, nas tentativas, que o pid segue sendo este processo.
                let codex_cano_alive = target.provider == "codex" && adoption.is_none() && respawn.task.is_none()
                    && matches!(event,None | Some(IoEvent::End { code:None })) && engine.view()["alive"] != false && {
                    let (pid,key) = (target.binding.pid,target.key.clone());
                    let birth = tokio::task::spawn_blocking(move||(super::process::liveness(pid,&key) == super::process::Liveness::Ours)
                        .then(||crate::terminal_facts::agent_birth(pid)).flatten()).await.unwrap_or_else(|join| {
                        tracing::warn!(key=%target.key,session=%target.name,reason=%join,"prova do cano vivo falhou; segue como queda");
                        None
                    });
                    if birth.is_some() { engine.cano_birth = birth; }
                    birth.is_some()
                };
                match event {
                    // Erro de UMA mensagem segue (como o leitor Python); só o erro de leitura encerra.
                    Some(IoEvent::Line(line)) => {
                        last_output = clock(start).monotonic_s;
                        // A saída voltou: uma nova perda nesta vida vai ao diário de novo.
                        if engine.output_lost() { output_lost_reported = false; }
                        effects.extend(engine.heard());
                        match engine.apply(EngineInput::Line(line),clock(start)) {
                            Ok(next)=>effects.extend(next),
                            Err(failure)=>if crate::warn_limit::allow(Some(&target.key),"cli_line") {
                                tracing::warn!(key=%target.key,session=%target.name,reason=%failure.message,"mensagem da CLI ignorada");
                            },
                        }
                    },
                    Some(event @ (IoEvent::WriteAck { .. } | IoEvent::WriteGuardAck { .. })) => {
                        let (operation_id,outcome,guard_error) = match event {
                            IoEvent::WriteAck { operation_id,outcome } => (operation_id,outcome,None),
                            IoEvent::WriteGuardAck { operation_id,outcome,error } => (operation_id,outcome,Some(error)),
                            _ => unreachable!(),
                        };
                        if let Some(attempt) = attempts.get(&operation_id) {
                            let logical_id = attempt.logical_id.clone();
                            let volatile = attempt.volatile;
                            let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                            jobs.spawn(async move {
                                let result = async {
                                  if volatile { return Ok(()); }
                                  let state = queue.snapshot().await.map_err(io_failure)?;
                                  if state.operations.get(&operation_id).is_some_and(|phase|matches!(phase.status,Status::Accepted | Status::Rejected | Status::Confirmed)) { return Ok(()); }
                                  queue.exec(generation,&format!("ack:{operation_id}:{}",unique()),sample,Action::Finish {
                                    id:operation_id,status:match outcome { WriteOutcome::Written=>Status::Accepted,
                                        WriteOutcome::NotWritten=>Status::Rejected,WriteOutcome::Unknown=>Status::Unknown },
                                    result:{
                                        let mut receipt = json!({"write_outcome":outcome});
                                        if let Some(error) = &guard_error {
                                            receipt["operation_id"] = json!(logical_id);
                                            receipt["code"] = json!(if outcome == WriteOutcome::Unknown { "erro_sem_confirmacao_resposta" } else { error.code.as_str() });
                                            if outcome == WriteOutcome::Unknown { receipt["source_code"] = json!(error.code); }
                                        }
                                        receipt
                                    } }).await.map(|_|()).map_err(io_failure)
                                }.await;
                                Job::Ack { logical_id,outcome,guard_error,result }
                            });
                        }
                    }
                    // O motivo de uma queda do Claude (`not logged in`, CLI quebrada) só sai pelo stderr.
                    Some(IoEvent::Stderr(text)) => if target.provider == "claude" {
                        effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_stderr","linha":text})),clock(start))?);
                    },
                    // Queda durante uma subida pedida é a do processo que ela encerrou: não agenda outra.
                    Some(IoEvent::End { .. }) if respawn.task.is_some() => respawn.swallowed = true,
                    None if respawn.task.is_some() => { io_open = false; respawn.swallowed = true; },
                    // Claude e Codex sem terminal: o socket caiu ou o leitor parou num envelope que não conhece
                    // (versões diferentes de cano e servidor), sem `cano_saiu`. O processo segue; religa ao mesmo cano
                    // com espera crescente, sem erro, sem queda, sem contar subida e sem estado novo. Cano morto vira
                    // queda pela tentativa ou pela conferência.
                    lost @ (None | Some(IoEvent::End { code:None })) if (target.provider == "claude" || codex_cano_alive) && engine.view()["alive"] != false => {
                        if lost.is_some() {
                            tracing::warn!(key=%target.key,session=%target.name,"o cano mandou um envelope que o servidor não conhece; religando");
                            effects.push_back(Effect::Diag { event:DiagEvent::CanoProtocolInvalid,code:"cano_protocol_invalid".into() });
                        }
                        io_open = false;
                        std::mem::replace(&mut io,super::codex_terminal::closed_io()).stop().await;
                        transport_epoch += 1;
                        reconnect.cancel();
                        let sample = clock(start);
                        (cano_lost,lost_at,stall_dirty) = (true,sample.epoch_s,true);
                        cano_check_at = Some(sample.monotonic_s + engine.reconnect_waits.check.as_secs_f64());
                        tracing::info!(key=%target.key,session=%target.name,provider=%target.provider,"socket do cano caiu; religando ao mesmo processo");
                        // Codex: o motor da volta (religado ou subido de novo) parte do turno que rodava.
                        if respawn.next_at.is_none() && !respawn.kill { respawn.was_working = engine.view()["in_progress"] == true; }
                        effects.extend(engine.transport_lost("",&std::mem::take(&mut handed))?);
                        reconnect.schedule(sample.monotonic_s,false);
                    },
                    Some(IoEvent::End { code }) => {
                        if respawn.next_at.is_none() { respawn.was_working = engine.view()["in_progress"] == true; }
                        effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":code})),clock(start))?);
                        if target.provider=="claude" {publish(&events,&target,&mut revision,"claude_exit",json!({"conversation":engine.view()["conversation"]}));}
                        respawn.kill = true;
                        effects.extend(after_exit(&mut respawn,&mut engine,&target,clock(start).monotonic_s,closed.load(Ordering::Acquire)));
                    }
                    None => {
                        io_open = false; enter_error(&mut error,&target,failure("cano_closed"));
                        if respawn.next_at.is_none() && !respawn.kill { respawn.was_working = engine.view()["in_progress"] == true; }
                        effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":null})),clock(start))?);
                        if target.provider=="claude" {publish(&events,&target,&mut revision,"claude_exit",json!({"conversation":engine.view()["conversation"]}));}
                        effects.extend(after_exit(&mut respawn,&mut engine,&target,clock(start).monotonic_s,closed.load(Ordering::Acquire)));
                    },
                }
            }
            relaunched = async { respawn.task.as_mut().unwrap().await }, if respawn.task.is_some() => {
                respawn.task = None;
                match relaunched.unwrap_or_else(|_|Err((failure("respawn_panic"),Value::Null))) {
                    Ok(Relaunched { target:next,spawned,connection,patch }) => {
                        io.stop().await;
                        let snapshot = connection.snapshot.clone();
                        io = connection.start(next.generation,128).hold_lease(queue.lease());
                        io_open = true;
                        // Subida pedida no meio da queda do socket: o processo e o socket agora são estes.
                        reconnect.cancel();
                        (cano_lost,cano_check_at,stall_due,stall_shown) = (false,None,None,false);
                        if error.as_ref().is_some_and(|current|current.code == "cano_closed") { error = None; }
                        // A vida nova parte da vista desta (conversa, modelo, modo) e do que a pessoa trocou.
                        let mut metadata = next.metadata.clone();
                        if let Some(fields) = engine.view().as_object() {
                            for (key,value) in fields { if key != "public_state" && key != "conversation" { metadata[key] = value.clone(); } }
                        }
                        // O que uma subida anterior gravou no arquivo vale aqui também: o comando já saiu dele.
                        for fields in [std::mem::take(&mut respawn.carried),patch].iter().filter_map(Value::as_object) {
                            for (key,value) in fields { metadata[key] = value.clone(); }
                        }
                        metadata["in_progress"] = json!(std::mem::take(&mut respawn.was_working));
                        target = next;
                        let mut renewed = engine.renewed(metadata,target.generation,clock(start))?;
                        renewed.set_fresh_process(spawned);
                        engine = renewed;
                        (respawn.kill,respawn.swallowed) = (false,false);
                        effects.extend(engine.hydrate(snapshot)?);
                        let id = format!("bootstrap:{}:{}:{}",target.key,target.generation,unique());
                        match queue.exec(target.generation,&format!("prepare:{id}"),clock(start),Action::Prepare { id:id.clone(),payload:json!({"kind":"bootstrap"}),entry_id:None }).await {
                            Ok(_)=>effects.extend(engine.initialize(id)?),
                            Err(failure)=>enter_error(&mut error,&target,io_failure(failure)),
                        }
                        for (operation_id,payload) in respawn.user.take().into_iter().chain(std::mem::take(&mut respawn.waiting)) {
                            effects.push_back(Effect::Reply { operation_id,disposition:Disposition::Accepted,payload });
                        }
                        drain_requested = true;
                    }
                    Err((failure,written)) => {
                        if !written.is_null() { respawn.carried = written; }
                        tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,reason=%failure.message,attempt=respawn.failures,"processo da sessão não subiu de novo");
                        let user = respawn.user.take();
                        if let Some((operation_id,_)) = &user { fail_root(&mut roots,operation_id,failure.clone()); }
                        for (operation_id,_) in std::mem::take(&mut respawn.waiting) { fail_root(&mut roots,&operation_id,failure.clone()); }
                        let gone = std::mem::take(&mut respawn.swallowed);
                        if gone {
                            effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":null})),clock(start))?);
                            respawn.kill = true;
                        }
                        respawn.last = Some(failure.clone());
                        // Pedido da pessoa (gravar o modo, encerrar) que falhou no kill: o processo segue vivo e a
                        // sessão como estava. Falha depois do kill (gravação) já o derrubou: a queda engolida
                        // (`gone`) ou a que chegar depois agenda pelo caminho normal.
                        if user.is_none() || gone {
                            effects.extend(engine.set_problem("codex_headless_nao_subiu",Some(failure.message)));
                            effects.extend(after_exit(&mut respawn,&mut engine,&target,clock(start).monotonic_s,closed.load(Ordering::Acquire)));
                        }
                        // Pedida no meio da queda do socket: a religação, parada durante a subida, recomeça (cano morto
                        // vira queda pela tentativa ou pela conferência).
                        if cano_lost && reconnect.task.is_none() && reconnect.next_at.is_none() {
                            reconnect.schedule(clock(start).monotonic_s,stall_due.is_some() || stall_shown);
                        }
                    }
                }
            }
            attempt = async { reconnect.task.as_mut().unwrap().await }, if reconnect.task.is_some() => {
                use super::cano_reconnect::Attempt as Reconnection;
                reconnect.task = None;
                let attempt = attempt.unwrap_or_else(|_|Reconnection::Retry(failure("reconnect_panic")));
                // Tentativa de outra época (queda nova, parada) não vale: o socket dela fecha ao sair daqui.
                let current = reconnect.epoch == transport_epoch && !closed.load(Ordering::Acquire);
                match attempt {
                    Reconnection::Connected(RuntimeConnection::WebSocket(connection)) if adoption.is_some() && current => {
                        transport_epoch += 1;
                        io = connection.start(target.generation,128).hold_lease(queue.lease());
                        io_open = true;
                        reconnect.connected();
                        tracing::info!(key=%target.key,session=%target.name,"Codex com terminal reconectado; assinando a mesma conversa");
                        // A TUI pode ter aberto outra conversa durante a queda (`/new`): o vínculo segue a do arquivo da
                        // sessão desta vida, como o `ensure_running` do Python, antes de assinar.
                        if let Some(life) = adoption.as_ref().and_then(|adoption|adoption.cell.sidecar()).cloned() {
                            let meta = tokio::task::spawn_blocking(move||super::codex_sidecar::load(&super::codex_sidecar::path(&life.dir,&life.name))
                                .filter(|meta|meta.get("key").and_then(Value::as_str) == Some(life.key.as_str()))).await.ok().flatten();
                            if let Some(meta) = meta && let Some(thread) = meta.get("thread_id").and_then(Value::as_str)
                                && engine.view()["thread_id"].as_str().is_some_and(|current|current != thread) {
                                let moved = json!({"method":"thread/started","params":{"thread":{"id":thread,"path":meta.get("rollout_path"),
                                    "cwd":meta.get("cwd"),"source":"cli"}}});
                                effects.extend(engine.apply(EngineInput::Line(moved),clock(start))?);
                            }
                        }
                        // Mesma thread, inicializada e assinada de novo; conectado e assinado voltam separados.
                        let id = format!("bootstrap:{}:{}:{}",target.key,target.generation,unique());
                        match queue.exec(target.generation,&format!("prepare:{id}"),clock(start),Action::Prepare { id:id.clone(),payload:json!({"kind":"bootstrap"}),entry_id:None }).await {
                            Ok(_)=>effects.extend(engine.initialize(id)?),
                            Err(failure)=>enter_error(&mut error,&target,io_failure(failure)),
                        }
                        // A fila e os recibos se acertam com o transcript antes de qualquer entrega nova.
                        if !confirming {
                            confirming = true;
                            let view = engine.view();
                            let job = confirm_inputs(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                            jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
                        }
                        drain_requested = true;
                    }
                    Reconnection::Retry(failure) if adoption.is_some() => {
                        if crate::warn_limit::allow(Some(&target.key),"codex_terminal_reconnect") {
                            tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,"reconexão ao app-server do Codex falhou; tentando de novo");
                        }
                        effects.extend(engine.set_problem(super::codex_terminal::CONTROL_LOST,Some(format!("reconectando ({})",failure.code))));
                        reconnect.schedule(clock(start).monotonic_s,false);
                    }
                    Reconnection::Gone(failure) if adoption.is_some() => {
                        tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,"app-server do Codex com terminal saiu ou mudou; a vigia decide");
                        effects.extend(engine.set_problem(super::codex_terminal::DISCONNECTED,Some(failure.message)));
                        if let Some(adoption) = &adoption { adoption.cell.wake(); }
                    }
                    // Daqui para baixo, o cano do Claude e do Codex sem terminal.
                    _ if adoption.is_some() || !current || !cano_lost => {},
                    Reconnection::Connected(RuntimeConnection::Cano(connection)) => {
                        let snapshot = connection.snapshot.clone();
                        // Saiu entre a queda e a volta: é queda, com o código que o cano guardou.
                        if let Some(rc) = snapshot.saiu { cano_gone = Some(Some(rc)); continue; }
                        transport_epoch += 1;
                        std::mem::replace(&mut io,connection.start(target.generation,128).hold_lease(queue.lease())).stop().await;
                        io_open = true;
                        reconnect.connected();
                        (cano_lost,cano_check_at,stall_due) = (false,None,None);
                        last_output = clock(start).monotonic_s;
                        tracing::info!(key=%target.key,session=%target.name,provider=%target.provider,"sessão sem terminal religada ao mesmo cano");
                        effects.extend(engine.reattach(snapshot)?);
                        // O Codex abre a mesma thread de novo na conexão nova (o `initialize` repetido é sucesso). O Claude
                        // com `initialize` em voo na queda reenvia: o retrato não guarda a resposta e a CLI aceita o segundo.
                        if std::mem::take(&mut boot_pending) || target.provider == "codex" || engine.view()["iniciando"] == true {
                            let id = format!("bootstrap:{}:{}:{}",target.key,target.generation,unique());
                            match queue.exec(target.generation,&format!("prepare:{id}"),clock(start),Action::Prepare { id:id.clone(),payload:json!({"kind":"bootstrap"}),entry_id:None }).await {
                                Ok(_)=>effects.extend(engine.initialize(id)?),
                                Err(failure)=>enter_error(&mut error,&target,io_failure(failure)),
                            }
                        }
                        if std::mem::take(&mut stall_shown) { effects.extend(engine.clear_problem(DELIVERY_STALLED)); }
                        // O que pode ter saído antes da queda se acerta com o transcript antes da entrega nova.
                        if !confirming {
                            confirming = true;
                            let view = engine.view();
                            let job = confirm_inputs(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                            jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
                        }
                        drain_requested = true;
                    }
                    Reconnection::Connected(RuntimeConnection::WebSocket(_)) => {},
                    Reconnection::Retry(failure) => {
                        if crate::warn_limit::allow(Some(&target.key),"cano_reconnect") {
                            tracing::info!(key=%target.key,session=%target.name,provider=%target.provider,code=%failure.code,"religação ao cano falhou; tentando de novo");
                        }
                        reconnect.schedule(clock(start).monotonic_s,stall_due.is_some() || stall_shown);
                    }
                    Reconnection::Gone(_) => cano_gone = Some(None),
                    // Sem prova de que o pid é o mesmo processo, para de tentar; a sessão segue e a fila espera.
                    Reconnection::Stopped(failure) => {
                        tracing::warn!(key=%target.key,session=%target.name,provider=%target.provider,code=%failure.code,reason=%failure.message,"religação ao cano parou");
                        effects.push_back(Effect::Diag { event:DiagEvent::CanoReconnectStopped,code:"cano_reconnect_stopped".into() });
                        reconnect.cancel();
                    }
                }
            }
            result = jobs.join_next(), if !jobs.is_empty() => {
                let job = result.ok_or_else(||failure("job_missing"))?.map_err(|_|failure("job_panic"))?;
                match job {
                    Job::Root { id,result } => {
                        let stored = match result { Ok(stored)=>stored,Err(failure)=>{ fail_root(&mut roots,&id,failure); continue; } };
                        let pending = roots.get_mut(&id).unwrap();
                        // A fila já tem o desfecho (linha confirmada ou operação final): responde sem escrever no fio.
                        if let Some(reply) = stored {
                            pending.preparing = false;
                            pending.result = Some(reply.clone());
                            for response in pending.responses.drain(..) { let _ = response.send(Ok(reply.clone())); }
                            continue;
                        }
                        // Esgotada: o prazo já respondeu (adiada, sem escrita); uma segunda resposta a tornaria incerta.
                        if pending.timed_out { pending.preparing = false; continue; }
                        if pending.cancelled {
                            pending.preparing = false;
                            effects.push_back(Effect::Reply { operation_id:id,disposition:Disposition::Rejected,
                                payload:json!({"error":"input cancelado antes do envio"}) });
                            continue;
                        }
                        if matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer) {
                            let target = target.clone(); let command = pending.command.clone();
                            let queue=queue.clone();let from_queue=pending.from_queue;
                            jobs.spawn(async move {
                                let result=async {
                                    if from_queue {
                                        let state=queue.snapshot().await.map_err(io_failure)?;
                                        if let Some(prepared)=state.rows.iter().find(|row|row["id"]==command.operation_id)
                                            .map(|row|&row["batch_prompt"]).filter(|prepared|prepared.is_object()) {return Ok(prepared.clone());}
                                    }
                                    run_local("prepare_prompt".into(),command.payload,&target,None).await
                                }.await;
                                Job::PreparedInput { id,result }
                            });
                        } else {
                            pending.ready_to_run = true;
                        }
                    }
                    Job::PreparedInput { id,result } => {
                        let Some(pending) = roots.get_mut(&id) else { continue };
                        if pending.cancelled || pending.timed_out { continue; }
                        match result {
                            Ok(payload) => {
                                for (key,value) in payload.as_object().cloned().unwrap_or_default() { pending.command.payload[key] = value; }
                                if target.provider == "claude" && pending.command.payload["native_candidate"] == true {
                                    // O recado nativo sai pelo socket, fora de `attempts`: a partir daqui ele pode ter
                                    // sido escrito, e nem o prazo nem uma falha podem devolvê-lo à fila.
                                    native.insert(id.clone());
                                    let queue = queue.clone(); let target = target.clone();
                                    let command = pending.command.clone(); let original = pending.original.clone(); let view = engine.view(); let sample = clock(start);
                                    let lifecycle = engine.lifecycle.clone();
                                    jobs.spawn(async move {
                                        let result = async {
                                            // Uma fase por tentativa: a adiada volta pelo drain e não pode reaproveitar os recibos da anterior.
                                            let phase = format!("{id}:native_message:{}",unique());
                                            // A classe do recado sai da memória de modo do Rust.
                                            let text = command.payload["text"].as_str().unwrap_or("");
                                            let sender = crate::terminal_input::message_prefix(text).map(|(sender,_)|sender).filter(|_|!text.starts_with("[painel:"));
                                            // Leitura que falha sobe com o código; sem registro ligado (só fora do servidor) vai sem
                                            // classe, e o envio recusa com `native_mode_class_missing` em vez de adivinhar.
                                            let registry = lifecycle.as_ref().and_then(std::sync::Weak::upgrade);
                                            let class = match &registry {
                                                Some(registry)=>match registry.message_class(sender,&target.name).await {
                                                    Ok(class)=>Some(class),
                                                    // Nada foi escrito ainda: o recado segue pelo cano como texto comum, nunca incerto.
                                                    Err(error)=>{
                                                        if crate::warn_limit::allow(Some(&target.key),&error.code) {
                                                            tracing::warn!(key=%target.key,session=%target.name,code=%error.code,"classe do recado não se leu; vai pelo cano");
                                                        }
                                                        return Ok(json!({"outcome":"not_written","error_code":error.code}));
                                                    },
                                                },
                                                None=>None,
                                            };
                                            let payload = json!({"text":command.payload["text"],"mode":class});
                                            queue.exec(target.generation,&format!("prepare:{phase}"),sample,Action::Prepare { id:phase.clone(),
                                                payload:json!({"kind":"native_message","request_id":id,"payload":payload}),entry_id:None }).await.map_err(io_failure)?;
                                            let cursor = capture_cursor(&target,&view).await?;
                                            queue.exec(target.generation,&format!("native-cursor:{phase}"),sample,Action::BindDispatch { id:id.clone(),cursor }).await.map_err(io_failure)?;
                                            queue.exec(target.generation,&format!("native-dispatch:{phase}"),sample,Action::BeginDispatch { id:id.clone(),wire_id:phase.clone(),staged:false }).await.map_err(io_failure)?;
                                            queue.exec(target.generation,&format!("dispatch:{phase}"),sample,Action::BeginDispatch { id:phase.clone(),wire_id:phase.clone(),staged:false }).await.map_err(io_failure)?;
                                            let route = registry.as_ref().map(|registry|registry.native_route(&target.key,&id));
                                            let result = super::native_message::send(&target.metadata,text,class,route,&crate::terminal_input::ProcessIo::default()).await?;
                                            let outcome = result["outcome"].as_str().ok_or_else(||failure("native_outcome"))?;
                                            let status = match outcome { "written"=>Status::Accepted,"not_written"=>Status::Rejected,"unknown"=>Status::Unknown,_=>return Err(failure("native_outcome")) };
                                            queue.exec(target.generation,&format!("finish:{phase}"),sample,Action::Finish { id:phase.clone(),status,result:result.clone() }).await.map_err(io_failure)?;
                                            if outcome == "not_written" {
                                                queue.exec(target.generation,&format!("native-defer:{phase}"),sample,Action::Finish { id:id.clone(),status:Status::Deferred,result:json!({"not_written":true}) }).await.map_err(io_failure)?;
                                                let state = queue.snapshot().await.map_err(io_failure)?;
                                                let entry_id = state.operations[&id].entry_id.clone();
                                                queue.exec(target.generation,&format!("native-fallback:{phase}"),sample,Action::Prepare { id:id.clone(),payload:original,entry_id }).await.map_err(io_failure)?;
                                            }
                                            Ok(result)
                                        }.await;
                                        Job::NativeInput { id,result }
                                    });
                                } else { pending.ready_to_run = true; }
                            }
                            Err(error)=>defer_unwritten(&target.provider,&mut roots,&attempts,&native,&id,error,&mut effects),
                        }
                    }
                    Job::Write { wire,generation,result } => {
                        let attempt = attempts.get(&wire).unwrap();
                        prepared_writes.insert(attempt.order,(wire,generation,result));
                    }
                    Job::Ack { logical_id,outcome,guard_error,result } => {
                        match result {
                            Ok(())=>{
                                let mut next = engine.apply(EngineInput::WriteAck { operation_id:logical_id.clone(),outcome },clock(start))?;
                                if let Some(failure) = guard_error {
                                    for effect in &mut next {
                                        if let Effect::Reply { operation_id,payload,.. } = effect && operation_id == &logical_id {
                                            // A resposta terminal continua dizendo que a decisão não foi confirmada.
                                            let confirmed = payload.get("decision_confirmed").cloned();
                                            *payload = if outcome == WriteOutcome::NotWritten {
                                                json!({"code":failure.code,"error":failure.message,"write_outcome":"not_written"})
                                            } else { json!({"code":"erro_sem_confirmacao_resposta","source_code":failure.code,"write_outcome":"unknown"}) };
                                            if let Some(confirmed) = confirmed { payload["decision_confirmed"] = confirmed; }
                                        }
                                    }
                                }
                                effects.extend(next);
                                engine.write_sources.lock().map_err(|_|failure("interaction_sources"))?.remove(&logical_id);
                                engine.async_guards.lock().map_err(|_|failure("interaction_sources"))?.remove(&logical_id);
                            },
                            Err(failure)=>{
                                fail_root(&mut roots,&logical_id,failure.clone());
                                publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                                enter_error(&mut error,&target,failure);
                            }
                        }
                    }
                    Job::Finished { generation,reply,result } => {
                        if generation!=target.generation {continue;}
                        // Mensagem adiada com o socket fora: ficou na fila e antecipa a religação.
                        if cano_lost && reply.disposition == Disposition::Deferred { stall_dirty = true; reconnect.wake(clock(start).monotonic_s); }
                        // Resposta reservada já gravada: o desfecho é o da reserva (sai na vez dela); a falha do
                        // diário vai ao log, como na própria gravação da reserva.
                        let reserved = reply.payload["reserved"] == true;
                        if let (Err(failure),false) = (&result,reserved) { fail_root(&mut roots,&reply.operation_id,failure.clone()); enter_error(&mut error,&target,failure.clone()); }
                        else {
                          if let Err(failure) = &result {
                              tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,"desfecho da resposta reservada não foi gravado; a reserva segue");
                          }
                          if !roots.contains_key(&reply.operation_id) {
                              if let Some(command) = initial.operations.get(&reply.operation_id).and_then(|operation|serde_json::from_value::<RuntimeCommand>(operation.payload.clone()).ok()) {
                                  roots.insert(reply.operation_id.clone(),Pending::stored(command,reply.clone()));
                              }
                          }
                          if let Some(pending) = roots.get_mut(&reply.operation_id) {
                            pending.preparing = false;
                            pending.result = Some(reply.clone());
                            for response in pending.responses.drain(..) { let _ = response.send(Ok(reply.clone())); }
                            if reply.disposition == Disposition::Deferred && matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer | OperationKind::DispatchAsyncAnswer)
                                && !rowless(&target.provider,&pending.command) {
                                let queue = queue.clone(); let id = reply.operation_id.clone(); let generation = target.generation; let sample = clock(start);
                                let wake = pending.command.kind == OperationKind::DispatchAsyncAnswer;
                                jobs.spawn(async move {
                                    let result = queue.exec(generation,&format!("unclaim:{}",unique()),sample,
                                        Action::SetDelivered { entry_id:id,value:false,steered:false }).await.map(|_|()).map_err(io_failure);
                                    // A reserva só pode acordar depois de a entrada voltar à fila no disco.
                                    if wake { Job::Queued { wake:true,result } } else { Job::Saved(result) }
                                });
                            }
                            // Reserva recusada (pergunta encerrada, outra conversa, turno recusado): a entrada não volta a sair.
                            if reply.disposition == Disposition::Rejected && pending.command.kind == OperationKind::DispatchAsyncAnswer {
                                let queue = queue.clone(); let id = reply.operation_id.clone(); let generation = target.generation; let sample = clock(start);
                                jobs.spawn(async move { Job::Saved(queue.exec(generation,&format!("async-abandon:{}",unique()),sample,
                                    Action::Abandon { entry_id:id }).await.map(|_|()).map_err(io_failure)) });
                            }
                          }
                        }
                    }
                    Job::Queued { wake,result } => {
                        // Fila mexida com o socket fora: o aviso se refaz e a pessoa por perto antecipa a religação.
                        if cano_lost && wake { stall_dirty = true; reconnect.wake(clock(start).monotonic_s); }
                        match result {
                            Ok(()) if wake=>{effects.extend(engine.queue_problem(None));drain_requested = true;},
                            Ok(())=>{},
                            Err(failure)=>enter_error(&mut error,&target,failure),
                        }
                    }
                    Job::Policy { request_id,kind,phase_id,conversation,result } => {
                        let _ = phase_id;
                        match result {
                            Ok(payload) => {
                                // O Python só religa quando o arquivo da sessão já tem a conversa da vista: a
                                // conversa nova gravada sai de novo na vista, para a religação acontecer agora.
                                let current = engine.view()["conversation"].clone();
                                let conversation = conversation.is_some_and(|value|!value.is_null() && value == current);
                                if kind == "session.patch_meta" && payload["updated"] == true && conversation && durable_view["conversation"] == current {
                                    publish(&events,&target,&mut revision,"view",durable_view.clone());
                                }
                                // Recusado como velho: Fast de outra thread e thread já trocada são esperados (só log);
                                // a conversa ATUAL recusada deixa o arquivo para trás e aparece como problema.
                                if kind == "session.patch_meta" && payload["stale"] == true {
                                    if crate::warn_limit::allow(Some(&target.key),"session_patch_stale") {
                                        tracing::warn!(key=%target.key,session=%target.name,code="session_patch_stale",
                                            "o arquivo da sessão recusou o patch como velho");
                                    }
                                    if conversation {
                                        let message = "o arquivo da sessão não aceitou a conversa nova";
                                        publish(&events,&target,&mut revision,"problem",json!({"error_code":"session_patch_stale","message":message}));
                                        effects.extend(engine.set_problem("session_patch_stale",Some(message.into())));
                                    }
                                }
                                effects.extend(engine.apply(EngineInput::PolicyResult { request_id,payload },clock(start))?);
                            }
                            // Linha de status, carimbo, uso e registro que falham só perdem aquela parte: a sessão
                            // segue no Rust (o motivo já foi para o log pelo cliente da política). Sidecar e catálogo
                            // de skills seguram estado da sessão: a falha deles continua levando-a ao Python.
                            Err(failure) if COSMETIC_POLICIES.contains(&kind.as_str()) => {
                                if crate::warn_limit::allow(Some(&target.key),&format!("policy:{kind}")) {
                                    tracing::warn!(key=%target.key,session=%target.name,policy=%kind,code=%failure.code,"serviço cosmético falhou; a sessão segue no Rust");
                                }
                                engine.forget_policy(&request_id);
                                publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                            },
                            Err(failure)=>{
                                publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                                // O "Pular" do Codex espera esta gravação para responder: ela falhou, a pergunta volta.
                                if target.provider == "codex" && kind == "session.patch_meta" {
                                    effects.extend(engine.apply(EngineInput::PolicyResult { request_id,payload:json!({"updated":false,"error":failure.code}) },clock(start))?);
                                }
                                enter_error(&mut error,&target,failure);
                            },
                        }
                    }
                    Job::View { generation,version,view,result } => {
                        if generation!=target.generation || sid_transition {continue;}
                        match result {
                            Ok(()) if version > published_state_version => {
                                published_state_version = version;
                                // Confirmar em todo idle, como o adapter Python: sem isto nenhuma entrada vira
                                // confirmed, a poda não as alcança e a fila enche.
                                let state = view["public_state"]["state"].as_str().unwrap_or("").to_owned();
                                // Sem pane não há hook `Notification`: o marcador do state_hook põe a espera na mesma
                                // esteira do push de "aguardando" e da pausa do loop das sessões com terminal.
                                // A queda grava `dead` (push de "caiu", como no tmux); a saída limpa só desfaz a espera.
                                if target.provider == "claude" {
                                    let (marker,write) = state_marker(&last_marker,&view);
                                    if write { write_state_marker(&target,&view,marker); }
                                    last_marker = marker.to_owned();
                                }
                                if target.provider == "codex" && state == "idle" && last_state != "idle" && confirming { confirm_again = true; }
                                if state == "idle" && last_state != "idle" && !confirming {
                                    confirming = true;
                                    let job = confirm_inputs(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                                    jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
                                }
                                last_state = state;
                                // Vista igual à publicada não sai: cada aparelho redesenharia a tela à toa.
                                if view != durable_view {
                                    live.update(|state| {
                                        state.binding=view["conversation"].as_str().filter(|conversation|!conversation.is_empty()).map(|conversation|crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:conversation.into()});
                                        state.public_state = view["public_state"].clone();
                                    });
                                    durable_view = view.clone();
                                    publish(&events,&target,&mut revision,"view",view.clone());
                                    publish(&events,&target,&mut revision,"state",view["public_state"].clone());
                                }
                            }
                            Ok(()) => {},
                            Err(failure) => {
                                publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                                enter_error(&mut error,&target,failure);
                            }
                        }
                    }
                    Job::ClaudeSid {context,result}=> {
                        sid_commit_inflight=false;
                        if !sid_transition || !context.current(&target,&engine.view(),closed.load(Ordering::Acquire)) {sid_retry=None;continue;}
                        let previous=context.previous.clone();
                        match result {
                            Ok(next) if next.metadata["session_id"]==engine.view()["conversation"]=> {
                                let old_generation=target.generation;
                                engine.advance_claude_generation(next.generation)?;
                                target=next;sid_transition=false;mode_commit=None;
                                if engine.view()["initialized"]!=true {effects.extend(engine.initialize(format!("bootstrap-sid:{}:{}",target.key,target.generation))?);}
                                if let Some((key,store))=&engine.interactions {store.forget_binding(&crate::interactions::InteractionBinding {key:key.clone(),conversation:previous,generation:old_generation}).map_err(crate::interactions::runtime_error)?;}
                                state_version+=1;
                                save_view(&queue,target.generation,clock(start),&state_gate,state_version,&engine.view(),false).await?;
                                durable_view=engine.view();
                                live.update(|state| {state.binding=Some(crate::interactions::InteractionBinding {key:target.key.clone(),conversation:target.metadata["session_id"].as_str().unwrap_or("").into(),generation:target.generation});state.public_state=durable_view["public_state"].clone();});
                                publish(&events,&target,&mut revision,"reset",json!({"conversation":target.metadata["session_id"],"generation":target.generation}));
                                drain_requested=true;
                            },
                            Ok(_)=>enter_error(&mut error,&target,failure("lifecycle_sid_changed_again")),
                            Err(failed)=> {
                                if let Some(at)=context.retry_at(&target,&engine.view(),closed.load(Ordering::Acquire),&failed,clock(start).monotonic_s) {sid_retry=Some((context,at));}
                                else {sid_retry=None;enter_error(&mut error,&target,failed);}
                            },
                        }
                    },
                    Job::Saved(result) => { stall_dirty |= cano_lost; if let Err(failure) = result {
                        publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                        enter_error(&mut error,&target,failure);
                    } },
                    Job::Drained {generation,result} => {
                        if generation!=target.generation || sid_transition {
                            drain_active=false;
                            for response in drain_waiters.drain(..) {let _=response.send(Err(failure("runtime_sid_transition")));}
                            continue;
                        }
                        match result {
                            Ok(commands) if commands.is_empty() => {
                                drain_active = false;
                                for waiter in drain_waiters.drain(..) { let _ = waiter.send(Ok(json!({"sent":0}))); }
                            }
                            Ok(commands) => for command in commands {
                                let (response,receive) = oneshot::channel();
                                let internal = internal.clone();
                                jobs.spawn(async move {
                                    let result = match internal.send(Message::Command { command,response,from_queue:true,generation }).await {
                                        Ok(())=>receive.await.map_err(|_|failure("runtime_closed")).and_then(|result|result),
                                        Err(_)=>Err(failure("runtime_closed")),
                                    };
                                    Job::DrainFinished(result)
                                });
                            },
                            Err(failure)=>{
                                drain_active = false;
                                for waiter in drain_waiters.drain(..) { let _ = waiter.send(Err(failure.clone())); }
                                if failure.code == super::queue::BATCH_TOO_LARGE { effects.extend(engine.queue_problem(Some(failure.message))); }
                                else { enter_error(&mut error,&target,failure); }
                            },
                        }
                    }
                    Job::DrainFinished(result) => {
                        drain_active = false;
                        let count = result.as_ref().map(drained_count);
                        for waiter in drain_waiters.drain(..) {
                            let _ = waiter.send(count.clone().map(|sent|json!({"sent":sent})).map_err(Clone::clone));
                        }
                        if let Err(failure) = result { enter_error(&mut error,&target,failure); }
                    }
                    Job::Confirmed { response,result } => {
                        if response.is_none() { confirming = false; }
                        // Sem terminal a conferência volta sozinha até cada entrada confirmar ou desistir; falha tenta de novo.
                        if target.provider == "claude" {
                            confirm_recheck = match &result {
                                Ok((_,_,due)) => due.map(|due|Instant::now() + Duration::from_secs_f64((due - clock(start).epoch_s).max(0.0) + 0.5)),
                                Err(_) => Some(Instant::now() + CONFIRM_RETRY),
                            };
                        }
                        match result {
                            Ok((ids,legacy,_))=>{
                                // Sem desligar ao confirmar: outra entrada da janela pode esperar a vez dela; o
                                // prazo encerra a vigília, que só custa um `stat` por volta.
                                for id in &ids { effects.extend(engine.confirm_input(id)); }
                                if let Some(response) = response { let _ = response.send(Ok(json!({"confirmed":ids.len() + legacy}))); }
                            }
                            Err(error)=>{
                                if crate::warn_limit::allow(Some(&target.key),&error.code) {
                                    tracing::warn!(key=%target.key,session=%target.name,code=%error.code,"confirmação da fila falhou");
                                }
                                if let Some(response) = response { let _ = response.send(Err(error)); }
                            }
                        }
                        if !confirming && std::mem::take(&mut confirm_again) {
                            confirming = true;
                            let view = engine.view();
                            let job = confirm_inputs(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                            jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
                        }
                    }
                    Job::TurnCheck { since,result } => {
                        turn_checking = false;
                        let sample = clock(start);
                        let now = sample.monotonic_s;
                        let lost = engine.output_lost();
                        let after = engine.reconnect_waits.output_lost.as_secs_f64();
                        // O instante do fim: o carimbo da linha; sem ele, a primeira vez que esta conferência a viu.
                        let result = result.map(|found|found.map(|(offset,stamp)|stamp.unwrap_or_else(|| {
                            match end_seen { Some((seen,at)) if seen == offset => at, _ => {
                                if crate::warn_limit::allow(Some(&target.key),"turn_end_stamp") {
                                    tracing::warn!(key=%target.key,session=%target.name,offset,"fim de turno sem carimbo legível; o silêncio conta de agora");
                                }
                                end_seen = Some((offset,sample.epoch_s));
                                sample.epoch_s
                            } }
                        })));
                        // Antes da saída perdida, o silêncio conta também do fim do turno: o `result` sai logo depois dele.
                        let quiet = |ended:f64|lost || since == last_output && sample.epoch_s - ended >= after;
                        match result {
                            // Sem terminal o Claude grava o fim do turno no transcript antes do `result`: com o cano mudo
                            // depois dele, o `result` não vem mais e o turno termina pelo transcript.
                            Ok(Some(ended)) if engine.turn_watch() && quiet(ended) => {
                                if !lost && !std::mem::replace(&mut output_lost_reported,true) {
                                    tracing::warn!(key=%target.key,session=%target.name,"o cano não repassa a saída do Claude; o fim dos turnos vem do transcript");
                                    effects.push_back(Effect::Diag { event:DiagEvent::CanoOutputLost,code:"cano_output_lost".into() });
                                }
                                effects.extend(engine.transcript_turn_end());
                            }
                            Ok(Some(ended)) if !lost => turn_check_after = now + (after - (sample.epoch_s - ended)).clamp(TURN_POLL.as_secs_f64(),after),
                            Ok(_) => turn_check_after = now + if lost { TURN_POLL.as_secs_f64() } else { after },
                            Err(failure) => {
                                if crate::warn_limit::allow(Some(&target.key),"turn_check") {
                                    tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,"fim do turno no transcript não conferido");
                                }
                                turn_check_after = now + engine.reconnect_waits.output_lost.as_secs_f64();
                            }
                        }
                    }
                    Job::Steered { id,result } => {
                        let (disposition,payload) = match result {
                            Ok(ids)=>(Disposition::Accepted,json!({"ids":ids})),
                            Err(error)=>(Disposition::Unknown,json!({"error":error.message,"error_code":error.code})),
                        };
                        effects.push_back(Effect::Reply { operation_id:id,disposition,payload });
                    }
                    Job::Reserved { operation_id,reservation,result,persisted } => {
                        if let Err(failure) = &result {
                            tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,persisted=?persisted,"gravação da resposta reservada falhou");
                        }
                        match persisted {
                            // Gravada (mesmo com erro depois): ela sai na vez dela e o cartão fecha, como o envio na hora
                            // do Python; a decisão em si continua sem confirmação até o turno.
                            Some(true) => {
                                effects.extend(engine.settle_async_answer(&reservation.entry_id,true));
                                // Gravada sem erro: aceita. Com erro da fila (mesmo gravada), o incerto que manda conferir.
                                let disposition = if result.is_ok() { Disposition::Accepted } else { Disposition::Unknown };
                                effects.push_back(Effect::Reply { operation_id,disposition,payload:reservation.reply_payload() });
                            }
                            Some(false) => {
                                effects.extend(engine.settle_async_answer(&reservation.entry_id,false));
                                fail_root(&mut roots,&operation_id,result.err().unwrap_or_else(||failure("reserve_lost")));
                            }
                            // Sem saber o que ficou no disco: nada sai nesta vida, a retomada decide pela operação gravada.
                            None => {
                                engine.uncertain_async_answer(&reservation.entry_id);
                                effects.push_back(Effect::Reply { operation_id,disposition:Disposition::Unknown,
                                    payload:json!({"entry_id":reservation.entry_id,"decision_confirmed":false}) });
                            }
                        }
                    }
                    Job::NativeInput { id,result } => {
                        match result {
                            Ok(result) if result["outcome"] == "not_written" => {
                                native.remove(&id);
                                if let Some(root) = roots.get_mut(&id) {
                                    if !root.cancelled && !root.timed_out { root.ready_to_run = true; }
                                }
                            }
                            Ok(result)=>effects.push_back(Effect::Reply { operation_id:id,
                                disposition:if result["outcome"] == "written" { Disposition::Accepted } else { Disposition::Unknown },payload:result }),
                            Err(error)=>{
                                effects.push_back(Effect::Reply { operation_id:id,disposition:Disposition::Unknown,payload:json!({"error_code":error.code,"error":error.message}) });
                            }
                        }
                    }
                }
            }
            // Só acorda; a limpeza da retenção solta é a do topo do laço.
            _ = turn_hold_released.notified(), if turn_hold.is_some() => {}
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(confirm_recheck.unwrap_or_else(Instant::now))), if confirm_recheck.is_some() && !confirming => {
                confirm_recheck = None;
                confirming = true;
                let view = engine.view();
                let job = confirm_inputs(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
            }
            _ = tokio::time::sleep(CONFIRM_POLL), if confirm_until.is_some() && !confirming => {
                if confirm_until.is_some_and(|until|Instant::now() >= until) { confirm_until = None; continue; }
                // O transcript da conversa vigente: a TUI do Codex com terminal pode ter trocado de conversa.
                let view = engine.view();
                if target.provider == "claude" && !prompt_check.swap(true,Ordering::AcqRel) {
                    let base = account_base(&target,&prompt_env);
                    let conversation = view["conversation"].as_str().unwrap_or("").to_owned();
                    jobs.spawn(mark_prompt_started(queue.clone(),base,conversation,target.generation,clock(start),prompt_last.clone(),prompt_check.clone()));
                }
                let transcript = transcript_of(&target,&view);
                let len = std::fs::metadata(&transcript).ok().map(|m|m.len());
                if len != confirm_seen {
                    confirm_seen = len;
                    confirming = true;
                    let job = confirm_inputs(queue.clone(),receipt.clone(),transcript,receipt_conversation(&target,&view),target.generation,clock(start),give_up_now(&target,&view));
                    jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
                }
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                let sample = clock(start);
                if !sid_commit_inflight && sid_retry.as_ref().is_some_and(|(_,at)|sample.monotonic_s>=*at) {
                    let (context,_)=sid_retry.take().unwrap();
                    if sid_transition && context.current(&target,&engine.view(),closed.load(Ordering::Acquire)) {
                        sid_commit_inflight=true;
                        spawn_sid_commit(&mut jobs,engine.lifecycle.clone(),target.clone(),queue.clone(),context);
                    }
                }
                effects.extend(engine.apply(EngineInput::Tick,sample)?);
                if turn_check_due.is_some_and(|due|sample.monotonic_s >= due) {
                    turn_checking = true;
                    let view = engine.view();
                    let job = transcript_turn_ended(queue.clone(),receipt.clone(),transcript_of(&target,&view),receipt_conversation(&target,&view),engine.active_input());
                    let since = last_output;
                    jobs.spawn(async move { Job::TurnCheck { since,result:job.await } });
                }
                for (id,pending) in &mut roots {
                    if pending.result.is_none() && !pending.timed_out && sample.monotonic_s >= pending.deadline {
                        pending.timed_out = true;
                        // Sem escrita começada a entrada não chegou à CLI: volta para a fila em vez de ficar incerta.
                        let prefix = format!("{id}:");
                        let written = native.contains(id) || attempts.values().any(|attempt|attempt.logical_id == *id || attempt.logical_id.starts_with(&prefix));
                        let unsent = !written && matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer | OperationKind::DispatchAsyncAnswer)
                            && !rowless(&target.provider,&pending.command);
                        if crate::warn_limit::allow(Some(&target.key),"deadline") {
                            tracing::warn!(key=%target.key,session=%target.name,operation=%id,unsent,"operação sem resposta em 30 s");
                        }
                        let disposition = if unsent { Disposition::Deferred } else if !written && rowless(&target.provider,&pending.command) { Disposition::Rejected } else { Disposition::Unknown };
                        effects.push_back(Effect::Reply { operation_id:id.clone(),disposition,payload:json!({"error":"operação sem resposta"}) });
                    }
                }
                if let Some(adoption) = &adoption && reconnect.task.is_none() && reconnect.next_at.is_some_and(|at|sample.monotonic_s >= at) {
                    reconnect.next_at = None;
                    reconnect.epoch = transport_epoch;
                    let adoption = adoption.clone();
                    reconnect.task = Some(tokio::spawn(async move { super::codex_terminal::reconnect(&adoption).await }));
                }
                // Subida pedida em curso (Codex) é dona do processo: tentativa e conferência esperam o desfecho dela.
                if cano_lost && respawn.task.is_none() && reconnect.task.is_none() && reconnect.next_at.is_some_and(|at|sample.monotonic_s >= at) {
                    reconnect.next_at = None;
                    reconnect.epoch = transport_epoch;
                    reconnect.task = Some(tokio::spawn(super::cano_reconnect::attempt(target.binding.clone(),target.key.clone(),engine.cano_birth)));
                }
                if cano_lost && respawn.task.is_none() && cano_check_at.is_some_and(|at|sample.monotonic_s >= at) {
                    let (pid,key,birth) = (target.binding.pid,target.key.clone(),engine.cano_birth);
                    let life = tokio::task::spawn_blocking(move||super::cano_reconnect::cano_life(pid,&key,birth)).await;
                    if matches!(life,Ok(super::cano_reconnect::CanoLife::Gone)) { cano_gone = Some(None); }
                    else { cano_check_at = Some(sample.monotonic_s + engine.reconnect_waits.check.as_secs_f64()); }
                }
                if cano_lost && !stall_shown && stall_due.is_some_and(|due|sample.monotonic_s >= due) {
                    stall_shown = true;
                    tracing::warn!(key=%target.key,session=%target.name,"mensagem sem entrega com o cano vivo e o socket fora");
                    let agent = if target.provider == "codex" { "Codex" } else { "Claude" };
                    effects.extend(engine.show_problem(DELIVERY_STALLED,Some(format!("mensagem esperando: o {agent} segue rodando, mas a conexão com ele ainda não voltou"))));
                }
                if respawn.task.is_none() && respawn.next_at.is_some_and(|at|sample.monotonic_s >= at) {
                    respawn.next_at = None;
                    respawn.failures += 1;
                    let kill = std::mem::take(&mut respawn.kill);
                    if let Err(failure) = start_respawn(&mut respawn,&engine,&target,kill,Value::Null,None) {
                        tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,"religação sem como subir o processo");
                    }
                }
            },
        }
    }
    closed.store(true,Ordering::Release);
    reconnect.cancel();
    finish_respawn(&mut respawn,&mut target).await;
    jobs.abort_all(); while jobs.join_next().await.is_some() {}
    io.stop().await;
    Arc::try_unwrap(queue).map_err(|_|failure("queue_busy"))?.shutdown().await.map_err(io_failure)
}

type SavedState = (Arc<QueueActor>,ClockSample,Arc<Mutex<SavedView>>,u64,Value);

fn start_respawn(respawn:&mut Respawn,engine:&RuntimeEngine,target:&RuntimeTarget,kill:bool,patch:Value,saved:Option<SavedState>) -> Result<(),RuntimeError> {
    let (Some(launch),Some(policy)) = (&engine.launch,&engine.policy) else {
        return Err(RuntimeError::new("lifecycle_required","esta sessão não sobe o próprio processo"));
    };
    respawn.swallowed = false;
    respawn.task = Some(tokio::spawn(relaunch(policy.clone(),target.clone(),launch.sidecar_dir.clone(),kill,patch,saved)));
    Ok(())
}

/// Subida em curso termina antes de o ator parar: abortada no meio, deixaria um processo que ninguém
/// conhece. O que ela subiu fica gravado no arquivo da sessão e é o que um `kill` encerra.
async fn finish_respawn(respawn:&mut Respawn,target:&mut RuntimeTarget) {
    if let Some(task) = respawn.task.take() {
        if let Ok(Ok(relaunched)) = task.await { target.binding = relaunched.target.binding; }
    }
}

/// Queda do cano de sessão que o Rust sobe: um timer, com espera dobrando; passado o teto, desiste.
fn after_exit(respawn:&mut Respawn,engine:&mut RuntimeEngine,target:&RuntimeTarget,now:f64,closed:bool) -> Vec<Effect> {
    let Some(launch) = &engine.launch else { return Vec::new() };
    if closed || respawn.next_at.is_some() || respawn.task.is_some() { return Vec::new(); }
    if respawn.failures < RESPAWN_MAX {
        respawn.next_at = Some(now + launch.backoff.as_secs_f64() * f64::from(1u32 << respawn.failures));
        return Vec::new();
    }
    if crate::warn_limit::allow(Some(&target.key),"respawn_gave_up") {
        tracing::warn!(key=%target.key,session=%target.name,code=%respawn.last.as_ref().map_or("",|last|last.code.as_str()),
            "religação desistiu depois de 3 subidas seguidas; só ação da pessoa abre outra rodada");
    }
    // Regra 6: problema mais específico que a última subida deixou fica; a queda genérica vira o do teto.
    if engine.problem().is_none_or(|problem|problem == "headless_caiu") {
        let detail = respawn.last.as_ref().map_or_else(||"o Codex caiu 3 vezes seguidas ao subir".to_owned(),|last|last.message.clone());
        return engine.set_problem("codex_headless_nao_subiu",Some(detail));
    }
    Vec::new()
}

fn cano_of(binding:&CanoBinding) -> super::process::Cano {
    super::process::Cano { pid:binding.pid,escuta:binding.escuta.clone(),token:binding.token.clone(),ts:0.0,versao:binding.versao,extra:Default::default() }
}

/// Grava o que a pessoa trocou, encerra o processo de antes (quando ele já não serve) e sobe outro
/// pela regra 1: cano vivo e da sessão é reaproveitado, nunca dois.
/// O erro leva o que já foi gravado no arquivo: o processo antigo morre ANTES da gravação, então falha
/// no kill ou na gravação deixa tudo no modo de antes, e só a falha da subida deixa o modo novo gravado.
async fn relaunch(policy:PolicyClient,mut target:RuntimeTarget,sidecar_dir:std::path::PathBuf,kill:bool,patch:Value,saved:Option<SavedState>) -> Result<Relaunched,(RuntimeError,Value)> {
    if kill && target.binding.pid != 0 {
        super::process::kill(&cano_of(&target.binding),&target.key,&sidecar_dir).await
            .map_err(|error|(RuntimeError::new(error.code(),"o processo antigo da sessão não encerrou"),Value::Null))?;
    }
    if let Some((queue,sample,gate,version,view)) = saved { save_view(&queue,target.generation,sample,&gate,version,&view,true).await.map_err(|error|(error,Value::Null))?; }
    if !patch.is_null() {
        let written = super::gateway::launch_sidecar(&target,&sidecar_dir,"session.patch_meta",patch.clone()).await.map_err(|error|(error,Value::Null))?;
        if written["updated"] != true { return Err((RuntimeError::new("session_patch_stale","o arquivo da sessão não aceitou o modo novo"),Value::Null)); }
    }
    let spawned = super::gateway::launch_if_needed(&policy,&mut target,&sidecar_dir).await.map_err(|error|(error,patch.clone()))?;
    match super::cano::connect(&target.binding).await {
        Ok(connection)=>Ok(Relaunched { target,spawned:spawned.is_some(),connection,patch }),
        Err(error)=>{
            if let Some(cano) = spawned { super::gateway::discard(&target,&cano,&sidecar_dir).await; }
            Err((error,patch))
        }
    }
}

async fn kill_process(engine:&RuntimeEngine,target:&RuntimeTarget) -> Result<(),RuntimeError> {
    let Some(launch) = &engine.launch else { return Err(RuntimeError::new("close_kill","esta sessão não sobe o próprio processo")) };
    if target.binding.pid == 0 { return Ok(()); }
    super::process::kill(&cano_of(&target.binding),&target.key,&launch.sidecar_dir).await
        .map_err(|error|RuntimeError::new(error.code(),"o processo da sessão não encerrou; arquivo e fila conservados"))
}

/// Prova cada entrada despachada e ainda não confirmada contra o transcript, a partir do cursor do
/// despacho. Uma leitura do transcript por rodada; o estado só é relido depois de uma confirmação.
/// Devolve as operações confirmadas, quantas entradas legadas saíram junto e, no Claude sem terminal, quando
/// a próxima entrada ainda sem prova vence o prazo (a conferência volta nessa hora).
async fn confirm_inputs(queue:Arc<QueueActor>,receipt:Arc<std::sync::Mutex<ReceiptIndex>>,path:std::path::PathBuf,conversation:Option<String>,
    generation:u64,sample:ClockSample,give_up:Option<bool>) -> Result<(Vec<String>,usize,Option<f64>),RuntimeError> {
    let (confirmed,legacy) = prove_inputs(queue.clone(),receipt,path,conversation,generation,sample).await?;
    match give_up {
        None => return Ok((confirmed,legacy,None)),
        // Turno em curso, pedido em aberto ou processo morto: a CLI ainda pode consumir a mensagem; desistir
        // agora poria "não chegou" (e o Descartar) numa entrada que ainda vai rodar. Volta depois.
        Some(false) => return Ok((confirmed,legacy,Some(sample.epoch_s + CONFIRM_RETRY.as_secs_f64()))),
        Some(true) => {},
    }
    let outcome = queue.exec(generation,&format!("give-up:{}",unique()),sample,
        Action::GiveUpUnproved { now:sample.epoch_s,grace:HEADLESS_PROOF_GRACE_S }).await.map_err(io_failure)?;
    if let Some(given) = outcome["given_up"].as_array().filter(|given|!given.is_empty()) {
        tracing::info!(entries=?given,"entrega sem prova no prazo: entrada marcada como não chegou");
    }
    Ok((confirmed,legacy,outcome["next_due"].as_f64()))
}

/// Com a saída do cano perdida, o transcript é relido neste intervalo enquanto houver turno aberto.
const TURN_POLL:Duration = Duration::from_millis(250);

/// O transcript termina no fim de um turno que já consumiu toda entrada entregue: a entrada ativa (`active`)
/// e toda outra despachada estão provadas nele, e nenhuma mensagem da pessoa veio depois do fim. `Some` com o
/// início da linha do fim e o carimbo dela, quando legível.
async fn transcript_turn_ended(queue:Arc<QueueActor>,receipt:Arc<std::sync::Mutex<ReceiptIndex>>,path:std::path::PathBuf,conversation:Option<String>,
    active:Option<String>) -> Result<Option<(u64,Option<f64>)>,RuntimeError> {
    let scanner = receipt.clone();
    let ended = tokio::task::spawn_blocking(move || {
        let mut receipt = scanner.lock().map_err(|_|failure("receipt_panic"))?;
        if let Some(conversation) = conversation { receipt.rebind(&conversation); }
        receipt.scan(&path).map_err(io_failure)?;
        Ok::<_,RuntimeError>(receipt.turn_ended())
    }).await.map_err(|_|failure("receipt_job"))??;
    let Some(end) = ended else { return Ok(None) };
    // Lida depois do transcript: entrada que chegou a ele depois da leitura ainda aparece aqui sem prova.
    let state = queue.snapshot().await.map_err(io_failure)?;
    // A operação pode seguir `accepted` com a linha da fila já confirmada: vale qualquer das duas provas.
    let proved = |id:&str|state.operations.get(id).is_some_and(|op|op.status == Status::Confirmed
        || op.entry_id.as_deref().is_some_and(|entry|state.rows.iter().any(|row|row["id"] == entry && row["confirmed"] == true)));
    let consumed = active.as_deref().is_none_or(proved) && unproved_inputs(&state).is_empty() && super::queue::legacy_rows(&state).is_empty();
    Ok(consumed.then_some(end))
}

/// Entradas despachadas (com cursor) ainda sem a linha delas provada no transcript.
fn unproved_inputs(state:&super::queue::State) -> Vec<(String,String,super::receipt::DispatchCursor)> {
    let confirmed_rows:BTreeSet<&str> = state.rows.iter().filter(|r|r["confirmed"] == true).filter_map(|r|r["id"].as_str()).collect();
    state.operations.iter()
        .filter(|(_,op)|op.status != Status::Confirmed && matches!(op.payload["kind"].as_str(),Some("input" | "steer" | "dispatch_async_answer")))
        .filter_map(|(id,op)|Some((id.clone(),op.entry_id.clone()?,serde_json::from_value(op.dispatch_cursor.clone()).ok()?)))
        .filter(|(_,entry,_)|!confirmed_rows.contains(entry.as_str())).collect()
}

/// Marcador desta vista e se ele muda o arquivo: entrar ou sair da espera, a queda (`dead`) e a saída dela
/// (a vida nova volta a `idle`; sem isto o `dead` ficava gravado para sempre).
fn state_marker<'a>(last:&str,view:&'a Value) -> (&'a str,bool) {
    let state = view["public_state"]["state"].as_str().unwrap_or("");
    let marker = if view["alive"] == false && view["public_state"]["problema"] == "headless_processo_caiu" { "dead" } else { state };
    let watched = |m:&str|matches!(m,"awaiting_input" | "dead");
    (marker,marker != last && (watched(marker) || watched(last)))
}

/// `<config_dir>/.hangar-state/<sid>.json`, o mesmo arquivo do state_hook da conta.
fn write_state_marker(target:&RuntimeTarget,view:&Value,state:&str) {
    write_state_marker_in(target,view,state,&crate::accounts::environment::AccountEnvironment::capture());
}

/// Sem `config_dir` o lançador sobe a CLI sem `CLAUDE_CONFIG_DIR` próprio: ela usa a pasta padrão do ambiente
/// (`CLAUDE_CONFIG_DIR` herdado, senão `~/.claude`), e o marcador vai para a mesma. Sem conversa não há arquivo.
fn write_state_marker_in(target:&RuntimeTarget,view:&Value,state:&str,env:&crate::accounts::environment::AccountEnvironment) {
    let Some(sid) = view["conversation"].as_str().filter(|sid|!sid.is_empty()) else {
        if crate::warn_limit::allow(Some(&target.key),"state_marker_unbound") {
            tracing::warn!(key=%target.key,session=%target.name,state,"marcador de estado não gravado: a sessão ainda não tem conversa; o push de aguardando não sai");
        }
        return;
    };
    let dir = account_base(target,env).join(".hangar-state");
    let tmp = dir.join(format!("{sid}.json.tmp"));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()|std::fs::write(&tmp,json!({"state":state,"ts":clock(Instant::now()).epoch_s}).to_string()))
        .and_then(|()|std::fs::rename(&tmp,dir.join(format!("{sid}.json"))));
    if let Err(error) = written && crate::warn_limit::allow(Some(&target.key),"state_marker_write") {
        tracing::warn!(key=%target.key,session=%target.name,%error,"marcador de estado não gravado");
    }
}

fn account_base(target:&RuntimeTarget,env:&crate::accounts::environment::AccountEnvironment) -> std::path::PathBuf {
    target.metadata["config_dir"].as_str().filter(|dir|!dir.is_empty()).map(std::path::PathBuf::from)
        .unwrap_or_else(||env.claude_base.clone())
}

/// Claude sem terminal: a entrada entregue cujos hooks `UserPromptSubmit` já começaram passa a aceita
/// (`consumed`) antes de a CLI ecoá-la, o que só acontece depois de todos os hooks. Cada marcador aceita uma
/// entrada só, a mais antiga com o texto: duas iguais entregues juntas precisam de dois começos de hook.
/// É só uma antecipação: falhar aqui fica no diário e a prova normal segue.
async fn mark_prompt_started(queue:Arc<QueueActor>,base:std::path::PathBuf,conversation:String,generation:u64,sample:ClockSample,last:Arc<std::sync::Mutex<f64>>,busy:Arc<AtomicBool>) -> Job {
    let result:Result<(),RuntimeError> = async {
        if conversation.is_empty() { return Ok(()); }
        let Some((sha1,ts)) = tokio::task::spawn_blocking(move ||crate::terminal_input::prompt_mark(&base,&conversation)).await.map_err(|_|failure("prompt_mark_panic"))? else { return Ok(()) };
        if ts <= *last.lock().map_err(|_|failure("prompt_mark_panic"))? { return Ok(()); }
        let state = queue.snapshot().await.map_err(io_failure)?;
        let Some(entry) = state.rows.iter().filter(|row|row["delivered"] == true && row["confirmed"] != true && row["consumed"] != true)
            .filter(|row|row["dispatched_ts"].as_f64().is_some_and(|since|since <= ts) && row["text"].as_str().is_some_and(|text|crate::terminal_input::prompt_sha1(text) == sha1))
            .min_by(|a,b|a["dispatched_ts"].as_f64().partial_cmp(&b["dispatched_ts"].as_f64()).unwrap_or(std::cmp::Ordering::Equal))
            .and_then(|row|row["id"].as_str().map(str::to_owned)) else { return Ok(()) };
        queue.exec(generation,&format!("prompt-started:{}",unique()),sample,Action::MarkConsumed { entry_ids:vec![entry] }).await.map_err(io_failure)?;
        *last.lock().map_err(|_|failure("prompt_mark_panic"))? = ts;
        Ok(())
    }.await;
    busy.store(false,Ordering::Release);
    if let Err(failure) = result && crate::warn_limit::allow(None,"prompt_mark") {
        tracing::warn!(code=%failure.code,"aceite antecipado pelo hook não gravado; a entrada segue esperando o eco");
    }
    Job::Saved(Ok(()))
}

/// O processo não sobe mais: o que esperava na fila vira "não chegou" (descartável), em vez de sair horas
/// depois quando alguém reabrir a sessão. Devolve quantas entradas mudaram.
pub async fn abandon_queued(handle:&RuntimeHandle) -> Result<usize,RuntimeError> {
    let rows = handle.queue(format!("abandon-load:{}",unique()),Action::Load).await?;
    let mut count = 0;
    for id in rows.as_array().into_iter().flatten().filter(|row|row["delivered"] == false).filter_map(|row|row["id"].as_str()) {
        handle.queue(format!("abandon:{id}:{}",unique()),Action::Abandon { entry_id:id.into() }).await?;
        count += 1;
    }
    Ok(count)
}

/// Desistência só no Claude sem terminal (`None` nos outros), e só com o processo vivo e parado (`Some(true)`).
fn give_up_now(target:&RuntimeTarget,view:&Value) -> Option<bool> {
    (target.provider == "claude").then(|| view["alive"] == true && view["in_progress"] != true && view["iniciando"] != true
        && view["pending"].as_array().is_none_or(Vec::is_empty) && view["question"].is_null())
}

/// Prazo do Claude sem terminal para a linha da entrada aparecer no transcript (hooks lentos incluídos).
const HEADLESS_PROOF_GRACE_S:f64 = 60.0;

async fn prove_inputs(queue:Arc<QueueActor>,receipt:Arc<std::sync::Mutex<ReceiptIndex>>,path:std::path::PathBuf,conversation:Option<String>,
    generation:u64,sample:ClockSample) -> Result<(Vec<String>,usize),RuntimeError> {
    // Conversa trocada pela TUI: cursores e leitura da anterior não provam entrega na nova.
    if let Some(conversation) = conversation { receipt.lock().map_err(|_|failure("receipt_panic"))?.rebind(&conversation); }
    let mut state = queue.snapshot().await.map_err(io_failure)?;
    let candidates = unproved_inputs(&state);
    let mut confirmed = Vec::new();
    if candidates.is_empty() && super::queue::legacy_rows(&state).is_empty() { return Ok((confirmed,0)); }
    let scanner = receipt.clone(); let transcript = path.clone();
    tokio::task::spawn_blocking(move || scanner.lock().map_err(|_|failure("receipt_panic"))?.scan(&transcript).map(|_|()).map_err(io_failure))
        .await.map_err(|_|failure("receipt_job"))??;
    for (id,entry,cursor) in candidates {
        let Some(row) = state.rows.iter().find(|r|r["id"] == entry.as_str()).cloned() else { continue };
        if row["confirmed"] == true { continue; }
        let receipt = receipt.clone(); let used = state.used_occurrences.clone(); let transcript = path.clone();
        let proof = tokio::task::spawn_blocking(move || {
            let receipt = receipt.lock().map_err(|_|failure("receipt_panic"))?;
            receipt.match_after(&transcript,&cursor,&row,&used).map_err(io_failure)
        }).await.map_err(|_|failure("receipt_job"))??;
        if let Some(proof) = proof {
            let accepted = queue.exec(generation,&format!("proof:{}",unique()),sample,Action::ConfirmOccurrence { id:id.clone(),proof }).await.map_err(io_failure)?;
            if accepted == true { confirmed.push(id); state = queue.snapshot().await.map_err(io_failure)?; }
        }
    }
    // Depois das despachadas: uma linha que prova a entrega nova não pode ser gasta por uma legada.
    let mut legacy = 0;
    for row in super::queue::legacy_rows(&state) {
        let Some(entry_id) = row["id"].as_str().map(str::to_owned) else { continue };
        let receipt = receipt.clone(); let used = state.used_occurrences.clone();
        let found = tokio::task::spawn_blocking(move || receipt.lock().map(|r|r.match_legacy(&row,&used)).map_err(|_|failure("receipt_panic")))
            .await.map_err(|_|failure("receipt_job"))??;
        let Some((occurrence,normalized_text)) = found else { continue };
        // Falha numa legada não desfaz as despachadas já confirmadas acima: fica para a próxima rodada.
        match queue.exec(generation,&format!("legacy-proof:{}",unique()),sample,Action::ConfirmLegacy { entry_id,occurrence,normalized_text }).await {
            Ok(accepted) => if accepted == true { legacy += 1; state = queue.snapshot().await.map_err(io_failure)?; },
            Err(error) => { let _ = io_failure(error); }  // io_failure já registra no log
        }
    }
    Ok((confirmed,legacy))
}

/// O Codex aprende o rollout depois de conectar (thread sem turno não tem arquivo; o sem terminal é
/// registrado antes dela) e, com terminal, segue a thread que a TUI adotar; caminho vazio é ausência, e o
/// recibo não o abre.
fn transcript_of(target:&RuntimeTarget,view:&Value) -> std::path::PathBuf {
    match view["rollout_path"].as_str().filter(|path|!path.is_empty()) {
        Some(path) if target.provider == "codex" => path.into(),
        _ => target.transcript.clone(),
    }
}

/// Codex (thread que só nasce depois do registro, a TUI que adota outra, o `resume` que cai em `start`) e
/// Claude sem terminal (`/clear`, `system/init` com outro sid) trocam de conversa sob o mesmo ator: o recibo
/// segue a conversa da vista, como os cursores novos.
fn receipt_conversation(target:&RuntimeTarget,view:&Value) -> Option<String> {
    matches!(target.provider.as_str(),"claude" | "codex").then(||view["conversation"].as_str().unwrap_or("").to_owned())
}

async fn capture_cursor(target:&RuntimeTarget,view:&Value) -> Result<Value,RuntimeError> {
    let path = transcript_of(target,view); let provider = target.provider.clone(); let conversation = view["conversation"].as_str().unwrap_or("").to_owned();
    tokio::task::spawn_blocking(move ||ReceiptIndex::new(&provider,&conversation).capture(&path))
        .await.map_err(|_|failure("cursor_job"))?.map_err(io_failure).and_then(|cursor|serde_json::to_value(cursor).map_err(|_|failure("cursor_json")))
}

const COSMETIC_POLICIES:[&str;4] = ["format_status","reload_stamp","last_usage","unknown_private"];

/// Serviço puro do ator: roda fora do laço, já que `prepare_prompt` lê imagem e `format_status` lê o `settings.json`.
async fn run_local(kind:String,payload:Value,target:&RuntimeTarget,quota:Option<Value>) -> Result<Value,RuntimeError> {
    let mut meta = target.metadata.clone();
    // Os mesmos campos que o Python juntava ao sidecar antes de rodar o serviço.
    meta["provider"] = json!(target.provider); meta["name"] = json!(target.name); meta["key"] = json!(target.key);
    meta["generation"] = json!(target.generation); meta["jsonl"] = json!(target.transcript.to_string_lossy());
    tokio::task::spawn_blocking(move||local_policy::run(&kind,&payload,&meta,quota.as_ref()).unwrap_or_else(||Err(failure("policy_unavailable"))))
        .await.map_err(|_|failure("policy_job"))?
}

#[derive(Default)]
struct SavedView { version:u64, durable:Option<Value>, latest:Value }

/// Campos que mudam a cada evento e que ninguém relê do disco: o motor parte de `in_progress:false`
/// e o estado público é recalculado. O contador sai da comparação porque a escrita no fio salva a
/// vista inteira (`force`) antes de usar um ID novo.
const VOLATILE_VIEW:[&str;11] = ["public_state","alive","iniciando","in_progress","pending","question","deliverable","runtime_counter","turn_id","connected","subscribed"];

fn durable_part(view:&Value) -> Value {
    let mut durable = view.clone();
    if let Some(fields) = durable.as_object_mut() { for key in VOLATILE_VIEW { fields.remove(key); } }
    durable
}

async fn save_view(queue:&QueueActor,generation:u64,sample:ClockSample,gate:&Mutex<SavedView>,version:u64,view:&Value,force:bool) -> Result<(),RuntimeError> {
    let mut saved = gate.lock().await;
    if version >= saved.version { saved.version = version; saved.latest = view.clone(); }
    else if !force { return Ok(()); }
    // A forçada pode chegar depois de uma mudança de estado mais nova que pulou o disco: grava a vista
    // mais nova conhecida, cujo contador é o maior.
    let latest = saved.latest.clone();
    let durable = durable_part(&latest);
    if force || saved.durable.as_ref() != Some(&durable) {
        queue.exec(generation,&format!("state:{}",unique()),sample,
            Action::AdvanceLifecycle { generation,state:json!({"view":latest}) }).await.map_err(io_failure)?;
        saved.durable = Some(durable);
    }
    Ok(())
}

/// Operação que a fila devolve já final nunca volta a ser enviada; sem resposta guardada no
/// formato de RuntimeReply, a disposição sai do status.
fn current_stored_reply(command:&RuntimeCommand,state:&super::queue::State,from_queue:bool)->Result<Option<RuntimeReply>,RuntimeError> {
    let Some(saved)=state.operations.get(&command.operation_id) else {return Ok(None)};
    if saved.payload!=serde_json::to_value(command).map_err(|_|failure("command_shape"))? {return Err(failure("operation_reused"));}
    let final_reopen=saved.status==Status::Accepted && saved.result["disposition"]=="accepted" && saved.result["payload"]["reopened"]==true;
    let uncertain=matches!(saved.status,Status::Unknown|Status::Dispatching) || !final_reopen && state.operations.values().any(|phase|phase.payload["logical_id"]==command.operation_id && matches!(phase.status,Status::Unknown|Status::Dispatching));
    if uncertain {return Ok(Some(RuntimeReply {operation_id:command.operation_id.clone(),disposition:Disposition::Unknown,payload:json!({"stored":true})}));}
    if from_queue && saved.status==Status::Deferred {return Ok(None);}
    Ok(serde_json::from_value(saved.result.clone()).ok())
}

fn stored_reply(id:&str,prepared:&Value) -> Option<RuntimeReply> {
    let disposition = match prepared["status"].as_str()? {
        "accepted" | "confirmed"=>Disposition::Accepted, "rejected"=>Disposition::Rejected, _=>return None,
    };
    serde_json::from_value(prepared["result"].clone()).ok()
        .or_else(||Some(RuntimeReply { operation_id:id.into(),disposition,payload:Value::Null }))
}

/// Retenção de turnos sem drenagem em curso: recusa se há turno (socket ligado e sessão não entregável);
/// senão reaproveita a retenção viva ou abre outra.
fn turn_hold_reply(view:&Value,turn_hold:&mut Option<std::sync::Weak<()>>,released:&Arc<Notify>) -> Result<TurnHold,RuntimeError> {
    if view["connected"] == true && view["deliverable"] == false {
        return Err(RuntimeError::new(TURN_HOLD_BUSY,"a sessão está trabalhando — espere ela terminar"));
    }
    let token = turn_hold.as_ref().and_then(std::sync::Weak::upgrade).unwrap_or_else(|| {
        let token = Arc::new(());
        *turn_hold = Some(Arc::downgrade(&token));
        token
    });
    Ok(TurnHold { token:Some(token),released:released.clone() })
}

/// Quem pediu o drain espera quando há o que sair: fila comum entregável, resposta reservada apta ou drain em curso.
fn drain_has_work(deliverable:bool,reserved_ready:bool,active:bool) -> bool { deliverable || reserved_ready || active }

/// Entrada que saiu no drain. A reservada despachada conta também quando o desfecho é incerto: ela foi ao
/// fio e nunca volta, então dizer `sent:0` esconderia uma entrega.
fn drained_count(reply:&RuntimeReply) -> usize {
    let dispatched_reservation = reply.disposition == Disposition::Unknown && reply.operation_id.starts_with("async-answer:");
    usize::from(reply.disposition == Disposition::Accepted || dispatched_reservation)
}

fn status(disposition:Disposition) -> Status {
    match disposition { Disposition::Accepted=>Status::Accepted,Disposition::Deferred=>Status::Deferred,
        Disposition::Rejected=>Status::Rejected,Disposition::Unknown=>Status::Unknown }
}

/// Alguma tentativa pode ter chegado ao fio? Não conta a que a recuperação marcou como não escrita nem a
/// de fase recusada ou devolvida sem escrita: contá-las deixaria incerta, para sempre, uma resposta que nunca saiu.
fn maybe_written(state:&super::queue::State,op:&super::queue::Operation) -> bool {
    op.wire_attempts.iter().any(|(wire,attempt)|attempt["status"] != "not_written"
        && state.operations.get(wire).is_none_or(|phase|!matches!(phase.status,Status::Rejected | Status::Deferred)))
}

fn recover_phase(state:&super::queue::State,phase:&super::queue::Operation) -> bool {
    phase.payload["generation"].as_u64() == Some(state.generation)
        && phase.payload["frame"].is_object() && (matches!(phase.status,Status::Unknown | Status::Dispatching)
        || phase.payload["logical_id"].as_str().and_then(|id|state.operations.get(id))
            .is_some_and(|root|matches!(root.status,Status::Unknown | Status::Dispatching)))
}

/// Falha antes de qualquer escrita no fio: nada chegou à CLI, então a entrada volta para a fila
/// (adiada, como o `deferred` do Python) em vez de ficar incerta e presa para sempre.
/// Orientação com texto do Codex vai direto ao turno (`turn/steer`), sem linha na fila, como no Python: recusada
/// ou sem turno, ela é erro de quem orientou e nunca fica na fila esperando um turno que não vem.
fn rowless(provider:&str,command:&RuntimeCommand) -> bool {
    provider == "codex" && command.kind == OperationKind::Steer && command.payload["entry_id"].as_str().is_none()
}

fn defer_unwritten(provider:&str,roots:&mut BTreeMap<String,Pending>,attempts:&BTreeMap<String,Attempt>,native:&BTreeSet<String>,id:&str,error:RuntimeError,effects:&mut VecDeque<Effect>) {
    let prefix = format!("{id}:");
    let written = native.contains(id) || attempts.values().any(|attempt|attempt.logical_id == id || attempt.logical_id.starts_with(&prefix));
    let input = roots.get(id).is_some_and(|pending|matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer | OperationKind::DispatchAsyncAnswer)
        && !rowless(provider,&pending.command));
    if written || !input { return fail_root(roots,id,error); }
    let Some(pending) = roots.get_mut(id) else { return };
    if crate::warn_limit::allow(Some(id),&error.code) {
        tracing::warn!(operation=%id,code=%error.code,reason=%error.message,"entrada adiada antes de qualquer escrita");
    }
    pending.preparing = false;
    // Adiada não é erro: quem espera (API ou drain) vê a entrada de volta na fila, e o drain não põe
    // a sessão inteira em erro por isso.
    let payload = json!({"error_code":error.code});
    let reply = RuntimeReply { operation_id:id.into(),disposition:Disposition::Deferred,payload:payload.clone() };
    for response in pending.responses.drain(..) { let _ = response.send(Ok(reply.clone())); }
    pending.result = Some(reply);
    effects.push_back(Effect::Reply { operation_id:id.into(),disposition:Disposition::Deferred,payload });
}

fn fail_root(roots:&mut BTreeMap<String,Pending>,id:&str,error:RuntimeError) {
    if let Some(pending) = roots.get_mut(id) {
        pending.error = Some(error.clone()); pending.preparing = false;
        for response in pending.responses.drain(..) { let _ = response.send(Err(error.clone())); }
    }
}

/// O que o ator escreve no canal do hub; só envia quando muda.
struct LiveOut { sender:Option<LiveSender>,state:LiveState }

impl LiveOut {
    fn send(&self) { if let Some(sender) = &self.sender { sender.send_replace(Some(Arc::new(self.state.clone()))); } }
    fn update(&mut self,change:impl FnOnce(&mut LiveState)) {
        if self.sender.is_none() { return; }
        let before = self.state.clone();
        change(&mut self.state);
        if self.state != before { self.send(); }
    }
    /// `false`: sem canal, quem chamou publica no `events`.
    fn channel(&mut self,channel:&str,data:&Value) -> bool {
        if self.sender.is_none() { return false; }
        let text = data["text"].as_str().unwrap_or("").to_owned();
        self.update(|state| match channel { "preview"=>state.preview = text, "thinking"=>state.thinking = text, _=>state.tool = text });
        true
    }
    fn error(&mut self,error:&Option<RuntimeError>) {
        let same = match (&self.state.error,error) { (None,None)=>true, (Some((code,message)),Some(e))=>*code == e.code && *message == e.message, _=>false };
        if !same { self.update(|state|state.error = error.as_ref().map(|e|(e.code.clone(),e.message.clone()))); }
    }
}

struct Revision { value:u64,counter:Arc<AtomicU64> }

fn publish(events:&broadcast::Sender<RuntimeEvent>,target:&RuntimeTarget,revision:&mut Revision,channel:&str,data:Value) {
    revision.value += 1;
    revision.counter.store(revision.value,Ordering::Release);
    let _ = events.send(RuntimeEvent { key:target.key.clone(),generation:target.generation,revision:revision.value,channel:channel.into(),data });
}

type ModsWaiters = BTreeMap<u64,oneshot::Sender<Result<Value,ModsError>>>;

/// Pedido de app que chegou à vez. Já vencido (o `timeout` do app venceu com a mensagem na caixa), não
/// roda: virar clique depois que o app mostrou erro seria um clique fantasma. Quem pediu e já desistiu
/// (a rota cortou a chamada no orçamento dela, antes deste prazo) também não.
fn take_mods(engine:&mut RuntimeEngine,waiters:&mut ModsWaiters,token:&mut u64,call:ModsCall,deadline:Instant,
    response:oneshot::Sender<Result<Value,ModsError>>,clock:ClockSample) -> Vec<Effect> {
    if response.is_closed() { return Vec::new(); }
    let now = Instant::now();
    if now >= deadline { let _ = response.send(Err(crate::mods::model::no_answer())); return Vec::new(); }
    *token += 1;
    match engine.mods_call(*token,call,deadline.duration_since(now).as_secs_f64(),clock) {
        Ok(next) => { waiters.insert(*token,response); next }
        Err(error) => { let _ = response.send(Err(error)); Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_target()->RuntimeTarget {
        RuntimeTarget {key:"key".into(),generation:4,name:"s".into(),provider:"claude".into(),metadata:json!({"session_id":"a","lifecycle_pending":false}),
            binding:CanoBinding {pid:42,escuta:"fixture".into(),token:"fixture".into(),versao:2},lease_path:"lease".into(),state_path:"state".into(),projection_dir:"projection".into(),transcript:"a.jsonl".into(),created:1.0}
    }
    #[tokio::test]
    async fn headless_codex_registered_before_its_thread_confirms_the_input_in_the_rollout() {
        // Registrado antes da thread: o alvo não tem transcript e o recibo nasceu sem conversa; o rollout só
        // aparece com o primeiro turno.
        let dir=tempfile::tempdir().unwrap();let rollout=dir.path().join("rollout.jsonl");
        let mut target=commit_target();target.provider="codex".into();target.generation=5;target.transcript="".into();
        let view=json!({"conversation":"thread-1","headless":true,"rollout_path":rollout});
        let store=super::super::queue::Store::open(&dir.path().join("state"),dir.path(),super::super::queue::State::new("key",5,"s",vec![])).unwrap();
        let queue=Arc::new(QueueActor::start(store,super::super::queue::acquire_lease(&dir.path().join("lease")).unwrap()));
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1.0};
        let receipt=Arc::new(std::sync::Mutex::new(ReceiptIndex::new("codex","")));
        queue.exec(5,"append",clock,Action::Append {text:"Olá".into(),delivered:true,ts:None,pre_transcript:false,entry_id:Some("msg".into())}).await.unwrap();
        queue.exec(5,"prepare",clock,Action::Prepare {id:"msg".into(),payload:json!({"kind":"input"}),entry_id:Some("msg".into())}).await.unwrap();
        let cursor=capture_cursor(&target,&view).await.unwrap();
        queue.exec(5,"cursor",clock,Action::BindDispatch {id:"msg".into(),cursor}).await.unwrap();
        let now=chrono::Utc::now().to_rfc3339();
        std::fs::write(&rollout,format!("{}\n{}\n",json!({"timestamp":now,"type":"session_meta","payload":{"id":"thread-1"}}),
            json!({"timestamp":now,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Olá"}]}}))).unwrap();
        let (confirmed,_,_)=confirm_inputs(queue.clone(),receipt,transcript_of(&target,&view),receipt_conversation(&target,&view),5,clock,None).await.unwrap();
        assert_eq!(confirmed,vec!["msg".to_owned()],"a fala no rollout da thread não confirmou a entrada");
        Arc::try_unwrap(queue).ok().unwrap().shutdown().await.unwrap();
    }
    #[test]
    fn metadata_commit_after_a_server_restart_with_the_same_pid_does_not_reuse_a_journaled_id() {
        let dir=tempfile::tempdir().unwrap();let state=dir.path().join("state");
        let commit=|boot:&str,session:&str| {
            let mut store=super::super::queue::Store::open(&state,dir.path(),super::super::queue::State::new("key",2,"s",vec![])).unwrap();
            store.exec(2,&format!("claude-metadata:{boot}:0"),ClockSample {monotonic_s:0.0,epoch_s:1.0},
                Action::AdvanceLifecycle {generation:2,state:json!({"metadata":{"key":"key","session_id":session}})}).map(|_|())
        };
        commit(&boot_id(4242,1_000),"a").unwrap();
        commit(&boot_id(4242,2_000),"b").expect("servidor novo com o mesmo pid e o contador zerado não pode repetir o id");
    }
    #[test]
    fn sdk_sid_b_blocks_a_rows_then_new_ids_and_binding_use_generation_b() {
        let dir=tempfile::tempdir().unwrap();let mut target=commit_target();
        let interactions=Arc::new(crate::interactions::InteractionStore::default());
        let mut engine=RuntimeEngine::new("claude",json!({"name":"s","session_id":"a","initialized":true}),4,ClockSample {monotonic_s:0.0,epoch_s:1.0}).unwrap().with_interactions("key".into(),interactions.clone()).unwrap();
        let effects=engine.apply(EngineInput::Line(json!({"type":"system","subtype":"init","session_id":"b"})),ClockSample {monotonic_s:1.0,epoch_s:2.0}).unwrap();
        assert!(effects.iter().any(|effect|matches!(effect,Effect::PersistClaude {patch,..} if patch["session_id"]=="b")));
        let mut initial=super::super::queue::State::new("key",4,"s",vec![json!({"id":"a-row","text":"a-message","delivered":false,"ts":1.0})]);initial.runtime_state["view"]=json!({"conversation":"a"});
        let mut store=super::super::queue::Store::open(&dir.path().join("state"),dir.path(),initial).unwrap();
        assert!(block_claude_sid(&mut target,&engine.view()));assert!(!claude_delivery_allowed(&target,&engine.view(),false));assert_eq!(store.state().rows.len(),1);
        store.exec(4,"sdk-sid-commit",ClockSample {monotonic_s:1.0,epoch_s:2.0},Action::AdvanceLifecycle {generation:5,state:json!({"conversation_reset":{"previous":"a","next":"b"},"metadata":{"key":"key","session_id":"b"}})}).unwrap();
        engine.advance_claude_generation(5).unwrap();target.generation=5;target.metadata["session_id"]=json!("b");target.metadata["lifecycle_pending"]=json!(false);
        assert!(store.state().rows.is_empty());
        let output=engine.initialize("b-init".into()).unwrap();
        assert!(output.iter().any(|effect|matches!(effect,Effect::Write {frame,..} if frame["request_id"].as_str().is_some_and(|id|id.starts_with("hangar:5:")))));
        assert!(store.exec(4,"late-a-claim",ClockSample {monotonic_s:1.0,epoch_s:2.0},Action::Claim {min_ts:0.0,limit:None,entry_id:None}).is_err());
    }
    #[test]
    fn dead_marker_is_rewritten_when_the_new_life_comes_back() {
        let crashed = json!({"alive":false,"public_state":{"state":"idle","problema":"headless_processo_caiu"}});
        let back = json!({"alive":true,"public_state":{"state":"idle"}});
        assert_eq!(state_marker("idle",&crashed),("dead",true));
        assert_eq!(state_marker("dead",&back),("idle",true),"o `dead` ficava gravado depois da subida");
        assert_eq!(state_marker("idle",&back),("idle",false));
        assert_eq!(state_marker("working",&json!({"alive":true,"public_state":{"state":"awaiting_input"}})),("awaiting_input",true));
    }
    #[test]
    fn marker_without_config_dir_goes_to_the_default_folder_the_cli_uses() {
        use crate::accounts::environment::AccountEnvironment;
        let home=tempfile::tempdir().unwrap();
        let env=AccountEnvironment::from_map([("HOME".to_owned(),home.path().to_string_lossy().into_owned())].into());
        let mut target=commit_target(); target.metadata["config_dir"]=Value::Null;
        write_state_marker_in(&target,&json!({"conversation":"sid-default"}),"awaiting_input",&env);
        let marker:Value=serde_json::from_slice(&std::fs::read(home.path().join(".claude/.hangar-state/sid-default.json")).unwrap()).unwrap();
        assert_eq!(marker["state"],"awaiting_input");
        // Pasta padrão vinda do ambiente (`CLAUDE_CONFIG_DIR` herdado) vence o `~/.claude`.
        let other=home.path().join("conta");
        let env=AccountEnvironment::from_map([("HOME".to_owned(),home.path().to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".to_owned(),other.to_string_lossy().into_owned())].into());
        write_state_marker_in(&target,&json!({"conversation":"sid-env"}),"awaiting_input",&env);
        assert!(other.join(".hangar-state/sid-env.json").exists());
        // Sem conversa não há arquivo.
        write_state_marker_in(&target,&json!({"conversation":""}),"awaiting_input",&env);
        assert_eq!(std::fs::read_dir(other.join(".hangar-state")).unwrap().count(),1);
    }
    #[tokio::test]
    async fn headless_claude_receipt_follows_the_conversation_after_a_sid_change() {
        // Recibo nasceu na conversa "a"; depois da troca para "b" (o `/clear` do chat), o envio
        // seguinte captura o cursor em "b" e a linha dele em "b" precisa confirmar.
        let dir=tempfile::tempdir().unwrap();let transcript=dir.path().join("b.jsonl");
        std::fs::write(&transcript,"").unwrap();
        let mut target=commit_target();target.generation=5;target.metadata["session_id"]=json!("b");target.transcript=transcript.clone();
        let view=json!({"conversation":"b","headless":true});
        let store=super::super::queue::Store::open(&dir.path().join("state"),dir.path(),super::super::queue::State::new("key",5,"s",vec![])).unwrap();
        let queue=Arc::new(QueueActor::start(store,super::super::queue::acquire_lease(&dir.path().join("lease")).unwrap()));
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1.0};
        let receipt=Arc::new(std::sync::Mutex::new(ReceiptIndex::new("claude","a")));
        queue.exec(5,"append",clock,Action::Append {text:"Olá".into(),delivered:true,ts:None,pre_transcript:false,entry_id:Some("msg".into())}).await.unwrap();
        queue.exec(5,"prepare",clock,Action::Prepare {id:"msg".into(),payload:json!({"kind":"input"}),entry_id:Some("msg".into())}).await.unwrap();
        let cursor=capture_cursor(&target,&view).await.unwrap();
        queue.exec(5,"cursor",clock,Action::BindDispatch {id:"msg".into(),cursor}).await.unwrap();
        std::fs::write(&transcript,format!("{}\n",json!({"type":"user","uuid":"u-1","sessionId":"b","message":{"role":"user","content":"Olá"}}))).unwrap();
        let (confirmed,_,_)=confirm_inputs(queue.clone(),receipt,transcript_of(&target,&view),receipt_conversation(&target,&view),5,clock,None).await.unwrap();
        assert_eq!(confirmed,vec!["msg".to_owned()],"a linha da conversa nova não confirmou a entrada");
        Arc::try_unwrap(queue).ok().unwrap().shutdown().await.unwrap();
    }
    #[test]
    fn sid_b_blocks_claim_before_async_metadata_commit() {
        let mut target=commit_target();let view=json!({"conversation":"b","deliverable":true});
        assert!(block_claude_sid(&mut target,&view));
        assert_eq!(target.metadata["lifecycle_pending"],true);assert_eq!(target.generation,4);
        assert!(!claude_delivery_allowed(&target,&view,false));
    }
    #[tokio::test]
    async fn late_bind_dispatch_a_is_retired_without_an_order_gap_or_error_in_b() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");
        let mut state=super::super::queue::State::new("key",4,"s",vec![]);state.runtime_state["view"]=json!({"conversation":"a"});
        let store=super::super::queue::Store::open(&path,dir.path(),state).unwrap();
        let queue=Arc::new(QueueActor::start(store,super::super::queue::acquire_lease(&dir.path().join("lease")).unwrap()));
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1.0};
        queue.exec(4,"prepare-a",clock,Action::Prepare {id:"wire-a".into(),payload:json!({"kind":"input"}),entry_id:None}).await.unwrap();
        queue.exec(4,"unknown-a",clock,Action::Finish {id:"wire-a".into(),status:Status::Unknown,result:json!({})}).await.unwrap();
        let (release,wait)=oneshot::channel();let old_queue=queue.clone();
        let old=tokio::spawn(async move {wait.await.unwrap();Job::Write {wire:"wire-a".into(),generation:4,result:old_queue.exec(4,"late-cursor-a",clock,Action::BindDispatch {id:"wire-a".into(),cursor:json!({})}).await.map(|_|()).map_err(io_failure)}});
        queue.exec(4,"sid-b",clock,Action::AdvanceLifecycle {generation:5,state:json!({"conversation_reset":{"previous":"a","next":"b"},"metadata":{"key":"key","session_id":"b"}})}).await.unwrap();
        let attempts=BTreeMap::from([("wire-a".into(),Attempt {logical_id:"a".into(),phase_id:"wire-a".into(),frame:json!({"message":"a"}),order:1,epoch:0,generation:4,volatile:false}),
            ("wire-b".into(),Attempt {logical_id:"b".into(),phase_id:"wire-b".into(),frame:json!({"message":"b"}),order:2,epoch:0,generation:5,volatile:false})]);
        let mut ready=BTreeMap::from([(2,("wire-b".into(),5,Ok(())))]);let mut next=1;
        assert!(next_current_write(&mut ready,&mut next,&attempts,5).is_none());
        release.send(()).unwrap();let Job::Write {wire,generation,result}=old.await.unwrap() else {panic!("resultado de escrita")};assert!(result.is_err());
        ready.insert(1,(wire,generation,result));let (attempt,result)=next_current_write(&mut ready,&mut next,&attempts,5).unwrap();
        assert_eq!(attempt.logical_id,"b");assert!(result.is_ok());assert_eq!(next,3);assert!(ready.is_empty());
        assert_eq!(queue.snapshot().await.unwrap().operations["wire-a"].status,Status::Unknown);
        Arc::try_unwrap(queue).ok().unwrap().shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn repeat_dispatching_keeps_both_live_senders_until_one_durable_completion() {
        let dir=tempfile::tempdir().unwrap();let command=RuntimeCommand {operation_id:"live".into(),kind:OperationKind::ListModels,payload:json!({})};
        let mut store=super::super::queue::Store::open(&dir.path().join("state"),dir.path(),super::super::queue::State::new("key",4,"s",vec![])).unwrap();let clock=ClockSample {monotonic_s:0.0,epoch_s:1.0};
        store.exec(4,"prepare",clock,Action::Prepare {id:"live".into(),payload:serde_json::to_value(&command).unwrap(),entry_id:None}).unwrap();
        store.exec(4,"write-once",clock,Action::BeginDispatch {id:"live".into(),wire_id:"one-write".into(),staged:false}).unwrap();
        let (first,mut receive_first)=oneshot::channel();let (second,mut receive_second)=oneshot::channel();
        let mut pending=Pending::stored(command.clone(),RuntimeReply {operation_id:"live".into(),disposition:Disposition::Unknown,payload:Value::Null});pending.result=None;pending.responses.push(first);
        assert!(join_active_pending(&mut pending,&command,second).is_ok());
        assert_eq!(pending.responses.len(),2);assert!(matches!(receive_first.try_recv(),Err(oneshot::error::TryRecvError::Empty)));assert!(matches!(receive_second.try_recv(),Err(oneshot::error::TryRecvError::Empty)));
        let answer=RuntimeReply {operation_id:"live".into(),disposition:Disposition::Accepted,payload:json!({"models":[]})};
        store.exec(4,"finish",clock,Action::Finish {id:"live".into(),status:Status::Accepted,result:serde_json::to_value(&answer).unwrap()}).unwrap();
        for sender in pending.responses.drain(..) {assert!(sender.send(Ok(answer.clone())).is_ok());}
        assert!(receive_first.await.unwrap().unwrap()==answer);assert!(receive_second.await.unwrap().unwrap()==answer);
        assert_eq!(store.state().operations["live"].wire_attempts.len(),1);
        // Operação final gravada guarda só os metadados da resposta (`queue::slim`), não a lista de modelos.
        assert!(current_stored_reply(&command,store.state(),false).unwrap()==Some(RuntimeReply {payload:Value::Null,..answer}));
    }
    #[test]
    fn durable_reopen_result_overrides_initial_rejection_without_replaying_unknown() {
        let dir=tempfile::tempdir().unwrap();let command=RuntimeCommand {operation_id:"mode".into(),kind:OperationKind::SetPermissionMode,payload:json!({"mode":"bypassPermissions"})};
        let mut store=super::super::queue::Store::open(&dir.path().join("state"),dir.path(),super::super::queue::State::new("key",5,"s",vec![])).unwrap();
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1.0};
        store.exec(5,"prepare",clock,Action::Prepare {id:"mode".into(),payload:serde_json::to_value(&command).unwrap(),entry_id:None}).unwrap();
        let rejected=RuntimeReply {operation_id:"mode".into(),disposition:Disposition::Rejected,payload:json!({"error":"boot-rejected"})};
        store.exec(5,"rejected",clock,Action::Finish {id:"mode".into(),status:Status::Rejected,result:serde_json::to_value(rejected).unwrap()}).unwrap();
        let initial=store.state().clone();
        let accepted=RuntimeReply {operation_id:"mode".into(),disposition:Disposition::Accepted,payload:json!({"reopened":true,"mode":"bypassPermissions"})};
        store.exec(5,"finished-reopen",clock,Action::Finish {id:"mode".into(),status:Status::Accepted,result:serde_json::to_value(&accepted).unwrap()}).unwrap();
        assert_eq!(current_stored_reply(&command,&initial,false).unwrap().unwrap().disposition,Disposition::Rejected);
        assert_eq!(current_stored_reply(&command,store.state(),false).unwrap(),Some(accepted.clone()));
        let changed=RuntimeCommand {payload:json!({"mode":"manual"}),..command.clone()};assert_eq!(current_stored_reply(&changed,store.state(),false).unwrap_err().code,"operation_reused");
        // A fila não rebaixa resultado final a incerto: o desfecho tardio não reabre a resposta gravada.
        store.exec(5,"uncertain",clock,Action::Finish {id:"mode".into(),status:Status::Unknown,result:json!({})}).unwrap();
        assert_eq!(current_stored_reply(&command,store.state(),false).unwrap(),Some(accepted));
    }
    #[test]
    fn ready_requires_same_operation_life_digest_and_successful_persistence() {
        let mut target=commit_target();target.metadata["lifecycle_pending"]=json!(true);
        let proof=ModeProof {binding:crate::interactions::InteractionBinding {key:"key".into(),conversation:"a".into(),generation:4},operation_id:"set-mode".into(),mode:"bypassPermissions".into()};
        let view=json!({"conversation":"a","permission_mode":"bypassPermissions"});
        let receipt=ModeCommit {proof:proof.clone(),digest:"saved".into()};
        assert!(mode_ready_proof(&target,&view,None,&proof,"saved",None).is_err());
        assert!(mode_ready_proof(&target,&view,Some(&receipt),&proof,"saved",Some(&failure("lifecycle_sidecar_write"))).is_err());
        assert!(mode_ready_proof(&target,&view,Some(&receipt),&proof,"changed",None).is_err());
        let mut wrong=proof.clone();wrong.operation_id="other".into();assert!(mode_ready_proof(&target,&view,Some(&receipt),&wrong,"saved",None).is_err());
        wrong=proof.clone();wrong.binding.generation=5;assert!(mode_ready_proof(&target,&view,Some(&receipt),&wrong,"saved",None).is_err());
        mode_ready_proof(&target,&view,Some(&receipt),&proof,"saved",None).unwrap();
        assert_eq!(target.metadata["lifecycle_pending"],true,"validar não libera a fila antes da gravação de Ready");
    }
    #[test]
    fn queued_interaction_guard_reads_the_live_owner_after_preparation() {
        use crate::interactions::{InteractionBinding,InteractionKind,InteractionStore,native_target};
        let clock = ClockSample { monotonic_s:1.0,epoch_s:1.0 };
        let store = Arc::new(InteractionStore::default());
        let mut engine = RuntimeEngine::new("claude",json!({"name":"s","session_id":"sid","initialized":true}),1,clock).unwrap()
            .with_interactions("key".into(),store.clone()).unwrap();
        let request = json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"A"}});
        engine.apply(EngineInput::Line(json!({"type":"control_request","request_id":0,"request":request})),clock).unwrap();
        let binding = InteractionBinding { key:"key".into(),conversation:"sid".into(),generation:1 };
        let target = native_target(InteractionKind::Approval,"sid",&json!(0));
        let token = store.observe(&binding,&target,Some(request)).unwrap().unwrap();
        engine.command(RuntimeCommand { operation_id:"choose".into(),kind:OperationKind::Select,payload:json!({"option":1,"interaction_id":token}) },clock).unwrap();
        let guard = engine.interaction_guard("choose").unwrap().unwrap();
        assert!(guard(futures_util::task::noop_waker_ref()).is_ok());
        // O Store/poll ainda mostram A; a pendência do dono já virou B enquanto o frame espera.
        engine.apply(EngineInput::Line(json!({"type":"control_request","request_id":0,"request":{
            "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"B"}}})),clock).unwrap();
        assert_eq!(guard(futures_util::task::noop_waker_ref()).unwrap_err(),"erro_interacao_desatualizada");
        assert!(store.expected(&binding,Some(&token)).is_ok());
    }

    #[test]
    fn reserved_answer_guard_reads_the_real_question_right_before_the_bytes() {
        use crate::interactions::{InteractionBinding,InteractionKind,InteractionStore,native_target};
        const QUESTION:&str = "async:thread-1:q1:0";
        let clock = ClockSample { monotonic_s:1.0,epoch_s:1.0 };
        let store = Arc::new(InteractionStore::default());
        let mut engine = RuntimeEngine::new("codex",json!({"name":"s","thread_id":"thread-1","headless":false,"cwd":"/work"}),1,clock).unwrap()
            .with_interactions("key".into(),store.clone()).unwrap();
        let write = |effects:&[Effect],method:&str| effects.iter().find_map(|e|match e { Effect::Write { frame,.. } if frame["method"] == method=>Some(frame.clone()),_=>None }).unwrap();
        let initialize = write(&engine.initialize("boot".into()).unwrap(),"initialize");
        let resume = write(&engine.apply(EngineInput::Line(json!({"id":initialize["id"],"result":{}})),clock).unwrap(),"thread/resume");
        engine.apply(EngineInput::Line(json!({"id":resume["id"],"result":{"thread":{"id":"thread-1","path":"/r/one.jsonl","status":{"type":"idle"},
            "turns":[{"id":"t0","status":"completed","items":[{"id":"q1","type":"agentMessage","delivery":"async","questions":[{"title":"Qual pasta?","options":["A","B"]}]}]}]}}})),clock).unwrap();
        let question = engine.view()["async_questions"][0][1].clone();
        let binding = InteractionBinding { key:"key".into(),conversation:"thread-1".into(),generation:1 };
        let token = store.observe(&binding,&native_target(InteractionKind::Question,"thread-1",&json!(QUESTION)),Some(question)).unwrap().unwrap();
        let reserved = engine.command(RuntimeCommand { operation_id:"answer".into(),kind:OperationKind::AnswerQuestions,
            payload:json!({"request_id":QUESTION,"interaction_id":token,"answers":[{"question_id":"answer","kind":"option","indices":[0]}]}) },clock).unwrap();
        let (reservation,command) = reserved.iter().find_map(|e|match e { Effect::ReserveAsyncAnswer { reservation,command,.. }=>Some((reservation.clone(),command.clone())),_=>None }).unwrap();
        engine.settle_async_answer(&reservation.entry_id,true);
        write(&engine.command(command,clock).unwrap(),"turn/start");
        let guard = engine.interaction_guard(&reservation.operation_id).unwrap().expect("a reservada leva guarda até o escritor");
        assert!(guard(futures_util::task::noop_waker_ref()).is_ok());
        // A TUI troca de conversa depois da preparação e antes dos bytes: a pergunta real deixou de estar aberta.
        engine.apply(EngineInput::Line(json!({"method":"thread/started","params":{"thread":{"id":"thread-2","path":"/r/two.jsonl","cwd":"/work","source":"cli"}}})),clock).unwrap();
        assert_eq!(guard(futures_util::task::noop_waker_ref()).unwrap_err(),"erro_interacao_desatualizada");
    }

    #[test]
    fn drain_waits_for_an_eligible_reservation_and_counts_a_dispatched_one() {
        assert!(drain_has_work(false,true,false),"reserva apta com a fila comum barrada");
        assert!(!drain_has_work(false,false,false));
        let reply = |id:&str,disposition| RuntimeReply { operation_id:id.into(),disposition,payload:Value::Null };
        assert_eq!(drained_count(&reply("async-answer:1:3",Disposition::Unknown)),1,"reservada despachada nunca vira sent:0");
        assert_eq!(drained_count(&reply("async-answer:1:3",Disposition::Accepted)),1);
        assert_eq!(drained_count(&reply("entry",Disposition::Unknown)),0);
        assert_eq!(drained_count(&reply("async-answer:1:3",Disposition::Deferred)),0);
    }

    #[tokio::test]
    async fn mods_call_to_an_actor_that_never_answers_gives_up_with_a_code() {
        // Ator vivo que não lê a caixa: o pedido do app não pode pendurar o registro da sessão.
        let (sender,_inbox) = mpsc::channel(1);
        let handle = RuntimeHandle { sender,task:Arc::new(Mutex::new(None)),closed:Arc::new(AtomicBool::new(false)),
            events:broadcast::channel(1).0,stopped:Arc::new(Mutex::new(None)),key:"key".into(),generation:1 };
        let call = || ModsCall::Show { site:"p".into() };
        let soon = || Instant::now() + Duration::from_millis(50);
        let first = tokio::time::timeout(Duration::from_secs(2),handle.mods(call(),soon())).await.unwrap();
        assert_eq!(first.unwrap_err().code,"erro_mod_clique_sem_resposta");
        // A caixa cheia (o primeiro pedido ficou nela) também cai no prazo, agora no envio.
        let second = tokio::time::timeout(Duration::from_secs(2),handle.mods(call(),soon())).await.unwrap();
        assert_eq!(second.unwrap_err().code,"erro_mod_clique_sem_resposta");
        assert_eq!(MODS_CALL_LIMIT,Duration::from_secs(7));
    }

    #[test]
    fn an_actor_panic_marks_the_live_channel_and_a_finished_run_does_not() {
        let (live,rx) = tokio::sync::watch::channel(Some(Arc::new(LiveState { public_state:json!({"state":"working"}),..Default::default() })));
        // Saída que chegou ao fim: desarmada, o valor fica como o ator deixou.
        let mut done = LivePanicMark(Some(live.clone()));
        let _ = done.0.take();
        drop(done);
        assert!(rx.borrow().as_ref().unwrap().error.is_none());
        let armed = LivePanicMark(Some(live));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || { let _armed = armed; panic!("pânico simulado do ator"); }));
        assert!(panicked.is_err());
        let state = rx.borrow().clone().unwrap();
        assert_eq!(state.error.as_ref().map(|(code,_)|code.as_str()),Some("runtime_panic"));
        assert_eq!(state.public_state["state"],"working","o problema vai sobre o último estado");
    }

    #[test]
    fn an_actor_panic_clears_the_band_and_a_normal_exit_does_not() {
        let mods = crate::mods::state::Mods::default();
        let (sender,_inbox) = mpsc::channel(1);
        let link = RuntimeHandle { sender,task:Arc::new(Mutex::new(None)),closed:Arc::new(AtomicBool::new(false)),
            events:broadcast::channel(1).0,stopped:Arc::new(Mutex::new(None)),key:"key".into(),generation:1 };
        mods.attach("session",1,Arc::new(link));
        let band = json!({"above":{"type":"Button","key":"k"},"panes":[],"shown_id":null,"columns":110,"source":"surface"});
        let ui = |mods:&crate::mods::state::Mods| mods.replay("session").into_iter().find(|(event,_)|*event == "plugin_ui")
            .map(|(_,data)|serde_json::from_str::<Value>(&data).unwrap()).unwrap();
        // Saída normal: desarmada, a faixa fica para o `close` limpar.
        mods.publish_ui("session",1,band.clone());
        let mut guard = ClearOnDrop { mods:Some(mods.clone()),name:"session".into(),life:1 };
        guard.mods = None;
        drop(guard);
        assert_eq!(ui(&mods)["above"]["type"],"Button");
        // Pânico no meio do ator: a guarda solta no desenrolar publica a interface vazia.
        let armed = mods.clone();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ClearOnDrop { mods:Some(armed),name:"session".into(),life:1 };
            panic!("pânico simulado do ator");
        }));
        assert!(panicked.is_err());
        assert!(ui(&mods)["above"].is_null() && ui(&mods)["panes"] == json!([]));
        assert!(mods.owns("session"),"a posse só sai no close");
    }

    #[test]
    fn mods_call_that_waited_past_its_deadline_in_the_inbox_does_not_run() {
        let mut engine = RuntimeEngine::new("claude",json!({"name":"session","initialized":true}),1,ClockSample { monotonic_s:0.0,epoch_s:0.0 })
            .unwrap().with_mods(crate::mods::state::Mods::default());
        let (mut waiters,mut token) = (ModsWaiters::new(),0u64);
        let press = || ModsCall::Press { site:"above-prompt".into(),plugin:"m".into(),key:"k".into() };
        let sample = ClockSample { monotonic_s:1.0,epoch_s:1.0 };
        // Vencido: responde sem levar o pedido à superfície (nenhum efeito, nenhum token gasto).
        let (response,mut receive) = oneshot::channel();
        let effects = take_mods(&mut engine,&mut waiters,&mut token,press(),Instant::now() - Duration::from_millis(1),response,sample);
        assert!(effects.is_empty() && waiters.is_empty() && token == 0);
        assert_eq!(receive.try_recv().unwrap().unwrap_err().code,"erro_mod_clique_sem_resposta");
        // No prazo, mas quem pediu já desistiu (a rota cortou a chamada): também não roda.
        let (response,receive) = oneshot::channel();
        drop(receive);
        let effects = take_mods(&mut engine,&mut waiters,&mut token,press(),Instant::now() + Duration::from_secs(5),response,sample);
        assert!(effects.is_empty() && waiters.is_empty() && token == 0);
        // No prazo: vai à superfície, que ainda não ligou e responde que o botão não está lá.
        let (response,_receive) = oneshot::channel();
        let effects = take_mods(&mut engine,&mut waiters,&mut token,press(),Instant::now() + Duration::from_secs(5),response,sample);
        assert!(effects.iter().any(|effect|matches!(effect,Effect::Surface { effect:SurfaceEffect::Reply { token:1,result:Err(error) } }
            if error.code == "erro_mod_botao_inexistente")));
        assert!(waiters.contains_key(&1));
    }

    #[test]
    fn steer_queue_refusals_carry_a_code() {
        assert_eq!(steer_refusal(&json!({"alive":true,"in_progress":false}),"claude"),Some(("steer_no_turn","Não há turno em andamento para orientar")));
        assert_eq!(steer_refusal(&json!({"alive":true,"in_progress":true,"pending":[{"request_id":"r"}]}),"claude").map(|r|r.0),Some("steer_question_open"));
        assert_eq!(steer_refusal(&json!({"alive":true,"in_progress":true,"pending":[{"request_id":"r"}]}),"codex"),None);
        assert_eq!(steer_refusal(&json!({"alive":true,"in_progress":true,"pending":[],"question":null}),"claude"),None);
    }

    #[tokio::test]
    async fn a_forced_save_after_a_skipped_newer_change_still_saves_the_counter() {
        // Mudança de estado mais nova que pulou o disco não pode fazer a gravação forçada (antes de
        // uma escrita no fio) ser descartada: o contador dos IDs precisa estar salvo.
        let dir = tempfile::tempdir().unwrap();
        let lease = super::super::queue::acquire_lease(&dir.path().join("key.lock")).unwrap();
        let store = super::super::queue::Store::open(&dir.path().join("state.json"),&dir.path().join("projection"),
            super::super::queue::State::new("key",1,"session",vec![])).unwrap();
        let queue = QueueActor::start(store,lease);
        let gate = Mutex::new(SavedView::default());
        let sample = ClockSample { monotonic_s:0.0,epoch_s:0.0 };
        let view = |counter:u64,working:bool| json!({"model":"m","runtime_counter":counter,"in_progress":working});
        save_view(&queue,1,sample,&gate,1,&view(1,false),false).await.unwrap();
        save_view(&queue,1,sample,&gate,3,&view(3,true),false).await.unwrap();
        save_view(&queue,1,sample,&gate,2,&view(2,true),true).await.unwrap();
        let state = queue.snapshot().await.unwrap();
        assert_eq!(state.runtime_state["view"]["runtime_counter"],3);
        queue.shutdown().await.unwrap();
    }
}
