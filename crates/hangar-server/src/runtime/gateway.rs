use super::{actor::{LaunchConfig,PolicyClient,RuntimeActor,RuntimeEngine,RuntimeHandle},cano,process,protocol::*,queue::{Action,QueueActor,State as QueueState,Store,acquire_lease}};
use axum::{Router,body::to_bytes,extract::{ConnectInfo,State,Request},http::StatusCode,
    middleware::{self,Next},response::{IntoResponse,Response,sse::{Event,KeepAlive,Sse}},routing::{get,post}};
use serde::Deserialize;
use serde_json::{Value,json};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration,SystemTime,UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast,Mutex};

#[derive(Clone)]
pub(crate) enum EntryHandle { Headless(RuntimeHandle), Terminal {target:super::terminal::TerminalTarget,handle:super::terminal::TerminalHandle} }
impl EntryHandle {
    pub(crate) async fn snapshot(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h)=>h.snapshot().await,Self::Terminal {handle,..}=>handle.snapshot().await}}
    /// Só o ator sem terminal tem motor próprio; o de terminal não é sessão Codex.
    pub(crate) async fn view(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h)=>h.view().await,Self::Terminal {..}=>Err(failure("runtime_terminal"))}}
    pub(crate) async fn stop(&self)->Result<(),RuntimeError> {match self {Self::Headless(h)=>h.stop().await,Self::Terminal {handle,..}=>handle.stop().await}}
    pub(crate) async fn command(&self,command:RuntimeCommand)->Result<RuntimeReply,RuntimeError> {match self {Self::Headless(h)=>h.command(command).await,Self::Terminal {handle,..}=>handle.command(command).await}}
    pub(crate) async fn queue(&self,id:String,action:Action)->Result<Value,RuntimeError> {match self {Self::Headless(h)=>h.queue(id,action).await,Self::Terminal {handle,..}=>handle.queue(id,action).await}}
    pub(crate) async fn drain(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h)=>h.drain().await,Self::Terminal {handle,..}=>handle.drain().await}}
    pub(crate) async fn confirm(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h)=>h.confirm().await,Self::Terminal {handle,..}=>handle.confirm().await}}
    pub(crate) async fn ensure_projection(&self)->Result<Value,RuntimeError> {match self {Self::Headless(h)=>h.ensure_projection().await,Self::Terminal {handle,..}=>handle.ensure_projection().await}}
}
struct Entry { generation:u64,handle:EntryHandle,lease_path:std::path::PathBuf,name:String,mods_life:u64,provider:String }

/// Entrada aberta de uma sessão, achada por nome, para as rotas de escrita do Rust.
#[allow(dead_code)] // o `handle` é lido pelas rotas de escrita que entram depois
pub struct WriteTarget { pub key:String,pub generation:u64,pub provider:String,pub terminal:bool,pub healthy:bool,pub(crate) handle:EntryHandle }

/// Prazo do fechamento da porta: abaixo do `OP_TIMEOUT_S` (75 s) do transporte no Python.
const INGRESS_CLOSE_WAIT:Duration = Duration::from_secs(60);
pub struct RuntimeRegistry {
    entries:Mutex<BTreeMap<String,Entry>>,
    events:broadcast::Sender<RuntimeEvent>,
    policy:PolicyClient,
    instance:String,
    lifecycle:Mutex<BTreeMap<String,Arc<Mutex<()>>>>,
    revisions:Mutex<BTreeMap<String,Arc<AtomicU64>>>,
    mods:Option<crate::mods::state::Mods>,
    ingress:super::ingress::IngressGates,
    /// Primeira espera da religação do processo que cai (dobra a cada subida seguida).
    respawn_base:Duration,
    /// Último valor da sessão sem terminal por nome, para o feed do hub (`live`).
    live:std::sync::Mutex<BTreeMap<String,LiveSender>>,
}

/// A trava pode demorar a soltar: as tarefas de E/S de um ator que saiu, ou o `LockFileEx` de um
/// Rust que acabou de cair no Windows. Espera até 3 s por ela.
/// Só "trava ocupada" espera; outro erro (pasta sem permissão, disco cheio) responde na hora.
async fn wait_lease(path:&std::path::Path) -> Result<Arc<std::fs::File>,RuntimeError> {
    for attempt in 0..60 {
        if attempt > 0 { tokio::time::sleep(Duration::from_millis(50)).await; }
        let path = path.to_owned();
        match tokio::task::spawn_blocking(move ||acquire_lease(&path)).await.map_err(|_|failure("queue_job"))? {
            Ok(lease)=>return Ok(lease),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock=>{},
            Err(error)=>return Err(RuntimeError::new("runtime_lease",&format!("trava inacessível: {:?}",error.kind()))),
        }
    }
    Err(RuntimeError::new("runtime_lease","trava da sessão presa por outro dono há 3 s"))
}

/// Abre a fila sob a trava e roda `Recover`: entrada que estava em despacho quando o dono anterior
/// parou vira incerta antes de o ator olhar para ela.
async fn open_store(state_path:&std::path::Path,projection_dir:&std::path::Path,key:&str,generation:u64,name:&str,
    lease:Arc<std::fs::File>) -> Result<Store,RuntimeError> {
    let (state_path,projection_dir) = (state_path.to_owned(),projection_dir.to_owned());
    let initial = QueueState::new(key,generation,name,Vec::new());
    static OPENS:AtomicU64 = AtomicU64::new(0);
    let epoch_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0);
    let call_id = format!("open-recover:{}:{}:{}",std::process::id(),epoch_s,OPENS.fetch_add(1,std::sync::atomic::Ordering::Relaxed));
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let mut store = Store::open(&state_path,&projection_dir,initial)?;
        if store.state().generation != generation { return Ok(Err(failure("runtime_generation"))); }
        store.exec(generation,&call_id,ClockSample { monotonic_s:0.0,epoch_s },Action::Recover)?;
        store.ensure_projection()?;
        Ok(Ok(store))
    }).await
        .map_err(|_|failure("queue_job"))?.map_err(|error:std::io::Error|{
            // A frase da recusa da fila é fixa e diz por que a abertura falhou; o resto só pelo tipo.
            let reason = super::queue::refusal(&error).map_or_else(||format!("{:?}",error.kind()),str::to_owned);
            RuntimeError::new("queue_io",&format!("fila recusou: {reason}"))
        })?
}

/// Negação do `_rust_failed` do Python (erro vazio não é erro), exceto `terminal_facts`, que aqui é doente.
fn healthy(terminal:bool,view:&Value)->bool {
    let failed = match &view["error"] {Value::Null=>false,Value::String(code)=>!code.is_empty(),_=>true};
    if terminal { !failed || view["error"]=="receipt_scan" } else { !failed && view["view"]["alive"] != false }
}

fn failure(code:&str) -> RuntimeError { RuntimeError::new(code,"runtime indisponível para esta chave ou geração") }

/// Regra 1 do módulo de processo: cano gravado `Ours` conecta (mesmo sem `versao`); morto, de outro
/// programa ou ausente, sobe outro com o ambiente que o Python calcula agora (nunca guardado).
pub(super) async fn launch_if_needed(policy:&PolicyClient,target:&mut RuntimeTarget,sidecar_dir:&std::path::Path) -> Result<Option<process::Cano>,RuntimeError> {
    let recorded = target.binding.pid;
    if recorded != 0 {
        let key = target.key.clone();
        let state = tokio::task::spawn_blocking(move||process::liveness(recorded,&key)).await.map_err(|_|failure("launch_job"))?;
        if state == process::Liveness::Ours { return Ok(None); }
    }
    let env = launch_policy(policy,target,"launch_env",json!({})).await?;
    if let Some(code) = env["error"].as_str() { return Err(RuntimeError::new(code,"o Python não montou o comando da sessão")); }
    let shape = ||failure("launch_env_shape");
    let program:Vec<String> = serde_json::from_value(env["program"].clone()).map_err(|_|shape())?;
    let vars:BTreeMap<String,String> = serde_json::from_value(env["env"].clone()).map_err(|_|shape())?;
    let cano_extra = match &env["cano_extra"] { Value::Null=>Default::default(),Value::Object(extra)=>extra.clone(),_=>return Err(shape()) };
    let cwd = target.metadata["cwd"].as_str().filter(|cwd|!cwd.is_empty()).ok_or_else(||failure("launch_cwd"))?;
    let spec = process::LaunchSpec { provider:process::Provider::from_str(&target.provider).ok_or_else(||failure("runtime_provider"))?,
        key:target.key.clone(),cwd:cwd.into(),program,env:vars.into_iter().collect(),cano_extra,sidecar_dir:sidecar_dir.to_owned() };
    let cano = match process::spawn(&spec).await {
        Ok(cano)=>cano,
        Err(error)=>{
            // O gravado já não serve (morto ou de outro programa): o arquivo da sessão para de apontá-lo.
            if recorded != 0 && matches!(error,process::ProcessError::NotListening) { clear_cano(policy,target,recorded).await; }
            let detail = match &error { process::ProcessError::Spawn(detail)=>detail.as_str(),_=>"o processo da sessão não subiu" };
            return Err(RuntimeError::new(error.code(),detail));
        }
    };
    let value = serde_json::to_value(&cano).map_err(|_|failure("cano_json"))?;
    if let Err(error) = launch_policy(policy,target,"session.patch_meta",json!({"cano":value})).await {
        // Sem o arquivo da sessão apontando para ele, o processo ficaria sem dono.
        if let Err(stop) = process::kill(&cano,&target.key,sidecar_dir).await {
            tracing::warn!(key=%target.key,code=stop.code(),"cano não gravado não foi encerrado");
        }
        return Err(error);
    }
    target.binding = CanoBinding { pid:cano.pid,escuta:cano.escuta.clone(),token:cano.token.clone(),versao:cano.versao };
    target.metadata["cano"] = value;
    Ok(Some(cano))
}
pub(super) async fn launch_policy(policy:&PolicyClient,target:&RuntimeTarget,kind:&str,payload:Value) -> Result<Value,RuntimeError> {
    let phase = format!("launch:{}",crate::mods::state::random_hex(8));
    policy.run_for(&target.key,target.generation,kind,&RequestId::String(phase.clone()),payload,&phase).await
}
async fn clear_cano(policy:&PolicyClient,target:&RuntimeTarget,pid:u32) {
    if let Err(error) = launch_policy(policy,target,"session.clear_cano",json!({"pid":pid})).await {
        tracing::warn!(key=%target.key,code=%error.code,"cano não saiu do arquivo da sessão");
    }
}
/// O cano subido agora e que não deu conexão: morre e sai do arquivo da sessão.
pub(super) async fn discard(policy:&PolicyClient,target:&RuntimeTarget,cano:&process::Cano,sidecar_dir:&std::path::Path) {
    if let Err(stop) = process::kill(cano,&target.key,sidecar_dir).await {
        tracing::warn!(key=%target.key,code=stop.code(),"cano sem conexão não foi encerrado");
        return;
    }
    clear_cano(policy,target,cano.pid).await;
}

/// Prazo da devolução da janela esticada ao abrir a sessão com terminal (`unstretch`).
const UNSTRETCH_MAX:Duration = Duration::from_secs(2);

/// Interface dos mods da sessão com terminal (fase 3): o Rust é o dono do terminal aqui, então é dono do
/// clique e da faixa dela. Liga a sessão ao `Mods` na vida `life` com um elo novo; o processo é a chave
/// durável mais o pane e a criação dele, que o renomear mantém.
async fn attach_terminal_mods(mods:&crate::mods::state::Mods,target:&super::terminal::TerminalTarget,
    handle:&super::terminal::TerminalHandle,life:u64) {
    // Antes de ligar: nenhum pedido de app pode estar esticando a janela enquanto ela é lida.
    crate::mods::click::unstretch(handle,std::time::Instant::now()+UNSTRETCH_MAX).await;
    let link = crate::mods::terminal::TerminalLink::anchored(target.name.clone(),life,Arc::new(handle.clone()),mods.clone(),
        crate::mods::click::Limits::default(),handle.anchor());
    let process = format!("{}:{}:{}",target.key,target.binding.pane,target.binding.created);
    mods.attach_terminal_keyed(&target.name,&process,target.plugin_key.as_deref(),life,link.clone());
    // O vigia sobe depois de ligar: o `attach_terminal` para o vigia do elo que estava no nome, e se fosse
    // este mesmo elo o derrubaria. No psmux não há vigia (o `watch_notices` recusa no Windows): lá o mínimo
    // volta na operação seguinte, no `prepare` do clique. O mínimo vale já na abertura: a janela pode ter
    // ficado pequena com um terminal que se desligou.
    if !target.binding.windows { link.watch(&target.binding.mux_argv); }
    tokio::spawn(async move { link.floor().await; });
}

impl RuntimeRegistry {
    pub fn new(upstream:SocketAddr,secret:String,instance:String) -> Self {
        Self { entries:Mutex::new(BTreeMap::new()),events:broadcast::channel(1024).0,
            policy:PolicyClient::new(upstream,secret,instance.clone()),instance,lifecycle:Mutex::new(BTreeMap::new()),revisions:Mutex::new(BTreeMap::new()),mods:None,ingress:Default::default(),
            respawn_base:Duration::from_secs(5),live:Default::default() }
    }
    pub fn with_respawn_base(mut self,base:Duration) -> Self { self.respawn_base = base; self }
    /// Interface dos mods: sessão Claude sem terminal aberta aqui vira superfície remota e publica no `Mods`.
    pub fn with_mods(mut self,mods:crate::mods::state::Mods) -> Self { self.mods = Some(mods); self }
    pub fn ingress(&self) -> &super::ingress::IngressGates { &self.ingress }
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> { self.events.subscribe() }
    /// Último valor da sessão `name` para o feed do hub; `None` enquanto ela não está aberta aqui.
    pub fn live(&self,name:&str) -> LiveReceiver { self.live_sender(name).subscribe() }
    /// Canal do nome, criado na primeira procura (feed ou ator, o que vier antes). Canal vazio e sem
    /// receptor sai do mapa aqui: sem isto o mapa guardaria todo nome que já teve chat aberto. Canal
    /// que um ator já segura (cópia fora do mapa) fica, mesmo vazio: o ator só escreve no primeiro passo.
    fn live_sender(&self,name:&str) -> LiveSender {
        let mut map = self.live.lock().unwrap_or_else(|e|e.into_inner());
        map.retain(|key,sender|key == name || sender.receiver_count() > 0 || sender.sender_count() > 1 || sender.borrow().is_some());
        map.entry(name.to_owned()).or_insert_with(||tokio::sync::watch::channel(None).0).clone()
    }
    pub async fn handle(&self,key:&str,generation:u64) -> Result<RuntimeHandle,RuntimeError> {
        match self.entry(key,generation).await? {EntryHandle::Headless(handle)=>Ok(handle),_=>Err(failure("runtime_provider"))}
    }
    async fn entry(&self,key:&str,generation:u64)->Result<EntryHandle,RuntimeError> {
        self.entries.lock().await.get(key).filter(|entry|entry.generation==generation)
            .map(|entry|entry.handle.clone()).ok_or_else(||failure("runtime_binding"))
    }
    /// Abre a sessão no Rust: trava, fila com `Recover`, conexão ao cano e ator. Responde sem
    /// esperar o `initialize`; o ator o faz e drena a fila quando a sessão fica entregável.
    pub async fn open(&self,target:RuntimeTarget) -> Result<Value,RuntimeError> { self.open_inner(target,None,false).await }
    /// `open` que pode subir o processo: sobe só se o cano gravado não for `Ours` (vivo e da chave).
    pub async fn open_with_launch(&self,target:RuntimeTarget,sidecar_dir:std::path::PathBuf) -> Result<Value,RuntimeError> {
        self.open_inner(target,Some(sidecar_dir),true).await
    }
    /// `open` de sessão cujo processo o Rust administra (religar, reiniciar, encerrar) sem subir agora.
    pub async fn open_managed(&self,target:RuntimeTarget,sidecar_dir:std::path::PathBuf) -> Result<Value,RuntimeError> {
        self.open_inner(target,Some(sidecar_dir),false).await
    }
    /// Abertura que falha fica no canal do hub até a próxima abertura (a mesma regra do `close` com
    /// erro): sem isso o chat mostraria a sessão parada. Geração ou provedor errado é pedido torto,
    /// não a sessão falhando.
    async fn open_inner(&self,target:RuntimeTarget,managed:Option<std::path::PathBuf>,spawn:bool) -> Result<Value,RuntimeError> {
        let name = target.name.clone();
        let result = self.open_attempt(target,managed,spawn).await;
        if let Err(error) = &result && !["runtime_generation","runtime_provider"].contains(&error.code.as_str()) {
            super::actor::mark_live_error(&self.live_sender(&name),&error.code,&error.message);
        }
        result
    }
    async fn open_attempt(&self,mut target:RuntimeTarget,managed:Option<std::path::PathBuf>,spawn:bool) -> Result<Value,RuntimeError> {
        let launch = managed.as_ref().filter(|_|spawn);
        let barrier = self.barrier(&target.key).await;
        let _guard = barrier.lock().await;
        let existing = self.entries.lock().await.get(&target.key).map(|entry|(entry.generation,entry.handle.clone()));
        let handle = if let Some((generation,handle)) = existing {
            if generation != target.generation { return Err(failure("runtime_generation")); }
            match handle {EntryHandle::Headless(handle)=>handle,_=>return Err(failure("runtime_provider"))}
        } else {
        if !["claude","codex"].contains(&target.provider.as_str()) || target.binding.versao != 2 { return Err(failure("runtime_provider")); }
        let lease = wait_lease(&target.lease_path).await?;
        let store = open_store(&target.state_path,&target.projection_dir,&target.key,target.generation,&target.name,lease.clone()).await?;
        let mut metadata = target.metadata.clone();
        if let Some(fields) = store.state().runtime_state["view"].as_object() {
            if fields.get("conversation").is_some_and(|conversation|conversation == &metadata[if target.provider == "claude" { "session_id" } else { "thread_id" }]) {
                for (key,value) in fields { if key != "public_state" { metadata[key] = value.clone(); } }
            }
        }
        let queue = QueueActor::start(store,lease);
        // Depois da fila: o `session.patch_meta` do Python lê o estado dela.
        let launched = match &launch {
            Some(sidecar_dir)=>launch_if_needed(&self.policy,&mut target,sidecar_dir).await,
            None=>Ok(None),
        };
        let connection = match &launched {
            Err(error)=>Err(error.clone()),
            Ok(_)=>cano::connect(&target.binding).await,
        };
        let connection = match connection {
            Ok(connection)=>connection,
            Err(error)=>{
                if let Err(stop) = queue.shutdown().await {
                    tracing::warn!(key=%target.key,code=%error.code,stop=?stop.kind(),"fila não fechou depois da falha ao conectar no cano");
                }
                // Só o cano subido nesta chamada morre: um vivo de antes pode estar no meio de um turno.
                if let (Ok(Some(cano)),Some(sidecar_dir)) = (launched,launch) { discard(&self.policy,&target,&cano,sidecar_dir).await; }
                return Err(error);
            },
        };
        let fresh = matches!(launched,Ok(Some(_)));
        let epoch_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs_f64()).unwrap_or(0.0);
        let revision = self.revisions.lock().await.entry(target.key.clone()).or_insert_with(||Arc::new(AtomicU64::new(0))).clone();
        let mut engine = RuntimeEngine::new(&target.provider,metadata,target.generation,ClockSample { monotonic_s:0.0,epoch_s })?
            .with_policy(self.policy.clone()).with_publisher(self.events.clone()).with_revision(revision);
        engine.set_fresh_process(fresh);
        if let Some(sidecar_dir) = &managed { engine = engine.with_launch(LaunchConfig { sidecar_dir:sidecar_dir.clone(),backoff:self.respawn_base }); }
        if let Some(mods) = &self.mods { engine = engine.with_mods(mods.clone()); }
        engine = engine.with_live(self.live_sender(&target.name));
        // Dono único dos pedidos dos apps até o `close` (S9). Só o Claude tem superfície. Registrado antes
        // de a tarefa do ator existir: a primeira faixa publicada já encontra a sessão no `Mods`.
        // A vida no `Mods` é única no servidor; o processo é a chave durável mais o cano, que o renomear mantém.
        let life = engine.mods_life();
        let handle = RuntimeActor::spawn_with(target.clone(),queue,connection,engine,|handle| {
            if target.provider == "claude" && let Some(mods) = &self.mods {
                let process = format!("{}:{}:{}",target.key,target.binding.pid,target.binding.escuta);
                mods.attach_process(&target.name,&process,life,Arc::new(handle.clone()));
            }
        });
        self.entries.lock().await.insert(target.key.clone(),Entry { generation:target.generation,handle:EntryHandle::Headless(handle.clone()),
            lease_path:target.lease_path.clone(),name:target.name.clone(),mods_life:life,provider:target.provider.clone() });
        handle
        };
        let snapshot = match handle.snapshot().await {
            Ok(snapshot) if snapshot["error"].is_null() && snapshot["view"]["alive"] != false=>snapshot,
            result=>{
                // Ator morto ou cano já saído: fecha agora, senão a entrada presa responderia ao próximo `open`.
                let error = match result {
                    Err(error)=>error,
                    Ok(snapshot)=>snapshot["error"].as_str().map_or_else(||failure("cano_exited"),|code|RuntimeError::new(code,"ator do runtime terminou ao abrir")),
                };
                if let Err(close) = self.close_locked(&target.key,target.generation,false).await {
                    tracing::warn!(key=%target.key,code=%close.code,"sessão não fechou depois de abrir com erro");
                }
                return Err(error);
            }
        };
        Ok(json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}))
    }
    pub async fn open_terminal(&self,target:super::terminal::TerminalTarget)->Result<Value,RuntimeError> {
        let barrier=self.barrier(&target.key).await; let _guard=barrier.lock().await;
        let existing=self.entries.lock().await.get(&target.key).map(|e|(e.generation,e.handle.clone(),e.mods_life));
        let handle=match existing {
            Some((generation,EntryHandle::Terminal {target:old,handle},life)) if generation==target.generation && old.binding==target.binding
                && old.transcript==target.transcript && old.state_path==target.state_path && old.projection_dir==target.projection_dir && old.lease_path==target.lease_path=>{
                // Reabertura da mesma vida: se o nome saiu do `Mods`, a sessão volta a ser ligada, na vida da
                // entrada, que é a que o `close` esquece. Nome com outra vida fica com ela: é uma sessão mais
                // nova e viva, e tomá-lo a deixaria sem dono e sem o nome de nascimento.
                if let Some(mods)=&self.mods && life!=0 && mods.life(&target.name).is_none() {
                    attach_terminal_mods(mods,&target,&handle,life).await;
                }
                handle
            },
            Some(_)=>return Err(failure("runtime_generation")),
            None=>{
                let lease=wait_lease(&target.lease_path).await?;
                let store=open_store(&target.state_path,&target.projection_dir,&target.key,target.generation,&target.name,lease.clone()).await?;
                let revision=self.revisions.lock().await.entry(target.key.clone()).or_insert_with(||Arc::new(AtomicU64::new(0))).clone();
                // A âncora nasce com o executor, que a lê, e passa ao elo pelo `handle`, também na reabertura.
                let options=super::terminal::TerminalOptions {anchor:super::terminal::ModsAnchor::default(),..Default::default()};
                let handle=super::terminal::TerminalActor::spawn(target.clone(),QueueActor::start(store,lease),self.policy.clone(),options,self.events.clone(),revision);
                // A vida no `Mods` é única no servidor (`new_life`).
                let life=match &self.mods {
                    Some(mods)=>{
                        let life=mods.new_life();
                        attach_terminal_mods(mods,&target,&handle,life).await;
                        life
                    },
                    None=>0,
                };
                self.entries.lock().await.insert(target.key.clone(),Entry {generation:target.generation,handle:EntryHandle::Terminal {target:target.clone(),handle:handle.clone()},lease_path:target.lease_path.clone(),name:target.name.clone(),mods_life:life,provider:"claude".into()});handle
            }
        };
        let snapshot=handle.snapshot().await?;
        Ok(json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}))
    }
    /// Fecha a sessão no Rust: o ator para e solta a trava; o cano segue vivo.
    pub async fn close(&self,key:&str,generation:u64) -> Result<Value,RuntimeError> {
        let barrier = self.barrier(key).await;
        let _guard = barrier.lock().await;
        self.close_locked(key,generation,false).await
    }
    /// Encerrar a sessão: o ator para e o processo dela morre, com os arquivos do cano. Sem a sessão
    /// aberta aqui não há o que matar (`killed: false`): quem pediu decide.
    pub async fn close_with_kill(&self,key:&str,generation:u64) -> Result<Value,RuntimeError> {
        let barrier = self.barrier(key).await;
        let _guard = barrier.lock().await;
        self.close_locked(key,generation,true).await
    }
    async fn close_locked(&self,key:&str,generation:u64,kill:bool) -> Result<Value,RuntimeError> {
        let (handle,lease_path,name,life) = match self.entries.lock().await.get(key) {
            None=>return Ok(if kill { json!({"closed":true,"killed":false}) } else { json!({"closed":true}) }),
            Some(entry) if entry.generation == generation=>(entry.handle.clone(),entry.lease_path.clone(),entry.name.clone(),entry.mods_life),
            _=>return Err(failure("runtime_generation")),
        };
        let stopped = match (&handle,kill) {
            (EntryHandle::Headless(handle),true)=>handle.stop_killing().await,
            (EntryHandle::Terminal {..},true)=>return Err(failure("close_kill")),
            _=>handle.stop().await,
        };
        if let Err(error) = stopped {
            // `stop` sempre junta a tarefa do ator: se ele saiu por erro, a posse acaba com ele. Sem
            // isto a entrada morta ficava para sempre, a sessão não reabria e o retrato de eventos
            // de todas as sessões caía. Só solta depois de a trava estar livre de fato.
            if wait_lease(&lease_path).await.is_err() {
                tracing::warn!(key,code=%error.code,"ator terminou mas a trava não liberou em 3 s; sessão segue presa");
                return Err(error);
            }
            if kill {
                // O ator parou e soltou a trava: a sessão saiu daqui, só o processo não morreu. Responder
                // erro deixava o Python achando que ela seguia aqui; `killed: false` e quem pediu decide.
                tracing::warn!(key,code=%error.code,"sessão fechada sem encerrar o processo");
                self.entries.lock().await.remove(key);
                self.clear_live(&name).await;
                if let Some(mods) = &self.mods { mods.forget(&name,life); }
                return Ok(json!({"closed":true,"killed":false}));
            }
            tracing::warn!(key,code=%error.code,"ator do runtime já tinha terminado; sessão liberada");
        }
        self.entries.lock().await.remove(key);
        self.clear_live(&name).await;
        // A sessão saiu do Rust: os apps perdem a faixa e os pedidos voltam a não ter dono (S9). Com ou sem
        // terminal, esquece só esta vida: outra sessão que tenha tomado o nome (outra vida) fica.
        if let Some(mods) = &self.mods { mods.forget(&name,life); }
        Ok(if kill { json!({"closed":true,"killed":true}) } else { json!({"closed":true}) })
    }
    /// Sessão fora do Rust: o feed mostra a parada (`None`), e o canal sem receptor sai do mapa.
    async fn clear_live(&self,name:&str) {
        // Outra vida com o mesmo nome (reaberta antes deste fechamento) segue dona do canal.
        if self.entries.lock().await.values().any(|entry|entry.name == name) { return; }
        let mut map = self.live.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(sender) = map.get(name) {
            // Vida que acabou com erro: o problema fica até a próxima abertura, que escreve por cima.
            // ponytail: o canal com erro de sessão apagada fica no mapa; um por nome, sem crescer.
            if sender.borrow().as_ref().is_some_and(|state|state.error.is_some()) { return; }
            sender.send_replace(None);
            if sender.receiver_count() == 0 { map.remove(name); }
        }
    }
    async fn barrier(&self,key:&str) -> Arc<Mutex<()>> {
        self.lifecycle.lock().await.entry(key.into()).or_insert_with(||Arc::new(Mutex::new(()))).clone()
    }
    /// Chave do ator de entrada terminal da sessão `name` e o retrato dele; `None` sem ator aberto.
    pub async fn terminal_view(&self,name:&str) -> Option<(String,Value)> {
        let found = self.entries.lock().await.iter().find_map(|(key,entry)| match &entry.handle {
            EntryHandle::Terminal {target,handle} if target.name==name=>Some((key.clone(),handle.clone())),
            _=>None,
        });
        let (key,handle) = found?;
        match handle.snapshot().await {
            Ok(view)=>Some((key,view)),
            Err(error)=>Some((key,json!({"error":error.code,"view":{}}))),
        }
    }
    /// Entrada aberta da sessão `name`, para escrever. Chamar depois do `ingress().enter`: a procurada
    /// antes de esperar a porta pode ser a que o relançamento parou.
    /// `healthy` é a negação do `_rust_failed` do Python; `terminal_facts` conta como doente aqui
    /// (vínculo trocado, só o `prepare_session` refaz), `receipt_scan` não.
    pub async fn writable(&self,name:&str) -> Option<WriteTarget> {
        // Nome repetido é defeito de quem abriu a entrada; a escrita vai para a geração mais nova.
        let (key,generation,provider,handle) = self.entries.lock().await.iter().filter(|(_,e)|e.name==name).max_by_key(|(_,e)|e.generation)
            .map(|(key,e)|(key.clone(),e.generation,e.provider.clone(),e.handle.clone()))?;
        let terminal = matches!(&handle,EntryHandle::Terminal {..});
        let healthy = match tokio::time::timeout(Duration::from_secs(1),handle.snapshot()).await {
            Ok(Ok(view))=>healthy(terminal,&view),
            _=>false,
        };
        Some(WriteTarget { key,generation,provider,terminal,healthy,handle })
    }
    /// A entrada mais nova do nome é de terminal? Sem retrato: serve a decisão que vem antes da porta.
    pub async fn is_terminal(&self,name:&str) -> bool {
        self.entries.lock().await.values().filter(|e|e.name==name).max_by_key(|e|e.generation)
            .is_some_and(|e|matches!(e.handle,EntryHandle::Terminal {..}))
    }
    /// Sessão do ator de entrada terminal de chave `key`.
    pub async fn terminal_name(&self,key:&str) -> Option<String> {
        match &self.entries.lock().await.get(key)?.handle { EntryHandle::Terminal {target,..}=>Some(target.name.clone()), EntryHandle::Headless(_)=>None }
    }
    pub async fn snapshots(&self) -> Result<Vec<RuntimeEvent>,RuntimeError> {
        let entries:Vec<_> = self.entries.lock().await.iter().map(|(key,entry)|(key.clone(),entry.generation,entry.handle.clone())).collect();
        let mut output = Vec::new();
        for (key,generation,handle) in entries {
            // Uma sessão sem ator não tira o retrato das outras: ela fica de fora até ser liberada.
            match handle.snapshot().await {
                Ok(data)=>output.push(RuntimeEvent { key,generation,revision:data["revision"].as_u64().unwrap_or(0),channel:"snapshot".into(),data }),
                Err(error)=>if crate::warn_limit::allow(Some(&key),&error.code) { tracing::warn!(key,code=%error.code,"sessão fora do retrato inicial dos eventos") },
            }
        }
        Ok(output)
    }
    /// Retrato de todas as sessões, por chave, para a lista. Diferente de `snapshots`, a sessão cujo
    /// ator não respondeu fica com `{"error": código}`: fora do retrato ela pareceria parada.
    pub async fn list_snapshots(&self) -> BTreeMap<String,Value> {
        let entries:Vec<_> = self.entries.lock().await.iter().map(|(key,entry)|(key.clone(),entry.handle.clone())).collect();
        let asks = entries.into_iter().map(|(key,handle)| async move {
            let data = match tokio::time::timeout(Duration::from_secs(1),handle.snapshot()).await {
                Ok(Ok(data))=>data,
                Ok(Err(error))=>json!({"error":error.code}),
                Err(_)=>json!({"error":"runtime_snapshot_timeout"}),
            };
            (key,data)
        });
        futures_util::future::join_all(asks).await.into_iter().collect()
    }
    pub async fn shutdown(&self) -> Result<(),RuntimeError> {
        let entries:Vec<_> = self.entries.lock().await.iter().map(|(key,entry)|(key.clone(),entry.generation)).collect();
        for (key,generation) in entries { self.close(&key,generation).await?; }
        Ok(())
    }
}

#[derive(Clone)]
struct Gateway { registry:Arc<RuntimeRegistry>,secret:String,instance:String,protocol:u32 }

pub fn startup_line(protocol:u32,instance:&str,port:u16) -> String {
    json!({"type":"runtime_ready","protocol":protocol,"instance":instance,"port":port}).to_string()
}

pub async fn serve(listener:tokio::net::TcpListener,registry:Arc<RuntimeRegistry>,secret:String,instance:String,protocol:u32) -> std::io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() { return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"IPC somente no loopback")); }
    let state = Gateway { registry,secret,instance,protocol };
    let router = Router::new().route("/runtime/op",post(operation)).route("/runtime/events",get(events))
        .layer(middleware::from_fn_with_state(state.clone(),authorize)).with_state(state);
    axum::serve(axum::serve::ListenerExt::tap_io(listener,crate::nodelay),router.into_make_service_with_connect_info::<SocketAddr>()).await
}

async fn authorize(State(state):State<Gateway>,request:Request,next:Next) -> Response {
    let local = request.extensions().get::<ConnectInfo<SocketAddr>>().is_some_and(|info|info.0.ip().is_loopback());
    let secret = request.headers().get("x-hangar-internal").map(|h|h.as_bytes()).unwrap_or(&[]);
    let instance = request.headers().get("x-hangar-runtime-instance").map(|h|h.as_bytes()).unwrap_or(&[]);
    if !local || secret.ct_eq(state.secret.as_bytes()).unwrap_u8() != 1 || instance.ct_eq(state.instance.as_bytes()).unwrap_u8() != 1 {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    protocol:u32,instance:String,key:String,generation:u64,operation_id:String,clock:ClockSample,command:Value,
}

async fn operation(State(state):State<Gateway>,request:Request) -> Response {
    let body = match to_bytes(request.into_body(),MAX_ENVELOPE).await { Ok(body)=>body,Err(_)=>return refuse(StatusCode::PAYLOAD_TOO_LARGE,None,"body") };
    let envelope:Envelope = match serde_json::from_slice(&body) { Ok(envelope)=>envelope,Err(_)=>return refuse(StatusCode::BAD_REQUEST,None,"envelope") };
    let _ = envelope.clock;
    let mismatch = if envelope.protocol != state.protocol { Some("protocol") } else if envelope.instance != state.instance { Some("instance") }
        else if envelope.operation_id.is_empty() { Some("operation_id") } else { None };
    if let Some(check) = mismatch { return refuse(StatusCode::CONFLICT,Some(&envelope.key),check); }
    let result = dispatch(&state.registry,&envelope).await;
    match result {
        Ok(result)=>json_response(StatusCode::OK,json!({"ok":true,"result":result})),
        Err(error)=>{
            if crate::warn_limit::allow(Some(&envelope.key),&error.code) {
                // Só o nome vem do descritor: o resto dele é caminho e credencial do cano.
                let name = envelope.command["descriptor"]["name"].as_str().unwrap_or("");
                tracing::warn!(key=%envelope.key,session=%name,kind=%envelope.command["kind"].as_str().unwrap_or("?"),
                    code=%error.code,reason=%error.message,"runtime recusou operação");
            }
            json_response(StatusCode::SERVICE_UNAVAILABLE,json!({"ok":false,"error_code":error.code,"message":error.message}))
        }
    }
}

/// Recusa antes do despacho: diz qual conferência falhou, sem o corpo.
fn refuse(status:StatusCode,key:Option<&str>,check:&'static str) -> Response {
    if crate::warn_limit::allow(key,check) {
        tracing::warn!(status=status.as_u16(),key=%key.unwrap_or(""),check=%check,"runtime recusou envelope");
    }
    status.into_response()
}

fn json_response(status:StatusCode,value:Value) -> Response {
    (status,[("content-type","application/json")],value.to_string()).into_response()
}

async fn dispatch(registry:&RuntimeRegistry,envelope:&Envelope) -> Result<Value,RuntimeError> {
    let command = &envelope.command;
    let kind = command["kind"].as_str().ok_or_else(||failure("command_kind"))?;
    let fields:&[&str] = match kind {
        "open"=>&["kind","descriptor","launch"],
        "submit"=>&["kind","text","steer","pre_transcript"],
        "control"=>&["kind","control","payload"],
        "queue"=>&["kind","action"],
        "ingress"=>&["kind","name","closed","held"],
        "close"=>&["kind","kill"],
        "snapshot" | "drain" | "confirm" | "ensure_projection"=>&["kind"],
        _=>return Err(failure("command_kind")),
    };
    if !command.as_object().is_some_and(|object|object.keys().all(|key|fields.contains(&key.as_str()))) {
        return Err(failure("command_fields"));
    }
    if kind == "ingress" {
        let name = command["name"].as_str().filter(|n|!n.is_empty()).ok_or_else(||failure("ingress_payload"))?;
        let closed = command["closed"].as_bool().ok_or_else(||failure("ingress_payload"))?;
        // `held`: fechamento da troca de conversa; a reabertura correspondente leva o mesmo `held`.
        let held = match &command["held"] { Value::Null=>false, value=>value.as_bool().ok_or_else(||failure("ingress_payload"))? };
        let gates = registry.ingress();
        match (closed,held) {
            (true,false)=>gates.close(name,INGRESS_CLOSE_WAIT).await.map_err(|_|failure("ingress_busy"))?,
            (true,true)=>gates.hold(name,INGRESS_CLOSE_WAIT).await.map_err(|_|failure("ingress_busy"))?,
            (false,false)=>gates.open(name),
            (false,true)=>gates.release(name),
        }
        return Ok(json!({"closed":closed}));
    }
    if kind == "open" {
        // `launch`: quem abre pode subir o processo (só sem terminal, e só se o gravado não for nosso).
        let launch = match &command["launch"] { Value::Null=>false, value=>value.as_bool().ok_or_else(||failure("open_launch"))? };
        return match descriptor(&command["descriptor"],launch)? {
            Target::Headless(target,sidecar_dir) if launch=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                let sidecar_dir = sidecar_dir.filter(|dir|!dir.as_os_str().is_empty()).ok_or_else(||failure("launch_sidecar_dir"))?;
                registry.open_with_launch(target,sidecar_dir).await
            },
            // Codex sem terminal com a pasta do arquivo: o Rust administra o processo mesmo sem subir agora.
            Target::Headless(target,Some(sidecar_dir)) if target.provider == "codex" && !sidecar_dir.as_os_str().is_empty()=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                registry.open_managed(target,sidecar_dir).await
            },
            Target::Headless(target,_)=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                registry.open(target).await
            },
            Target::Terminal(_) if launch=>Err(failure("open_launch")),
            Target::Terminal(target)=>{
                if target.key!=envelope.key || target.generation!=envelope.generation{return Err(failure("runtime_binding"));}
                registry.open_terminal(target).await
            }
        };
    }
    if kind == "close" {
        let kill = match &command["kill"] { Value::Null=>false, value=>value.as_bool().ok_or_else(||failure("close_kill"))? };
        return if kill { registry.close_with_kill(&envelope.key,envelope.generation).await } else { registry.close(&envelope.key,envelope.generation).await };
    }
    let handle = registry.entry(&envelope.key,envelope.generation).await?;
    match kind {
        "submit"=> {
            // Só envios: as consultas que o Python repete (confirm, snapshot) inflariam a conta.
            crate::migration_status::count_private(if matches!(&handle,EntryHandle::Terminal {..}) {"send_terminal"} else {"send_headless"});
            if matches!(&handle,EntryHandle::Terminal {..}) && (command.get("steer").is_some_and(|value|!value.is_boolean())
                || command.get("pre_transcript").is_some_and(|value|!value.is_boolean())) {return Err(failure("terminal_payload"));}
            let kind = if command["steer"] == true && matches!(&handle,EntryHandle::Headless(_)) { OperationKind::Steer } else { OperationKind::Input };
            let reply = handle.command(RuntimeCommand { operation_id:envelope.operation_id.clone(),kind,
                payload:json!({"text":command["text"],"pre_transcript":command["pre_transcript"].as_bool().unwrap_or(false)}) }).await?;
            serde_json::to_value(reply).map_err(|_|failure("reply_json"))
        }
        "control"=> {
            if let EntryHandle::Terminal {handle,..}=&handle {
                let control=command["control"].as_str().ok_or_else(||failure("control_kind"))?;
                let reply=handle.control(envelope.operation_id.clone(),control.into(),command["payload"].clone()).await?;
                return serde_json::to_value(reply).map_err(|_|failure("reply_json"));
            }
            let control:OperationKind = serde_json::from_value(command["control"].clone()).map_err(|_|failure("control_kind"))?;
            let reply = handle.command(RuntimeCommand { operation_id:envelope.operation_id.clone(),kind:control,payload:command["payload"].clone() }).await?;
            serde_json::to_value(reply).map_err(|_|failure("reply_json"))
        }
        "queue"=>handle.queue(envelope.operation_id.clone(),serde_json::from_value::<Action>(command["action"].clone()).map_err(|_|failure("queue_action"))?).await,
        "snapshot"=>handle.snapshot().await,
        "drain"=>handle.drain().await,
        "confirm"=>handle.confirm().await,
        "ensure_projection"=>handle.ensure_projection().await,
        _=>Err(failure("command_kind")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    name:String,key:String,provider:String,headless:bool,meta:Value,jsonl:String,
    projection_dir:std::path::PathBuf,state_path:std::path::PathBuf,lock_path:std::path::PathBuf,generation:u64,
    /// Pasta do arquivo da sessão, onde o cano subido pelo Rust põe socket e log; só no `open` com `launch`.
    #[serde(default)] sidecar_dir:Option<std::path::PathBuf>,
}

/// Sem terminal leva a pasta do arquivo da sessão (`sidecar_dir`), que só o `open` com `launch` usa.
enum Target {Headless(RuntimeTarget,Option<std::path::PathBuf>),Terminal(super::terminal::TerminalTarget)}
fn descriptor(value:&Value,launch:bool) -> Result<Target,RuntimeError> {
    let descriptor:Descriptor = serde_json::from_value(value.clone()).map_err(|_|failure("descriptor_shape"))?;
    if descriptor.meta["key"] != descriptor.key || descriptor.key.is_empty() { return Err(failure("descriptor_binding")); }
    if !descriptor.headless {
        if descriptor.provider!="claude" || descriptor.meta.get("cano").is_some(){return Err(failure("descriptor_provider"));}
        let binding:crate::terminal_input::TerminalBinding=serde_json::from_value(descriptor.meta["terminal"].clone()).map_err(|_|failure("terminal_binding"))?;
        if binding.generation!=descriptor.generation || binding.name!=descriptor.name || binding.conversation.is_empty() || binding.mux_argv.is_empty()
            || binding.mux_argv.iter().any(|v|v.contains('\0')) || descriptor.jsonl.is_empty()
            || (!binding.windows && (!binding.pane.starts_with('%') || binding.pane[1..].parse::<u64>().is_err()))
            || (binding.windows && !binding.pane.starts_with(&format!("={}:",binding.name))) {return Err(failure("terminal_binding"));}
        return Ok(Target::Terminal(super::terminal::TerminalTarget {key:descriptor.key,generation:descriptor.generation,name:descriptor.name,
            created:descriptor.meta["created"].as_f64().unwrap_or(binding.created as f64),binding,
            plugin_key:descriptor.meta["plugin_key"].as_str().filter(|key|!key.is_empty()).map(str::to_owned),
            lease_path:descriptor.lock_path,state_path:descriptor.state_path,projection_dir:descriptor.projection_dir,transcript:descriptor.jsonl.into()}));
    }
    if descriptor.meta.get("terminal").is_some(){return Err(failure("descriptor_provider"));}
    let cano = &descriptor.meta["cano"];
    // Subida pedida sem cano gravado: pid 0 é "nenhum", e a subida grava o novo antes de conectar.
    let binding = if launch && cano["pid"].as_u64().is_none_or(|pid|pid == 0) { CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 } }
    else if launch { CanoBinding { pid:cano["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok()).ok_or_else(||failure("cano_pid"))?,
        escuta:cano["escuta"].as_str().unwrap_or_default().into(),token:cano["token"].as_str().unwrap_or_default().into(),
        // Cano vivo da sessão sem `versao` gravada já fala a 2.
        versao:cano["versao"].as_u64().and_then(|version|u32::try_from(version).ok()).unwrap_or(2) } }
    else { CanoBinding { pid:cano["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok()).ok_or_else(||failure("cano_pid"))?,
        escuta:cano["escuta"].as_str().ok_or_else(||failure("cano_address"))?.into(),
        token:cano["token"].as_str().ok_or_else(||failure("cano_token"))?.into(),
        // O Codex subido pelo Python grava o `cano` antes de conectar, sem `versao`, e já fala a 2.
        versao:match cano["versao"].as_u64().and_then(|version|u32::try_from(version).ok()) {
            Some(version)=>version, None if descriptor.provider == "codex"=>2, None=>return Err(failure("cano_version")) } } };
    Ok(Target::Headless(RuntimeTarget { key:descriptor.key,generation:descriptor.generation,name:descriptor.name,provider:descriptor.provider,
        created:descriptor.meta["created"].as_f64().unwrap_or(0.0),metadata:descriptor.meta,binding,
        lease_path:descriptor.lock_path,state_path:descriptor.state_path,projection_dir:descriptor.projection_dir,transcript:descriptor.jsonl.into() },
        descriptor.sidecar_dir))
}

async fn events(State(state):State<Gateway>) -> Response {
    let receiver = state.registry.subscribe();
    let initial = match state.registry.snapshots().await { Ok(events)=>std::collections::VecDeque::from(events),Err(error)=>{
        tracing::warn!(code=%error.code,reason=%error.message,"runtime sem retrato inicial dos eventos");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    } };
    let stream = futures_util::stream::unfold((receiver,initial),| (mut receiver,mut initial) | async move {
        let event = if let Some(event) = initial.pop_front() { event } else {
            match receiver.recv().await { Ok(event)=>event,Err(_)=>return None }
        };
        Some((Ok::<_,Infallible>(Event::default().event("runtime").data(serde_json::to_string(&event).unwrap())),(receiver,initial)))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_follows_python_rust_failed() {
        assert!(!healthy(true,&json!({"error":"terminal_facts","view":{}})));
        assert!(healthy(true,&json!({"error":"receipt_scan","view":{}})));
        assert!(!healthy(true,&json!({"error":"queue_io","view":{}})));
        assert!(healthy(true,&json!({"error":null,"view":{}})));
        assert!(!healthy(false,&json!({"error":null,"view":{"alive":false}})));
        assert!(!healthy(false,&json!({"error":"cano_exited","view":{"alive":true}})));
        assert!(healthy(false,&json!({"error":null,"view":{"alive":true}})));
        assert!(healthy(false,&json!({"error":"","view":{}})));
    }

    #[test]
    fn terminal_and_headless_sources_do_not_overlap() {
        let value = json!({"name":"session","key":"key","provider":"codex","headless":false,
            "meta":{"key":"key","cano":{"pid":42,"escuta":"tcp:127.0.0.1:1","token":"test","versao":2}},
            "jsonl":"chat.jsonl","projection_dir":"projection","state_path":"state","lock_path":"lock","generation":1});
        assert!(descriptor(&value,false).is_err());
        let mut headless = value;
        headless["headless"] = json!(true);
        assert!(descriptor(&headless,false).is_ok());
    }

    #[test]
    fn launch_accepts_a_session_without_cano() {
        let value = json!({"name":"session","key":"key","provider":"codex","headless":true,"meta":{"key":"key","cano":null},
            "jsonl":"","projection_dir":"projection","state_path":"state","lock_path":"lock","generation":1,"sidecar_dir":"dir"});
        assert!(descriptor(&value,false).is_err(),"sem `launch`, sem cano não abre");
        let Ok(Target::Headless(target,sidecar_dir)) = descriptor(&value,true) else { panic!("subida sem cano gravado") };
        assert_eq!((target.binding.pid,target.binding.versao),(0,2));
        assert_eq!(sidecar_dir.as_deref(),Some(std::path::Path::new("dir")));
    }

    #[test]
    fn held_empty_live_channel_survives_another_lookup() {
        // O `open` pega o canal e o ator só escreve no primeiro passo: outra procura no meio não o apaga.
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let held = registry.live_sender("a");
        let _other = registry.live_sender("b");
        held.send_replace(Some(Arc::new(LiveState::default())));
        assert!(registry.live("a").borrow().is_some(),"o feed lê o mesmo canal que o ator segura");
        drop(held);
        registry.live_sender("a").send_replace(None);
        let _ = registry.live_sender("c");
        assert!(!registry.live.lock().unwrap().contains_key("a"),"vazio, sem dono e sem receptor: sai");
    }

    #[tokio::test]
    async fn failed_codex_open_shows_its_error_until_the_next_open() {
        let dir = tempfile::tempdir().unwrap();
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let rx = registry.live("cx");
        // Cano gravado que não atende: a conexão falha.
        let target = RuntimeTarget { key:"k".into(),generation:1,name:"cx".into(),provider:"codex".into(),
            metadata:json!({"name":"cx","key":"k","headless":true}),
            binding:CanoBinding { pid:42,escuta:"tcp:127.0.0.1:9".into(),token:"t".into(),versao:2 },
            lease_path:dir.path().join("q.lock"),state_path:dir.path().join("q.json"),projection_dir:dir.path().join("projection"),
            transcript:dir.path().join("rollout.jsonl"),created:0.0 };
        let error = registry.open(target.clone()).await.unwrap_err();
        assert_eq!(rx.borrow().as_ref().and_then(|s|s.error.clone()).map(|(code,_)|code),Some(error.code.clone()));
        // Pedido torto (provedor de fora) não acende problema.
        registry.live_sender("cx").send_replace(None);
        let other = RuntimeTarget { provider:"pi".into(),..target };
        assert_eq!(registry.open(other).await.unwrap_err().code,"runtime_provider");
        assert!(rx.borrow().is_none());
    }

    #[tokio::test]
    async fn close_keeps_the_error_of_a_life_that_failed() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let rx = registry.live("a");
        let failed = LiveState { error:Some(("queue_io".into(),"fila recusou".into())),..Default::default() };
        registry.live_sender("a").send_replace(Some(Arc::new(failed)));
        registry.clear_live("a").await;
        assert!(rx.borrow().as_ref().is_some_and(|s|s.error.is_some()),"o hub segue mostrando a falha depois do close");
        registry.live_sender("a").send_replace(Some(Arc::new(LiveState::default())));
        registry.clear_live("a").await;
        assert!(rx.borrow().is_none(),"vida que acabou bem: o canal esvazia");
    }

    #[tokio::test]
    async fn one_sse_reader_multiple_keys() {
        let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test".into(),"instance".into());
        let mut receiver = registry.subscribe();
        assert_eq!(registry.events.receiver_count(),1);
        for key in ["claude-key","codex-key"] {
            assert!(registry.events.send(RuntimeEvent { key:key.into(),generation:1,revision:1,channel:"state".into(),data:json!({}) }).is_ok());
        }
        assert_eq!(receiver.recv().await.unwrap().key,"claude-key");
        assert_eq!(receiver.recv().await.unwrap().key,"codex-key");
    }
}
