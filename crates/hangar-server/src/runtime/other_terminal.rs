//! Um proprietário por vida para entrada, observação e fila de Pi, omp e Kimi.
use std::path::PathBuf;
mod io;
pub use io::ProcessOtherIo;
mod native;
mod observe;
mod storage;
pub use storage::open_queue;
pub use observe::{PendingInteraction, approval_open as kimi_approval_open, picker_open as kimi_picker_open, menu as menu_interaction};

pub fn read_pending_interaction(target:&OtherTarget)->Result<Option<PendingInteraction>,RuntimeError> {observe::pending(target)}
use crate::terminal_input::TerminalBinding;

#[derive(Clone, Debug)]
pub struct OtherTarget {
    pub key: String,
    pub generation: u64,
    pub name: String,
    pub provider: hangar_transcript::Provider,
    pub binding: TerminalBinding,
    pub pane_address: String,
    pub dirs: crate::list::discover_other::Dirs,
    pub cwd: String,
    pub pane_pid: i64,
    pub agent_pid: i64,
    pub agent_birth: f64,
    pub transcript: PathBuf,
    pub state_path: PathBuf,
    pub lease_path: PathBuf,
    pub projection_dir: PathBuf,
    pub config_dirs: Vec<PathBuf>,
    pub created: f64,
}
/// Identidade pública opaca, distinta para cada geração da mesma chave.
pub fn lifecycle_id(key:&str,generation:u64)->String {
    format!("{}:g:{generation}",crate::list::links::session_life(Some(key),None).unwrap_or_default())
}
impl OtherTarget {
    pub fn lifecycle_id(&self)->String {lifecycle_id(&self.key,self.generation)}
    pub fn same_life(&self, other: &Self) -> bool {
        self.key == other.key && self.generation == other.generation && self.name == other.name
            && self.provider == other.provider && self.binding == other.binding
            && self.agent_pid == other.agent_pid && self.agent_birth == other.agent_birth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target() -> OtherTarget {
        OtherTarget {key:"life-a".into(),generation:1,name:"pi-a".into(),provider:hangar_transcript::Provider::Pi,
            binding:TerminalBinding {name:"pi-a".into(),pane:"%1".into(),conversation:"conversation-a".into(),generation:1,created:100,
                mux_argv:vec!["tmux".into()],windows:false,clipboard_lock_path:None},pane_address:"%1".into(),dirs:crate::list::discover_other::Dirs {home:"home".into(),claude:"home/.claude".into(),codex_home:"home/.codex".into(),pi_sessions:"home/.pi/agent/sessions".into(),omp_config:"home/.omp".into(),omp_agent:"home/.omp/agent".into(),kimi_home:"home/.kimi-code".into()},cwd:"project".into(),pane_pid:123,agent_pid:123,
            agent_birth:100.5,transcript:"conversation-a.jsonl".into(),state_path:"state.json".into(),lease_path:"state.lock".into(),projection_dir:"queue".into(),config_dirs:vec![],created:100.0}
    }
    #[test]
    fn other_terminal_same_life_requires_the_process_birth_and_conversation() {
        let original=target();
        assert!(original.same_life(&target()));
        let mut reused=target(); reused.agent_birth=101.0;
        assert!(!original.same_life(&reused));
        reused=target();reused.binding.conversation="conversation-b".into();
        assert!(!original.same_life(&reused));
        reused=target();reused.generation=2;
        assert!(!original.same_life(&reused));
        reused=target();reused.provider=hangar_transcript::Provider::Kimi;
        assert!(!original.same_life(&reused));
    }
}

use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, Mutex};
use super::{protocol::{LiveSender, RuntimeCommand, RuntimeError, RuntimeReply, Disposition}, queue::{Action, QueueActor}};

#[derive(Clone, Debug)]
pub struct OtherOpening {
    pub spec: crate::session_open::SessionSpec,
    pub binding: TerminalBinding,
    pub pane_address: String,
    pub projection_dir: PathBuf,
}
impl OtherOpening {
    pub fn lifecycle_id(&self)->String {lifecycle_id(&self.spec.key,self.binding.generation)}
}
#[derive(Clone, Debug)]
pub enum OtherDiscoveryState { Ready(OtherTarget), Opening(OtherOpening) }
pub type OtherDiscovery = Arc<dyn Fn(String) -> BoxFuture<'static, Option<Result<OtherDiscoveryState, RuntimeError>>> + Send + Sync>;
pub struct OtherPreviewContext<'a> {
    pub target: &'a OtherTarget,
    pub state: &'a hangar_api::state::StateEvent,
    pub frame: Option<&'a str>,
    pub files: &'a [crate::state::preview::HookFile],
    pub marker: Option<(&'a str, f64)>,
    pub now: f64,
}
pub trait OtherPreviewSource: Send + Sync {
    fn interested(&self, target: &OtherTarget) -> bool;
    fn update(&self, context: OtherPreviewContext<'_>) -> Result<Option<hangar_api::preview::PreviewEvent>, RuntimeError>;
    fn forget(&self, target: &OtherTarget);
}
#[derive(Clone, Debug)]
pub struct OtherFrame {
    pub text: String,
    pub typed: Option<String>,
    pub styled: Option<String>,
    pub editor: Option<String>,
}
impl OtherFrame {
    pub fn from_styled(styled:String)->Self {
        Self {text:crate::terminal_input::unstyle(&styled,true),typed:Some(crate::terminal_input::unstyle(&styled,false)),styled:Some(styled),editor:None}
    }
}
#[derive(Clone, Debug)]
pub struct OtherWriteGuard {
    pub target: crate::interactions::InteractionTarget,
    pub semantic: Value,
}
/// Fronteira de E/S: testes não substituem fila, interações nem regras do próprio runtime.
pub trait OtherIo: Send + Sync {
    fn prove<'a>(&'a self, target: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>>;
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>>;
    fn write<'a>(&'a self, target: &'a OtherTarget, value: &'a str, literal: bool, guard: Option<&'a OtherWriteGuard>) -> BoxFuture<'a, Result<(), RuntimeError>>;
    fn release<'a>(&'a self) -> BoxFuture<'a, ()>;
}
#[derive(Clone)]
pub struct OtherOptions {
    pub pool: crate::terminal_control::TerminalPool,
    pub inbox: Arc<crate::provider_models::inbox::Store>,
    pub processes: Arc<dyn crate::list::procs::ProcessView>,
    pub interactions: Arc<crate::interactions::InteractionStore>,
    pub preview: Option<Arc<dyn OtherPreviewSource>>,
    pub tick: Duration,
    pub io_override: Option<Arc<dyn OtherIo>>,
}
impl OtherOptions {
    pub fn new(pool: crate::terminal_control::TerminalPool, inbox: Arc<crate::provider_models::inbox::Store>, processes: Arc<dyn crate::list::procs::ProcessView>, interactions: Arc<crate::interactions::InteractionStore>) -> Self {
        Self { pool, inbox, processes, interactions, preview: None, tick: Duration::from_millis(500), io_override: None }
    }
}
fn failed(code: &str) -> RuntimeError { RuntimeError::new(code, "não foi possível comprovar a operação desta vida do terminal") }
type Response = oneshot::Sender<Result<RuntimeReply, RuntimeError>>;
enum Message {
    Control { id: String, kind: String, payload: Value, response: Response },
    Queue { id: String, action: Action, response: oneshot::Sender<Result<Value, RuntimeError>> },
    Query { kind: String, response: oneshot::Sender<Result<Value, RuntimeError>> },
    Stop(oneshot::Sender<Result<(), RuntimeError>>),
}
#[derive(Clone)]
pub struct OtherHandle {
    tx: mpsc::Sender<Message>,
    closed: Arc<AtomicBool>,
    task: Arc<Mutex<Option<tokio::task::JoinHandle<Result<(), RuntimeError>>>>>,
    snapshot: tokio::sync::watch::Receiver<Value>,
    frame: tokio::sync::watch::Receiver<Option<Arc<OtherFrame>>>,
}
impl OtherHandle {
    pub fn spawn(target: OtherTarget, queue: QueueActor, options: OtherOptions, live: LiveSender) -> Self {
        let (tx,mut rx)=mpsc::channel(64);
        let closed=Arc::new(AtomicBool::new(false));let actor_closed=closed.clone();
        let (snapshots,snapshot)=tokio::sync::watch::channel(snapshot_value(&target,&queue.initial_state().rows,None));
        let io=options.io_override.clone().unwrap_or_else(||Arc::new(io::ProcessOtherIo::new(&target,&options)));
        let (frames,frame)=tokio::sync::watch::channel(None);
        let task=tokio::spawn(async move {
            let mut observer=Observer::new(&target,&options)?;
            let mut tick=tokio::time::interval(options.tick);tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let message=tokio::select! {
                    message=rx.recv()=>message,
                    _=tick.tick()=>{
                        if let Err(error)=observer.round(&target,&options,&io,&live,&snapshots,&frames).await {
                            tracing::warn!(code=%error.code,"observação sem prova; conserva o último retrato");
                        }
                        if let Err(error)=native::drain(&target,&queue,&options,&io,&snapshots).await {tracing::warn!(code=%error.code,"drenagem nativa espera prova; não redigita");}
                        continue;
                    }
                };
                let Some(message)=message else {break;};
                match message {
                    Message::Control {id,kind,payload,response}=>{
                        let result=if kind=="input" && target.provider==hangar_transcript::Provider::Kimi {
                            Err(failed("other_terminal_physical_delivery_unavailable"))
                        }else if kind=="input" {
                            match payload["text"].as_str().filter(|text|super::queue::queue_text_ok(text)) {
                                Some(text)=>{
                                    let appended=queue.exec(target.generation,&format!("other-input:{id}"),clock(),Action::Append {text:text.into(),delivered:false,ts:None,pre_transcript:payload["pre_transcript"]==true,entry_id:Some(id.clone())}).await;
                                    match appended {
                                        Ok(_)=>{
                                            let persisted=publish_snapshot(&target,&queue,&snapshots,None).await;
                                            match persisted {
                                                Ok(())=>match native::drain(&target,&queue,&options,&io,&snapshots).await {
                                                    Ok(_)=>native::input_reply(&target,&queue,id).await,
                                                    Err(error)=>Err(error),
                                                },
                                                Err(error)=>Err(error),
                                            }
                                        },
                                        Err(_)=>Err(failed("queue_io")),
                                    }
                                },
                                None=>Err(failed("invalid_text")),
                            }
                        }else{Err(failed("other_terminal_control_unavailable"))};
                        let _=response.send(result);
                    },
                    Message::Queue {id,action,response}=>{
                        let result=queue.exec(target.generation,&id,clock(),action).await.map_err(|_|failed("queue_io"));
                        let _=publish_snapshot(&target,&queue,&snapshots,None).await;
                        let _=response.send(result);
                    },
                    Message::Query {kind,response}=>{
                        let value=match kind.as_str() {
                            "confirm"=>Err(failed("other_terminal_confirmation_unavailable")),
                            "drain"=>native::drain(&target,&queue,&options,&io,&snapshots).await,
                            "projection"=>queue.exec(target.generation,"other-projection",clock(),Action::EnsureProjection).await.map_err(|_|failed("queue_io")),
                            _=>publish_snapshot(&target,&queue,&snapshots,None).await.map(|_|snapshots.borrow().clone()),
                        };
                        let _=response.send(value);
                    },
                    Message::Stop(response)=>{let _=response.send(Ok(()));break;},
                }
            }
            actor_closed.store(true,Ordering::Release);
            io.release().await;
            if let Some(preview)=&options.preview {preview.forget(&target);}
            queue.shutdown().await.map_err(|_|failed("queue_io"))
        });
        Self {tx,closed,task:Arc::new(Mutex::new(Some(task))),snapshot,frame}
    }
    pub fn last_frame(&self)->tokio::sync::watch::Receiver<Option<Arc<OtherFrame>>> {self.frame.clone()}
    pub async fn command(&self, command: RuntimeCommand) -> Result<RuntimeReply, RuntimeError> {
        let kind=serde_json::to_value(command.kind).map_err(|_|failed("runtime_command"))?.as_str().ok_or_else(||failed("runtime_command"))?.to_owned();
        self.control(command.operation_id,kind,command.payload).await
    }
    pub async fn control(&self,id:String,kind:String,payload:Value)->Result<RuntimeReply,RuntimeError> {
        if self.closed.load(Ordering::Acquire) {return Err(failed("runtime_stopping"));}
        let (response,rx)=oneshot::channel();self.tx.send(Message::Control {id,kind,payload,response}).await.map_err(|_|failed("runtime_closed"))?;
        rx.await.map_err(|_|failed("runtime_closed"))?
    }
    pub async fn queue(&self,id:String,action:Action)->Result<Value,RuntimeError> {
        let (response,rx)=oneshot::channel();self.tx.send(Message::Queue {id,action,response}).await.map_err(|_|failed("runtime_closed"))?;
        rx.await.map_err(|_|failed("runtime_closed"))?
    }
    async fn query(&self,kind:&str)->Result<Value,RuntimeError> {
        let (response,rx)=oneshot::channel();self.tx.send(Message::Query {kind:kind.into(),response}).await.map_err(|_|failed("runtime_closed"))?;
        rx.await.map_err(|_|failed("runtime_closed"))?
    }
    pub async fn snapshot(&self)->Result<Value,RuntimeError> {if self.closed.load(Ordering::Acquire) {Err(failed("runtime_closed"))}else{Ok(self.snapshot.borrow().clone())}}
    pub async fn drain(&self)->Result<Value,RuntimeError> {self.query("drain").await}
    pub async fn confirm(&self)->Result<Value,RuntimeError> {self.query("confirm").await}
    pub async fn ensure_projection(&self)->Result<Value,RuntimeError> {self.query("projection").await}
    pub async fn stop(&self)->Result<(),RuntimeError> {
        let mut task=self.task.lock().await;
        let Some(task)=task.take() else {return Ok(())};
        self.closed.store(true,Ordering::Release);
        let (response,rx)=oneshot::channel();let sent=self.tx.send(Message::Stop(response)).await;
        let reply=if sent.is_ok(){rx.await.map_err(|_|failed("runtime_closed"))?}else{Ok(())};
        task.await.map_err(|_|failed("runtime_panic"))??;reply
    }
}

/// A geração só nasce em um quando não existe estado durável desta chave.
pub fn queue_identity(queue_dir:&std::path::Path,key:&str)->Result<(u64,PathBuf,PathBuf),RuntimeError> {
    if !super::codex_terminal::valid_key(key) {return Err(failed("runtime_binding"));}
    let (state,lease)=super::codex_terminal::queue_files(queue_dir,key);
    let generation=match super::queue::read_replaced(&state) {
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>1,
        Err(_)=>return Err(failed("queue_io")),
        Ok(bytes)=>{
            let state:super::queue::State=serde_json::from_slice(&bytes).map_err(|_|failed("queue_io"))?;
            if state.owner_key!=key || state.generation==0 {return Err(failed("runtime_generation"));}
            state.generation
        }
    };
    Ok((generation,state,lease))
}

fn clock()->super::protocol::ClockSample {
    super::protocol::ClockSample {monotonic_s:0.0,epoch_s:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|time|time.as_secs_f64())}
}

fn snapshot_value(target:&OtherTarget,rows:&[Value],alive:Option<bool>)->Value {
    json!({"key":target.key,"generation":target.generation,"error":null,"queue":rows,
        "view":{"alive":alive,"conversation":target.binding.conversation,"transcript":target.transcript,"terminal":true,"headless":false,"public_state":null}})
}
async fn publish_snapshot(target:&OtherTarget,queue:&QueueActor,snapshot:&tokio::sync::watch::Sender<Value>,alive:Option<bool>)->Result<(),RuntimeError> {
    let state=queue.snapshot().await.map_err(|_|failed("queue_io"))?;
    let mut next=snapshot.borrow().clone();
    next["queue"]=serde_json::to_value(&state.rows).map_err(|_|failed("queue_io"))?;
    if let Some(alive)=alive {next["view"]["alive"]=json!(alive);}
    next["key"]=json!(target.key);next["generation"]=json!(target.generation);
    snapshot.send_replace(next);Ok(())
}

struct Observer {
    cache:Arc<std::sync::Mutex<observe::FileCache>>,
    memory:crate::terminal_state::ReducerMemory,
    pending:Option<crate::interactions::InteractionTarget>,
    binding:crate::interactions::InteractionBinding,
    working_revision:u64,
    in_progress:bool,
}
impl Observer {
    fn new(target:&OtherTarget,options:&OtherOptions)->Result<Self,RuntimeError> {
        let binding=crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:target.binding.conversation.clone()};
        options.interactions.bind(&binding).map_err(crate::interactions::runtime_error)?;
        Ok(Self {cache:Arc::new(std::sync::Mutex::new(Default::default())),memory:Default::default(),pending:None,binding,working_revision:0,in_progress:false})
    }
    async fn round(&mut self,target:&OtherTarget,options:&OtherOptions,io:&Arc<dyn OtherIo>,live:&LiveSender,snapshot:&tokio::sync::watch::Sender<Value>,frames:&tokio::sync::watch::Sender<Option<Arc<OtherFrame>>>)->Result<(),RuntimeError> {
        let observed=self.read(target,io).await;
        match observed {
            Ok((mut observed,frame))=>{
                if let Some(previous)=&self.pending && observed.pending.as_ref().is_none_or(|pending|pending.target!=*previous) {
                    options.interactions.observe(&self.binding,previous,None).map_err(crate::interactions::runtime_error)?;
                }
                self.pending=observed.pending.as_ref().map(|pending|pending.target.clone());
                if let Some(pending)=observed.pending {
                    let id=options.interactions.observe(&self.binding,&pending.target,Some(pending.semantic)).map_err(crate::interactions::runtime_error)?;
                    observed.state.interaction_id=id.clone();
                    if pending.target.kind==crate::interactions::InteractionKind::Question {
                        observed.state.codex_question=Some(json!({"provider":target.provider.as_str(),"request_id":pending.native_id,"tool_use_id":pending.native_id,
                            "interaction_id":id,"questions":pending.questions}).as_object().unwrap().clone());
                    }
                }
                let progress=observed.state.state=="working" || self.in_progress && observed.state.state=="awaiting_input";
                if progress && !self.in_progress {self.working_revision=self.working_revision.saturating_add(1);}
                self.in_progress=progress;self.memory=observed.memory;
                let prior=live.borrow().clone();
                let mut next=super::protocol::LiveState {binding:Some(self.binding.clone()),public_state:serde_json::to_value(&observed.state).map_err(|_|failed("other_terminal_state_invalid"))?,
                    error:None,..prior.as_deref().cloned().unwrap_or_default()};
                if let Some(preview)=&options.preview && let Some(event)=preview.update(OtherPreviewContext {target,state:&observed.state,
                    frame:frame.styled.as_deref().or(Some(&frame.text)),files:&observed.files,
                    marker:observed.marker.as_ref().map(|(state,ts)|(state.as_str(),*ts)),now:observed.now})? {
                    next.preview=event.text;next.preview_flags=Some((event.md,event.full,event.vivo));
                }
                let mut view=snapshot.borrow().clone();view["error"]=Value::Null;
                view["view"]["alive"]=json!(true);view["view"]["public_state"]=next.public_state.clone();
                view["view"]["in_progress"]=json!(progress);view["view"]["working_revision"]=json!(self.working_revision);
                snapshot.send_replace(view);
                frames.send_replace(Some(Arc::new(frame)));
                if prior.as_ref().is_none_or(|prior|**prior!=next) {live.send_replace(Some(Arc::new(next)));}
                Ok(())
            },
            Err(error)=>{
                let previous=live.borrow().clone();
                let mut next=previous.as_deref().cloned().unwrap_or_default();next.binding=Some(self.binding.clone());
                next.error=Some((error.code.clone(),error.message.clone()));
                live.send_replace(Some(Arc::new(next)));
                if error.code=="other_terminal_life_changed" || error.code=="other_terminal_conversation_changed" {
                    options.interactions.forget_binding(&self.binding).map_err(crate::interactions::runtime_error)?;
                    if let Some(preview)=&options.preview {preview.forget(target);}
                    let mut view=snapshot.borrow().clone();view["error"]=json!(error.code);snapshot.send_replace(view);
                }
                Err(error)
            }
        }
    }
    async fn read(&self,target:&OtherTarget,io:&Arc<dyn OtherIo>)->Result<(observe::Observation,OtherFrame),RuntimeError> {
        if !io.prove(target).await? {return Err(failed("other_terminal_life_changed"));}
        let frame=io.capture().await?;
        let (owned,text,memory,cache)=(target.clone(),frame.text.clone(),self.memory.clone(),self.cache.clone());
        let observed=tokio::task::spawn_blocking(move || {
            let mut cache=cache.lock().map_err(|_|failed("other_terminal_files_unavailable"))?;
            observe::read(&owned,&text,memory,&mut cache)
        }).await.map_err(|_|failed("other_terminal_observation_worker"))??;
        if !io.prove(target).await? {return Err(failed("other_terminal_life_changed"));}
        Ok((observed,frame))
    }
}
