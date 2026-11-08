//! Costuras públicas da adoção e dos recibos dos agentes com terminal.
use hangar_server::runtime::receipt::ReceiptIndex;
use serde_json::json;
use std::{collections::BTreeMap, fs::OpenOptions, io::Write};

fn line(provider: &str, id: &str, text: &str) -> String {
    // Formatos medidos preservados nas suítes anteriores de transcript Pi e Kimi.
    let value=if provider=="kimi" {json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":text}],"toolCalls":[],"origin":{"kind":"user"},"id":id},"time":1786453187986u64})}
        else {json!({"type":"message","id":id,"timestamp":"2026-01-01T00:00:00.000Z","message":{"role":"user","timestamp":1767225600000u64,"content":[{"type":"text","text":text}]}})};
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
        fn write<'a>(&'a self,_:&'a OtherTarget,_:&'a str,_:bool,_:Option<&'a hangar_server::runtime::other_terminal::OtherWriteGuard>)->futures_util::future::BoxFuture<'a,Result<(),hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async {panic!("captura incerta não autoriza escrever")})}
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

#[tokio::test]
async fn other_queue_contract_append_keeps_the_last_thousand_even_when_all_are_waiting() {
    use hangar_server::runtime::{other_terminal::open_queue, protocol::ClockSample, queue::Action};
    for provider in ["pi", "omp", "kimi"] {
        let root = tempfile::tempdir().unwrap();
        let mut target = adoption_target(root.path());
        target.provider = hangar_transcript::Provider::parse(provider).unwrap();
        std::fs::create_dir_all(&target.projection_dir).unwrap();
        let rows = (0..1000).map(|id| format!("{}\n", json!({"id":id.to_string(),"text":format!("pending-{id}"),"ts":200.0,"delivered":false}))).collect::<String>();
        std::fs::write(target.projection_dir.join("pi-a.jsonl"), rows).unwrap();
        let queue = open_queue(&target).await.unwrap();
        let appended = queue.exec(1, "overflow-append", ClockSample {monotonic_s:0.0,epoch_s:201.0},
            Action::Append {text:"nova".into(),delivered:false,ts:Some(201.0),pre_transcript:false,entry_id:Some("new".into())}).await;
        let state = queue.snapshot().await.unwrap();
        queue.shutdown().await.unwrap();
        assert!(appended.is_ok(), "{provider}: a entrada nova continua sendo aceita: {appended:?}");
        assert_eq!(state.rows.len(), 1000, "{provider}");
        assert_eq!(state.rows[0]["id"], "1", "{provider}");
        assert_eq!(state.rows.last().unwrap()["text"], "nova", "{provider}");
        assert!(state.rows.iter().all(|row| row["id"] != "0"), "{provider}");
    }
}

async fn contract_receipts(provider: &str, initial: Vec<serde_json::Value>, committed: &str, dispatched: bool) -> serde_json::Value {
    use hangar_server::runtime::{other_terminal::*, protocol::ClockSample, queue::{Action,Status}};
    let root = tempfile::tempdir().unwrap();
    let mut target = adoption_target(root.path());
    target.provider = hangar_transcript::Provider::parse(provider).unwrap();
    std::fs::create_dir_all(&target.projection_dir).unwrap();
    std::fs::write(target.projection_dir.join("pi-a.jsonl"), initial.iter().map(|row|format!("{row}\n")).collect::<String>()).unwrap();
    std::fs::write(&target.transcript, "").unwrap();
    let queue = open_queue(&target).await.unwrap();
    let clock = ClockSample {monotonic_s:0.0,epoch_s:400.0};
    if dispatched {
        let cursor = ReceiptIndex::new(provider, &target.binding.conversation).capture(&target.transcript).unwrap();
        for row in &initial {
            let id = row["id"].as_str().unwrap();
            let op = format!("other-send:{id}");
            for (suffix, action) in [
                ("prepare", Action::Prepare {id:op.clone(),entry_id:Some(id.into()),payload:json!({"kind":"input","payload":{"text":row["text"],"conversation":target.binding.conversation,"_terminal_generation":1}})}),
                ("bind", Action::BindDispatch {id:op.clone(),cursor:serde_json::to_value(&cursor).unwrap()}),
                ("begin", Action::BeginDispatch {id:op.clone(),wire_id:format!("terminal:1:{id}"),staged:false}),
                ("finish", Action::Finish {id:op,status:Status::Unknown,result:json!({})}),
            ] {
                queue.exec(1, &format!("{id}-{suffix}"), clock, action).await.unwrap();
            }
        }
    }
    std::fs::write(&target.transcript, line(provider, "echo", committed)).unwrap();
    let mut measured = ReceiptIndex::new(provider, &target.binding.conversation);
    let occurrences = measured.scan(&target.transcript).unwrap();
    assert_eq!(occurrences.len(), 1, "{provider}: o eco completo produz uma ocorrência");
    assert_eq!(occurrences[0].timestamp, Some(if provider == "kimi" {1786453187.986} else {1767225600.0}),
        "{provider}: o bilhete de entrega exige timestamp no envelope nativo");
    let io = std::sync::Arc::new(RecordingIo::default());
    let mut options = OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(),
        hangar_server::provider_models::inbox::Store::new(), std::sync::Arc::new(hangar_server::list::procs::SystemProcs::default()),
        std::sync::Arc::new(hangar_server::interactions::InteractionStore::default()));
    options.io_override = Some(io.clone());
    options.tick = std::time::Duration::from_secs(3600);
    let handle = OtherHandle::spawn(target, queue, options, tokio::sync::watch::channel(None).0);
    handle.confirm().await.unwrap();
    handle.confirm().await.unwrap();
    let snapshot = handle.snapshot().await.unwrap();
    handle.stop().await.unwrap();
    assert!(io.writes.lock().unwrap().is_empty(), "confirmar nunca redigita");
    snapshot
}

#[tokio::test]
async fn other_queue_contract_suffix_confirms_only_the_exact_or_longest_owner_once() {
    let long = "Vamos fazer ate as 23 com o Deepseek dps agnt para e volta amanha";
    for provider in ["pi", "omp", "kimi"] {
        for dispatched in [false, true] {
            let cases = [
                (vec![json!({"id":"Y","text":long,"ts":200.0,"delivered":true})], format!("{long}… eu tinha mandado isso"), vec!["Y"]),
                (vec![json!({"id":"X","text":"pode seguir","ts":200.0,"delivered":true,"attempts":2,"desistiu":true}),
                    json!({"id":"Y","text":"pode seguir com a Task 4 agora","ts":300.0,"delivered":true,"attempts":2})], "pode seguir com a Task 4 agora".into(), vec!["Y"]),
                (vec![json!({"id":"X","text":"Vamos fazer","ts":200.0,"delivered":true,"attempts":2,"desistiu":true}),
                    json!({"id":"Y","text":"Vamos fazer ate as 23 com o Deepseek","ts":300.0,"delivered":true,"attempts":2})], "Vamos fazer ate as 23 com o Deepseek — eu tinha mandado isso".into(), vec!["Y"]),
                (vec![json!({"id":"X","text":"seguir com a tarefa","ts":200.0,"delivered":true}),
                    json!({"id":"Y","text":"seguir com a tarefa","ts":200.0,"delivered":true})], "seguir com a tarefa".into(), vec!["X"]),
                (vec![json!({"id":"X","text":"ok","ts":200.0,"delivered":true,"attempts":2,"desistiu":true})], "ok, vamos fazer".into(), vec![]),
            ];
            for (initial, echo, expected) in cases {
                let snapshot = contract_receipts(provider, initial, &echo, dispatched).await;
                let confirmed = snapshot["queue"].as_array().unwrap().iter().filter(|row| row["confirmed"] == true)
                    .map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>();
                assert_eq!(confirmed, expected, "{provider} dispatched={dispatched} echo={echo}");
            }
        }
    }
}

#[tokio::test]
async fn other_queue_contract_late_echo_rescues_abandoned_legacy_without_an_operation() {
    let text = "Vamos fazer ate as 23 com o Deepseek dps agnt para e volta amanha";
    for provider in ["pi", "omp", "kimi"] {
        for echo in [text.to_owned(), format!("{text}… eu tinha mandado isso")] {
            let snapshot = contract_receipts(provider, vec![json!({
                "id":"late", "text":text, "ts":200.0, "delivered":true,
                "attempts":2, "desistiu":true, "desistiu_ts":300.0,
            })], &echo, false).await;
            let rows = snapshot["queue"].as_array().unwrap();
            assert_eq!(rows.len(), 1, "{provider}: a entrada conserva sua identidade");
            assert_eq!(rows[0]["id"], "late");
            assert_eq!(rows[0]["confirmed"], true, "{provider}: {echo}");
            assert!(rows[0].get("desistiu").is_none(), "{provider}: o eco tardio remove o aviso falso");
            assert_eq!(rows[0]["attempts"], 2, "{provider}: confirmar não inicia outra tentativa");
        }
    }
}

#[derive(Default)]
struct RecordingIo {writes:std::sync::Mutex<Vec<(String,bool)>>,captures:std::sync::atomic::AtomicUsize,captured:tokio::sync::Notify,before_prove:std::sync::Mutex<Option<Box<dyn FnOnce()+Send>>>}
impl hangar_server::runtime::other_terminal::OtherIo for RecordingIo {
    fn prove<'a>(&'a self,_:&'a hangar_server::runtime::other_terminal::OtherTarget)->futures_util::future::BoxFuture<'a,Result<bool,hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async move {if let Some(action)=self.before_prove.lock().unwrap().take() {action();}Ok(true)})}
    fn capture<'a>(&'a self)->futures_util::future::BoxFuture<'a,Result<hangar_server::runtime::other_terminal::OtherFrame,hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async move {self.captures.fetch_add(1,std::sync::atomic::Ordering::SeqCst);self.captured.notify_one();Ok(hangar_server::runtime::other_terminal::OtherFrame::from_styled("────────────────────────────────\n\n────────────────────────────────\n".into()))})}
    fn write<'a>(&'a self,_:&'a hangar_server::runtime::other_terminal::OtherTarget,text:&'a str,literal:bool,_:Option<&'a hangar_server::runtime::other_terminal::OtherWriteGuard>)->futures_util::future::BoxFuture<'a,Result<(),hangar_server::runtime::protocol::RuntimeError>> {Box::pin(async move {self.writes.lock().unwrap().push((text.into(),literal));Ok(())})}
    fn release<'a>(&'a self)->futures_util::future::BoxFuture<'a,()> {Box::pin(async {})}
}
fn start_recording(root:&std::path::Path,provider:&str,inbox:std::sync::Arc<hangar_server::provider_models::inbox::Store>,io:std::sync::Arc<RecordingIo>)->hangar_server::runtime::other_terminal::OtherHandle {
    start_recording_with_tick(root,provider,inbox,io,std::time::Duration::from_secs(3600))
}

#[tokio::test]
async fn other_terminal_receipt_failure_before_writing_keeps_the_entry_waiting_for_recovery() {
    use hangar_server::runtime::queue::Action;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let inbox = hangar_server::provider_models::inbox::Store::new();
    let io = Arc::new(RecordingIo::default());
    let handle = start_recording(root.path(), "pi", inbox.clone(), io.clone());
    let mut frames = handle.last_frame();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while frames.borrow().is_none() { frames.changed().await.unwrap(); }
    }).await.unwrap();
    handle.queue("append-recovery".into(), Action::Append {
        text:"continuar após recuperação".into(),delivered:false,ts:None,pre_transcript:false,
        entry_id:Some("recover-entry".into()),
    }).await.unwrap();
    let path = root.path().join("conversation-a.jsonl");
    let interrupted = path.clone();
    *io.before_prove.lock().unwrap() = Some(Box::new(move || {
        std::fs::remove_file(&interrupted).unwrap();
        std::fs::create_dir(&interrupted).unwrap();
    }));
    assert_eq!(handle.drain().await.unwrap_err().code, "receipt_scan");
    let waiting = handle.snapshot().await.unwrap();
    assert_eq!(waiting["queue"][0]["delivered"], false);
    assert!(io.writes.lock().unwrap().is_empty());
    let durable: serde_json::Value = serde_json::from_slice(&std::fs::read(root.path().join("state.json")).unwrap()).unwrap();
    assert_eq!(durable["rows"][0]["delivered"], false);
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, b"").unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registration = inbox.register("pi-a", tx);
    let sending = { let handle = handle.clone(); tokio::spawn(async move { handle.drain().await }) };
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
    assert_eq!(frame["id"], "recover-entry");
    assert_eq!(frame["text"], "continuar após recuperação");
    inbox.respond("pi-a", &json!({"id":frame["id"],"ok":true}));
    assert_eq!(sending.await.unwrap().unwrap()["drained"], 1);
    assert_eq!(handle.drain().await.unwrap()["drained"], 0);
    assert_eq!(handle.snapshot().await.unwrap()["queue"][0]["delivered"], true);
    assert!(rx.try_recv().is_err());
    assert!(io.writes.lock().unwrap().is_empty());
    handle.stop().await.unwrap();
    inbox.remove("pi-a", &registration);
}
fn start_recording_with_tick(root:&std::path::Path,provider:&str,inbox:std::sync::Arc<hangar_server::provider_models::inbox::Store>,io:std::sync::Arc<RecordingIo>,tick:std::time::Duration)->hangar_server::runtime::other_terminal::OtherHandle {
    start_recording_with_pane(root, provider, inbox, io, tick, "%1")
}
fn start_recording_with_pane(root:&std::path::Path,provider:&str,inbox:std::sync::Arc<hangar_server::provider_models::inbox::Store>,io:std::sync::Arc<RecordingIo>,tick:std::time::Duration,pane:&str)->hangar_server::runtime::other_terminal::OtherHandle {
    use hangar_server::runtime::{other_terminal::{OtherTarget,OtherOptions,OtherHandle},queue::{QueueActor,Store,State,acquire_lease}};
    let state_path=root.join("state.json");let lease_path=root.join("state.lock");
    let store=Store::open(&state_path,&root.join("queue"),State::new("life-a",1,"pi-a",vec![])).unwrap();
    let queue=QueueActor::start(store,acquire_lease(&lease_path).unwrap());
    let transcript=root.join("conversation-a.jsonl");std::fs::write(&transcript,b"").unwrap();
    let target=OtherTarget {key:"life-a".into(),generation:1,name:"pi-a".into(),provider:hangar_transcript::Provider::parse(provider).unwrap(),
        binding:hangar_server::terminal_input::TerminalBinding {name:"pi-a".into(),pane:pane.into(),conversation:"conversation-a".into(),generation:1,created:100,mux_argv:vec!["/does-not-exist".into()],windows:false,clipboard_lock_path:None},
        pane_address:pane.into(),dirs:directories(root),cwd:root.to_string_lossy().into_owned(),pane_pid:123,agent_pid:123,agent_birth:100.0,transcript,state_path,lease_path,projection_dir:root.join("queue"),config_dirs:vec![root.into()],created:100.0};
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

fn adoption_target(root:&std::path::Path)->hangar_server::runtime::other_terminal::OtherTarget {
    use hangar_server::runtime::other_terminal::OtherTarget;
    OtherTarget {key:"life-a".into(),generation:1,name:"pi-a".into(),provider:hangar_transcript::Provider::Pi,
        binding:hangar_server::terminal_input::TerminalBinding {name:"pi-a".into(),pane:"%1".into(),conversation:"conversation-a".into(),generation:1,created:100,mux_argv:vec!["absent-multiplexer".into()],windows:false,clipboard_lock_path:None},
        pane_address:"%1".into(),dirs:directories(root),cwd:root.to_string_lossy().into_owned(),pane_pid:123,agent_pid:123,agent_birth:100.0,
        transcript:root.join("conversation-a.jsonl"),state_path:root.join("runtime/life-a.json"),lease_path:root.join("runtime/life-a.lock"),projection_dir:root.join("queue"),config_dirs:vec![root.into()],created:100.0}
}
#[tokio::test]
async fn other_terminal_legacy_import_preserves_identity_flags_and_the_kickoff_window() {
    use hangar_server::runtime::other_terminal::open_queue;
    let root=tempfile::tempdir().unwrap();let target=adoption_target(root.path());
    std::fs::create_dir_all(&target.projection_dir).unwrap();
    let legacy=vec![json!({"id":"old","text":"vida anterior","ts":1.0,"delivered":false}),
        json!({"id":"waiting","text":"depois","ts":101.0,"delivered":false}),
        json!({"id":"sent","text":"digitado","ts":102.0,"delivered":true}),
        json!({"id":"confirmed","text":"confirmado","ts":103.0,"delivered":true,"confirmed":true}),
        json!({"id":"lost","text":"sem prova","ts":104.0,"delivered":true,"desistiu":true}),
        json!({"id":"kickoff","text":"continuar","ts":90.0,"delivered":false,"pre_transcript":true})];
    let contents=legacy.iter().map(|row|format!("{row}\n")).collect::<String>();
    std::fs::write(target.projection_dir.join("pi-a.jsonl"),contents).unwrap();
    let loaded=match open_queue(&target).await {
        Ok(queue)=>{let rows=queue.snapshot().await.unwrap().rows.clone();queue.shutdown().await.unwrap();Some(rows)},
        Err(_)=>None,
    };
    let fields=loaded.map(|rows|rows.into_iter().map(|row|(row["id"].as_str().unwrap_or_default().to_owned(),row["text"].as_str().unwrap_or_default().to_owned(),row["ts"].as_f64().unwrap_or(0.0),row["delivered"]==true,row["confirmed"]==true,row["desistiu"]==true,row["pre_transcript"]==true)).collect::<Vec<_>>());
    assert_eq!(fields,Some(vec![("waiting".into(),"depois".into(),101.0,false,false,false,false),
        ("sent".into(),"digitado".into(),102.0,true,false,false,false),
        ("confirmed".into(),"confirmado".into(),103.0,true,true,false,false),
        ("lost".into(),"sem prova".into(),104.0,true,false,true,false),
        ("kickoff".into(),"continuar".into(),90.0,false,false,false,true)]));
}
#[tokio::test]
async fn other_terminal_legacy_import_never_overwrites_runtime_and_never_treats_corruption_as_empty() {
    use hangar_server::runtime::{other_terminal::open_queue,queue::{Store,State}};
    let root=tempfile::tempdir().unwrap();let target=adoption_target(root.path());
    let row=json!({"id":"runtime","text":"fonte atual","ts":105.0,"delivered":false});
    let _store=Store::open(&target.state_path,&target.projection_dir,State::new("life-a",1,"pi-a",vec![row.clone()])).unwrap();
    std::fs::write(target.projection_dir.join("pi-a.jsonl"),b"invalid").unwrap();
    let loaded=match open_queue(&target).await {
        Ok(queue)=>{let rows=queue.snapshot().await.unwrap().rows.clone();queue.shutdown().await.unwrap();Some(rows)},
        Err(_)=>None,
    };
    let fields=loaded.map(|rows|rows.into_iter().map(|row|(row["id"].as_str().unwrap_or_default().to_owned(),row["text"].as_str().unwrap_or_default().to_owned(),row["delivered"]==true)).collect::<Vec<_>>());
    assert_eq!(fields,Some(vec![("runtime".into(),"fonte atual".into(),false)]));
    let bad=tempfile::tempdir().unwrap();let target=adoption_target(bad.path());std::fs::create_dir(&target.projection_dir).unwrap();
    let projection=target.projection_dir.join("pi-a.jsonl");std::fs::write(&projection,b"{invalid\n").unwrap();
    let rejected=open_queue(&target).await.is_err();
    assert!(rejected);assert!(!target.state_path.exists());
    assert_eq!(std::fs::read(projection).unwrap(),b"{invalid\n");
}

#[tokio::test]
async fn other_terminal_boundary_ready_adoption_reuses_one_entry_and_exposes_the_same_binding() {
    use hangar_server::runtime::{gateway::RuntimeRegistry,other_terminal::{OtherOptions,OtherDiscoveryState}};
    use std::sync::Arc;
    let root=tempfile::tempdir().unwrap();let target=adoption_target(root.path());std::fs::write(&target.transcript,b"").unwrap();
    let mut options=OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(),hangar_server::provider_models::inbox::Store::new(),Arc::new(hangar_server::list::procs::SystemProcs::default()),Arc::new(hangar_server::interactions::InteractionStore::default()));
    options.io_override=Some(Arc::new(RecordingIo::default()));
    let environment=hangar_server::accounts::environment::AccountEnvironment::from_map(std::collections::BTreeMap::from([
        ("HOME".into(),root.path().to_string_lossy().into_owned()),("USERPROFILE".into(),root.path().to_string_lossy().into_owned())]));
    let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"test-only".into(),"registry-test".into()).with_interactions(options.interactions.clone()).with_codex_environment(environment));
    let found=target.clone();registry.bind_other(options,hangar_server::runtime::other_terminal::OtherDiscovery {find:Arc::new(move |_| {let found=found.clone();Box::pin(async move {Some(Ok(OtherDiscoveryState::Ready(found)))})}),names:Arc::new(||Box::pin(async{Ok(vec![])}))}).unwrap();
    let first=registry.open_other_terminal(target.clone()).await;let second=registry.open_other_terminal(target.clone()).await;
    let writable=registry.writable("pi-a").await.map(|target|(target.key,target.generation,target.provider,target.terminal));
    let info=registry.other_info("pi-a").await.map(|info|(info.provider,info.session_key,info.jsonl,info.lifecycle_id));
    let entries=registry.list_snapshots().await;
    registry.shutdown().await.unwrap();
    assert!(first.is_ok() && second.is_ok());
    for provider in ["pi","omp","kimi"] {
        let owner=hangar_server::session_write::Provider::from_str(provider).map(|provider|hangar_server::session_write::decide(hangar_server::session_write::WriteRoute::Input,provider,true));
        assert_eq!(owner,Some(hangar_server::session_write::Owner::Rust),"a entrada reconhecida não pode voltar ao Python");
    }
    assert_eq!(writable,Some(("life-a".into(),1,"pi".into(),true)));
    assert_eq!(info,Some(("pi".into(),"conversation-a".into(),Some(target.transcript),Some("k:life-a:g:1".into()))));
    assert_eq!(entries.keys().cloned().collect::<Vec<_>>(),vec!["life-a".to_owned()]);
}

#[cfg(unix)]
struct TerminalProcessView { cwd:std::path::PathBuf }
#[cfg(unix)]
impl hangar_server::list::procs::ProcessView for TerminalProcessView {
    fn children(&self,_:std::time::Duration)->std::io::Result<std::sync::Arc<hangar_server::list::procs::ChildrenMap>> {Ok(std::sync::Arc::new(Default::default()))}
    fn argv(&self,_:i64)->Vec<String> {vec!["pi".into()]}
    fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(self.cwd.clone())}
    fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
    fn start_time(&self,_:i64)->Option<f64> {Some(100.0)}
}
#[cfg(unix)]
fn native_question(id:&str)->String {
    format!("{}\n",json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":id,"name":"question","arguments":{"question":"Prosseguir?","options":[{"label":"Sim"},{"label":"Não"}]}}]}}))
}
#[cfg(unix)]
fn process_terminal(root:&std::path::Path,source:bool,swap:bool)->(hangar_server::runtime::other_terminal::OtherTarget,hangar_server::runtime::other_terminal::ProcessOtherIo,std::path::PathBuf) {
    use hangar_server::runtime::other_terminal::{OtherOptions,ProcessOtherIo};
    use std::{os::unix::fs::PermissionsExt,sync::Arc};
    let mut target=adoption_target(root);
    std::fs::write(&target.transcript,native_question("question-a")).unwrap();
    if source {
        let tickets=target.dirs.claude.join(".hangar-pi");std::fs::create_dir_all(&tickets).unwrap();
        std::fs::write(tickets.join("1.json"),json!({"file":target.transcript,"ts":101.0}).to_string()).unwrap();
    }
    let replacement=root.join("next-question.jsonl");std::fs::write(&replacement,native_question("question-b")).unwrap();
    let marker=root.join("written");let program=root.join("fake-mux");
    let quote=|path:&std::path::Path|format!("'{}'",path.to_string_lossy().replace('\'',"'\\''"));
    let changing=if swap {format!("cp {} {}\n",quote(&replacement),quote(&target.transcript))}else{String::new()};
    let script=format!("#!/bin/sh\ncase \"$1\" in\ndisplay-message)\n{changing}printf '%s\\n' 'pi-a\t%1\t100\t123';;\nsend-keys) printf '%s\\n' effect >> {};;\n*) exit 1;;\nesac\n",quote(&marker));
    std::fs::write(&program,script).unwrap();std::fs::set_permissions(&program,std::fs::Permissions::from_mode(0o700)).unwrap();
    target.binding.mux_argv=vec![program.to_string_lossy().into_owned()];
    let options=OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(),hangar_server::provider_models::inbox::Store::new(),Arc::new(TerminalProcessView {cwd:root.into()}),Arc::new(hangar_server::interactions::InteractionStore::default()));
    let io=ProcessOtherIo::new(&target,&options);(target,io,marker)
}
#[cfg(unix)]
#[tokio::test]
async fn other_terminal_boundary_io_refuses_effect_when_the_native_conversation_proof_disappears() {
    use hangar_server::runtime::other_terminal::OtherIo;
    let root=tempfile::tempdir().unwrap();let (target,io,marker)=process_terminal(root.path(),true,false);
    assert!(io.prove(&target).await.unwrap());
    std::fs::remove_file(target.dirs.claude.join(".hangar-pi/1.json")).unwrap();
    let result=io.write(&target,"C-s",false,None).await;
    assert!(result.is_err(),"PID vivo sem conversa comprovada não autoriza tecla");
    assert!(!marker.exists(),"a recusa precisa acontecer antes de qualquer efeito");
}
#[cfg(unix)]
#[tokio::test]
async fn other_terminal_boundary_io_refuses_the_first_key_when_the_question_changes_during_prove() {
    use hangar_server::runtime::other_terminal::{OtherIo,OtherWriteGuard,read_pending_interaction};
    let root=tempfile::tempdir().unwrap();let (target,io,marker)=process_terminal(root.path(),true,true);
    let pending=read_pending_interaction(&target).unwrap().unwrap();
    let guard=OtherWriteGuard {target:pending.target,semantic:pending.semantic};
    let result=io.write(&target,"C-s",false,Some(&guard)).await;
    assert!(result.is_err(),"a pergunta trocada durante o await de prova não recebe a primeira tecla");
    assert!(!marker.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn other_terminal_boundary_io_readonly_retry_rechecks_the_guard_and_keeps_the_unguarded_transport() {
    use hangar_server::terminal_input::{ProcessIo,TerminalIo,CommandRequest,IoFailure};
    use std::os::unix::fs::PermissionsExt;
    for guarded in [true,false] {
        let root=tempfile::tempdir().unwrap();let changed=root.path().join("changed");let written=root.path().join("written");let program=root.path().join("readonly-mux");
        let quote=|path:&std::path::Path|format!("'{}'",path.to_string_lossy().replace('\'',"'\\''"));
        let script=format!("#!/bin/sh\nif [ \"$2\" = '-c' ]; then printf effect > {}; exit 0; fi\nprintf changed > {}\nprintf '%s\\n' 'client is read-only' >&2\nexit 1\n",quote(&written),quote(&changed));
        std::fs::write(&program,script).unwrap();std::fs::set_permissions(&program,std::fs::Permissions::from_mode(0o700)).unwrap();
        let request=CommandRequest {program:program.to_string_lossy().into_owned(),args:vec!["send-keys".into(),"-t".into(),"%1".into(),"Enter".into()],stdin:vec![]};
        let io=ProcessIo::default();
        let result=if guarded {io.command_guarded(request,move|| {let changed=changed.clone();Box::pin(async move {
            if changed.exists(){Err(IoFailure {code:"erro_interacao_desatualizada",may_have_written:false})}else{Ok(())}
        })}).await}else{io.command(request).await};
        if guarded {
            assert!(matches!(result,Err(IoFailure {code:"erro_interacao_desatualizada",may_have_written:false})));
            assert!(!written.exists(),"a reserva do cliente somente de leitura não contorna o cartão trocado");
        }else{
            assert!(result.unwrap().success,"Claude/Codex conservam a reserva já existente");
            assert_eq!(std::fs::read(written).unwrap(),b"effect");
        }
    }
}

#[cfg(unix)]
fn partial_windows_terminal(root:&std::path::Path,refuse:bool)->(hangar_server::runtime::other_terminal::OtherTarget,hangar_server::runtime::other_terminal::ProcessOtherIo,std::path::PathBuf) {
    use hangar_server::runtime::other_terminal::{OtherOptions,ProcessOtherIo};
    use std::sync::Arc;
    let (mut target,_,written)=process_terminal(root,true,false);
    target.binding.windows=true;target.pane_address="=pi-a:0.0".into();
    let checked=root.join("first-effect-checked");let replacement=root.join("next-question.jsonl");
    let quote=|path:&std::path::Path|format!("'{}'",path.to_string_lossy().replace('\'',"'\\''"));
    let status=if refuse {"exit 1"}else{":"};
    // A primeira conferência após a escrita ainda aceita a pergunta. A próxima prova já lê a nova.
    let script=format!("#!/bin/sh\ncase \"$1\" in\ndisplay-message)\ncase \"$5\" in\n*pane_pid*)\nif [ -f {} ]; then\nif [ -f {} ]; then cp {} {}; else printf checked > {}; fi\nfi\nprintf '%s\\n' 'pi-a\t%1\t100\t123';;\n*) printf '%s\\n' 'pi-a\t%1\t100';;\nesac;;\nsend-keys)\nfor argument in \"$@\"; do last=$argument; done\nprintf '%s\\n' \"$last\" >> {}\n{status};;\n*) exit 1;;\nesac\n",quote(&written),quote(&checked),quote(&replacement),quote(&target.transcript),quote(&checked),quote(&written));
    std::fs::write(&target.binding.mux_argv[0],script).unwrap();
    let options=OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(),hangar_server::provider_models::inbox::Store::new(),Arc::new(TerminalProcessView {cwd:root.into()}),Arc::new(hangar_server::interactions::InteractionStore::default()));
    let io=ProcessOtherIo::new(&target,&options);(target,io,written)
}
#[cfg(unix)]
#[tokio::test]
async fn other_terminal_partial_write_preserves_unknown_after_the_first_windows_chunk() {
    use hangar_server::runtime::other_terminal::{OtherIo,OtherWriteGuard,read_pending_interaction};
    let root=tempfile::tempdir().unwrap();let (target,io,written)=partial_windows_terminal(root.path(),false);
    let pending=read_pending_interaction(&target).unwrap().unwrap();let guard=OtherWriteGuard {target:pending.target,semantic:pending.semantic};
    let result=io.write(&target,&"x".repeat(513),true,Some(&guard)).await;
    assert_eq!(std::fs::read_to_string(written).unwrap(),format!("{}\n","x".repeat(512)),"só o primeiro bloco foi escrito antes da troca");
    assert_eq!(result.unwrap_err().code,"other_terminal_delivery_unconfirmed","Unknown não pode virar recusa sem efeito que autoriza repetir o bloco");
}
#[cfg(unix)]
#[tokio::test]
async fn other_terminal_partial_write_preserves_prior_effect_between_windows_lines() {
    use hangar_server::runtime::other_terminal::{OtherIo,OtherWriteGuard,read_pending_interaction};
    let root=tempfile::tempdir().unwrap();let (target,io,written)=partial_windows_terminal(root.path(),false);
    let pending=read_pending_interaction(&target).unwrap().unwrap();let guard=OtherWriteGuard {target:pending.target,semantic:pending.semantic};
    let result=io.write(&target,"primeira\nsegunda",true,Some(&guard)).await;
    assert_eq!(std::fs::read_to_string(written).unwrap(),"primeira\n","o separador e a segunda linha não podem alcançar a pergunta nova");
    assert_eq!(result.unwrap_err().code,"other_terminal_delivery_unconfirmed","uma linha já aceita impede classificar a falha seguinte como ausência de efeito");
}

#[cfg(unix)]
#[tokio::test]
async fn other_terminal_partial_write_distinguishes_no_effect_from_a_missing_key_ack() {
    use hangar_server::runtime::other_terminal::{OtherIo,OtherWriteGuard,read_pending_interaction};
    for wrote in [false,true] {
        let root=tempfile::tempdir().unwrap();
        let (target,io,written)=if wrote {partial_windows_terminal(root.path(),true)}else{process_terminal(root.path(),true,true)};
        let pending=read_pending_interaction(&target).unwrap().unwrap();let guard=OtherWriteGuard {target:pending.target,semantic:pending.semantic};
        let result=io.write(&target,"C-s",false,Some(&guard)).await;
        if wrote {
            assert_eq!(std::fs::read_to_string(written).unwrap(),"C-s\n");
            assert_eq!(result.unwrap_err().code,"other_terminal_delivery_unconfirmed","falha de ACK não comprova ausência da tecla");
        }else{
            assert!(!written.exists());
            assert_eq!(result.unwrap_err().code,"erro_interacao_desatualizada","recusa comprovada antes do efeito conserva o código próprio");
        }
    }
}

#[tokio::test]
async fn other_terminal_kimi_unproved_delivery_becomes_visible_without_redraining_even_when_idle() {
    use hangar_server::runtime::queue::State;
    let root = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let state = State::new("life-a", 1, "pi-a", vec![json!({
        "id":"e1", "text":"MENSAGEM-UNICA", "ts":now - 31.0, "delivered":true,
    })]);
    std::fs::write(root.path().join("state.json"), serde_json::to_vec(&state).unwrap()).unwrap();
    let io = std::sync::Arc::new(RecordingIo::default());
    let handle = start_recording(root.path(), "kimi", hangar_server::provider_models::inbox::Store::new(), io.clone());
    handle.confirm().await.unwrap();
    handle.drain().await.unwrap();
    handle.drain().await.unwrap();
    let snapshot = handle.snapshot().await.unwrap();
    handle.stop().await.unwrap();
    let row = &snapshot["queue"][0];
    assert_eq!(row["delivered"], true);
    assert_eq!(row["desistiu"], true);
    assert!(row.get("confirmed").is_none());
    assert!(row.get("attempts").is_none());
    assert!(io.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn other_terminal_editor_question_reaches_the_posix_pane_without_a_name_registration() {
    use hangar_server::runtime::protocol::{RuntimeCommand,OperationKind};
    let root = tempfile::tempdir().unwrap();
    let inbox = hangar_server::provider_models::inbox::Store::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let line = inbox.register("%33", tx);
    assert!(inbox.line_for(Some("pi-a"), None).is_none());
    let io = std::sync::Arc::new(RecordingIo::default());
    let handle = start_recording_with_pane(root.path(), "pi", inbox.clone(), io, std::time::Duration::from_secs(3600), "%33");
    let mut frames = handle.last_frame();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while frames.borrow().is_none() { frames.changed().await.unwrap(); }
    }).await.unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = seen.clone();
    let replies = inbox.clone();
    let responder = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            observed.lock().unwrap().push(frame.clone());
            replies.respond("%33", &json!({"id":frame["id"],"resposta":""}));
        }
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(8), handle.command(RuntimeCommand {
        operation_id:"pane-editor".into(), kind:OperationKind::Input, payload:json!({"text":"/status"}),
    })).await.unwrap().expect("comando de barra usa entrada direta, sem tentar criar linha na fila");
    assert_eq!(result.payload["queued"], false);
    assert!(handle.snapshot().await.unwrap()["queue"].as_array().unwrap().is_empty());
    handle.stop().await.unwrap();
    inbox.remove("%33", &line);
    responder.abort();
    let frames = seen.lock().unwrap();
    assert!(!frames.is_empty(), "a pergunta sai pelo pane %33 sem linha por nome");
    assert!(frames.iter().all(|frame| frame["pedir"] == "editor"));
}

#[tokio::test]
async fn other_terminal_inbox_ack_reaches_the_posix_pane_without_a_name_registration() {
    use hangar_server::runtime::protocol::{RuntimeCommand,OperationKind,Disposition};
    let root=tempfile::tempdir().unwrap();
    let inbox=hangar_server::provider_models::inbox::Store::new();
    let (outgoing,mut incoming)=tokio::sync::mpsc::unbounded_channel();
    let registered=inbox.register("%33",outgoing);
    let io=std::sync::Arc::new(RecordingIo::default());
    let handle=start_recording_with_pane(root.path(),"pi",inbox.clone(),io.clone(),std::time::Duration::from_secs(3600),"%33");
    let cloned=handle.clone();
    let mut sending=tokio::spawn(async move {cloned.command(RuntimeCommand {operation_id:"pane-request".into(),kind:OperationKind::Input,payload:json!({"text":"seguir com a tarefa"})}).await});
    let (frame,result)=tokio::select! {
        frame=incoming.recv()=>{let frame=frame.unwrap();inbox.respond("%33",&json!({"id":frame["id"],"ok":true}));(Some(frame),sending.await.unwrap())},
        result=&mut sending=>(None,result.unwrap()),
    };
    let snapshot=handle.snapshot().await.unwrap();
    handle.stop().await.unwrap();
    inbox.remove("%33",&registered);
    assert_eq!(result.unwrap().disposition,Disposition::Accepted);
    assert_eq!(frame.as_ref().map(|frame|&frame["text"]),Some(&json!("seguir com a tarefa")));
    assert_eq!(frame.unwrap()["id"],snapshot["queue"][0]["id"]);
    assert!(io.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn other_terminal_kimi_working_preserves_unproved_delivery_until_the_turn_ends() {
    use hangar_server::runtime::queue::State;
    let root = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let state = State::new("life-a", 1, "pi-a", vec![json!({"id":"e1","text":"MENSAGEM-UNICA","ts":now - 31.0,"delivered":true})]);
    std::fs::write(root.path().join("state.json"), serde_json::to_vec(&state).unwrap()).unwrap();
    std::fs::create_dir(root.path().join(".hangar-state")).unwrap();
    std::fs::write(root.path().join(".hangar-state/conversation-a.json"), json!({"state":"working","ts":now}).to_string()).unwrap();
    let io = std::sync::Arc::new(RecordingIo::default());
    let handle = start_recording(root.path(), "kimi", hangar_server::provider_models::inbox::Store::new(), io.clone());
    let mut frames = handle.last_frame();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while frames.borrow().is_none() { frames.changed().await.unwrap(); }
    }).await.unwrap();
    assert_eq!(handle.snapshot().await.unwrap()["view"]["public_state"]["state"], "working");
    handle.confirm().await.unwrap();
    handle.drain().await.unwrap();
    let snapshot = handle.snapshot().await.unwrap();
    handle.stop().await.unwrap();
    assert_eq!(snapshot["queue"][0]["delivered"], true);
    assert_ne!(snapshot["queue"][0]["desistiu"], true);
    assert!(io.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn other_terminal_slash_uses_the_driver_without_creating_a_queue_entry() {
    use hangar_server::runtime::protocol::{RuntimeCommand,OperationKind};
    for provider in ["pi", "omp", "kimi"] {
        let root = tempfile::tempdir().unwrap();
        let io = std::sync::Arc::new(RecordingIo::default());
        let handle = start_recording(root.path(), provider, hangar_server::provider_models::inbox::Store::new(), io.clone());
        let mut frames = handle.last_frame();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while frames.borrow().is_none() { frames.changed().await.unwrap(); }
        }).await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(8), handle.command(RuntimeCommand {
            operation_id:"slash-direct".into(), kind:OperationKind::Input, payload:json!({"text":"/status"}),
        })).await.unwrap().expect("comando de barra chega ao driver");
        assert_eq!(reply.payload["queued"], false, "{provider}");
        assert!(handle.snapshot().await.unwrap()["queue"].as_array().unwrap().is_empty(), "{provider}");
        handle.stop().await.unwrap();
        assert!(io.writes.lock().unwrap().iter().any(|(text,literal)| *literal && text == "/status"), "{provider}");
    }
}
