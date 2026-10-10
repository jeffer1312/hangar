use super::{cano::{CanoConnection,IoEvent,WireFrame},claude::ClaudeEngine,codex::Engine as CodexEngine,local_policy,
    protocol::*,queue::{Action,QueueActor,Status},receipt::ReceiptIndex};
use crate::mods::model::{ModsCall,ModsError,SurfaceEffect};
use serde_json::{Value,json};
use std::collections::{BTreeMap,BTreeSet,VecDeque};
use std::sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}};
use std::time::{Duration,Instant,SystemTime,UNIX_EPOCH};
use tokio::sync::{broadcast,mpsc,oneshot,Mutex,Notify};
use tokio::task::{JoinHandle,JoinSet};

enum Core { Claude(ClaudeEngine),Codex(CodexEngine) }

/// Teto da política no Python. A publicação no terminal espera o aviso do plugin, que só sai depois
/// dos hooks do UserPromptSubmit: o teto dela fica acima da espera do Python (`PUBLICA_S`).
const POLICY_TIMEOUT:Duration=Duration::from_secs(15);
const PUBLISH_POLICY_TIMEOUT:Duration=Duration::from_secs(40);
/// Python subindo depois de um restart (ou parado um instante): a política que só grava o arquivo da
/// sessão espera por ele em vez de pôr a sessão em erro. Primeira espera e teto da série.
const READY_FIRST_WAIT:Duration=Duration::from_secs(1);
const READY_MAX_WAIT:Duration=Duration::from_secs(16);
const READY_BUDGET:Duration=Duration::from_secs(120);
/// Teto de um pedido de app aos mods: o mais longo da superfície (desenho de novo e clique, 6 s) mais
/// 1 s de folga para o relógio do ator, abaixo dos 8 s em que o app desiste. O registro serializa os
/// pedidos da sessão, então um pedido sem teto prenderia os seguintes.
const MODS_CALL_LIMIT:Duration=Duration::from_secs(crate::mods::surface::APP_CALL_MAX_S as u64 + 1);

/// A cota do Claude muda devagar: uma consulta ao Python por conta a cada 5 minutos basta.
const QUOTA_TTL:Duration=Duration::from_secs(300);
const QUOTA_ACCOUNTS:usize=16;
/// Python fora do ar: sem isto cada linha de status esperaria o prazo inteiro de novo.
const QUOTA_DOWN_TTL:Duration=Duration::from_secs(30);

#[derive(Clone)]
pub struct PolicyClient {
    upstream:std::net::SocketAddr,
    secret:String,
    instance:String,
    http:crate::proxy::HttpClient,
    /// `config_dir` -> (quando veio, `{"windows":[…]}`).
    quota:Arc<std::sync::Mutex<BTreeMap<String,(Instant,Value)>>>,
    /// `config_dir` -> quando a consulta não chegou ao Python (transporte ou prazo).
    quota_down:Arc<std::sync::Mutex<BTreeMap<String,(Instant,())>>>,
    diag:crate::diag::DiagClient,
    /// (primeira espera, teto) da repetição enquanto o Python não está pronto.
    ready_retry:(Duration,Duration),
}

/// Guarda no mapa limitado a `QUOTA_ACCOUNTS`, tirando a entrada mais antiga para caber a nova.
fn remember<T>(cache:&std::sync::Mutex<BTreeMap<String,(Instant,T)>>,config_dir:&str,value:T) {
    let Ok(mut cache) = cache.lock() else { return };
    if !cache.contains_key(config_dir) && cache.len() >= QUOTA_ACCOUNTS {
        if let Some(oldest) = cache.iter().min_by_key(|(_,(at,_))|*at).map(|(dir,_)|dir.clone()) { cache.remove(&oldest); }
    }
    cache.insert(config_dir.to_owned(),(Instant::now(),value));
}

impl PolicyClient {
    pub fn new(upstream:std::net::SocketAddr,secret:String,instance:String) -> Self {
        Self { upstream,diag:crate::diag::DiagClient::new(upstream,secret.clone()),secret,instance,http:crate::proxy::client(),
            quota:Default::default(),quota_down:Default::default(),ready_retry:(READY_FIRST_WAIT,READY_BUDGET) }
    }
    pub fn with_ready_retry(mut self,first_wait:Duration,budget:Duration) -> Self { self.ready_retry = (first_wait,budget); self }
    /// Janelas de cota da conta, só quando vai formatar. Falha formata sem janelas e deixa o cache
    /// anterior como está; o log leva o código, nunca o dado.
    pub async fn quota_windows(&self,key:&str,config_dir:&str) -> Option<Value> {
        let cached = self.quota.lock().ok().and_then(|cache|cache.get(config_dir).cloned());
        if let Some((at,value)) = &cached { if at.elapsed() < QUOTA_TTL { return Some(value.clone()); } }
        let down = self.quota_down.lock().ok().and_then(|down|down.get(config_dir).map(|(at,_)|at.elapsed() < QUOTA_DOWN_TTL));
        if down == Some(true) { return None; }
        let query = form_urlencoded::Serializer::new(String::new()).append_pair("config_dir",config_dir).finish();
        let fetched = tokio::time::timeout(POLICY_TIMEOUT,async {
            let request = axum::http::Request::get(format!("http://{}/internal/quota?{query}",self.upstream))
                .header("x-hangar-internal",&self.secret).body(axum::body::Body::empty()).map_err(|_|"quota_request")?;
            let response = self.http.request(request).await.map_err(|_|"quota_transport")?;
            if !response.status().is_success() { return Err("quota_refused"); }
            let bytes = axum::body::to_bytes(axum::body::Body::new(response.into_body()),MAX_ENVELOPE).await.map_err(|_|"quota_limit")?;
            let value:Value = serde_json::from_slice(&bytes).map_err(|_|"quota_json")?;
            if value["windows"].is_array() { Ok(value) } else { Err("quota_shape") }
        }).await.unwrap_or(Err("quota_timeout"));
        match fetched {
            Ok(value) => {
                if let Ok(mut down) = self.quota_down.lock() { down.remove(config_dir); }
                remember(&self.quota,config_dir,value.clone());
                Some(value)
            }
            Err(code) => {
                if matches!(code,"quota_transport" | "quota_timeout") { remember(&self.quota_down,config_dir,()); }
                if crate::warn_limit::allow(Some(key),code) { tracing::warn!(key=%key,code=%code,"cota do Claude indisponível; a linha sai sem janelas"); }
                None
            }
        }
    }
    async fn run(&self,target:&RuntimeTarget,kind:&str,request_id:&RequestId,payload:Value,phase_id:&str) -> Result<Value,RuntimeError> {
        self.run_for(&target.key,target.generation,kind,request_id,payload,phase_id).await
    }
    pub async fn run_for(&self,key:&str,generation:u64,kind:&str,request_id:&RequestId,payload:Value,phase_id:&str) -> Result<Value,RuntimeError> {
        // Só repete o que grava o arquivo da sessão ou calcula o comando (idempotente, e a mesma fase junta
        // a chamada ainda em curso no Python); recado nativo e terminal escrevem na conversa.
        let retries = kind.starts_with("session.") || kind == "launch_env";
        let (mut wait,budget) = self.ready_retry;
        let started = Instant::now();
        loop {
            match self.attempt(key,generation,kind,request_id,payload.clone(),phase_id).await {
                Ok(value) => return Ok(value),
                Err((error,detail)) if retries && not_ready(&error.code,&detail) && started.elapsed() + wait <= budget => {
                    if crate::warn_limit::allow(Some(key),"policy_not_ready") {
                        tracing::warn!(key=%key,generation,policy=%kind,code=%error.code,detail=%detail,"Python ainda não atende a política; tentando de novo");
                    }
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(READY_MAX_WAIT);
                }
                Err((error,detail)) => {
                    if crate::warn_limit::allow(Some(key),&error.code) {
                        tracing::warn!(key=%key,generation,policy=%kind,code=%error.code,detail=%detail,"política do Python falhou");
                    }
                    return Err(error);
                }
            }
        }
    }
    async fn attempt(&self,key:&str,generation:u64,kind:&str,request_id:&RequestId,payload:Value,phase_id:&str) -> Result<Value,(RuntimeError,String)> {
        let body = json!({"key":key,"generation":generation,"request_id":request_id,"phase_id":phase_id,"kind":kind,"payload":payload});
        let request = axum::http::Request::post(format!("http://{}/internal/runtime/policy",self.upstream))
            .header("x-hangar-internal",&self.secret).header("x-hangar-runtime-instance",&self.instance)
            .header("content-type","application/json").body(axum::body::Body::from(body.to_string()))
            .map_err(|_|(failure("policy_request"),String::new()))?;
        // Detalhe só de forma: status, tipo de erro, posição ou nome da exceção; nunca o corpo.
        let limit = if kind == "terminal_publish" { PUBLISH_POLICY_TIMEOUT } else { POLICY_TIMEOUT };
        tokio::time::timeout(limit,async {
            let response = self.http.request(request).await
                .map_err(|error|(failure("policy_transport"),format!("connect={}",error.is_connect())))?;
            if !response.status().is_success() { return Err((failure("policy_refused"),format!("status={}",response.status().as_u16()))); }
            let bytes = axum::body::to_bytes(axum::body::Body::new(response.into_body()),MAX_ENVELOPE)
                .await.map_err(|_|(failure("policy_limit"),String::new()))?;
            let value:Value = serde_json::from_slice(&bytes)
                .map_err(|error|(failure("policy_json"),format!("line={} column={}",error.line(),error.column())))?;
            if value["ok"] != true {
                let kind = value["error_type"].as_str().filter(|kind|kind.len() <= 64 && kind.bytes().all(|b|b.is_ascii_alphanumeric() || b == b'_'));
                return Err((failure("policy_failed"),format!("error_type={}",kind.unwrap_or("?"))));
            }
            Ok(value["data"].clone())
        }).await.unwrap_or_else(|_|Err((failure("policy_timeout"),String::new())))
    }
}

/// O Python ainda não atende: fora do ar, travado, ou 503 (sem coordenador, ou sem o registro da chave,
/// que ele só grava depois que o `open` do Rust responde). 400 é pedido torto e `ok:false` é recusa: definitivos.
fn not_ready(code:&str,detail:&str) -> bool {
    matches!(code,"policy_timeout" | "policy_transport") || code == "policy_refused" && detail == "status=503"
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
    /// A queda entregou `cano_saiu`: o processo filho morreu e o cano que ficou pode ser encerrado.
    kill:bool,
    /// O cano caiu durante uma subida pedida: a queda é dela, não agenda outra.
    swallowed:bool,
    was_working:bool,
    last:Option<RuntimeError>,
}
const RESPAWN_MAX:u8 = 3;

struct Relaunched { target:RuntimeTarget, spawned:bool, connection:CanoConnection, patch:Value }

pub struct RuntimeEngine {
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
}

impl RuntimeEngine {
    pub fn new(provider:&str,metadata:Value,generation:u64,clock:ClockSample) -> Result<Self,RuntimeError> {
        let core = match provider { "claude"=>Core::Claude(ClaudeEngine::new(metadata,generation,clock)),
            "codex"=>Core::Codex(CodexEngine::new(metadata,generation,clock)),_=>return Err(failure("provider")) };
        Ok(Self { core,launch:None,policy:None,publisher:None,revision:Arc::new(AtomicU64::new(0)),mods:None,mods_life:0,live:None })
    }
    /// O ator sobe o processo de novo quando ele cai, e reinicia/troca o sandbox (só Codex).
    pub fn with_launch(mut self,launch:LaunchConfig) -> Self { if matches!(self.core,Core::Codex(_)) { self.launch = Some(launch); } self }
    /// Motor da vida nova do processo, com as ligações desta.
    fn renewed(&self,metadata:Value,generation:u64,clock:ClockSample) -> Result<Self,RuntimeError> {
        let provider = match self.core { Core::Claude(_)=>"claude",Core::Codex(_)=>"codex" };
        let mut next = Self::new(provider,metadata,generation,clock)?;
        (next.launch,next.policy,next.publisher,next.revision,next.live) = (self.launch.clone(),self.policy.clone(),self.publisher.clone(),self.revision.clone(),self.live.clone());
        Ok(next)
    }
    fn problem(&self) -> Option<String> { match &self.core { Core::Codex(core)=>core.problem().map(str::to_owned),Core::Claude(_)=>None } }
    fn set_problem(&mut self,code:&str,detail:Option<String>) -> Vec<Effect> {
        match &mut self.core { Core::Codex(core)=>core.set_problem(code,detail),Core::Claude(_)=>Vec::new() }
    }
    pub fn with_policy(mut self,policy:PolicyClient) -> Self { self.policy = Some(policy); self }
    pub fn with_publisher(mut self,publisher:broadcast::Sender<RuntimeEvent>) -> Self { self.publisher = Some(publisher); self }
    pub fn with_revision(mut self,revision:Arc<AtomicU64>) -> Self { self.revision = revision; self }
    /// Canal em processo do hub: estado, erro e prévia da sessão sem terminal saem por ele, fora do `events`.
    pub fn with_live(mut self,live:LiveSender) -> Self { self.live = Some(live); self }
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
    fn view(&self) -> Value {
        match &self.core { Core::Claude(core)=>core.view(),Core::Codex(core)=> {
            let mut view = core.control_view(); view["public_state"] = core.view(); view["conversation"] = view["thread_id"].clone(); view
        } }
    }
    fn apply(&mut self,input:EngineInput,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        match &mut self.core { Core::Claude(core)=>core.apply(input,clock),Core::Codex(core)=>core.apply(input,clock) }
    }
    fn command(&mut self,command:RuntimeCommand,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        match &mut self.core { Core::Claude(core)=>core.command(command,clock),Core::Codex(core)=>core.command(command,clock) }
    }
    fn hydrate(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        match &mut self.core { Core::Claude(core)=>core.hydrate(snapshot),Core::Codex(core)=>core.hydrate(snapshot) }
    }
    fn deadline(&self) -> Option<f64> { match &self.core { Core::Claude(core)=>core.next_deadline(),Core::Codex(core)=>core.next_deadline() } }
    fn initialize(&mut self,id:String) -> Result<Vec<Effect>,RuntimeError> {
        match &mut self.core { Core::Claude(core)=>core.start_initialize(id),Core::Codex(core)=>core.bootstrap(true,id) }
    }
    fn write_is_current(&self,id:&str) -> bool { match &self.core { Core::Claude(core)=>core.write_is_current(id),Core::Codex(core)=>core.write_is_current(id) } }
    fn forget_policy(&mut self,id:&RequestId) { match &mut self.core { Core::Claude(core)=>core.forget_policy(id),Core::Codex(core)=>core.forget_policy(id) } }
    fn confirm_input(&mut self,id:&str) -> Vec<Effect> {
        match &mut self.core { Core::Claude(_)=>vec![Effect::Reply { operation_id:id.into(),disposition:Disposition::Accepted,payload:json!({"confirmed":true}) }],
            Core::Codex(core)=>core.confirm_input(id) }
    }
    fn restore(&mut self,state:&super::queue::State) {
        for phase in state.operations.values().filter(|phase|recover_phase(state,phase)) {
            if let Some(id) = phase.payload["logical_id"].as_str() {
                match &mut self.core {
                    Core::Codex(core)=>{
                        core.restore_rpc(id.into(),&phase.payload["frame"],phase.payload["state_revision"].as_u64().unwrap_or(0),
                            phase.payload["settings_revision"].as_u64().unwrap_or(0));
                        if let Some(call_id) = state.operations.get(id).and_then(|root|root.payload["payload"]["call_id"].as_str()) {
                            core.restore_voice_scope(id,call_id);
                        }
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

type Response = oneshot::Sender<Result<RuntimeReply,RuntimeError>>;
enum Message {
    Command { command:RuntimeCommand,response:Response,from_queue:bool },
    Queue { call_id:String,action:Action,response:oneshot::Sender<Result<Value,RuntimeError>> },
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
}

#[derive(Clone)]
pub struct RuntimeHandle {
    sender:mpsc::Sender<Message>,
    task:Arc<Mutex<Option<JoinHandle<Result<(),RuntimeError>>>>>,
    closed:Arc<AtomicBool>,
    events:broadcast::Sender<RuntimeEvent>,
    stopped:Arc<Mutex<Option<Result<(),RuntimeError>>>>,
    key:String,
}

impl RuntimeHandle {
    pub async fn command(&self,command:RuntimeCommand) -> Result<RuntimeReply,RuntimeError> {
        if self.closed.load(Ordering::Acquire) { return Err(failure("runtime_stopping")); }
        let (response,receive) = oneshot::channel();
        self.sender.send(Message::Command { command,response,from_queue:false }).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
    }
    pub async fn queue(&self,call_id:String,action:Action) -> Result<Value,RuntimeError> {
        let (response,receive) = oneshot::channel();
        self.sender.send(Message::Queue { call_id,action,response }).await.map_err(|_|self.gone("runtime_closed"))?;
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
    pub async fn drain(&self) -> Result<Value,RuntimeError> {
        let (send,receive) = oneshot::channel(); self.sender.send(Message::Drain(send)).await.map_err(|_|self.gone("runtime_closed"))?;
        receive.await.map_err(|_|self.gone("runtime_closed"))?
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
    pub async fn ensure_projection(&self) -> Result<Value,RuntimeError> {
        self.queue(format!("projection:{}",unique()),Action::EnsureProjection).await
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
        Self { original:serde_json::to_value(&command).unwrap(),command,responses:Vec::new(),result:Some(reply),preparing:false,
            cancelled:false,error:None,deadline:0.0,timed_out:false,ready_to_run:false,arrival:0 }
    }
}

#[derive(Clone)]
struct Attempt { logical_id:String,phase_id:String,frame:Value,order:u64 }

enum Job {
    Root { id:String,result:Result<Option<RuntimeReply>,RuntimeError> },
    Write { wire:String,result:Result<(),RuntimeError> },
    Ack { logical_id:String,outcome:WriteOutcome,result:Result<(),RuntimeError> },
    Finished { reply:RuntimeReply,result:Result<(),RuntimeError> },
    Policy { request_id:RequestId,kind:String,phase_id:String,conversation:Option<Value>,result:Result<Value,RuntimeError> },
    PreparedInput { id:String,result:Result<Value,RuntimeError> },
    Saved(Result<(),RuntimeError>),
    Queued { wake:bool,result:Result<(),RuntimeError> },
    View { version:u64,view:Value,result:Result<(),RuntimeError> },
    Drained(Result<Vec<RuntimeCommand>,RuntimeError>),
    DrainFinished(Result<RuntimeReply,RuntimeError>),
    Confirmed { response:Option<oneshot::Sender<Result<Value,RuntimeError>>>,result:Result<(Vec<String>,usize),RuntimeError> },
    Steered { id:String,result:Result<Vec<String>,RuntimeError> },
    NativeInput { id:String,result:Result<Value,RuntimeError> },
}

fn unique() -> String {
    static COUNTER:std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("{}:{}",std::process::id(),COUNTER.fetch_add(1,Ordering::Relaxed))
}

pub struct RuntimeActor;

impl RuntimeActor {
    pub fn spawn(target:RuntimeTarget,queue:QueueActor,connection:CanoConnection,engine:RuntimeEngine) -> RuntimeHandle {
        Self::spawn_with(target,queue,connection,engine,|_|{})
    }

    /// Como `spawn`, mas `before` recebe o handle antes de a tarefa do ator existir: o que ele registrar
    /// (o dono no `Mods`) já está lá quando o ator dá o primeiro passo. Pedidos que chegarem nesse meio
    /// esperam na caixa.
    pub fn spawn_with(target:RuntimeTarget,queue:QueueActor,connection:CanoConnection,engine:RuntimeEngine,
        before:impl FnOnce(&RuntimeHandle)) -> RuntimeHandle {
        let (sender,receiver) = mpsc::channel(64);
        let events = engine.publisher.clone().unwrap_or_else(||broadcast::channel(256).0);
        let closed = Arc::new(AtomicBool::new(false));
        let (key,name,life) = (target.key.clone(),target.name.clone(),engine.mods_life);
        let mods = engine.mods.clone();
        let live = engine.live.clone();
        let handle = RuntimeHandle { sender:sender.clone(),task:Arc::new(Mutex::new(None)),closed:closed.clone(),events:events.clone(),
            stopped:Arc::new(Mutex::new(None)),key:key.clone() };
        // O slot fica preso até receber a tarefa: um `stop` que chegue antes espera por ele e junta a tarefa.
        let slot = handle.task.clone();
        let mut slot = slot.try_lock().expect("slot da tarefa recém-criado");
        before(&handle);
        let run = run(target,queue,connection,engine,receiver,sender,closed,events);
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

async fn run(mut target:RuntimeTarget,queue:QueueActor,connection:CanoConnection,mut engine:RuntimeEngine,
    mut receiver:mpsc::Receiver<Message>,internal:mpsc::Sender<Message>,closed:Arc<AtomicBool>,events:broadcast::Sender<RuntimeEvent>) -> Result<(),RuntimeError> {
    let start = Instant::now();
    let initial = queue.initial_state().clone();
    engine.restore(&initial);
    let snapshot = connection.snapshot.clone();
    let mut io = connection.start(target.generation,128).hold_lease(queue.lease());
    let queue = Arc::new(queue);
    let mut jobs = JoinSet::new();
    let mut roots:BTreeMap<String,Pending> = BTreeMap::new();
    let preparation = Arc::new((Mutex::new(1u64),Notify::new()));
    let mut arrival = 0u64;
    let mut attempts:BTreeMap<String,Attempt> = initial.operations.values().filter(|phase|recover_phase(&initial,phase))
        .filter_map(|phase|Some((phase.id.clone(),Attempt { logical_id:phase.payload["logical_id"].as_str()?.into(),
            phase_id:phase.id.clone(),frame:phase.payload["frame"].clone(),order:0 }))).collect();
    let mut effects:VecDeque<Effect> = VecDeque::new();
    let mut revision = Revision { value:engine.revision.load(Ordering::Acquire),counter:engine.revision.clone() };
    let mut state_version = 0u64;
    let mut published_state_version = 0u64;
    let state_gate = Arc::new(Mutex::new(SavedView::default()));
    let mut durable_view = json!({"alive":true,"initialized":false,"ready":false});
    let mut last_state = String::new();
    let mut confirming = false;
    let mut channels:BTreeMap<String,Value> = ["preview","thinking","tool"].into_iter().map(|channel|
        (channel.into(),json!({"session":target.name,"text":"","md":true,"full":true,"vivo":true}))).collect();
    let mut live = LiveOut { sender:engine.live.clone(),state:LiveState { public_state:engine.view()["public_state"].clone(),..Default::default() } };
    live.send();
    let receipt = Arc::new(std::sync::Mutex::new(ReceiptIndex::new(&target.provider,engine.view()["conversation"].as_str().unwrap_or(""))));
    let mut sequence = initial.operations.keys().filter_map(|id|id.rsplit(':').next()?.parse::<u64>().ok()).max().unwrap_or(0);
    let mut write_order = 0u64;
    let mut next_write = 1u64;
    let mut native:BTreeSet<String> = BTreeSet::new();
    let mut prepared_writes:BTreeMap<u64,(String,Result<(),RuntimeError>)> = BTreeMap::new();
    let mut error:Option<RuntimeError> = None;
    let mut io_open = true;
    let mut drain_requested = true;
    let mut drain_active = false;
    let mut drain_waiters = Vec::new();
    let mut mods_waiters:ModsWaiters = BTreeMap::new();
    let mut mods_token = 0u64;
    let mut ui_writes = 0u64;
    let mut respawn = Respawn::default();
    effects.extend(engine.hydrate(snapshot)?);
    if engine.view()["initialized"] != true || target.provider == "codex" && engine.view()["ready"] != true {
        let id = format!("bootstrap:{}:{}",target.key,target.generation);
        queue.exec(target.generation,&format!("prepare:{id}"),clock(start),Action::Prepare { id:id.clone(),payload:json!({"kind":"bootstrap"}),entry_id:None }).await.map_err(io_failure)?;
        effects.extend(engine.initialize(id)?);
    }
    loop {
        live.error(&error);
        loop {
            let first_input = roots.values().filter(|root|root.preparing && matches!(root.command.kind,OperationKind::Input | OperationKind::Steer))
                .map(|root|root.arrival).min();
            let ready = roots.iter().filter(|(_,root)|root.ready_to_run && root.preparing
                && (!matches!(root.command.kind,OperationKind::Input | OperationKind::Steer) || Some(root.arrival) == first_input))
                .min_by_key(|(_,root)|root.arrival).map(|(id,_)|id.clone());
            let Some(id) = ready else { break };
            let pending = roots.get_mut(&id).unwrap();
            pending.preparing = false;
            if pending.cancelled || pending.timed_out { continue; }
            if pending.command.kind == OperationKind::SteerQueue {
                // Mesmas recusas do adapter Python: sem turno não há o que orientar, e o Claude parado numa
                // permissão ou pergunta não lê o stdin; a fila sumiria da tela até alguém responder.
                let view = engine.view();
                let refusal = if view["alive"] != true || view["in_progress"] != true { Some("Não há turno em andamento para orientar") }
                    else if target.provider == "claude" && (view["pending"].as_array().is_some_and(|p|!p.is_empty()) || !view["question"].is_null()) {
                        Some("Responda a permissão ou pergunta pendente antes de orientar") }
                    else { None };
                if let Some(text) = refusal {
                    effects.push_back(Effect::Reply { operation_id:id,disposition:Disposition::Rejected,payload:json!({"error":text}) });
                    continue;
                }
                let queue = queue.clone(); let target = target.clone(); let sender = internal.clone(); let sample = clock(start);
                let entry_id = pending.command.payload["entry_id"].as_str().map(str::to_owned);
                jobs.spawn(async move {
                    let result = async {
                        let claimed = queue.exec(target.generation,&format!("steer-claim:{id}"),sample,
                            Action::Claim { min_ts:target.created,limit:None,entry_id }).await.map_err(io_failure)?;
                        let mut replies = Vec::new();
                        for row in claimed.as_array().ok_or_else(||failure("queue_shape"))? {
                            let entry = row["id"].as_str().ok_or_else(||failure("queue_entry"))?.to_owned();
                            let command = RuntimeCommand { operation_id:format!("{id}:{entry}"),kind:OperationKind::Steer,
                                payload:json!({"text":row["text"],"entry_id":entry,"pre_transcript":row["pre_transcript"].as_bool().unwrap_or(false)}) };
                            let (response,receive) = oneshot::channel();
                            sender.send(Message::Command { command,response,from_queue:false }).await.map_err(|_|failure("runtime_closed"))?;
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
            match engine.command(pending.command.clone(),clock(start)) {
                Ok(next)=>effects.extend(next),Err(error)=>defer_unwritten(&mut roots,&attempts,&native,&id,error,&mut effects),
            }
        }
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::Write { frame,operation_id } => {
                    sequence += 1;
                    write_order += 1;
                    let logical_id = operation_id.unwrap_or_else(||format!("system:{}:{sequence}",target.generation));
                    let wire = format!("wire:{logical_id}:{sequence}");
                    let attempt = Attempt { logical_id:logical_id.clone(),phase_id:wire.clone(),frame:frame.clone(),order:write_order };
                    attempts.insert(wire.clone(),attempt.clone());
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
                            if authoritative.operations.get(&logical_id).is_none_or(|op|op.status == Status::Prepared) {
                                queue.exec(target.generation,&format!("logical-cursor:{wire}"),sample,Action::BindDispatch { id:logical_id.clone(),cursor }).await.map_err(io_failure)?;
                            }
                            queue.exec(target.generation,&format!("dispatch:{wire}"),sample,Action::BeginDispatch { id:wire.clone(),wire_id:wire.clone(),staged:false }).await.map_err(io_failure)?;
                            queue.exec(target.generation,&format!("logical-dispatch:{wire}"),sample,Action::BeginDispatch { id:logical_id.clone(),wire_id:wire.clone(),staged:false }).await.map_err(io_failure)?;
                            Ok(())
                        }.await;
                        Job::Write { wire,result }
                    });
                }
                Effect::Publish { channel,data } => {
                    if ["preview","thinking","tool"].contains(&channel.as_str()) {
                        channels.insert(channel.clone(),data.clone());
                        // Prévia só pelo canal do hub: no `events` ela subiria a
                        // revisão e o Python a decodificaria a cada delta.
                        if !live.channel(&channel,&data) { publish(&events,&target,&mut revision,&channel,data); }
                    } else if ["voice","voice_target","rate"].contains(&channel.as_str()) {
                        publish(&events,&target,&mut revision,&channel,data);
                    }
                }
                Effect::StateChanged => {
                    let view = engine.view();
                    // Subida boa zera o teto: só a que nunca fica pronta conta como seguida.
                    if view["ready"] == true && respawn.failures > 0 { respawn.failures = 0; respawn.last = None; }
                    state_version += 1;
                    let version = state_version;
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    let gate = state_gate.clone();
                    jobs.spawn(async move {
                        let result = save_view(&queue,generation,sample,&gate,version,&view,false).await;
                        Job::View { version,view,result }
                    });
                }
                Effect::Reply { operation_id,disposition,payload } => {
                    let reply = RuntimeReply { operation_id:operation_id.clone(),disposition,payload };
                    let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                    let related:Vec<_> = attempts.values().filter(|attempt|attempt.logical_id == operation_id).cloned().collect();
                    jobs.spawn(async move {
                        let result = async {
                            for attempt in related {
                                queue.exec(generation,&format!("finish:{}:{}",attempt.phase_id,unique()),sample,Action::Finish {
                                    id:attempt.phase_id,status:status(disposition),result:serde_json::to_value(&reply).unwrap() }).await.map_err(io_failure)?;
                            }
                            queue.exec(generation,&format!("finish:{operation_id}:{}",unique()),sample,Action::Finish {
                                id:operation_id,status:status(disposition),result:serde_json::to_value(&reply).unwrap() }).await.map_err(io_failure)?;
                            Ok(())
                        }.await;
                        Job::Finished { reply,result }
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
                    // Estes serviços não escrevem na CLI (formatar status, carimbo, sidecar, log): repetir é
                    // inofensivo, então não passam pelo diário. Quatro gravações por chamada, a cada mudança de
                    // estado, eram a maior parte do disco gasto por mensagem.
                    jobs.spawn(async move {
                        let result = async {
                            if let Some((version,view)) = save {
                                save_view(&queue,target.generation,sample,&gate,version,&view,false).await?;
                            }
                            if local_policy::is_local(&kind) {
                                let quota = match &policy {
                                    Some(policy) if kind == "format_status" && target.provider == "claude" =>
                                        policy.quota_windows(&target.key,target.metadata["config_dir"].as_str().unwrap_or("")).await,
                                    _ => None,
                                };
                                return run_local(kind.clone(),payload,&target,quota).await;
                            }
                            match policy {
                                Some(policy) => policy.run(&target,&kind,&request_id,payload,&phase_id).await,
                                None => Err(failure("policy_unavailable")),
                            }
                        }.await;
                        Job::Policy { request_id,kind,phase_id,conversation,result }
                    });
                }
                Effect::WakeQueue => { drain_requested = true; },
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
                        let frame = WireFrame { operation_id:format!("ui:{}:{ui_writes}",target.generation),frame,ephemeral:true,until };
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
                Effect::Respawn { operation_id,reason,patch,reply } => {
                    if respawn.task.is_some() {
                        fail_root(&mut roots,&operation_id,RuntimeError::new("erro_codex_reiniciando","o Codex já está subindo de novo; tente em instantes"));
                        continue;
                    }
                    // Ação da pessoa: nova rodada de tentativas.
                    (respawn.failures,respawn.next_at,respawn.user) = (0,None,Some((operation_id.clone(),reply)));
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
        while let Some((wire,result)) = prepared_writes.remove(&next_write) {
            next_write += 1;
            let attempt = attempts.get(&wire).unwrap();
            let ended = roots.iter().any(|(id,root)|(attempt.logical_id == *id || attempt.logical_id.starts_with(&format!("{id}:")))
                && (root.cancelled || root.timed_out));
            if result.is_err() || ended || !engine.write_is_current(&attempt.logical_id) {
                effects.extend(engine.apply(EngineInput::WriteAck { operation_id:attempt.logical_id.clone(),outcome:WriteOutcome::NotWritten },clock(start))?);
                if let Err(failure) = result { enter_error(&mut error,&target,failure); }
            } else if io.try_send(WireFrame { operation_id:wire,frame:attempt.frame.clone(),ephemeral:false,until:None }).is_err() {
                effects.extend(engine.apply(EngineInput::WriteAck { operation_id:attempt.logical_id.clone(),outcome:WriteOutcome::NotWritten },clock(start))?);
            }
        }
        if drain_requested && !drain_active && !closed.load(Ordering::Acquire) && engine.view()["deliverable"] == true
            && !roots.values().any(|root|root.preparing && matches!(root.command.kind,OperationKind::Input | OperationKind::Steer)) {
            drain_requested = false; drain_active = true;
            let queue = queue.clone(); let target = target.clone(); let sample = clock(start);
            jobs.spawn(async move {
                let result = async {
                    let claimed = queue.exec(target.generation,&format!("claim:{}",unique()),sample,
                        Action::Claim { min_ts:target.created,limit:Some(1),entry_id:None }).await.map_err(io_failure)?;
                    let state = queue.snapshot().await.map_err(io_failure)?;
                    let mut commands = Vec::new();
                    for row in claimed.as_array().ok_or_else(||failure("queue_shape"))? {
                        let id = row["id"].as_str().ok_or_else(||failure("queue_entry"))?;
                        let command = state.operations.get(id).and_then(|op|serde_json::from_value::<RuntimeCommand>(op.payload.clone()).ok())
                            .unwrap_or_else(||RuntimeCommand { operation_id:id.into(),kind:OperationKind::Input,
                                payload:json!({"text":row["text"],"pre_transcript":row["pre_transcript"].as_bool().unwrap_or(false)}) });
                        commands.push(command);
                    }
                    Ok(commands)
                }.await;
                Job::Drained(result)
            });
        }
        let deadline = engine.deadline().into_iter().chain(respawn.next_at).chain(roots.values().filter(|root|root.result.is_none() && !root.timed_out).map(|root|root.deadline))
            .min_by(f64::total_cmp).map(|seconds|start + Duration::from_secs_f64(seconds.max(0.0)))
            .unwrap_or_else(||Instant::now()+Duration::from_secs(3600));
        tokio::select! {
            message = receiver.recv() => {
                let Some(message) = message else { break };
                match message {
                    Message::Command { command,response,from_queue } => {
                        let id = command.operation_id.clone();
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
                        if let Some(saved) = initial.operations.get(&id) {
                            if saved.payload != serde_json::to_value(&command).unwrap() { let _ = response.send(Err(failure("operation_reused"))); continue; }
                            let uncertain = matches!(saved.status,Status::Unknown | Status::Dispatching) || initial.operations.values()
                                .any(|phase|phase.payload["logical_id"] == id && matches!(phase.status,Status::Unknown | Status::Dispatching));
                            if uncertain {
                                let reply = RuntimeReply { operation_id:id.clone(),disposition:Disposition::Unknown,payload:json!({"stored":true}) };
                                roots.insert(id,Pending::stored(command,reply.clone()));
                                let _ = response.send(Ok(reply)); continue;
                            }
                            if !(from_queue && saved.status == Status::Deferred) {
                                if let Ok(reply) = serde_json::from_value::<RuntimeReply>(saved.result.clone()) { let _ = response.send(Ok(reply)); continue; }
                            }
                        }
                        if command.kind == OperationKind::Interrupt {
                            for pending in roots.values_mut().filter(|root|root.preparing && matches!(root.command.kind,OperationKind::Input | OperationKind::Steer)) { pending.cancelled = true; }
                        }
                        arrival += 1;
                        let ticket = arrival;
                        roots.insert(id.clone(),Pending { original:serde_json::to_value(&command).unwrap(),command:command.clone(),responses:vec![response],
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
                                if matches!(command.kind,OperationKind::Input | OperationKind::Steer) {
                                    let entry_id = command.payload["entry_id"].as_str().unwrap_or(&id);
                                    let rows = queue.exec(target.generation,&format!("load:{id}"),sample,Action::Load).await.map_err(io_failure)?;
                                    if !rows.as_array().is_some_and(|rows|rows.iter().any(|row|row["id"] == entry_id)) {
                                        queue.exec(target.generation,&format!("append:{id}"),sample,Action::Append { text:command.payload["text"].as_str().ok_or_else(||failure("input_text"))?.into(),
                                            delivered:false,ts:None,pre_transcript:command.payload["pre_transcript"] == true,entry_id:Some(entry_id.into()) }).await.map_err(io_failure)?;
                                    }
                                }
                                let prepared = queue.exec(target.generation,&format!("prepare:{id}:{}",unique()),sample,Action::Prepare { id:id.clone(),payload:serde_json::to_value(&command).unwrap(),
                                    entry_id:matches!(command.kind,OperationKind::Input | OperationKind::Steer)
                                        .then(||command.payload["entry_id"].as_str().unwrap_or(&id).into()) }).await.map_err(io_failure)?;
                                Ok(stored_reply(&id,&prepared))
                            }.await;
                            *preparation.0.lock().await += 1;
                            preparation.1.notify_waiters();
                            Job::Root { id,result }
                        });
                    }
                    Message::Mods { call,deadline,response } => {
                        effects.extend(take_mods(&mut engine,&mut mods_waiters,&mut mods_token,call,deadline,response,clock(start)));
                    }
                    Message::Queue { call_id,action,response } => {
                        let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                        let wake = matches!(&action,Action::Append { delivered:false,.. });
                        jobs.spawn(async move {
                            let result = queue.exec(generation,&call_id,sample,action).await.map_err(io_failure);
                            let saved = result.as_ref().map(|_|()).map_err(Clone::clone);
                            let _ = response.send(result);
                            Job::Queued { wake,result:saved }
                        });
                    }
                    Message::Snapshot(response) => {
                        let _ = response.send(Ok(json!({"key":target.key,"generation":target.generation,"revision":revision.value,
                            "view":durable_view,"channels":channels,"error":error.as_ref().map(|e|e.code.clone())})));
                    }
                    Message::View(response) => { let _ = response.send(engine.view()); }
                    Message::Drain(response) => {
                        if engine.view()["deliverable"] != true && !drain_active {
                            let _ = response.send(Ok(json!({"sent":0})));
                        } else {
                            drain_requested = true;
                            drain_waiters.push(response);
                        }
                    }
                    Message::Confirm(response) => {
                        let job = confirm_inputs(queue.clone(),receipt.clone(),target.transcript.clone(),target.generation,clock(start));
                        jobs.spawn(async move { Job::Confirmed { response:Some(response),result:job.await } });
                    }
                    Message::Stop { response,kill } => {
                        closed.store(true,Ordering::Release);
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
                match event {
                    // Erro de UMA mensagem segue (como o leitor Python); só o erro de leitura encerra.
                    Some(IoEvent::Line(line)) => match engine.apply(EngineInput::Line(line),clock(start)) {
                        Ok(next)=>effects.extend(next),
                        Err(failure)=>if crate::warn_limit::allow(Some(&target.key),"cli_line") {
                            tracing::warn!(key=%target.key,session=%target.name,reason=%failure.message,"mensagem da CLI ignorada");
                        },
                    },
                    Some(IoEvent::WriteAck { operation_id,outcome }) => {
                        if let Some(attempt) = attempts.get(&operation_id) {
                            let logical_id = attempt.logical_id.clone();
                            let queue = queue.clone(); let generation = target.generation; let sample = clock(start);
                            jobs.spawn(async move {
                                let result = async {
                                  let state = queue.snapshot().await.map_err(io_failure)?;
                                  if state.operations.get(&operation_id).is_some_and(|phase|matches!(phase.status,Status::Accepted | Status::Rejected | Status::Confirmed)) { return Ok(()); }
                                  queue.exec(generation,&format!("ack:{operation_id}:{}",unique()),sample,Action::Finish {
                                    id:operation_id,status:match outcome { WriteOutcome::Written=>Status::Accepted,
                                        WriteOutcome::NotWritten=>Status::Rejected,WriteOutcome::Unknown=>Status::Unknown },
                                    result:json!({"write_outcome":outcome}) }).await.map(|_|()).map_err(io_failure)
                                }.await;
                                Job::Ack { logical_id,outcome,result }
                            });
                        }
                    }
                    Some(IoEvent::Stderr(_)) => {},
                    // Queda durante uma subida pedida é a do processo que ela encerrou: não agenda outra.
                    Some(IoEvent::End { .. }) if respawn.task.is_some() => respawn.swallowed = true,
                    Some(IoEvent::End { code }) => {
                        if respawn.next_at.is_none() { respawn.was_working = engine.view()["in_progress"] == true; }
                        effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":code})),clock(start))?);
                        respawn.kill = true;
                        effects.extend(after_exit(&mut respawn,&mut engine,&target,clock(start).monotonic_s,closed.load(Ordering::Acquire)));
                    }
                    None if respawn.task.is_some() => { io_open = false; respawn.swallowed = true; },
                    None => {
                        io_open = false; enter_error(&mut error,&target,failure("cano_closed"));
                        if respawn.next_at.is_none() && !respawn.kill { respawn.was_working = engine.view()["in_progress"] == true; }
                        effects.extend(engine.apply(EngineInput::Line(json!({"type":"cano_saiu","rc":null})),clock(start))?);
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
                        if let Some((operation_id,payload)) = respawn.user.take() {
                            effects.push_back(Effect::Reply { operation_id,disposition:Disposition::Accepted,payload });
                        }
                        drain_requested = true;
                    }
                    Err((failure,written)) => {
                        if !written.is_null() { respawn.carried = written; }
                        tracing::warn!(key=%target.key,session=%target.name,code=%failure.code,reason=%failure.message,attempt=respawn.failures,"processo da sessão não subiu de novo");
                        let user = respawn.user.take();
                        if let Some((operation_id,_)) = &user { fail_root(&mut roots,operation_id,failure.clone()); }
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
                            jobs.spawn(async move {
                                // Cálculo puro (texto → blocos): sem efeito, não entra no diário e pode repetir.
                                let result = run_local("prepare_prompt".into(),command.payload,&target,None).await;
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
                                    // O recado nativo sai pelo Python, fora de `attempts`: a partir daqui ele pode ter
                                    // sido escrito, e nem o prazo nem uma falha podem devolvê-lo à fila.
                                    native.insert(id.clone());
                                    let queue = queue.clone(); let target = target.clone(); let policy = engine.policy.clone().unwrap();
                                    let command = pending.command.clone(); let original = pending.original.clone(); let view = engine.view(); let sample = clock(start);
                                    jobs.spawn(async move {
                                        let result = async {
                                            // Uma fase por tentativa: a adiada volta pelo drain e não pode reaproveitar os recibos da anterior.
                                            let phase = format!("{id}:native_message:{}",unique());
                                            let payload = json!({"text":command.payload["text"]});
                                            queue.exec(target.generation,&format!("prepare:{phase}"),sample,Action::Prepare { id:phase.clone(),
                                                payload:json!({"kind":"native_message","request_id":id,"payload":payload}),entry_id:None }).await.map_err(io_failure)?;
                                            let cursor = capture_cursor(&target,&view).await?;
                                            queue.exec(target.generation,&format!("native-cursor:{phase}"),sample,Action::BindDispatch { id:id.clone(),cursor }).await.map_err(io_failure)?;
                                            queue.exec(target.generation,&format!("native-dispatch:{phase}"),sample,Action::BeginDispatch { id:id.clone(),wire_id:phase.clone(),staged:false }).await.map_err(io_failure)?;
                                            queue.exec(target.generation,&format!("dispatch:{phase}"),sample,Action::BeginDispatch { id:phase.clone(),wire_id:phase.clone(),staged:false }).await.map_err(io_failure)?;
                                            let result = policy.run(&target,"native_message",&RequestId::String(id.clone()),payload,&phase).await?;
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
                            Err(error)=>defer_unwritten(&mut roots,&attempts,&native,&id,error,&mut effects),
                        }
                    }
                    Job::Write { wire,result } => {
                        let attempt = attempts.get(&wire).unwrap();
                        prepared_writes.insert(attempt.order,(wire,result));
                    }
                    Job::Ack { logical_id,outcome,result } => {
                        match result {
                            Ok(())=>effects.extend(engine.apply(EngineInput::WriteAck { operation_id:logical_id,outcome },clock(start))?),
                            Err(failure)=>{
                                fail_root(&mut roots,&logical_id,failure.clone());
                                publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                                enter_error(&mut error,&target,failure);
                            }
                        }
                    }
                    Job::Finished { reply,result } => {
                        if let Err(failure) = result { fail_root(&mut roots,&reply.operation_id,failure.clone()); enter_error(&mut error,&target,failure); }
                        else {
                          if !roots.contains_key(&reply.operation_id) {
                              if let Some(command) = initial.operations.get(&reply.operation_id).and_then(|operation|serde_json::from_value::<RuntimeCommand>(operation.payload.clone()).ok()) {
                                  roots.insert(reply.operation_id.clone(),Pending::stored(command,reply.clone()));
                              }
                          }
                          if let Some(pending) = roots.get_mut(&reply.operation_id) {
                            pending.preparing = false;
                            pending.result = Some(reply.clone());
                            for response in pending.responses.drain(..) { let _ = response.send(Ok(reply.clone())); }
                            if reply.disposition == Disposition::Deferred && matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer) {
                                let queue = queue.clone(); let id = reply.operation_id.clone(); let generation = target.generation; let sample = clock(start);
                                jobs.spawn(async move { Job::Saved(queue.exec(generation,&format!("unclaim:{}",unique()),sample,
                                    Action::SetDelivered { entry_id:id,value:false,steered:false }).await.map(|_|()).map_err(io_failure)) });
                            }
                          }
                        }
                    }
                    Job::Queued { wake,result } => {
                        match result {
                            Ok(()) if wake=>drain_requested = true,
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
                                enter_error(&mut error,&target,failure);
                            },
                        }
                    }
                    Job::View { version,view,result } => {
                        match result {
                            Ok(()) if version > published_state_version => {
                                published_state_version = version;
                                // Confirmar em todo idle, como o adapter Python: sem isto nenhuma entrada vira
                                // confirmed, a poda não as alcança e a fila enche.
                                let state = view["public_state"]["state"].as_str().unwrap_or("").to_owned();
                                if state == "idle" && last_state != "idle" && !confirming {
                                    confirming = true;
                                    let job = confirm_inputs(queue.clone(),receipt.clone(),target.transcript.clone(),target.generation,clock(start));
                                    jobs.spawn(async move { Job::Confirmed { response:None,result:job.await } });
                                }
                                last_state = state;
                                // Vista igual à publicada não sai: cada aparelho redesenharia a tela à toa.
                                if view != durable_view {
                                    live.update(|state|state.public_state = view["public_state"].clone());
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
                    Job::Saved(result) => { if let Err(failure) = result {
                        publish(&events,&target,&mut revision,"problem",json!({"error_code":failure.code,"message":failure.message}));
                        enter_error(&mut error,&target,failure);
                    } },
                    Job::Drained(result) => {
                        match result {
                            Ok(commands) if commands.is_empty() => {
                                drain_active = false;
                                for waiter in drain_waiters.drain(..) { let _ = waiter.send(Ok(json!({"sent":0}))); }
                            }
                            Ok(commands) => for command in commands {
                                let (response,receive) = oneshot::channel();
                                let internal = internal.clone();
                                jobs.spawn(async move {
                                    let result = match internal.send(Message::Command { command,response,from_queue:true }).await {
                                        Ok(())=>receive.await.map_err(|_|failure("runtime_closed")).and_then(|result|result),
                                        Err(_)=>Err(failure("runtime_closed")),
                                    };
                                    Job::DrainFinished(result)
                                });
                            },
                            Err(failure)=>{
                                drain_active = false;
                                for waiter in drain_waiters.drain(..) { let _ = waiter.send(Err(failure.clone())); }
                                enter_error(&mut error,&target,failure);
                            },
                        }
                    }
                    Job::DrainFinished(result) => {
                        drain_active = false;
                        let count = result.as_ref().map(|reply|usize::from(reply.disposition == Disposition::Accepted));
                        for waiter in drain_waiters.drain(..) {
                            let _ = waiter.send(count.clone().map(|sent|json!({"sent":sent})).map_err(Clone::clone));
                        }
                        if let Err(failure) = result { enter_error(&mut error,&target,failure); }
                    }
                    Job::Confirmed { response,result } => {
                        if response.is_none() { confirming = false; }
                        match result {
                            Ok((ids,legacy))=>{
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
                    }
                    Job::Steered { id,result } => {
                        let (disposition,payload) = match result {
                            Ok(ids)=>(Disposition::Accepted,json!({"ids":ids})),
                            Err(error)=>(Disposition::Unknown,json!({"error":error.message,"error_code":error.code})),
                        };
                        effects.push_back(Effect::Reply { operation_id:id,disposition,payload });
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
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                let sample = clock(start);
                effects.extend(engine.apply(EngineInput::Tick,sample)?);
                for (id,pending) in &mut roots {
                    if pending.result.is_none() && !pending.timed_out && sample.monotonic_s >= pending.deadline {
                        pending.timed_out = true;
                        // Sem escrita começada a entrada não chegou à CLI: volta para a fila em vez de ficar incerta.
                        let prefix = format!("{id}:");
                        let written = native.contains(id) || attempts.values().any(|attempt|attempt.logical_id == *id || attempt.logical_id.starts_with(&prefix));
                        let unsent = !written && matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer);
                        if crate::warn_limit::allow(Some(&target.key),"deadline") {
                            tracing::warn!(key=%target.key,session=%target.name,operation=%id,unsent,"operação sem resposta em 30 s");
                        }
                        effects.push_back(Effect::Reply { operation_id:id.clone(),disposition:if unsent { Disposition::Deferred } else { Disposition::Unknown },
                            payload:json!({"error":"operação sem resposta"}) });
                    }
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
        let written = super::gateway::launch_policy(&policy,&target,"session.patch_meta",patch.clone()).await.map_err(|error|(error,Value::Null))?;
        if written["updated"] != true { return Err((RuntimeError::new("session_patch_stale","o arquivo da sessão não aceitou o modo novo"),Value::Null)); }
    }
    let spawned = super::gateway::launch_if_needed(&policy,&mut target,&sidecar_dir).await.map_err(|error|(error,patch.clone()))?;
    match super::cano::connect(&target.binding).await {
        Ok(connection)=>Ok(Relaunched { target,spawned:spawned.is_some(),connection,patch }),
        Err(error)=>{
            if let Some(cano) = spawned { super::gateway::discard(&policy,&target,&cano,&sidecar_dir).await; }
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
/// Devolve as operações confirmadas e quantas entradas legadas saíram junto.
async fn confirm_inputs(queue:Arc<QueueActor>,receipt:Arc<std::sync::Mutex<ReceiptIndex>>,path:std::path::PathBuf,
    generation:u64,sample:ClockSample) -> Result<(Vec<String>,usize),RuntimeError> {
    let mut state = queue.snapshot().await.map_err(io_failure)?;
    let confirmed_rows:std::collections::BTreeSet<&str> = state.rows.iter().filter(|r|r["confirmed"] == true).filter_map(|r|r["id"].as_str()).collect();
    let candidates:Vec<(String,String,super::receipt::DispatchCursor)> = state.operations.iter()
        .filter(|(_,op)|op.status != Status::Confirmed && matches!(op.payload["kind"].as_str(),Some("input" | "steer")))
        .filter_map(|(id,op)|Some((id.clone(),op.entry_id.clone()?,serde_json::from_value(op.dispatch_cursor.clone()).ok()?)))
        .filter(|(_,entry,_)|!confirmed_rows.contains(entry.as_str())).collect();
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

async fn capture_cursor(target:&RuntimeTarget,view:&Value) -> Result<Value,RuntimeError> {
    let path = target.transcript.clone(); let provider = target.provider.clone(); let conversation = view["conversation"].as_str().unwrap_or("").to_owned();
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
const VOLATILE_VIEW:[&str;9] = ["public_state","alive","iniciando","in_progress","pending","question","deliverable","runtime_counter","turn_id"];

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
            Action::SetRuntimeState { state:json!({"view":latest}) }).await.map_err(io_failure)?;
        saved.durable = Some(durable);
    }
    Ok(())
}

/// Operação que a fila devolve já final nunca volta a ser enviada; sem resposta guardada no
/// formato de RuntimeReply, a disposição sai do status.
fn stored_reply(id:&str,prepared:&Value) -> Option<RuntimeReply> {
    let disposition = match prepared["status"].as_str()? {
        "accepted" | "confirmed"=>Disposition::Accepted, "rejected"=>Disposition::Rejected, _=>return None,
    };
    serde_json::from_value(prepared["result"].clone()).ok()
        .or_else(||Some(RuntimeReply { operation_id:id.into(),disposition,payload:Value::Null }))
}

fn status(disposition:Disposition) -> Status {
    match disposition { Disposition::Accepted=>Status::Accepted,Disposition::Deferred=>Status::Deferred,
        Disposition::Rejected=>Status::Rejected,Disposition::Unknown=>Status::Unknown }
}

fn recover_phase(state:&super::queue::State,phase:&super::queue::Operation) -> bool {
    phase.payload["generation"].as_u64() == Some(state.generation)
        && phase.payload["frame"].is_object() && (matches!(phase.status,Status::Unknown | Status::Dispatching)
        || phase.payload["logical_id"].as_str().and_then(|id|state.operations.get(id))
            .is_some_and(|root|matches!(root.status,Status::Unknown | Status::Dispatching)))
}

/// Falha antes de qualquer escrita no fio: nada chegou à CLI, então a entrada volta para a fila
/// (adiada, como o `deferred` do Python) em vez de ficar incerta e presa para sempre.
fn defer_unwritten(roots:&mut BTreeMap<String,Pending>,attempts:&BTreeMap<String,Attempt>,native:&BTreeSet<String>,id:&str,error:RuntimeError,effects:&mut VecDeque<Effect>) {
    let prefix = format!("{id}:");
    let written = native.contains(id) || attempts.values().any(|attempt|attempt.logical_id == id || attempt.logical_id.starts_with(&prefix));
    let input = roots.get(id).is_some_and(|pending|matches!(pending.command.kind,OperationKind::Input | OperationKind::Steer));
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

    /// Python falso da política: cada conexão leva a próxima resposta do roteiro (`None` derruba a conexão).
    async fn scripted_policy(script:Vec<Option<(u16,Value)>>) -> (std::net::SocketAddr,Arc<AtomicU64>) {
        use tokio::io::{AsyncBufReadExt,AsyncReadExt,AsyncWriteExt,BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        tokio::spawn(async move {
            for reply in script {
                let Ok((stream,_)) = listener.accept().await else { return };
                counter.fetch_add(1,Ordering::SeqCst);
                let mut reader = BufReader::new(stream);
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n" { break; }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                }
                let mut body = vec![0;length]; reader.read_exact(&mut body).await.unwrap();
                let Some((status,data)) = reply else { continue };
                let data = data.to_string();
                let response = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{data}",data.len());
                reader.get_mut().write_all(response.as_bytes()).await.unwrap();
            }
        });
        (address,calls)
    }

    async fn call(script:Vec<Option<(u16,Value)>>,kind:&str) -> (Result<Value,RuntimeError>,u64) {
        let (address,calls) = scripted_policy(script).await;
        let client = PolicyClient::new(address,"secret".into(),"instance".into()).with_ready_retry(Duration::from_millis(10),Duration::from_secs(5));
        let result = client.run_for("key",1,kind,&RequestId::String("phase".into()),json!({"model":"m"}),"phase").await;
        (result,calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn python_not_ready_is_retried_and_a_definite_refusal_fails_at_once() {
        let ok = Some((200,json!({"ok":true,"data":{"updated":true}})));
        // Sem registro da chave ou sem coordenador (503) e conexão caída: o arquivo da sessão espera o Python.
        let (result,calls) = call(vec![Some((503,json!({}))),Some((503,json!({}))),None,ok.clone()],"session.patch_meta").await;
        assert_eq!(result.unwrap()["updated"],true);
        assert_eq!(calls,4);
        // Pedido torto (400): repetir não muda nada.
        let (result,calls) = call(vec![Some((400,json!({}))),ok.clone()],"session.patch_meta").await;
        assert_eq!(result.unwrap_err().code,"policy_refused");
        assert_eq!(calls,1);
        // O serviço respondeu e recusou: definitivo, uma chamada só.
        let (result,calls) = call(vec![Some((200,json!({"ok":false,"error_type":"ValueError"}))),ok.clone()],"session.patch_meta").await;
        assert_eq!(result.unwrap_err().code,"policy_failed");
        assert_eq!(calls,1);
        let (result,calls) = call(vec![Some((500,json!({}))),ok.clone()],"session.patch_meta").await;
        assert_eq!(result.unwrap_err().code,"policy_refused");
        assert_eq!(calls,1);
        // O recado nativo escreve na conversa: não repete nem com o Python subindo.
        let (result,calls) = call(vec![Some((503,json!({}))),ok],"native_message").await;
        assert_eq!(result.unwrap_err().code,"policy_refused");
        assert_eq!(calls,1);
    }

    #[tokio::test]
    async fn mods_call_to_an_actor_that_never_answers_gives_up_with_a_code() {
        // Ator vivo que não lê a caixa: o pedido do app não pode pendurar o registro da sessão.
        let (sender,_inbox) = mpsc::channel(1);
        let handle = RuntimeHandle { sender,task:Arc::new(Mutex::new(None)),closed:Arc::new(AtomicBool::new(false)),
            events:broadcast::channel(1).0,stopped:Arc::new(Mutex::new(None)),key:"key".into() };
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
            events:broadcast::channel(1).0,stopped:Arc::new(Mutex::new(None)),key:"key".into() };
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
