//! Costuras públicas da adoção e dos recibos dos agentes com terminal.
use hangar_server::runtime::receipt::ReceiptIndex;
use serde_json::json;
use std::{collections::BTreeMap, fs::OpenOptions, io::Write};

fn line(provider: &str, id: &str, text: &str) -> String {
    // Formatos medidos preservados nas suítes anteriores de transcript Pi e Kimi.
    let value=if provider=="kimi" {json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":text}],"toolCalls":[],"origin":{"kind":"user"},"id":id},"time":1786453187986u64})}
        else {json!({"type":"message","id":id,"timestamp":"2026-01-01T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":text}]}})};
    format!("{value}\n")
}
#[test]
fn other_terminal_receipt_proves_the_new_occurrence_only_and_keeps_partial_lines() {
    for provider in ["pi","omp","kimi"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("transcript.jsonl");
        std::fs::write(&path,line(provider,"old","seguir com a tarefa")).unwrap();
        let mut index=ReceiptIndex::new(provider,"conversation-a");
        let cursor=index.capture(&path).unwrap();
        index.scan(&path).unwrap();
        let row=json!({"id":"entry-a","text":"seguir com a tarefa","ts":1786453180.0});
        assert!(index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().is_none());
        let new=line(provider,"new","seguir com a tarefa");
        let mut file=OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(new.trim_end_matches('\n').as_bytes()).unwrap();file.flush().unwrap();
        index.scan(&path).unwrap();
        assert!(index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().is_none());
        file.write_all(b"\n").unwrap();file.flush().unwrap();
        index.scan(&path).unwrap();
        let proof=index.match_after(&path,&cursor,&row,&BTreeMap::new()).unwrap().expect("a nova mensagem completa prova a entrega");
        assert_eq!(proof.occurrence.text,"seguir com a tarefa");
        assert_eq!(proof.occurrence.offset,cursor.offset);
        let mut used=BTreeMap::new();used.insert(proof.occurrence.id,json!({}));
        assert!(index.match_after(&path,&cursor,&row,&used).unwrap().is_none());
        let mut other=cursor.clone();other.conversation="conversation-b".into();
        assert!(index.match_after(&path,&other,&row,&BTreeMap::new()).unwrap().is_none());
        assert!(index.match_after(&path,&cursor,&json!({"text":"seguir"}),&BTreeMap::new()).unwrap().is_none());
    }
}

#[tokio::test]
async fn other_terminal_empty_transport_keeps_input_durably_in_the_common_queue() {
    use hangar_server::runtime::{other_terminal::{OtherTarget,OtherOptions,OtherHandle},queue::{QueueActor,Store,State,acquire_lease},protocol::{OperationKind,RuntimeCommand}};
    use hangar_server::{terminal_input::TerminalBinding,terminal_control::TerminalPool,list::procs::SystemProcs,interactions::InteractionStore,provider_models::inbox};
    use std::sync::Arc;
    let dir=tempfile::tempdir().unwrap();let state_path=dir.path().join("state.json");
    let lease=acquire_lease(&dir.path().join("state.lock")).unwrap();
    let store=Store::open(&state_path,&dir.path().join("queue"),State::new("life-a",1,"pi-a",vec![])).unwrap();
    let queue=QueueActor::start(store,lease);
    let target=OtherTarget {key:"life-a".into(),generation:1,name:"pi-a".into(),provider:hangar_transcript::Provider::Pi,
        binding:TerminalBinding {name:"pi-a".into(),pane:"%1".into(),conversation:"conversation-a".into(),generation:1,created:100,mux_argv:vec!["/does-not-exist".into()],windows:false,clipboard_lock_path:None},
        pane_address:"%1".into(),dirs:directories(dir.path()),cwd:dir.path().to_string_lossy().into_owned(),pane_pid:123,agent_pid:123,agent_birth:100.0,transcript:dir.path().join("conversation-a.jsonl"),state_path:state_path.clone(),lease_path:dir.path().join("state.lock"),projection_dir:dir.path().join("queue"),config_dirs:vec![],created:100.0};
    struct UnavailableCapture;
    impl hangar_server::runtime::other_terminal::OtherIo for UnavailableCapture {
        fn prove<'a>(&'a self,_:&'a OtherTarget)->futures_util::future::BoxFuture<'a,Result<bool,hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async {Ok(true)})}
        fn capture<'a>(&'a self)->futures_util::future::BoxFuture<'a,Result<hangar_server::runtime::other_terminal::OtherFrame,hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async {Err(hangar_server::runtime::protocol::RuntimeError::new("capture_failed","captura indisponível"))})}
        fn write<'a>(&'a self,_:&'a OtherTarget,_:&'a str,_:bool)->futures_util::future::BoxFuture<'a,Result<(),hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async {panic!("captura incerta não autoriza escrever")})}
        fn release<'a>(&'a self)->futures_util::future::BoxFuture<'a,()> {Box::pin(async {})}
    }
    let mut options=OtherOptions::new(TerminalPool::new(),inbox::Store::new(),Arc::new(SystemProcs::default()),Arc::new(InteractionStore::default()));
    options.io_override=Some(Arc::new(UnavailableCapture));
    let handle=OtherHandle::spawn(target,queue,options,tokio::sync::watch::channel(None).0);
    let sent=handle.command(RuntimeCommand {operation_id:"request-a".into(),kind:OperationKind::Input,payload:json!({"text":"seguir com a tarefa"})}).await;
    let snapshot=handle.snapshot().await;
    handle.stop().await.unwrap();
    let sent=sent.unwrap();assert_eq!(sent.disposition,hangar_server::runtime::protocol::Disposition::Deferred);
    let snapshot=snapshot.unwrap();
    assert_eq!(snapshot["queue"][0]["text"],"seguir com a tarefa");
    assert_eq!(snapshot["queue"][0]["delivered"],false);
    let durable:serde_json::Value=serde_json::from_slice(&std::fs::read(state_path).unwrap()).unwrap();
    assert_eq!(durable["rows"][0]["text"],"seguir com a tarefa");
}

#[test]
fn other_terminal_queue_identity_preserves_generation_and_refuses_unreadable_or_foreign_state() {
    use hangar_server::runtime::other_terminal::queue_identity;
    let dir=tempfile::tempdir().unwrap();
    assert_eq!(queue_identity(dir.path(),"life-a").ok().map(|value|value.0),Some(1));
    std::fs::create_dir(dir.path().join("runtime")).unwrap();
    let path=dir.path().join("runtime/life-a.json");
    let state=hangar_server::runtime::queue::State::new("life-a",7,"pi-a",vec![]);
    std::fs::write(&path,serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(queue_identity(dir.path(),"life-a").ok().map(|value|value.0),Some(7));
    std::fs::write(&path,b"{").unwrap();
    assert!(queue_identity(dir.path(),"life-a").is_err());
    std::fs::write(&path,serde_json::to_vec(&hangar_server::runtime::queue::State::new("other-life",9,"pi-a",vec![])).unwrap()).unwrap();
    assert!(queue_identity(dir.path(),"life-a").is_err());
    assert!(queue_identity(dir.path(),"../life-a").is_err());
}

fn directories(root:&std::path::Path)->hangar_server::list::discover_other::Dirs {
    hangar_server::list::discover_other::Dirs {home:root.into(),claude:root.join(".claude"),codex_home:root.join(".codex"),pi_sessions:root.join(".pi/agent/sessions"),omp_config:root.join(".omp"),omp_agent:root.join(".omp/agent"),kimi_home:root.join(".kimi-code")}
}

#[derive(Default)]
struct RecordingIo {writes:std::sync::Mutex<Vec<(String,bool)>>,captures:std::sync::atomic::AtomicUsize,captured:tokio::sync::Notify,before_prove:std::sync::Mutex<Option<Box<dyn FnOnce()+Send>>>}
impl hangar_server::runtime::other_terminal::OtherIo for RecordingIo {
    fn prove<'a>(&'a self,_:&'a hangar_server::runtime::other_terminal::OtherTarget)->futures_util::future::BoxFuture<'a,Result<bool,hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async move {if let Some(action)=self.before_prove.lock().unwrap().take() {action();}Ok(true)})}
    fn capture<'a>(&'a self)->futures_util::future::BoxFuture<'a,Result<hangar_server::runtime::other_terminal::OtherFrame,hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async move {self.captures.fetch_add(1,std::sync::atomic::Ordering::SeqCst);self.captured.notify_one();Ok(hangar_server::runtime::other_terminal::OtherFrame::from_styled("────────────────────────────────\n\n────────────────────────────────\n".into()))})}
    fn write<'a>(&'a self,_:&'a hangar_server::runtime::other_terminal::OtherTarget,text:&'a str,literal:bool)->futures_util::future::BoxFuture<'a,Result<(),hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async move {self.writes.lock().unwrap().push((text.into(),literal));Ok(())})}
    fn release<'a>(&'a self)->futures_util::future::BoxFuture<'a,()> {Box::pin(async {})}
}
fn start_recording(root:&std::path::Path,provider:&str,inbox:std::sync::Arc<hangar_server::provider_models::inbox::Store>,io:std::sync::Arc<RecordingIo>)->hangar_server::runtime::other_terminal::OtherHandle {
    start_recording_with_tick(root,provider,inbox,io,std::time::Duration::from_secs(3600))
}
fn start_recording_with_tick(root:&std::path::Path,provider:&str,inbox:std::sync::Arc<hangar_server::provider_models::inbox::Store>,io:std::sync::Arc<RecordingIo>,tick:std::time::Duration)->hangar_server::runtime::other_terminal::OtherHandle {
    use hangar_server::runtime::{other_terminal::{OtherTarget,OtherOptions,OtherHandle},queue::{QueueActor,Store,State,acquire_lease}};
    let state_path=root.join("state.json");let lease_path=root.join("state.lock");
    let store=Store::open(&state_path,&root.join("queue"),State::new("life-a",1,"pi-a",vec![])).unwrap();
    let queue=QueueActor::start(store,acquire_lease(&lease_path).unwrap());
    let transcript=root.join("conversation-a.jsonl");std::fs::write(&transcript,b"").unwrap();
    let target=OtherTarget {key:"life-a".into(),generation:1,name:"pi-a".into(),provider:hangar_transcript::Provider::parse(provider).unwrap(),
        binding:hangar_server::terminal_input::TerminalBinding {name:"pi-a".into(),pane:"%1".into(),conversation:"conversation-a".into(),generation:1,created:100,mux_argv:vec!["/does-not-exist".into()],windows:false,clipboard_lock_path:None},
        pane_address:"%1".into(),dirs:directories(root),cwd:root.to_string_lossy().into_owned(),pane_pid:123,agent_pid:123,agent_birth:100.0,transcript,state_path,lease_path,projection_dir:root.join("queue"),config_dirs:vec![root.into()],created:100.0};
    let mut options=OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(),inbox,std::sync::Arc::new(hangar_server::list::procs::SystemProcs::default()),std::sync::Arc::new(hangar_server::interactions::InteractionStore::default()));
    options.io_override=Some(io);options.tick=tick;
    OtherHandle::spawn(target,queue,options,tokio::sync::watch::channel(None).0)
}
#[tokio::test]
async fn other_terminal_inbox_ack_is_delivery_without_any_keystroke() {
    use hangar_server::runtime::protocol::{RuntimeCommand,OperationKind,Disposition};
    use std::sync::Arc;
    for provider in ["pi","omp"] {
        let root=tempfile::tempdir().unwrap();let inbox=hangar_server::provider_models::inbox::Store::new();
        let (outgoing,mut incoming)=tokio::sync::mpsc::unbounded_channel();let registered=inbox.register("pi-a",outgoing);
        let io=Arc::new(RecordingIo::default());let handle=start_recording(root.path(),provider,inbox.clone(),io.clone());
        let cloned=handle.clone();let mut sending=tokio::spawn(async move {cloned.command(RuntimeCommand {operation_id:"request-a".into(),kind:OperationKind::Input,payload:json!({"text":"seguir com a tarefa"})}).await});
        let (frame,result)=tokio::select! {
            frame=incoming.recv()=>{let frame=frame.unwrap();inbox.respond("pi-a",&json!({"id":frame["id"],"ok":true}));(Some(frame),sending.await.unwrap())},
            result=&mut sending=>(None,result.unwrap()),
        };
        let snapshot=handle.snapshot().await.unwrap();handle.stop().await.unwrap();inbox.remove("pi-a",&registered);
        assert_eq!(result.unwrap().disposition,Disposition::Accepted);
        assert_eq!(frame.as_ref().map(|frame|&frame["text"]),Some(&json!("seguir com a tarefa")));
        assert_eq!(frame.unwrap()["id"],snapshot["queue"][0]["id"]);
        assert_eq!(snapshot["queue"][0]["delivered"],true);
        assert!(io.writes.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn other_terminal_disconnect_during_input_never_falls_back_or_redrains_the_same_entry() {
    use hangar_server::runtime::protocol::{RuntimeCommand,OperationKind,Disposition};
    use std::sync::Arc;
    let root=tempfile::tempdir().unwrap();let inbox=hangar_server::provider_models::inbox::Store::new();
    let (outgoing,mut incoming)=tokio::sync::mpsc::unbounded_channel();let registered=inbox.register("pi-a",outgoing);
    let io=Arc::new(RecordingIo::default());let handle=start_recording(root.path(),"pi",inbox.clone(),io.clone());
    let cloned=handle.clone();let mut sending=tokio::spawn(async move {cloned.command(RuntimeCommand {operation_id:"request-a".into(),kind:OperationKind::Input,payload:json!({"text":"seguir com a tarefa"})}).await});
    let mut extra=None;
    let (observed,result)=tokio::select! {
        frame=incoming.recv()=>{
            let observed=frame.is_some();let mut draining=Box::pin(handle.drain());
            assert!(futures_util::poll!(&mut draining).is_pending(),"o drain foi submetido enquanto a entrega espera ACK");
            inbox.remove("pi-a",&registered);
            let (new_tx,mut new_rx)=tokio::sync::mpsc::unbounded_channel();let new_line=inbox.register("pi-a",new_tx);
            let result=sending.await.unwrap();draining.await.unwrap();extra=new_rx.try_recv().ok();inbox.remove("pi-a",&new_line);
            (observed,result)
        },
        result=&mut sending=>(false,result.unwrap()),
    };
    let snapshot=handle.snapshot().await.unwrap();handle.stop().await.unwrap();inbox.remove("pi-a",&registered);
    assert!(observed,"a tentativa deve chegar ao canal nativo antes da desconexão controlada");
    assert_eq!(result.unwrap().disposition,Disposition::Unknown);
    assert!(extra.is_none(),"drain não pode tentar de novo após escrita possível");
    assert_eq!(snapshot["queue"][0]["delivered"],true);
    assert_eq!(snapshot["queue"][0]["confirmed"],serde_json::Value::Null);
    assert!(io.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn other_terminal_observes_a_working_hook_without_a_frontend_or_claude_spinner_expiry() {
    use std::sync::{Arc,atomic::Ordering};
    let root=tempfile::tempdir().unwrap();std::fs::create_dir(root.path().join(".hangar-state")).unwrap();
    std::fs::write(root.path().join(".hangar-state/conversation-a.json"),br#"{"state":"working","ts":1786453187}"#).unwrap();
    let io=Arc::new(RecordingIo::default());
    let handle=start_recording_with_tick(root.path(),"pi",hangar_server::provider_models::inbox::Store::new(),io.clone(),std::time::Duration::from_millis(10));
    let observed=tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {let notified=io.captured.notified();if io.captures.load(Ordering::SeqCst)>=14 {break;}notified.await;}
    }).await.is_ok();
    let snapshot=handle.snapshot().await.unwrap();handle.stop().await.unwrap();
    assert!(observed,"o produtor deve rodar sem leitor de frontend");
    assert_eq!(snapshot["view"]["public_state"]["state"],"working");
    assert_eq!(snapshot["view"]["alive"],true);
}

#[tokio::test]
async fn other_terminal_capture_gate_accepts_real_provider_names_and_still_refuses_unknown_ones() {
    use hangar_server::terminal_control::{TerminalPool,CaptureRequest,Limits};
    let root=tempfile::tempdir().unwrap();
    let pool=TerminalPool::with_program(root.path().join("absent-multiplexer"),None,Limits::default());
    for provider in ["pi","omp","kimi"] {
        let request=CaptureRequest {consumer:format!("test-{provider}"),name:"session-a".into(),provider:provider.into(),binding:"life-a".into(),target:"%1".into(),started:100.0,lines:20,colors:true,join:false};
        let result=pool.acquire(request).await;
        pool.release(&format!("test-{provider}")).await.unwrap();
        assert!(!matches!(result,Err(error) if error.0=="invalid capture request"),"provedor reconhecido precisa passar pelo portão");
    }
    let request=CaptureRequest {consumer:"test-unknown".into(),name:"session-a".into(),provider:"unsupported-provider".into(),binding:"life-a".into(),target:"%1".into(),started:100.0,lines:20,colors:true,join:false};
    let result=pool.acquire(request).await;
    assert!(matches!(result,Err(error) if error.0=="invalid capture request"));
}

#[tokio::test]
async fn other_terminal_disconnect_before_write_stays_pending_and_reconnect_drains_the_same_entry() {
    use hangar_server::runtime::protocol::{RuntimeCommand,OperationKind,Disposition};
    use std::sync::Arc;
    let root=tempfile::tempdir().unwrap();let inbox=hangar_server::provider_models::inbox::Store::new();
    let (outgoing,mut incoming)=tokio::sync::mpsc::unbounded_channel();let registered=inbox.register("pi-a",outgoing);
    let io=Arc::new(RecordingIo::default());let handle=start_recording(root.path(),"pi",inbox.clone(),io.clone());
    let mut frames=handle.last_frame();
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {if frames.borrow().is_some() {break;}frames.changed().await.unwrap();}
    }).await.expect("primeira rodada deve terminar antes de armar a desconexão");
    let (closing,line)=(inbox.clone(),registered.clone());
    *io.before_prove.lock().unwrap()=Some(Box::new(move ||closing.remove("pi-a",&line)));
    let first=handle.command(RuntimeCommand {operation_id:"request-a".into(),kind:OperationKind::Input,payload:json!({"text":"seguir com a tarefa"})}).await;
    let pending=handle.snapshot().await.unwrap();let original_frame=incoming.try_recv().ok();
    let (new_tx,mut new_rx)=tokio::sync::mpsc::unbounded_channel();let new_line=inbox.register("pi-a",new_tx);
    let mut draining=Box::pin(handle.drain());
    let (frame,result)=tokio::select! {
        frame=new_rx.recv()=>{let frame=frame.unwrap();inbox.respond("pi-a",&json!({"id":frame["id"],"ok":true}));(Some(frame),draining.await)},
        result=&mut draining=>(None,result),
    };
    let delivered=handle.snapshot().await.unwrap();
    let repeated=handle.drain().await;let duplicate=new_rx.try_recv().ok();
    handle.stop().await.unwrap();inbox.remove("pi-a",&new_line);
    assert_eq!(first.as_ref().ok().map(|reply|reply.disposition),Some(Disposition::Deferred),"ausência comprovada antes da escrita conserva o pedido pendente");
    assert_eq!(pending["queue"][0]["delivered"],false);
    assert!(original_frame.is_none(),"nada pode ter sido escrito na linha removida");
    assert_eq!(result.unwrap()["drained"],1);
    assert_eq!(frame.unwrap()["id"],pending["queue"][0]["id"]);
    assert_eq!(delivered["queue"][0]["delivered"],true);
    assert_eq!(repeated.unwrap()["drained"],0);
    assert!(duplicate.is_none());assert!(io.writes.lock().unwrap().is_empty());
}
