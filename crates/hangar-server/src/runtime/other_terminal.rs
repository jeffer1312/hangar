//! Um proprietário por vida para entrada, observação e fila de Pi, omp e Kimi.
use std::path::PathBuf;
mod io;
pub use io::ProcessOtherIo;
mod dispatch;
pub mod drive;
mod native;
mod observe;
pub(in crate::runtime) mod storage;
pub(in crate::runtime) mod targets;
pub use observe::{
    PendingInteraction, approval_open as kimi_approval_open, menu as menu_interaction,
    picker_open as kimi_picker_open,
};
pub use storage::open_queue;
pub use targets::discovery;

pub fn read_pending_interaction(
    target: &OtherTarget,
) -> Result<Option<PendingInteraction>, RuntimeError> {
    observe::pending(target)
}
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
pub fn lifecycle_id(key: &str, generation: u64) -> String {
    format!(
        "{}:g:{generation}",
        crate::list::links::session_life(Some(key), None).unwrap_or_default()
    )
}
impl OtherTarget {
    pub fn lifecycle_id(&self) -> String {
        lifecycle_id(&self.key, self.generation)
    }
    pub fn same_life(&self, other: &Self) -> bool {
        self.key == other.key
            && self.generation == other.generation
            && self.name == other.name
            && self.provider == other.provider
            && self.binding == other.binding
            && self.agent_pid == other.agent_pid
            && self.agent_birth == other.agent_birth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn target() -> OtherTarget {
        OtherTarget {
            key: "life-a".into(),
            generation: 1,
            name: "pi-a".into(),
            provider: hangar_transcript::Provider::Pi,
            binding: TerminalBinding {
                name: "pi-a".into(),
                pane: "%1".into(),
                conversation: "conversation-a".into(),
                generation: 1,
                created: 100,
                mux_argv: vec!["tmux".into()],
                windows: false,
                clipboard_lock_path: None,
            },
            pane_address: "%1".into(),
            dirs: crate::list::discover_other::Dirs {
                home: "home".into(),
                claude: "home/.claude".into(),
                codex_home: "home/.codex".into(),
                pi_sessions: "home/.pi/agent/sessions".into(),
                omp_config: "home/.omp".into(),
                omp_agent: "home/.omp/agent".into(),
                kimi_home: "home/.kimi-code".into(),
            },
            cwd: "project".into(),
            pane_pid: 123,
            agent_pid: 123,
            agent_birth: 100.5,
            transcript: "conversation-a.jsonl".into(),
            state_path: "state.json".into(),
            lease_path: "state.lock".into(),
            projection_dir: "queue".into(),
            config_dirs: vec![],
            created: 100.0,
        }
    }
    #[test]
    fn other_terminal_same_life_requires_the_process_birth_and_conversation() {
        let original = target();
        assert!(original.same_life(&target()));
        let mut reused = target();
        reused.agent_birth = 101.0;
        assert!(!original.same_life(&reused));
        reused = target();
        reused.binding.conversation = "conversation-b".into();
        assert!(!original.same_life(&reused));
        reused = target();
        reused.generation = 2;
        assert!(!original.same_life(&reused));
        reused = target();
        reused.provider = hangar_transcript::Provider::Kimi;
        assert!(!original.same_life(&reused));
    }
}

use super::{
    protocol::{Disposition, LiveSender, RuntimeCommand, RuntimeError, RuntimeReply},
    queue::{Action, QueueActor},
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone, Debug)]
pub struct OtherOpening {
    pub dirs: crate::list::discover_other::Dirs,
    pub config_dirs: Vec<PathBuf>,
    pub pane_pid: Option<i64>,
    pub agent: Option<OtherProcess>,
    pub spec: crate::session_open::SessionSpec,
    pub binding: TerminalBinding,
    pub pane_address: String,
    pub projection_dir: PathBuf,
}
impl OtherOpening {
    pub fn lifecycle_id(&self) -> String {
        lifecycle_id(&self.spec.key, self.binding.generation)
    }
}
#[derive(Clone, Debug)]
pub struct OtherProcess {
    pub pid: i64,
    pub birth: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherEnd {
    Requested,
    Disappeared,
    Replaced,
}
#[derive(Clone, Debug)]
pub struct OtherClosed {
    pub life: OtherDiscoveryState,
    pub reason: OtherEnd,
}
pub type OtherAfterClosed =
    Arc<dyn Fn(OtherClosed) -> BoxFuture<'static, Result<Vec<Value>, RuntimeError>> + Send + Sync>;
#[derive(Clone, Debug)]
pub enum OtherDiscoveryState {
    Ready(OtherTarget),
    Opening(OtherOpening),
}
#[derive(Clone)]
pub struct OtherDiscovery {
    pub find: Arc<
        dyn Fn(String) -> BoxFuture<'static, Option<Result<OtherDiscoveryState, RuntimeError>>>
            + Send
            + Sync,
    >,
    pub names: Arc<dyn Fn() -> BoxFuture<'static, Result<Vec<String>, RuntimeError>> + Send + Sync>,
}
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
    fn update(
        &self,
        context: OtherPreviewContext<'_>,
    ) -> Result<Option<hangar_api::preview::PreviewEvent>, RuntimeError>;
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
    pub fn from_styled(styled: String) -> Self {
        Self {
            text: crate::terminal_input::unstyle(&styled, true),
            typed: Some(crate::terminal_input::unstyle(&styled, false)),
            styled: Some(styled),
            editor: None,
        }
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
    fn write<'a>(
        &'a self,
        target: &'a OtherTarget,
        value: &'a str,
        literal: bool,
        guard: Option<&'a OtherWriteGuard>,
    ) -> BoxFuture<'a, Result<(), RuntimeError>>;
    fn release<'a>(&'a self) -> BoxFuture<'a, ()>;
}
#[derive(Clone)]
pub struct OtherOptions {
    pub pool: crate::terminal_control::TerminalPool,
    pub inbox: Arc<crate::provider_models::inbox::Store>,
    pub processes: Arc<dyn crate::list::procs::ProcessView>,
    pub interactions: Arc<crate::interactions::InteractionStore>,
    pub preview: Option<Arc<dyn OtherPreviewSource>>,
    pub after_closed: Option<OtherAfterClosed>,
    pub(crate) registry: Option<std::sync::Weak<super::gateway::RuntimeRegistry>>,
    pub tick: Duration,
    pub io_override: Option<Arc<dyn OtherIo>>,
}
impl OtherOptions {
    pub fn new(
        pool: crate::terminal_control::TerminalPool,
        inbox: Arc<crate::provider_models::inbox::Store>,
        processes: Arc<dyn crate::list::procs::ProcessView>,
        interactions: Arc<crate::interactions::InteractionStore>,
    ) -> Self {
        Self {
            pool,
            inbox,
            processes,
            interactions,
            preview: None,
            after_closed: None,
            registry: None,
            tick: Duration::from_millis(500),
            io_override: None,
        }
    }
}
fn failed(code: &str) -> RuntimeError {
    RuntimeError::new(
        code,
        "não foi possível comprovar a operação desta vida do terminal",
    )
}
type Response = oneshot::Sender<Result<RuntimeReply, RuntimeError>>;
enum Message {
    Control {
        id: String,
        kind: String,
        payload: Value,
        response: Response,
    },
    Queue {
        id: String,
        action: Action,
        response: oneshot::Sender<Result<Value, RuntimeError>>,
    },
    Query {
        kind: String,
        response: oneshot::Sender<Result<Value, RuntimeError>>,
    },
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
    pub fn spawn(
        target: OtherTarget,
        queue: QueueActor,
        options: OtherOptions,
        live: LiveSender,
    ) -> Self {
        let queue = Arc::new(queue);
        let (tx, mut rx) = mpsc::channel(64);
        let closed = Arc::new(AtomicBool::new(false));
        let actor_closed = closed.clone();
        let (snapshots, snapshot) =
            tokio::sync::watch::channel(snapshot_value(&target, &queue.initial_state().rows, None));
        let io = options
            .io_override
            .clone()
            .unwrap_or_else(|| Arc::new(io::ProcessOtherIo::new(&target, &options)));
        let (frames, frame) = tokio::sync::watch::channel(None);
        let task = tokio::spawn(async move {
            let mut observer = Observer::new(&target, &options)?;
            let receipt = Arc::new(std::sync::Mutex::new(super::receipt::ReceiptIndex::new(
                target.provider.as_str(),
                &target.binding.conversation,
            )));
            let mut tick = Box::pin(tokio::time::sleep(Duration::ZERO));
            let mut interested = false;
            let flight = Arc::new(AtomicBool::new(false));
            loop {
                let message = tokio::select! {
                    message=rx.recv()=>message,
                    _=&mut tick=>{
                        let next=options.preview.as_ref().is_some_and(|preview|preview.interested(&target));
                        if interested && !next {if let Some(preview)=&options.preview {preview.forget(&target);}let mut value=live.borrow().as_deref().cloned().unwrap_or_default();value.preview.clear();value.preview_flags=None;live.send_replace(Some(Arc::new(value)));}
                        interested=next;
                        if let Err(error)=observer.round(&target,&options,&io,&live,&snapshots,&frames,interested).await {
                            tracing::warn!(code=%error.code,"observação sem prova; conserva o último retrato");
                            if matches!(error.code.as_str(),"other_terminal_life_changed"|"other_terminal_conversation_changed"|"other_terminal_conversation_unavailable") && !flight.swap(true,Ordering::SeqCst) {
                                if let Some(registry)=options.registry.as_ref().and_then(std::sync::Weak::upgrade) {let name=target.name.clone();let flight=flight.clone();tokio::spawn(async move {let _=registry.reconcile_other(&name).await;flight.store(false,Ordering::SeqCst);});}else{flight.store(false,Ordering::SeqCst);}
                            }
                        }
                        let frame=frames.borrow().clone();
                        if let Err(error)=dispatch::drain(&target,&queue,&options,&io,&snapshots,frame.as_deref()).await {tracing::warn!(code=%error.code,"drenagem espera prova; não redigita");}
                        let _=confirm(&target,&queue,&receipt,&snapshots).await;
                        let cadence=if interested {Duration::from_millis(if live.borrow().as_ref().is_some_and(|state|state.public_state["state"]=="working") {150}else{750})}else{options.tick};
                        tick.as_mut().reset(tokio::time::Instant::now()+cadence);
                        continue;
                    }
                };
                let Some(message) = message else {
                    break;
                };
                match message {
                    Message::Control {
                        id,
                        kind,
                        payload,
                        response,
                    } => {
                        let result = async {
                            if kind == "input" {
                                if let Some(text) = payload["text"].as_str().filter(|text| text.trim_start().starts_with('/')) {
                                    let frame = frames.borrow().clone();
                                    return dispatch::direct_prompt(&target, &options, &io, &id, text, frame.as_deref()).await;
                                }
                                match payload["text"]
                                    .as_str()
                                    .filter(|text| super::queue::queue_text_ok(text))
                                {
                                    Some(text) => match queue
                                        .exec(
                                            target.generation,
                                            &format!("other-input:{id}"),
                                            clock(),
                                            Action::Append {
                                                text: text.into(),
                                                delivered: false,
                                                ts: None,
                                                pre_transcript: payload["pre_transcript"] == true,
                                                entry_id: Some(id.clone()),
                                            },
                                        )
                                        .await
                                    {
                                        Ok(_) => {
                                            let frame = frames.borrow().clone();
                                            match dispatch::drain(
                                                &target,
                                                &queue,
                                                &options,
                                                &io,
                                                &snapshots,
                                                frame.as_deref(),
                                            )
                                            .await
                                            {
                                                Ok(_) => {
                                                    native::input_reply(&target, &queue, id).await
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }
                                        Err(_) => Err(failed("queue_io")),
                                    },
                                    None => Err(failed("invalid_text")),
                                }
                            } else {
                                let mut payload = payload;
                                let mut control = kind.as_str();
                                if matches!(control, "steer_entry" | "steer_queue" | "steer_first")
                                {
                                    let state =
                                        queue.snapshot().await.map_err(|_| failed("queue_io"))?;
                                    if let Some(row) = state.rows.iter().find(|row| {
                                        payload["entry_id"]
                                            .as_str()
                                            .map_or(row["delivered"] == false, |id| row["id"] == id)
                                    }) {
                                        payload["entry_id"] = row["id"].clone();
                                        payload["text"] = row["text"].clone();
                                    }
                                    control = "steer_queue";
                                }
                                let frame = frames.borrow().clone();
                                match drive::control(
                                    &target,
                                    &*io,
                                    &options.interactions,
                                    &id,
                                    control,
                                    &payload,
                                    frame.as_deref(),
                                )
                                .await
                                {
                                    Ok(mut outcome) => {
                                        if let Some(text) = outcome.fallback_text {
                                            let entry = payload["entry_id"]
                                                .as_str()
                                                .unwrap_or(&id)
                                                .to_owned();
                                            let state = queue
                                                .snapshot()
                                                .await
                                                .map_err(|_| failed("queue_io"))?;
                                            let action = if state
                                                .rows
                                                .iter()
                                                .any(|row| row["id"] == entry)
                                            {
                                                Action::Promote {
                                                    entry_id: entry.clone(),
                                                }
                                            } else {
                                                Action::Append {
                                                    text,
                                                    delivered: false,
                                                    ts: None,
                                                    pre_transcript: false,
                                                    entry_id: Some(entry.clone()),
                                                }
                                            };
                                            queue
                                                .exec(
                                                    target.generation,
                                                    &format!("fallback:{id}"),
                                                    clock(),
                                                    action,
                                                )
                                                .await
                                                .map_err(|_| failed("queue_io"))?;
                                            let _ = dispatch::drain(
                                                &target, &queue, &options, &io, &snapshots, None,
                                            )
                                            .await?;
                                            outcome.reply =
                                                native::input_reply(&target, &queue, entry).await?;
                                        }
                                        if control == "interrupt"
                                            && payload["clear"] == true
                                            && outcome.reply.disposition == Disposition::Accepted
                                        {
                                            queue
                                                .exec(
                                                    target.generation,
                                                    &format!("interrupt-clear:{id}"),
                                                    clock(),
                                                    Action::Clear,
                                                )
                                                .await
                                                .map_err(|_| failed("queue_io"))?;
                                        }
                                        Ok(outcome.reply)
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                        }
                        .await;
                        let _ = publish_snapshot(&target, &queue, &snapshots, None).await;
                        let _ = response.send(result);
                    }
                    Message::Queue {
                        id,
                        action,
                        response,
                    } => {
                        let result = queue
                            .exec(target.generation, &id, clock(), action)
                            .await
                            .map_err(|_| failed("queue_io"));
                        let _ = publish_snapshot(&target, &queue, &snapshots, None).await;
                        let _ = response.send(result);
                    }
                    Message::Query { kind, response } => {
                        let value = match kind.as_str() {
                            "confirm" => confirm(&target, &queue, &receipt, &snapshots).await,
                            "drain" => {
                                let frame = frames.borrow().clone();
                                dispatch::drain(
                                    &target,
                                    &queue,
                                    &options,
                                    &io,
                                    &snapshots,
                                    frame.as_deref(),
                                )
                                .await
                            }
                            "projection" => queue
                                .exec(
                                    target.generation,
                                    "other-projection",
                                    clock(),
                                    Action::EnsureProjection,
                                )
                                .await
                                .map_err(|_| failed("queue_io")),
                            _ => publish_snapshot(&target, &queue, &snapshots, None)
                                .await
                                .map(|_| snapshots.borrow().clone()),
                        };
                        let _ = response.send(value);
                    }
                    Message::Stop(response) => {
                        let _ = response.send(Ok(()));
                        break;
                    }
                }
            }
            actor_closed.store(true, Ordering::Release);
            io.release().await;
            drive::forget(&target);
            if let Some(preview) = &options.preview {
                preview.forget(&target);
            }
            Arc::try_unwrap(queue)
                .map_err(|_| failed("queue_busy"))?
                .shutdown()
                .await
                .map_err(|_| failed("queue_io"))
        });
        Self {
            tx,
            closed,
            task: Arc::new(Mutex::new(Some(task))),
            snapshot,
            frame,
        }
    }
    pub fn last_frame(&self) -> tokio::sync::watch::Receiver<Option<Arc<OtherFrame>>> {
        self.frame.clone()
    }
    pub async fn command(&self, command: RuntimeCommand) -> Result<RuntimeReply, RuntimeError> {
        let kind = serde_json::to_value(command.kind)
            .map_err(|_| failed("runtime_command"))?
            .as_str()
            .ok_or_else(|| failed("runtime_command"))?
            .to_owned();
        self.control(command.operation_id, kind, command.payload)
            .await
    }
    pub async fn control(
        &self,
        id: String,
        kind: String,
        payload: Value,
    ) -> Result<RuntimeReply, RuntimeError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(failed("runtime_stopping"));
        }
        let (response, rx) = oneshot::channel();
        self.tx
            .send(Message::Control {
                id,
                kind,
                payload,
                response,
            })
            .await
            .map_err(|_| failed("runtime_closed"))?;
        rx.await.map_err(|_| failed("runtime_closed"))?
    }
    pub async fn queue(&self, id: String, action: Action) -> Result<Value, RuntimeError> {
        let (response, rx) = oneshot::channel();
        self.tx
            .send(Message::Queue {
                id,
                action,
                response,
            })
            .await
            .map_err(|_| failed("runtime_closed"))?;
        rx.await.map_err(|_| failed("runtime_closed"))?
    }
    async fn query(&self, kind: &str) -> Result<Value, RuntimeError> {
        let (response, rx) = oneshot::channel();
        self.tx
            .send(Message::Query {
                kind: kind.into(),
                response,
            })
            .await
            .map_err(|_| failed("runtime_closed"))?;
        rx.await.map_err(|_| failed("runtime_closed"))?
    }
    pub async fn snapshot(&self) -> Result<Value, RuntimeError> {
        if self.closed.load(Ordering::Acquire) {
            Err(failed("runtime_closed"))
        } else {
            Ok(self.snapshot.borrow().clone())
        }
    }
    pub async fn drain(&self) -> Result<Value, RuntimeError> {
        self.query("drain").await
    }
    pub async fn confirm(&self) -> Result<Value, RuntimeError> {
        self.query("confirm").await
    }
    pub async fn ensure_projection(&self) -> Result<Value, RuntimeError> {
        self.query("projection").await
    }
    pub async fn stop(&self) -> Result<(), RuntimeError> {
        let mut task = self.task.lock().await;
        let Some(task) = task.take() else {
            return Ok(());
        };
        self.closed.store(true, Ordering::Release);
        let (response, rx) = oneshot::channel();
        let sent = self.tx.send(Message::Stop(response)).await;
        let reply = if sent.is_ok() {
            rx.await.map_err(|_| failed("runtime_closed"))?
        } else {
            Ok(())
        };
        task.await.map_err(|_| failed("runtime_panic"))??;
        reply
    }
}

/// A geração só nasce em um quando não existe estado durável desta chave.
pub fn queue_identity(
    queue_dir: &std::path::Path,
    key: &str,
) -> Result<(u64, PathBuf, PathBuf), RuntimeError> {
    if !super::codex_terminal::valid_key(key) {
        return Err(failed("runtime_binding"));
    }
    let (state, lease) = super::codex_terminal::queue_files(queue_dir, key);
    let generation = match super::queue::read_replaced(&state) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 1,
        Err(_) => return Err(failed("queue_io")),
        Ok(bytes) => {
            let state: super::queue::State =
                serde_json::from_slice(&bytes).map_err(|_| failed("queue_io"))?;
            if state.owner_key != key || state.generation == 0 {
                return Err(failed("runtime_generation"));
            }
            state.generation
        }
    };
    Ok((generation, state, lease))
}

fn clock() -> super::protocol::ClockSample {
    super::protocol::ClockSample {
        monotonic_s: 0.0,
        epoch_s: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |time| time.as_secs_f64()),
    }
}

fn snapshot_value(target: &OtherTarget, rows: &[Value], alive: Option<bool>) -> Value {
    json!({"key":target.key,"generation":target.generation,"error":null,"queue":rows,
        "view":{"alive":alive,"conversation":target.binding.conversation,"transcript":target.transcript,"terminal":true,"headless":false,"public_state":null}})
}
async fn publish_snapshot(
    target: &OtherTarget,
    queue: &QueueActor,
    snapshot: &tokio::sync::watch::Sender<Value>,
    alive: Option<bool>,
) -> Result<(), RuntimeError> {
    let state = queue.snapshot().await.map_err(|_| failed("queue_io"))?;
    let mut next = snapshot.borrow().clone();
    next["queue"] = serde_json::to_value(&state.rows).map_err(|_| failed("queue_io"))?;
    if let Some(alive) = alive {
        next["view"]["alive"] = json!(alive);
    }
    next["key"] = json!(target.key);
    next["generation"] = json!(target.generation);
    snapshot.send_replace(next);
    Ok(())
}

struct Observer {
    cache: Arc<std::sync::Mutex<observe::FileCache>>,
    memory: crate::terminal_state::ReducerMemory,
    pending: Option<crate::interactions::InteractionTarget>,
    binding: crate::interactions::InteractionBinding,
    working_revision: u64,
    in_progress: bool,
}
impl Observer {
    fn new(target: &OtherTarget, options: &OtherOptions) -> Result<Self, RuntimeError> {
        let binding = crate::interactions::InteractionBinding {
            key: target.key.clone(),
            generation: target.generation,
            conversation: target.binding.conversation.clone(),
        };
        options
            .interactions
            .bind(&binding)
            .map_err(crate::interactions::runtime_error)?;
        Ok(Self {
            cache: Arc::new(std::sync::Mutex::new(Default::default())),
            memory: Default::default(),
            pending: None,
            binding,
            working_revision: 0,
            in_progress: false,
        })
    }
    async fn round(
        &mut self,
        target: &OtherTarget,
        options: &OtherOptions,
        io: &Arc<dyn OtherIo>,
        live: &LiveSender,
        snapshot: &tokio::sync::watch::Sender<Value>,
        frames: &tokio::sync::watch::Sender<Option<Arc<OtherFrame>>>,
        interested: bool,
    ) -> Result<(), RuntimeError> {
        let observed = self.read(target, io, interested).await;
        match observed {
            Ok((mut observed, frame)) => {
                if let Some(previous) = &self.pending
                    && observed
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.target != *previous)
                {
                    options
                        .interactions
                        .observe(&self.binding, previous, None)
                        .map_err(crate::interactions::runtime_error)?;
                }
                self.pending = observed
                    .pending
                    .as_ref()
                    .map(|pending| pending.target.clone());
                if let Some(pending) = observed.pending {
                    let id = options
                        .interactions
                        .observe(&self.binding, &pending.target, Some(pending.semantic))
                        .map_err(crate::interactions::runtime_error)?;
                    observed.state.interaction_id = id.clone();
                    if pending.target.kind == crate::interactions::InteractionKind::Question {
                        observed.state.codex_question=Some(json!({"interaction_id":id,"questions":question_payload(&pending.questions)}).as_object().unwrap().clone());
                    }
                }
                let progress = observed.state.state == "working"
                    || self.in_progress && observed.state.state == "awaiting_input";
                if progress && !self.in_progress {
                    self.working_revision = self.working_revision.saturating_add(1);
                }
                self.in_progress = progress;
                self.memory = observed.memory;
                let prior = live.borrow().clone();
                let mut next = super::protocol::LiveState {
                    binding: Some(self.binding.clone()),
                    public_state: serde_json::to_value(&observed.state)
                        .map_err(|_| failed("other_terminal_state_invalid"))?,
                    error: None,
                    ..prior.as_deref().cloned().unwrap_or_default()
                };
                if interested
                    && let Some(preview) = &options.preview
                    && let Some(event) = preview.update(OtherPreviewContext {
                        target,
                        state: &observed.state,
                        frame: frame.styled.as_deref().or(Some(&frame.text)),
                        files: &observed.files,
                        marker: observed
                            .marker
                            .as_ref()
                            .map(|(state, ts)| (state.as_str(), *ts)),
                        now: observed.now,
                    })?
                {
                    next.preview = event.text;
                    next.preview_flags = Some((event.md, event.full, event.vivo));
                }
                let mut view = snapshot.borrow().clone();
                view["error"] = Value::Null;
                view["view"]["alive"] = json!(true);
                view["view"]["public_state"] = next.public_state.clone();
                view["view"]["in_progress"] = json!(progress);
                view["view"]["working_revision"] = json!(self.working_revision);
                snapshot.send_replace(view);
                frames.send_replace(Some(Arc::new(frame)));
                if prior.as_ref().is_none_or(|prior| **prior != next) {
                    live.send_replace(Some(Arc::new(next)));
                }
                Ok(())
            }
            Err(error) => {
                let previous = live.borrow().clone();
                let mut next = previous.as_deref().cloned().unwrap_or_default();
                next.binding = Some(self.binding.clone());
                next.error = Some((error.code.clone(), error.message.clone()));
                live.send_replace(Some(Arc::new(next)));
                if error.code == "other_terminal_life_changed"
                    || error.code == "other_terminal_conversation_changed"
                {
                    options
                        .interactions
                        .forget_binding(&self.binding)
                        .map_err(crate::interactions::runtime_error)?;
                    if let Some(preview) = &options.preview {
                        preview.forget(target);
                    }
                    let mut view = snapshot.borrow().clone();
                    view["error"] = json!(error.code);
                    snapshot.send_replace(view);
                }
                Err(error)
            }
        }
    }
    async fn read(
        &self,
        target: &OtherTarget,
        io: &Arc<dyn OtherIo>,
        interested: bool,
    ) -> Result<(observe::Observation, OtherFrame), RuntimeError> {
        if !io.prove(target).await? {
            return Err(failed("other_terminal_life_changed"));
        }
        let frame = io.capture().await?;
        let (owned, text, memory, cache) = (
            target.clone(),
            frame.text.clone(),
            self.memory.clone(),
            self.cache.clone(),
        );
        let observed = tokio::task::spawn_blocking(move || {
            let mut cache = cache
                .lock()
                .map_err(|_| failed("other_terminal_files_unavailable"))?;
            observe::read(&owned, &text, memory, &mut cache, interested)
        })
        .await
        .map_err(|_| failed("other_terminal_observation_worker"))??;
        if !io.prove(target).await? {
            return Err(failed("other_terminal_life_changed"));
        }
        Ok((observed, frame))
    }
}

fn inbox_line(target: &OtherTarget, inbox: &crate::provider_models::inbox::Store) -> Option<String> {
    inbox.line_for(Some(&target.key), (!target.binding.windows).then_some(target.binding.pane.as_str()))
}

fn question_payload(questions: &[Value]) -> Vec<Value> {
    questions.iter().map(|question|json!({"header":question["header"].as_str().unwrap_or(""),"question":question["question"].as_str().unwrap_or(""),"multiSelect":question["multiSelect"].as_bool().unwrap_or(false),"options":question["options"].as_array().into_iter().flatten().map(|option|json!({"label":option["label"].as_str().unwrap_or(""),"description":option["description"].as_str().unwrap_or(""),"preview":option["preview"].as_str().unwrap_or("")})).collect::<Vec<_>>()})).collect()
}
async fn confirm(
    target: &OtherTarget,
    queue: &Arc<QueueActor>,
    receipt: &Arc<std::sync::Mutex<super::receipt::ReceiptIndex>>,
    snapshot: &tokio::sync::watch::Sender<Value>,
) -> Result<Value, RuntimeError> {
    let state = queue.snapshot().await.map_err(|_| failed("queue_io"))?;
    receipt.lock().map_err(|_| failed("receipt_panic"))?.set_other_rows(state);
    let (confirmed, legacy) = super::actor::prove_inputs(
        queue.clone(),
        receipt.clone(),
        target.transcript.clone(),
        Some(target.binding.conversation.clone()),
        target.generation,
        clock(),
    )
    .await?;
    if target.provider == hangar_transcript::Provider::Kimi
        && snapshot.borrow()["view"]["public_state"]["state"] != "working" {
        let state = queue.snapshot().await.map_err(|_| failed("queue_io"))?;
        let sample = clock();
        queue.exec(target.generation, &format!("other-unproved:{}", state.next_seq), sample,
            Action::GiveUpUnproved {now:sample.epoch_s, grace:30.0}).await.map_err(|_| failed("queue_io"))?;
    }
    publish_snapshot(target, queue, snapshot, None).await?;
    Ok(json!({"confirmed":confirmed.len()+legacy}))
}
#[cfg(test)]
mod deferred_actor_tests;

#[cfg(test)]
mod deferred_episode_tests;
