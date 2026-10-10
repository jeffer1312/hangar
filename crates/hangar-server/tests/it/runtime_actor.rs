use hangar_server::runtime::{actor::*,cano,protocol::*,queue::*};
use serde_json::{Value,json};
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};

async fn setup(reply_before_ack:bool) -> (RuntimeHandle,tokio::task::JoinHandle<usize>,tempfile::TempDir) {
    setup_behavior(reply_before_ack,false).await
}

async fn setup_behavior(reply_before_ack:bool,blocked_rpc:bool) -> (RuntimeHandle,tokio::task::JoinHandle<usize>,tempfile::TempDir) {
    setup_mode(reply_before_ack,blocked_rpc,false).await
}

async fn setup_mode(reply_before_ack:bool,blocked_rpc:bool,compound:bool) -> (RuntimeHandle,tokio::task::JoinHandle<usize>,tempfile::TempDir) {
    setup_recovered(reply_before_ack,blocked_rpc,compound,None,false).await
}

async fn setup_recovered(reply_before_ack:bool,blocked_rpc:bool,compound:bool,previous:Option<tempfile::TempDir>,late_reply:bool) -> (RuntimeHandle,tokio::task::JoinHandle<usize>,tempfile::TempDir) {
    let dir = previous.unwrap_or_else(||tempfile::tempdir().unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state_path = dir.path().join("key.queue-state.json");
    let state_check = state_path.clone();
    let late_frame = if late_reply {
        let state:State = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        let request_id = state.operations.values().find(|phase|phase.payload["logical_id"] == "op-1").unwrap().payload["frame"]["id"].clone();
        Some(json!({"type":"cano_output","frame":json!({"id":request_id,"result":{"data":[]}}).to_string()}))
    } else { None };
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        assert_eq!(header,"secret-test\n");
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        if let Some(frame) = late_frame { reader.get_mut().write_all(format!("{frame}\n").as_bytes()).await.unwrap(); }
        let mut sent = 0;
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap() == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let wire = envelope["operation_id"].as_str().unwrap();
            let state:State = serde_json::from_slice(&std::fs::read(&state_check).unwrap()).unwrap();
            assert!(state.operations[wire].status == Status::Dispatching,"journal precisa preceder os bytes");
            let frame:Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
            sent += 1;
            let result = if compound && frame["method"] == "thread/read" {
                json!({"thread":{"id":"thread-1","status":{"type":"active"},"turns":[{"id":"turn-1","status":"inProgress"}]}})
            } else if frame["method"] == "turn/start" { json!({"turn":{"id":"turn-1","status":"inProgress"}}) }
            else { json!({"data":[]}) };
            let reply = json!({"type":"cano_output","frame":json!({"id":frame["id"],"result":result}).to_string()});
            let ack = json!({"type":"cano_input_ack","operation_id":wire,"outcome":"written"});
            if blocked_rpc && frame["method"] == "model/list" {
                continue;
            }
            let frames = if reply_before_ack { vec![reply,ack] } else { vec![ack,reply] };
            for frame in frames { reader.get_mut().write_all(format!("{frame}\n").as_bytes()).await.unwrap(); }
        }
        sent
    });
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"codex".into(),
        metadata:json!({"name":"session","headless":true,"thread_id":"thread-1","initialized":true,"ready":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:state_path.clone(),projection_dir:dir.path().join("projection"),
        transcript:dir.path().join("chat.jsonl"),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("codex",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap();
    let handle = RuntimeActor::spawn(target,queue,connection,engine);
    (handle,server,dir)
}

fn command() -> RuntimeCommand { RuntimeCommand { operation_id:"op-1".into(),kind:OperationKind::ListModels,payload:json!({}) } }

#[tokio::test]
async fn no_sse_runtime_still_drains() {
    let (handle,server,dir) = setup(true).await;
    handle.queue("append".into(),Action::Append { text:"Olá".into(),delivered:false,ts:None,
        pre_transcript:false,entry_id:Some("input-entry".into()) }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
            if state.operations.get("input-entry").is_some_and(|operation|operation.status == Status::Accepted) { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
}

#[tokio::test]
async fn confirmed_prompt_does_not_consume_next_echo() {
    use hangar_server::runtime::receipt::ReceiptIndex;
    let (handle,server,dir) = setup(true).await;
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path, "").unwrap();
    for id in ["first", "second"] {
        let cursor = ReceiptIndex::new("codex", "thread-1").capture(&path).unwrap();
        handle.queue(format!("{id}:append"),Action::Append { text:"Olá".into(),delivered:true,ts:None,
            pre_transcript:false,entry_id:Some(id.into()) }).await.unwrap();
        handle.queue(format!("{id}:prepare"),Action::Prepare { id:id.into(),entry_id:Some(id.into()),
            payload:json!({"kind":"input"}) }).await.unwrap();
        handle.queue(format!("{id}:cursor"),Action::BindDispatch { id:id.into(),cursor:serde_json::to_value(cursor).unwrap() }).await.unwrap();
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file,"{}",json!({"type":"response_item", "payload":{"type":"message", "role":"user",
            "content":[{"type":"input_text","text":"Olá"}]}})).unwrap();
        assert_eq!(handle.confirm().await.unwrap()["confirmed"],1);
    }
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    assert!(state.rows.iter().all(|row|row["confirmed"] == true));
    // As duas confirmadas: não resta operação que possa casar os ecos, e o uso sai da poda.
    assert!(state.used_occurrences.is_empty());
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),0);
}

#[tokio::test]
async fn row_delivered_before_the_runtime_is_confirmed_once_by_a_later_echo() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    let echo = |text:&str| json!({"timestamp":"2026-10-06T12:00:00.000Z","type":"response_item","payload":{"type":"message",
        "role":"user","content":[{"type":"input_text","text":text}]}}).to_string();
    std::fs::write(&path, format!("{}\n{}\n{}\n",echo("sim"),echo("velha"),echo("continua"))).unwrap();
    // Entregues pelo Python antes da troca: sem operação nem cursor de despacho, já no estado quando
    // o ator sobe. Inseridas com ele vivo, a rodada do primeiro ocioso confirmaria "a" sem ver "b".
    // Desistida não chegou: um "continua" digitado depois não a dá por entregue.
    let rows = [("a","sim",1791287980.0,false),("b","sim",1791287995.0,false),("c","velha",1791290000.0,false),("d","continua",1791287990.0,true)]
        .map(|(id,text,ts,abandoned)|{
            let mut row = json!({"id":id,"text":text,"ts":ts,"delivered":true});
            if abandoned { row["desistiu"] = json!(true); }
            row
        });
    std::fs::write(dir.path().join("key.queue-state.json"),serde_json::to_vec(&State::new("key",1,"session",rows.to_vec())).unwrap()).unwrap();
    let (handle,server,dir) = setup_recovered(true,false,false,Some(dir),false).await;
    let confirmed = || {
        let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
        state.rows.iter().filter(|row|row["confirmed"] == true).filter_map(|row|row["id"].as_str().map(str::to_owned)).collect::<Vec<_>>()
    };
    // A rodada do primeiro ocioso corre junto desta: a contagem é de quem gravou primeiro, o estado não.
    handle.confirm().await.unwrap();
    // A única linha "sim" é do envio mais recente (b), não do perdido (a). A linha "velha" foi
    // gravada antes do envio de "c": não prova a entrega dele.
    assert_eq!(confirmed(),["b"]);
    for index in 0..300 { handle.queue(format!("fill:{index}"),Action::SetRuntimeState { state:json!({}) }).await.unwrap(); }
    // Compactada a fila, a linha usada continua gasta: o outro "sim" não a reaproveita.
    assert_eq!(handle.confirm().await.unwrap()["confirmed"],0);
    assert_eq!(confirmed(),["b"]);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),0);
}

#[tokio::test]
async fn resubmit_after_prune_of_confirmed_row_does_not_send_again() {
    let input = ||RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,payload:json!({"text":"Olá","entry_id":"msg"}) };
    let (handle,server,dir) = setup(true).await;
    assert!(handle.command(input()).await.unwrap().disposition == Disposition::Accepted);
    handle.queue("confirm".into(),Action::Confirm { entry_ids:vec!["msg".into()] }).await.unwrap();
    for index in 0..300 { handle.queue(format!("fill:{index}"),Action::SetRuntimeState { state:json!({}) }).await.unwrap(); }
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    assert!(!state.operations.contains_key("msg"));
    // Ator novo, sem a resposta guardada: a linha confirmada responde e nada vai ao fio.
    let (handle,server,_dir) = setup_recovered(true,false,false,Some(dir),false).await;
    assert!(handle.command(input()).await.unwrap().disposition == Disposition::Accepted);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),0);
}

#[tokio::test]
async fn prepare_before_every_write_and_cli_reply_before_ack_is_final() {
    let (handle,server,dir) = setup(true).await;
    let result = handle.command(command()).await.unwrap();
    assert!(result.disposition == Disposition::Accepted);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    let phase = state.operations.values().find(|operation|operation.payload["logical_id"] == "op-1").unwrap();
    assert_eq!(phase.result["disposition"],"accepted");
}

#[tokio::test]
async fn concurrent_same_id_has_one_dispatch() {
    let (handle,server,_dir) = setup(false).await;
    let (first,second) = tokio::join!(handle.command(command()),handle.command(command()));
    assert!(first.unwrap().disposition == Disposition::Accepted);
    assert!(second.unwrap().disposition == Disposition::Accepted);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
}

#[tokio::test]
async fn stop_joins_io_and_persistence_before_unlock() {
    let (handle,server,dir) = setup(false).await;
    handle.command(command()).await.unwrap();
    handle.stop().await.unwrap();
    server.await.unwrap();
    let _next = crate::lease_when_free(&dir.path().join("key.lock"));
}

#[tokio::test]
async fn published_view_has_durable_state() {
    let (handle,server,dir) = setup(false).await;
    let mut events = handle.subscribe();
    handle.command(command()).await.unwrap();
    while let Ok(event) = events.try_recv() {
        if event.channel == "view" {
            let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
            assert!(state.runtime_state["view"].is_object());
        }
    }
    handle.stop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn operation_id_cannot_be_reused_with_another_payload() {
    let (handle,server,_dir) = setup(false).await;
    handle.command(command()).await.unwrap();
    let mut changed = command();
    changed.payload = json!({"limit":7});
    let Err(error) = handle.command(changed).await else { panic!("ID reutilizada precisa falhar") };
    assert_eq!(error.code,"operation_reused");
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
}

#[tokio::test]
async fn blocked_rpc_does_not_block_interrupt_or_state() {
    let (handle,server,dir) = setup_behavior(false,true).await;
    let request = handle.clone();
    let pending = tokio::spawn(async move { request.command(command()).await });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
            if state.operations.contains_key("op-1") { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5),handle.command(RuntimeCommand {
        operation_id:"interrupt".into(),kind:OperationKind::Interrupt,payload:json!({}) })).await.unwrap().unwrap();
    assert!(result.disposition == Disposition::Accepted);
    assert!(handle.snapshot().await.unwrap()["view"].is_object());
    handle.stop().await.unwrap();
    assert!(pending.await.unwrap().is_err());
    assert_eq!(server.await.unwrap(),2);
}

#[tokio::test]
async fn queue_failure_prevents_write() {
    let (handle,server,dir) = setup(false).await;
    let path = dir.path().join("key.queue-state.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(handle.command(command()).await.is_err());
    assert!(handle.stop().await.is_err());
    assert_eq!(server.await.unwrap(),0);
}

#[tokio::test]
async fn ack_timeout_is_unknown_without_resend() {
    let (handle,server,dir) = setup_behavior(false,true).await;
    let result = handle.command(command()).await.unwrap();
    assert!(result.disposition == Disposition::Unknown);
    let again = handle.command(command()).await.unwrap();
    assert!(again.disposition == Disposition::Unknown);
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    assert!(state.operations["op-1"].status == Status::Unknown);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
}

#[tokio::test]
async fn wire_ids_distinguish_compound_control() {
    let (handle,server,dir) = setup_mode(false,false,true).await;
    let result = handle.command(RuntimeCommand { operation_id:"interrupt".into(),kind:OperationKind::Interrupt,payload:json!({}) }).await.unwrap();
    assert!(result.disposition == Disposition::Accepted);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),2);
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    let phases:Vec<_> = state.operations.values().filter(|operation|operation.payload["logical_id"] == "interrupt:read").collect();
    assert_eq!(phases.len(),1);
    assert_ne!(phases[0].id,"interrupt");
    assert!(state.operations["interrupt"].status == Status::Accepted);
    let final_phase = state.operations.values().find(|operation|operation.payload["logical_id"] == "interrupt").unwrap();
    assert_ne!(final_phase.id,phases[0].id);
}

#[test]
fn unknown_preparation_never_starts_new_mutable_phase() {
    use hangar_server::runtime::codex::Engine;
    let sample = ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 };
    let mut engine = Engine::new(json!({"name":"session","headless":true,"thread_id":"thread-1","ready":true,"initialized":true}),1,sample);
    engine.restore_rpc("mode:settings".into(),&json!({"id":"hangar:1:7","method":"thread/read",
        "params":{"threadId":"thread-1","includeTurns":false}}),0,0);
    let effects = engine.apply(EngineInput::Line(json!({"id":"hangar:1:7","result":{"thread":{"id":"thread-1","model":"test-model"}}})),sample).unwrap();
    assert!(!effects.iter().any(|effect|matches!(effect,Effect::Write { .. })));
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Reply { operation_id,disposition:Disposition::Accepted,.. } if operation_id == "mode:settings")));
}

#[tokio::test]
async fn late_wire_reply_resolves_parent() {
    let (handle,server,dir) = setup_behavior(false,true).await;
    assert!(handle.command(command()).await.unwrap().disposition == Disposition::Unknown);
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1);
    let (handle,server,_dir) = setup_recovered(false,false,false,Some(dir),true).await;
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            if handle.command(command()).await.unwrap().disposition == Disposition::Accepted { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),0);
}

/// Cano Claude falso que só conta o que chega ao fio; a política aponta para uma porta fechada.
async fn setup_claude_unreachable_policy(initialized:bool) -> (RuntimeHandle,tokio::task::JoinHandle<usize>,tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let closed = crate::refused_address();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        let mut sent = 0;
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap() == 0 { break; }
            sent += 1;
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
        }
        sent
    });
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":initialized}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript:dir.path().join("chat.jsonl"),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_policy(PolicyClient::new(closed,"secret".into(),"instance".into()));
    (RuntimeActor::spawn(target,queue,connection,engine),server,dir)
}

#[tokio::test]
async fn input_is_prepared_in_rust_even_when_the_python_policy_is_unreachable() {
    // O preparo do prompt é local: o Python fora do ar não adia mais a entrada.
    let (handle,server,dir) = setup_claude_unreachable_policy(true).await;
    let input = RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,payload:json!({"text":"Olá","entry_id":"msg"}) };
    assert!(handle.command(input).await.unwrap().disposition == Disposition::Accepted);
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    assert!(!state.operations.keys().any(|id|id.contains("prepare_prompt")),"cálculo puro não entra no diário");
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),1,"a mensagem chegou ao fio");
}

#[tokio::test]
async fn turn_end_confirms_the_delivered_input_without_being_asked() {
    // O adapter Python confirmava a fila em todo fim de turno; o ator faz o mesmo sozinho.
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("chat.jsonl");
    std::fs::write(&transcript,"").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let written = transcript.clone();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap() == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let frame:Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            if frame["type"] == "user" {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new().append(true).open(&written).unwrap();
                writeln!(file,"{}",json!({"type":"user","uuid":"u-1","message":{"role":"user","content":"Olá"}})).unwrap();
                let result = json!({"type":"cano_output","frame":json!({"type":"result","subtype":"success","is_error":false}).to_string()});
                reader.get_mut().write_all(format!("{result}\n").as_bytes()).await.unwrap();
            }
        }
    });
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript:transcript.clone(),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap();
    let handle = RuntimeActor::spawn(target,queue,connection,engine);
    let input = RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,payload:json!({"text":"Olá","entry_id":"msg"}) };
    assert!(handle.command(input).await.unwrap().disposition == Disposition::Accepted);
    let path = dir.path().join("key.queue-state.json");
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let state:State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if state.rows.iter().any(|row|row["id"] == "msg" && row["confirmed"] == true) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("fim de turno precisa confirmar a entrada entregue");
    handle.stop().await.unwrap();
    server.await.unwrap();
}

/// Serviço de política HTTP que responde `ok` a tudo e conta as chamadas.
async fn policy_server() -> (std::net::SocketAddr,std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream,_)) = listener.accept().await else { return };
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        if line == "\r\n" { break; }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                    }
                    let mut body = vec![0;length]; reader.read_exact(&mut body).await.unwrap();
                    // GET /internal/quota (sem corpo) não é uma política: responde sem contar.
                    let reply = if body.is_empty() { json!({"windows":[]}).to_string() } else {
                        counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                        let kind = serde_json::from_slice::<Value>(&body).unwrap()["kind"].clone();
                        let data = if kind == "prepare_prompt" { json!({"content":"Olá","notices":[],"native_candidate":false}) } else { json!({}) };
                        json!({"ok":true,"data":data}).to_string()
                    };
                    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}",reply.len());
                    reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                }
            });
        }
    });
    (address,calls)
}

#[tokio::test]
async fn status_formatting_and_state_changes_do_not_rewrite_the_journal() {
    // Uma mensagem custava ~60 regravações do estado: cada format_status passava pelo diário e
    // cada mudança de estado gravava a vista inteira, mesmo sem nada durável mudar.
    status_turn(false).await;
}

#[tokio::test]
async fn a_local_format_failure_is_cosmetic_and_leaves_the_session_alive() {
    // `rate_limit_info` que não é objeto faz o formatador local falhar (o Python também falhava):
    // a sessão só perde a linha de status e o turno termina.
    let problems = status_turn(true).await;
    assert!(problems.iter().any(|problem|problem["error_code"] == "policy_input"),"a falha precisa aparecer: {problems:?}");
}

/// Um turno completo com o cano falso; devolve os `problem` publicados. `bad_rate` manda um
/// `rate_limit_event` com `rate_limit_info` inválido antes do resultado.
async fn status_turn(bad_rate:bool) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("chat.jsonl");
    std::fs::write(&transcript,"").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap() == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            let mut events = vec![json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}),
                json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"oi"}}}),
                json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}),
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"oi"}]}}),
                json!({"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":1}})];
            if bad_rate { events.insert(1,json!({"type":"rate_limit_event","rate_limit_info":"boom"})); }
            for event in events {
                let frame = json!({"type":"cano_output","frame":event.to_string()});
                reader.get_mut().write_all(format!("{frame}\n").as_bytes()).await.unwrap();
            }
        }
    });
    let (policy,calls) = policy_server().await;    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript,created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_policy(PolicyClient::new(policy,"secret".into(),"instance".into()));
    let handle = RuntimeActor::spawn(target,queue,connection,engine);
    let mut events = handle.subscribe();
    let input = RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,payload:json!({"text":"Olá","entry_id":"msg"}) };
    assert!(handle.command(input).await.unwrap().disposition == Disposition::Accepted);
    let mut problems = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        let mut idle_seen = false;
        loop {
            // A condição vale depois de cada evento e também sem evento: o último `idle` pode vir antes do problema ou da chamada.
            if let Ok(event) = tokio::time::timeout(std::time::Duration::from_millis(100),events.recv()).await {
                let event = event.unwrap();
                if event.channel == "problem" { problems.push(event.data.clone()); }
                if event.channel == "state" && event.data["state"] == "idle" { idle_seen = true; }
            }
            // Preparo, status, uso, carimbo e evento desconhecido são locais: o Python não é chamado neste turno.
            if idle_seen && (!bad_rate || !problems.is_empty()) { break; }
        }
    }).await.expect("o turno precisa terminar");
    handle.stop().await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),0,"nenhum serviço deste turno precisa mais do Python");
    server.await.unwrap();
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    assert!(!state.operations.keys().any(|id|id.starts_with("policy:")),"serviço sem efeito não entra no diário");
    let views = state.operations.values().filter(|op|op.payload["kind"] == "set_runtime_state").count();
    assert!(views <= 3,"vista gravada {views} vezes num turno sem mudança durável relevante");
    problems
}

#[tokio::test]
async fn a_queue_refusal_carries_its_reason() {
    // O diário do Python e o log do Rust mostravam só "queue_io"; a frase da fila é fixa e diz a causa.
    let (handle,server,_dir) = setup(true).await;
    let append = ||Action::Append { text:"Olá".into(),delivered:true,ts:None,pre_transcript:false,entry_id:Some("same".into()) };
    handle.queue("first".into(),append()).await.unwrap();
    let error = handle.queue("second".into(),append()).await.unwrap_err();
    assert_eq!(error.code,"queue_io");
    assert!(error.message.contains("entrada da fila já existe"),"{}",error.message);
    handle.stop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn steering_the_queue_without_a_turn_is_refused_and_keeps_the_entry() {
    // Sessão ainda sem inicialização: o drain não entrega, e a entrada só pode mudar pela orientação.
    let (handle,server,dir) = setup_claude_unreachable_policy(false).await;
    handle.queue("append".into(),Action::Append { text:"Depois".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("later".into()) }).await.unwrap();
    let steer = RuntimeCommand { operation_id:"steer-1".into(),kind:OperationKind::SteerQueue,payload:json!({"entry_id":"later"}) };
    let reply = handle.command(steer).await.unwrap();
    assert!(reply.disposition == Disposition::Rejected);
    assert_eq!(reply.payload["error"],"Não há turno em andamento para orientar");
    // A entrada continua na fila e não ganha a marca de entregue: a orientação recusada não escreve nada.
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
            if state.rows.iter().any(|row|row["id"] == "later" && row["delivered"] == false) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap_or_else(|_|panic!("a entrada precisa continuar na fila: {}",std::fs::read_to_string(dir.path().join("key.queue-state.json")).unwrap()));
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),0);
}

#[tokio::test]
async fn local_command_result_confirms_the_slash_entry() {
    // Comando local não vira linha `user`: o `result` com local_command é a prova, como no adapter Python.
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("chat.jsonl");
    std::fs::write(&transcript,"").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let written = transcript.clone();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap() == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let frame:Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            if frame["type"] == "user" {
                let _ = &written;
                let result = json!({"type":"cano_output","frame":json!({"type":"result","subtype":"success","is_error":false,"local_command":"clear"}).to_string()});
                reader.get_mut().write_all(format!("{result}\n").as_bytes()).await.unwrap();
            }
        }
    });
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript:transcript.clone(),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap();
    let handle = RuntimeActor::spawn(target,queue,connection,engine);
    // Outra barra já reivindicada pelo drain e ainda não escrita: o result do /clear não pode confirmá-la.
    handle.queue("claimed".into(),Action::Append { text:"/cost".into(),delivered:true,ts:None,pre_transcript:false,entry_id:Some("next".into()) }).await.unwrap();
    let input = RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,payload:json!({"text":"/clear","entry_id":"msg"}) };
    assert!(handle.command(input).await.unwrap().disposition == Disposition::Accepted);
    let path = dir.path().join("key.queue-state.json");
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let state:State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if state.rows.iter().any(|row|row["id"] == "msg" && row["confirmed"] == true) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("comando local consumido precisa ficar confirmado");
    let state:State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(state.rows.iter().any(|row|row["id"] == "next" && row["confirmed"] != true),"barra ainda não escrita não é confirmada");
    handle.stop().await.unwrap();
    server.await.unwrap();
}


/// Cano Claude falso que, depois do retrato, manda as linhas dadas e aceita tudo o que chega.
async fn claude_cano(lines:Vec<Value>) -> (std::net::SocketAddr,tokio::task::JoinHandle<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        for line in lines {
            let frame = json!({"type":"cano_output","frame":line.to_string()});
            reader.get_mut().write_all(format!("{frame}\n").as_bytes()).await.unwrap();
        }
        let mut sent = 0;
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap() == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            sent += 1;
        }
        sent
    });
    (address,server)
}

async fn claude_actor(dir:&std::path::Path,cano:std::net::SocketAddr,policy:std::net::SocketAddr) -> RuntimeHandle {
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{cano}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_policy(PolicyClient::new(policy,"secret".into(),"instance".into()));
    RuntimeActor::spawn(target,queue,connection,engine)
}

#[tokio::test]
async fn a_failed_sidecar_update_still_hands_the_session_over() {
    // Só status/carimbo/uso/registro são perdoados; o sidecar segura a conversa atual (session_id
    // depois do /clear), e perdê-lo calado deixaria o app lendo o transcript velho.
    let dir = tempfile::tempdir().unwrap();
    let closed = crate::refused_address();
    let (cano,server) = claude_cano(vec![json!({"type":"system","subtype":"init","session_id":"sid-2"})]).await;
    let handle = claude_actor(dir.path(),cano,closed).await;
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            if handle.snapshot().await.unwrap()["error"] == "policy_transport" { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("falha do sidecar precisa pôr a sessão em erro");
    handle.stop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn the_deadline_never_puts_back_a_native_message_that_may_have_been_sent() {
    // O recado nativo sai pelo Python; se ele demora até o prazo de 30 s, a entrada pode já ter sido
    // entregue e não pode voltar para a fila (seria enviada de novo).
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let policy = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream,_)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        if line == "\r\n" { break; }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                    }
                    let mut body = vec![0;length]; reader.read_exact(&mut body).await.unwrap();
                    let kind = serde_json::from_slice::<Value>(&body).unwrap()["kind"].clone();
                    let data = if kind == "prepare_prompt" { json!({"content":"[de: par] oi","notices":[],"native_candidate":true}) }
                        else if kind == "native_message" { tokio::time::sleep(std::time::Duration::from_secs(33)).await; json!({"outcome":"written","msg_id":"m"}) }
                        else { json!({}) };
                    let reply = json!({"ok":true,"data":data}).to_string();
                    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}",reply.len());
                    if reader.get_mut().write_all(response.as_bytes()).await.is_err() { return; }
                }
            });
        }
    });
    let (cano,server) = claude_cano(vec![]).await;
    let handle = claude_actor(dir.path(),cano,policy).await;
    let input = RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,payload:json!({"text":"[de: par] oi","entry_id":"msg"}) };
    let reply = handle.command(input).await.unwrap();
    assert!(reply.disposition == Disposition::Unknown,"prazo com envio nativo em curso é incerto, nunca adiado");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let state:State = serde_json::from_slice(&std::fs::read(dir.path().join("key.queue-state.json")).unwrap()).unwrap();
    assert!(state.rows.iter().any(|row|row["id"] == "msg" && row["delivered"] == true),"a entrada não volta para a fila");
    assert!(state.operations.get("msg").is_some_and(|op|op.status != Status::Deferred));
    handle.stop().await.unwrap();
    assert_eq!(server.await.unwrap(),0,"nada foi escrito no cano: o recado foi pelo Python");
}

// --- Estado e prévia do Codex sem terminal pelo canal em processo (5B Task 9) ---

/// Cano Codex falso: depois do retrato manda o que o teste empurrar; responde e confirma cada escrita.
/// Devolve os métodos escritos no fio.
async fn codex_cano(pendentes:Vec<&'static str>) -> (std::net::SocketAddr,tokio::task::JoinHandle<Vec<String>>,tokio::sync::mpsc::UnboundedSender<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (push,mut pushed) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let (read,mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":pendentes,"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        write.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        let mut methods = Vec::new();
        loop {
            let mut raw = String::new();
            tokio::select! {
                line = pushed.recv() => {
                    let Some(line) = line else { continue };
                    let frame = json!({"type":"cano_output","frame":line.to_string()});
                    if write.write_all(format!("{frame}\n").as_bytes()).await.is_err() { break; }
                }
                n = reader.read_line(&mut raw) => {
                    if n.unwrap_or(0) == 0 { break; }
                    let envelope:Value = serde_json::from_str(&raw).unwrap();
                    let frame:Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
                    methods.push(frame["method"].as_str().unwrap_or("").to_owned());
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    write.write_all(format!("{ack}\n").as_bytes()).await.unwrap();
                    if frame.get("id").is_some() && frame.get("method").is_some() {
                        let result = if frame["method"] == "turn/start" { json!({"turn":{"id":"turn-1","status":"inProgress"}}) } else { json!({"data":[]}) };
                        let reply = json!({"type":"cano_output","frame":json!({"id":frame["id"],"result":result}).to_string()});
                        write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                    }
                }
            }
        }
        methods
    });
    (address,server,push)
}

async fn codex_live_actor(dir:&std::path::Path,cano:std::net::SocketAddr,live:Option<LiveSender>) -> RuntimeHandle {
    codex_live_actor_as(dir,cano,live,true).await
}

/// `headless: false` faz a hidratação recusar: é o ator que sai com erro.
async fn codex_live_actor_as(dir:&std::path::Path,cano:std::net::SocketAddr,live:Option<LiveSender>,headless:bool) -> RuntimeHandle {
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"codex".into(),
        metadata:json!({"name":"session","headless":headless,"thread_id":"thread-1","initialized":true,"ready":headless}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{cano}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let queue = QueueActor::start(store,lease);
    let connection = cano::connect(&target.binding).await.unwrap();
    let mut engine = RuntimeEngine::new("codex",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap();
    if let Some(live) = live { engine = engine.with_live(live); }
    RuntimeActor::spawn(target,queue,connection,engine)
}

async fn live_until(rx:&mut LiveReceiver,what:&str,check:impl Fn(&LiveState)->bool) -> std::sync::Arc<LiveState> {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            if let Some(state) = rx.borrow_and_update().clone() && check(&state) { return state; }
            rx.changed().await.unwrap();
        }
    }).await.unwrap_or_else(|_|panic!("{what}"))
}

fn delta(text:&str) -> Value { json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","delta":text}}) }

#[tokio::test]
async fn codex_preview_goes_to_live_not_to_events() {
    // Prévia no `events` sobe a `revision` e o Python a decodifica: com o canal em processo, ela não vai lá.
    let dir = tempfile::tempdir().unwrap();
    let (cano,server,push) = codex_cano(vec![]).await;
    let (live,mut rx) = tokio::sync::watch::channel(None);
    let handle = codex_live_actor(dir.path(),cano,Some(live)).await;
    let mut events = handle.subscribe();
    // A abertura ainda anda a `revision`; sob carga, por mais que uma janela fixa. Mede depois que ela para.
    let revision = || async { handle.snapshot().await.unwrap()["revision"].as_u64().unwrap() };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let before = loop {
        let seen = revision().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if revision().await == seen { break seen; }
        assert!(std::time::Instant::now() < deadline, "a abertura do ator não assentou");
    };
    while events.try_recv().is_ok() {}
    push.send(delta("olá")).unwrap();
    live_until(&mut rx,"prévia no canal em processo",|s|s.preview == "olá").await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    while let Ok(event) = events.try_recv() {
        assert!(!["preview","thinking","tool"].contains(&event.channel.as_str()),"prévia foi ao events: {}",event.channel);
    }
    assert_eq!(handle.snapshot().await.unwrap()["revision"].as_u64().unwrap(),before,"a revision não anda com a prévia");
    handle.stop().await.unwrap();
    drop(push);
    server.abort();
}

#[tokio::test]
async fn codex_view_and_actor_error_reach_live() {
    let dir = tempfile::tempdir().unwrap();
    let (cano,server,push) = codex_cano(vec![]).await;
    let (live,mut rx) = tokio::sync::watch::channel(None);
    let handle = codex_live_actor(dir.path(),cano,Some(live)).await;
    // `Job::View` leva o estado público ao canal.
    push.send(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}})).unwrap();
    live_until(&mut rx,"vista trabalhando no canal",|s|s.public_state["state"] == "working" && s.error.is_none()).await;
    handle.stop().await.unwrap();
    server.abort();
    // Ator que sai com erro deixa o erro marcado.
    let dir = tempfile::tempdir().unwrap();
    let (cano,server,_push) = codex_cano(vec![]).await;
    let (live,mut rx) = tokio::sync::watch::channel(None);
    let live_tx = live.clone();
    let handle = codex_live_actor_as(dir.path(),cano,Some(live),false).await;
    let state = live_until(&mut rx,"erro do ator no canal",|s|s.error.is_some()).await;
    assert!(!state.error.as_ref().unwrap().0.is_empty());
    let _ = handle.stop().await;
    server.abort();
    // O ator que volta (reaberto no mesmo canal) começa sem o erro.
    let (sender,mut rx) = (live_tx,rx);
    let dir = tempfile::tempdir().unwrap();
    let (cano,server,_push) = codex_cano(vec![]).await;
    let handle = codex_live_actor(dir.path(),cano,Some(sender)).await;
    live_until(&mut rx,"erro some com o ator de volta",|s|s.error.is_none()).await;
    handle.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn claude_headless_view_and_preview_go_to_live_not_to_events() {
    let dir = tempfile::tempdir().unwrap();
    let (cano,server) = claude_cano(vec![json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"oi"}}})]).await;
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{cano}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript:dir.path().join("chat.jsonl"),created:0.0 };
    let lease = acquire_lease(&target.lease_path).unwrap();
    let store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
    let (events,mut rx) = tokio::sync::broadcast::channel(64);
    let (live,mut live_rx) = tokio::sync::watch::channel(None);
    let engine = RuntimeEngine::new("claude",target.metadata.clone(),1,ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 }).unwrap()
        .with_publisher(events).with_live(live);
    let connection = cano::connect(&target.binding).await.unwrap();
    let handle = RuntimeActor::spawn(target,QueueActor::start(store,lease),connection,engine);
    // Turno que a CLI já tocava (reabertura): a vista sai `working` junto com a prévia.
    live_until(&mut live_rx,"prévia do Claude sem terminal no canal em processo",
        |s|s.preview == "oi" && s.public_state["state"] == "working").await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    while let Ok(event) = rx.try_recv() {
        assert!(!["preview","thinking","tool"].contains(&event.channel.as_str()),"prévia foi ao events: {}",event.channel);
    }
    handle.stop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn queued_input_drains_without_any_subscriber() {
    // O gatilho de entrega do `sse.py` some para o Codex: o ator drena sozinho no fim do turno.
    let dir = tempfile::tempdir().unwrap();
    let (cano,server,push) = codex_cano(vec![]).await;
    let (live,mut rx) = tokio::sync::watch::channel(None);
    let handle = codex_live_actor(dir.path(),cano,Some(live)).await;
    let first = handle.command(RuntimeCommand { operation_id:"in-1".into(),kind:OperationKind::Input,payload:json!({"text":"um"}) }).await.unwrap();
    assert!(first.disposition == Disposition::Accepted);
    push.send(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}})).unwrap();
    live_until(&mut rx,"turno aberto",|s|s.public_state["state"] == "working").await;
    handle.queue("append-2".into(),Action::Append { text:"dois".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("in-2".into()) }).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let path = dir.path().join("key.queue-state.json");
    let sent = |path:&std::path::Path| { let state:State = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        state.operations.get("in-2").is_some_and(|op|matches!(op.status,Status::Accepted | Status::Confirmed)) };
    assert!(!sent(&path),"com turno rodando a segunda espera");
    push.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async { while !sent(&path) { tokio::time::sleep(std::time::Duration::from_millis(20)).await; } })
        .await.expect("fim do turno drena a fila sem assinante nenhum");
    handle.stop().await.unwrap();
    drop(push);
    let methods = server.await.unwrap();
    assert_eq!(methods.iter().filter(|m|*m == "turn/start").count(),2,"{methods:?}");
}

// --- Codex sem terminal nasce e religa no Rust (5B Task 4) ---

#[cfg(target_os = "linux")]
mod launch {
    use super::*;
    use hangar_server::runtime::gateway::RuntimeRegistry;
    use hangar_server::runtime::process;
    use std::sync::{Arc,Mutex};
    use tokio::io::AsyncReadExt;

    fn unique_key() -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        format!("{:016x}{:04x}",nanos ^ ((std::process::id() as u64) << 32),rand_suffix())
    }
    fn rand_suffix() -> u16 { static N:std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0); N.fetch_add(1,std::sync::atomic::Ordering::Relaxed) }

    use crate::use_cano_bin;

    /// `codex app-server --stdio` falso: responde `initialize` e a abertura da conversa.
    fn fake_codex(dir:&std::path::Path) -> std::path::PathBuf {
        
        let path = dir.join("codex");
        crate::write_executable(&path, format!(r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg or "method" not in msg:
        continue
    method = msg["method"]
    if method == "initialize":
        result = {{"userAgent": "hangar/{} (x)"}}
    elif method in ("thread/start", "thread/resume"):
        result = {{"thread": {{"id": "thread-new", "path": "/tmp/rollout-thread-new.jsonl"}}, "model": "gpt-test"}}
    else:
        result = {{}}
    print(json.dumps({{"id": msg["id"], "result": result}}), flush=True)
"#,hangar_codex::version::CHECKED));
        path
    }

    fn env(key:&str,owner:&str) -> Value {
        let mut env:serde_json::Map<String,Value> = std::env::vars().filter(|(k,_)|!k.starts_with("HANGAR_CANO_")).map(|(k,v)|(k,json!(v))).collect();
        env.insert("HANGAR_CANO_KEY".into(),json!(key));
        env.insert("HANGAR_CANO_OWNER".into(),json!(owner));
        Value::Object(env)
    }

    /// Python falso da política: `launch_env` devolve o `codex` falso; o resto só é anotado.
    async fn policy(launch:Value) -> (std::net::SocketAddr,Arc<Mutex<Vec<(String,Value)>>>) {
        policy_failing(launch,Arc::new(std::sync::atomic::AtomicUsize::new(0))).await
    }
    /// Como `policy`, mas os próximos `fail` pedidos de `launch_env` respondem `codex_ausente`.
    async fn policy_failing(launch:Value,fail:Arc<std::sync::atomic::AtomicUsize>) -> (std::net::SocketAddr,Arc<Mutex<Vec<(String,Value)>>>) {
        policy_checking(launch,fail,None).await
    }
    /// Com `state`, o `session.patch_meta` recusa como velho (`stale`) o campo que difere da vista
    /// salva no arquivo da fila, como o Python faz.
    async fn policy_checking(launch:Value,fail:Arc<std::sync::atomic::AtomicUsize>,state:Option<std::path::PathBuf>) -> (std::net::SocketAddr,Arc<Mutex<Vec<(String,Value)>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream,_)) = listener.accept().await else { return };
                let (seen,launch,fail,state) = (seen.clone(),launch.clone(),fail.clone(),state.clone());
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut length = 0usize;
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                            if line == "\r\n" { break; }
                            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                        }
                        let mut body = vec![0;length]; reader.read_exact(&mut body).await.unwrap();
                        // Só as políticas interessam; o resto (diário) recebe `{}`.
                        let body:Value = serde_json::from_slice(&body).unwrap_or_default();
                        let kind = body["kind"].as_str().unwrap_or("").to_owned();
                        seen.lock().unwrap().push((kind.clone(),body["payload"].clone()));
                        let failing = kind == "launch_env" && fail.fetch_update(std::sync::atomic::Ordering::SeqCst,std::sync::atomic::Ordering::SeqCst,|left|left.checked_sub(1)).is_ok();
                        let stale = kind == "session.patch_meta" && state.as_ref().is_some_and(|path|{
                            let saved:Value = std::fs::read(path).ok().and_then(|raw|serde_json::from_slice(&raw).ok()).unwrap_or_default();
                            let view = &saved["runtime_state"]["view"];
                            body["payload"].as_object().unwrap().iter().any(|(key,value)|view.get(key).is_some_and(|old|old != value))
                        });
                        let data = if failing { json!({"error":"codex_ausente"}) } else if kind == "launch_env" { launch.clone() }
                            else if stale { json!({"updated":false,"stale":true}) } else if kind == "session.patch_meta" { json!({"updated":true}) } else { json!({}) };
                        if stale { seen.lock().unwrap().push(("stale".into(),body["payload"].clone())); }
                        // Thread nova responde devagar: a vista com ela sai antes, como na corrida real.
                        if state.is_some() && kind == "session.patch_meta" && body["payload"].get("thread_id").is_some() {
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                            seen.lock().unwrap().push(("patched".into(),json!(std::time::SystemTime::now())));
                        }
                        let reply = json!({"ok":true,"data":data}).to_string();
                        let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}",reply.len());
                        if reader.get_mut().write_all(response.as_bytes()).await.is_err() { return; }
                    }
                });
            }
        });
        (address,calls)
    }

    fn target(dir:&std::path::Path,key:&str,binding:CanoBinding) -> RuntimeTarget {
        RuntimeTarget { key:key.into(),generation:1,name:"cx".into(),provider:"codex".into(),
            metadata:json!({"name":"cx","key":key,"headless":true,"cwd":dir}),binding,
            lease_path:dir.join("q.lock"),state_path:dir.join("q.json"),projection_dir:dir.join("projection"),
            transcript:dir.join("rollout.jsonl"),created:0.0 }
    }

    async fn until_ready(registry:&RuntimeRegistry,key:&str) {
        let handle = registry.handle(key,1).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15),async {
            loop {
                if handle.snapshot().await.unwrap()["view"]["ready"] == true { break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.expect("o motor chega a ready no processo novo");
    }

    fn no_cano() -> CanoBinding { CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 } }

    #[tokio::test]
    async fn open_with_launch_and_no_cano_spawns_records_and_gets_ready() {
        use_cano_bin();
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let owner = dir.path().to_string_lossy().into_owned();
        let codex = fake_codex(dir.path());
        let (address,calls) = policy(json!({"program":[codex,"app-server","--stdio"],"env":env(&key,&owner),"cano_extra":{"marca":"m1"}})).await;
        let registry = RuntimeRegistry::new(address,"secret".into(),"instance".into());
        let opened = registry.open_with_launch(target(dir.path(),&key,no_cano()),dir.path().into()).await.unwrap();
        assert_eq!(opened["opened"],true);
        until_ready(&registry,&key).await;
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls[0].0,"launch_env","o ambiente é pedido ao Python a cada subida");
        let patch = calls.iter().find(|(kind,payload)|kind == "session.patch_meta" && payload.get("cano").is_some()).expect("o cano novo vai para o arquivo da sessão");
        let pid = patch.1["cano"]["pid"].as_u64().unwrap() as u32;
        assert_eq!(patch.1["cano"]["versao"],2);
        assert_eq!(patch.1["cano"]["marca"],"m1");
        assert!(matches!(process::liveness(pid,&key),process::Liveness::Ours));
        registry.close(&key,1).await.unwrap();
        let cano:process::Cano = serde_json::from_value(patch.1["cano"].clone()).unwrap();
        process::kill(&cano,&key,dir.path()).await.unwrap();
    }

    #[tokio::test]
    async fn open_with_launch_and_live_cano_connects_without_spawning() {
        use_cano_bin();
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let owner = dir.path().to_string_lossy().into_owned();
        let codex = fake_codex(dir.path());
        let env:Vec<(String,String)> = env(&key,&owner).as_object().unwrap().iter().map(|(k,v)|(k.clone(),v.as_str().unwrap().to_owned())).collect();
        let live = process::spawn(&process::LaunchSpec { provider:process::Provider::Codex,key:key.clone(),cwd:dir.path().into(),
            program:vec![codex.to_string_lossy().into_owned(),"app-server".into(),"--stdio".into()],env,
            cano_extra:Default::default(),sidecar_dir:dir.path().into() }).await.unwrap();
        let (address,calls) = policy(json!({})).await;
        let registry = RuntimeRegistry::new(address,"secret".into(),"instance".into());
        let binding = CanoBinding { pid:live.pid,escuta:live.escuta.clone(),token:live.token.clone(),versao:2 };
        registry.open_with_launch(target(dir.path(),&key,binding),dir.path().into()).await.unwrap();
        until_ready(&registry,&key).await;
        assert!(calls.lock().unwrap().iter().all(|(kind,_)|kind != "launch_env"),"cano vivo da sessão: conecta e não sobe outro");
        registry.close(&key,1).await.unwrap();
        process::kill(&live,&key,dir.path()).await.unwrap();
    }

    // --- Ciclo de vida no Rust (5B Task 5) ---

    #[tokio::test]
    async fn open_without_launch_still_manages_the_process() {
        use_cano_bin();
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let owner = dir.path().to_string_lossy().into_owned();
        let codex = fake_codex(dir.path());
        let env:Vec<(String,String)> = env(&key,&owner).as_object().unwrap().iter().map(|(k,v)|(k.clone(),v.as_str().unwrap().to_owned())).collect();
        let live = process::spawn(&process::LaunchSpec { provider:process::Provider::Codex,key:key.clone(),cwd:dir.path().into(),
            program:vec![codex.to_string_lossy().into_owned(),"app-server".into(),"--stdio".into()],env,
            cano_extra:Default::default(),sidecar_dir:dir.path().into() }).await.unwrap();
        let (address,_calls) = policy(json!({})).await;
        let registry = RuntimeRegistry::new(address,"secret".into(),"instance".into());
        let binding = CanoBinding { pid:live.pid,escuta:live.escuta.clone(),token:live.token.clone(),versao:2 };
        // Reaberta sem subir (a administração do Python): encerrar ainda mata o processo.
        registry.open_managed(target(dir.path(),&key,binding),dir.path().into()).await.unwrap();
        until_ready(&registry,&key).await;
        assert_eq!(registry.close_with_kill(&key,1).await.unwrap(),json!({"closed":true,"killed":true}));
        assert!(matches!(process::liveness(live.pid,&key),process::Liveness::Dead));
    }

    /// `codex` falso que anota cada subida e cada pedido em `calls.jsonl`. Com `crash` na pasta cai no
    /// primeiro pedido (antes de ficar pronto); sem ele, cai quando `die` aparece na pasta. `turn/start` abre um
    /// turno que só `turn/interrupt` fecha.
    fn lifecycle_codex(dir:&std::path::Path) -> std::path::PathBuf {
        
        let path = dir.join("codex");
        crate::write_executable(&path, format!(r#"#!/usr/bin/env python3
import json, os, sys, threading, time
d = os.path.dirname(os.path.abspath(__file__))
def note(entry):
    with open(os.path.join(d, "calls.jsonl"), "a") as f:
        f.write(json.dumps(entry) + "\n")
note({{"start": time.time(), "pid": os.getpid()}})
crash = os.path.exists(os.path.join(d, "crash"))
def watch():
    while not os.path.exists(os.path.join(d, "die")):
        time.sleep(0.02)
    os._exit(1)
if not crash:
    threading.Thread(target=watch, daemon=True).start()
def send(obj):
    print(json.dumps(obj), flush=True)
for line in sys.stdin:
    msg = json.loads(line)
    if "method" not in msg:
        continue
    method = msg["method"]
    note({{"method": method, "params": msg.get("params") or {{}}}})
    if crash:
        os._exit(1)
    if "id" not in msg:
        continue
    if method == "initialize":
        result = {{"userAgent": "hangar/{} (x)"}}
    elif method in ("thread/start", "thread/resume"):
        result = {{"thread": {{"id": "thread-new", "path": "/tmp/rollout-thread-new.jsonl"}}, "model": "gpt-test"}}
    elif method == "turn/start":
        result = {{"turn": {{"id": "turn-1", "status": "inProgress"}}}}
    elif method == "thread/read" and os.path.exists(os.path.join(d, "cut")):
        result = {{"thread": {{"id": "thread-new", "status": {{"type": "idle"}}, "turns": [{{"id": "turn-1", "status": "interrupted", "items": []}}]}}}}
    elif method == "turn/interrupt":
        send({{"id": msg["id"], "result": {{}}}})
        send({{"method": "turn/completed", "params": {{"threadId": "thread-new", "turn": {{"id": "turn-1", "status": "interrupted"}}}}}})
        continue
    else:
        result = {{}}
    send({{"id": msg["id"], "result": result}})
"#,hangar_codex::version::CHECKED));
        path
    }

    fn calls(dir:&std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("calls.jsonl")).unwrap_or_default().lines().map(|line|serde_json::from_str(line).unwrap()).collect()
    }
    fn starts(dir:&std::path::Path) -> Vec<f64> { calls(dir).iter().filter_map(|call|call["start"].as_f64()).collect() }

    /// Pids de cano gravados pelo `session.patch_meta {cano}`, na ordem.
    fn recorded(policy:&Arc<Mutex<Vec<(String,Value)>>>) -> Vec<process::Cano> {
        policy.lock().unwrap().iter().filter(|(kind,payload)|kind == "session.patch_meta" && payload.get("cano").is_some())
            .map(|(_,payload)|serde_json::from_value(payload["cano"].clone()).unwrap()).collect()
    }

    async fn lifecycle_session(dir:&std::path::Path,key:&str,mode:&str,base:std::time::Duration) -> (Arc<RuntimeRegistry>,Arc<Mutex<Vec<(String,Value)>>>) {
        lifecycle_session_failing(dir,key,mode,base,Arc::new(std::sync::atomic::AtomicUsize::new(0))).await
    }
    async fn lifecycle_session_failing(dir:&std::path::Path,key:&str,mode:&str,base:std::time::Duration,fail:Arc<std::sync::atomic::AtomicUsize>)
        -> (Arc<RuntimeRegistry>,Arc<Mutex<Vec<(String,Value)>>>) {
        use_cano_bin();
        let owner = dir.to_string_lossy().into_owned();
        let codex = lifecycle_codex(dir);
        let (address,calls) = policy_failing(json!({"program":[codex,"app-server","--stdio"],"env":env(key,&owner),"cano_extra":{}}),fail).await;
        let registry = Arc::new(RuntimeRegistry::new(address,"secret".into(),"instance".into()).with_respawn_base(base));
        let mut target = target(dir,key,no_cano());
        target.metadata["permission_mode"] = json!(mode);
        registry.open_with_launch(target,dir.into()).await.unwrap();
        until_ready(&registry,key).await;
        (registry,calls)
    }

    async fn until<F:Fn()->bool>(what:&str,check:F) {
        tokio::time::timeout(std::time::Duration::from_secs(20),async { while !check() { tokio::time::sleep(std::time::Duration::from_millis(20)).await; } })
            .await.unwrap_or_else(|_|panic!("{what}"));
    }

    fn control(id:&str,kind:OperationKind,payload:Value) -> RuntimeCommand { RuntimeCommand { operation_id:id.into(),kind,payload } }

    async fn cleanup(registry:&RuntimeRegistry,key:&str,dir:&std::path::Path,calls:&Arc<Mutex<Vec<(String,Value)>>>) {
        let _ = registry.close(key,1).await;
        for cano in recorded(calls) { let _ = process::kill(&cano,key,dir).await; }
    }

    #[tokio::test]
    async fn cano_exit_respawns_with_backoff_up_to_three() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let base = std::time::Duration::from_millis(300);
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",base).await;
        // A partir daqui toda subida cai antes de ficar pronta.
        std::fs::write(dir.path().join("crash"),"").unwrap();
        std::fs::write(dir.path().join("die"),"").unwrap();
        until("três subidas depois da queda",||starts(dir.path()).len() >= 4).await;
        tokio::time::sleep(base * 8 + std::time::Duration::from_secs(1)).await;
        let starts = starts(dir.path());
        assert_eq!(starts.len(),4,"teto de 3 subidas seguidas: {starts:?}");
        for (index,pair) in starts.windows(2).enumerate() {
            let wanted = base.as_secs_f64() * f64::from(1u32 << index);
            assert!(pair[1] - pair[0] >= wanted * 0.9,"espera {index} cresce dobrando: {:?} < {wanted}",pair[1] - pair[0]);
        }
        let handle = registry.handle(&key,1).await.unwrap();
        let snapshot = handle.snapshot().await.unwrap();
        assert_eq!(snapshot["view"]["public_state"]["problema"],"codex_headless_nao_subiu",
            "teto esgotado deixa a sessão com o problema: {}",snapshot["view"]["public_state"]);
        assert_eq!(recorded(&policy).len(),4,"cada subida grava o cano novo");
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    #[tokio::test]
    async fn new_thread_id_is_saved_in_the_view_before_the_patch() {
        // O Python recusa como velho o campo que difere da vista salva: a thread nova tem que estar
        // nela antes do `session.patch_meta {thread_id}`, senão o arquivo da sessão fica sem a thread.
        for _ in 0..5 {
            use_cano_bin();
            let dir = tempfile::tempdir().unwrap();
            let key = unique_key();
            let owner = dir.path().to_string_lossy().into_owned();
            let codex = fake_codex(dir.path());
            let (address,policy) = policy_checking(json!({"program":[codex,"app-server","--stdio"],"env":env(&key,&owner),"cano_extra":{}}),
                Arc::new(std::sync::atomic::AtomicUsize::new(0)),Some(dir.path().join("q.json"))).await;
            let registry = RuntimeRegistry::new(address,"secret".into(),"instance".into());
            let mut events = registry.subscribe();
            let views = Arc::new(Mutex::new(Vec::new()));
            let seen = views.clone();
            let listener = tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    if event.channel == "view" && event.data["conversation"] == "thread-new" { seen.lock().unwrap().push(std::time::SystemTime::now()); }
                }
            });
            registry.open_with_launch(target(dir.path(),&key,no_cano()),dir.path().into()).await.unwrap();
            until_ready(&registry,&key).await;
            until("thread gravada no arquivo da sessão",||policy.lock().unwrap().iter().any(|(kind,_)|kind == "patched")).await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            // O Python só religa com a conversa já no arquivo: a vista com ela sai de novo depois do patch.
            let patched:std::time::SystemTime = policy.lock().unwrap().iter().find(|(kind,_)|kind == "patched")
                .map(|(_,at)|serde_json::from_value(at.clone()).unwrap()).unwrap();
            assert!(views.lock().unwrap().iter().any(|at|*at >= patched),"vista com a thread nova depois do patch");
            listener.abort();
            let stale:Vec<Value> = policy.lock().unwrap().iter().filter(|(kind,_)|kind == "stale").map(|(_,payload)|payload.clone()).collect();
            assert!(stale.is_empty(),"patch recusado como velho: {stale:?}");
            cleanup(&registry,&key,dir.path(),&policy).await;
        }
    }

    #[tokio::test]
    async fn stale_patch_shows_a_problem() {
        use_cano_bin();
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let owner = dir.path().to_string_lossy().into_owned();
        let codex = fake_codex(dir.path());
        let other = dir.path().join("other.json");
        std::fs::write(&other,json!({"runtime_state":{"view":{"thread_id":"other"}}}).to_string()).unwrap();
        let (address,policy) = policy_checking(json!({"program":[codex,"app-server","--stdio"],"env":env(&key,&owner),"cano_extra":{}}),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),Some(other)).await;
        let registry = RuntimeRegistry::new(address,"secret".into(),"instance".into());
        registry.open_with_launch(target(dir.path(),&key,no_cano()),dir.path().into()).await.unwrap();
        until_ready(&registry,&key).await;
        tokio::time::timeout(std::time::Duration::from_secs(10),async {
            while handle_view(&registry,&key).await["public_state"]["problema"] != "session_patch_stale" {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("patch velho aparece como problema da sessão");
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    #[tokio::test]
    async fn restart_kills_and_respawns_on_the_same_thread() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",std::time::Duration::from_secs(5)).await;
        let old = recorded(&policy)[0].clone();
        let handle = registry.handle(&key,1).await.unwrap();
        let reply = handle.command(control("restart-1",OperationKind::Restart,json!({}))).await.unwrap();
        assert!(reply.disposition == Disposition::Accepted,"reiniciar responde aceito");
        assert!(matches!(process::liveness(old.pid,&key),process::Liveness::Dead),"o processo antigo morreu");
        let canos = recorded(&policy);
        assert_eq!(canos.len(),2);
        assert!(matches!(process::liveness(canos[1].pid,&key),process::Liveness::Ours));
        until_ready(&registry,&key).await;
        let resumed:Vec<Value> = calls(dir.path()).into_iter().filter(|call|call["method"] == "thread/resume").collect();
        assert_eq!(resumed.last().unwrap()["params"]["threadId"],"thread-new","a conversa continua na mesma thread");
        assert_eq!(starts(dir.path()).len(),2,"um processo novo, nunca dois");
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    #[tokio::test]
    async fn permission_same_sandbox_only_patches_meta() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",std::time::Duration::from_secs(5)).await;
        let handle = registry.handle(&key,1).await.unwrap();
        let reply = handle.command(control("perm-1",OperationKind::SetPermissionMode,json!({"mode":"full access"}))).await.unwrap();
        assert!(reply.disposition == Disposition::Accepted);
        assert_eq!(reply.payload,json!({"current":"Full Access"}));
        until("modo gravado no arquivo da sessão",||policy.lock().unwrap().iter()
            .any(|(kind,payload)|kind == "session.patch_meta" && payload == &json!({"permission_mode":"Full Access"})))
            .await;
        assert_eq!(starts(dir.path()).len(),1,"mesmo sandbox: nenhum processo novo");
        let unknown = handle.command(control("perm-2",OperationKind::SetPermissionMode,json!({"mode":"turbo"}))).await;
        assert_eq!(unknown.err().unwrap().code,"erro_modo_desconhecido");
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    #[tokio::test]
    async fn permission_other_sandbox_respawns_when_idle_and_refuses_when_busy() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",std::time::Duration::from_secs(5)).await;
        let handle = registry.handle(&key,1).await.unwrap();
        let input = handle.command(RuntimeCommand { operation_id:"in-1".into(),kind:OperationKind::Input,payload:json!({"text":"oi"}) }).await.unwrap();
        assert!(input.disposition == Disposition::Accepted);
        until("turno aberto",||calls(dir.path()).iter().any(|call|call["method"] == "turn/start")).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let busy = handle.command(control("perm-busy",OperationKind::SetPermissionMode,json!({"mode":"Ask for approval"}))).await;
        assert_eq!(busy.err().unwrap().code,"erro_permissao_ocupada");
        assert_eq!(starts(dir.path()).len(),1,"ocupada: nada morre, nada sobe");
        assert!(matches!(process::liveness(recorded(&policy)[0].pid,&key),process::Liveness::Ours));
        handle.command(control("stop-1",OperationKind::Interrupt,json!({}))).await.unwrap();
        until_idle(&handle).await;
        let reply = handle.command(control("perm-idle",OperationKind::SetPermissionMode,json!({"mode":"Ask for approval"}))).await.unwrap();
        assert_eq!(reply.payload,json!({"current":"Ask for approval"}));
        let canos = recorded(&policy);
        assert_eq!(canos.len(),2,"ocioso: processo novo");
        assert!(matches!(process::liveness(canos[0].pid,&key),process::Liveness::Dead));
        let order:Vec<String> = policy.lock().unwrap().iter().map(|(kind,payload)|
            if kind == "session.patch_meta" && payload.get("permission_mode").is_some() { "permission".into() } else { kind.clone() }).collect();
        let patched = order.iter().position(|kind|kind == "permission").expect("modo novo gravado");
        assert!(order[patched..].iter().any(|kind|kind == "launch_env"),"o comando novo é pedido depois de o modo ser gravado: {order:?}");
        until_ready(&registry,&key).await;
        let resumed:Vec<Value> = calls(dir.path()).into_iter().filter(|call|call["method"] == "thread/resume").collect();
        assert_eq!(resumed.last().unwrap()["params"]["sandbox"],"read-only");
        assert_eq!(resumed.last().unwrap()["params"]["threadId"],"thread-new");
        // A lista de modos (`GET /codex-permissions`) lê a vista publicada: a vida nova a mostra com o modo novo.
        let handle = registry.handle(&key,1).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            while handle.snapshot().await.unwrap()["view"]["permission_mode"] != "Ask for approval" { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
        }).await.expect("vista publicada com o modo novo");
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    async fn until_idle(handle:&RuntimeHandle) {
        tokio::time::timeout(std::time::Duration::from_secs(20),async {
            while handle.snapshot().await.unwrap()["view"]["in_progress"] != false { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
        }).await.expect("turno fechado");
    }

    #[tokio::test]
    async fn sandbox_switch_that_fails_to_start_keeps_the_new_mode_for_the_next_life() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let fail = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (registry,policy) = lifecycle_session_failing(dir.path(),&key,"Full Access",std::time::Duration::from_millis(300),fail.clone()).await;
        let handle = registry.handle(&key,1).await.unwrap();
        fail.store(1,std::sync::atomic::Ordering::SeqCst);
        let failed = handle.command(control("perm-1",OperationKind::SetPermissionMode,json!({"mode":"Ask for approval"}))).await;
        assert_eq!(failed.err().unwrap().code,"codex_ausente");
        assert!(matches!(process::liveness(recorded(&policy)[0].pid,&key),process::Liveness::Dead),"o processo antigo já tinha morrido");
        // A religação automática sobe com o modo gravado no arquivo, no comando e no `thread/resume`.
        until("religou depois da falha",||recorded(&policy).len() >= 2).await;
        until_ready(&registry,&key).await;
        let resumed:Vec<Value> = calls(dir.path()).into_iter().filter(|call|call["method"] == "thread/resume").collect();
        assert_eq!(resumed.last().unwrap()["params"]["sandbox"],"read-only");
        let view = handle_view(&registry,&key).await;
        assert_eq!(view["permission_mode"],"Ask for approval");
        assert!(view["public_state"]["problema"].is_null(),"a vida nova pronta não carrega o problema da subida que falhou: {}",view["public_state"]);
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    async fn handle_view(registry:&RuntimeRegistry,key:&str) -> Value {
        registry.handle(key,1).await.unwrap().snapshot().await.unwrap()["view"].clone()
    }

    #[tokio::test]
    async fn process_killed_mid_turn_respawns_and_shows_the_cut_turn() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",std::time::Duration::from_millis(300)).await;
        let handle = registry.handle(&key,1).await.unwrap();
        handle.command(RuntimeCommand { operation_id:"in-1".into(),kind:OperationKind::Input,payload:json!({"text":"oi"}) }).await.unwrap();
        until("turno aberto",||calls(dir.path()).iter().any(|call|call["method"] == "turn/start")).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        std::fs::write(dir.path().join("cut"),"").unwrap();
        // O app-server morre no meio do turno.
        process::kill(&recorded(&policy)[0],&key,dir.path()).await.unwrap();
        until("religou sozinho",||recorded(&policy).len() >= 2).await;
        until_ready(&registry,&key).await;
        tokio::time::timeout(std::time::Duration::from_secs(20),async {
            while handle_view(&registry,&key).await["public_state"]["problema"] != "codex_turno_cortado" {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("a sessão mostra o turno cortado depois de religar");
        assert_eq!(starts(dir.path()).len(),2,"um processo novo, nunca dois");
        cleanup(&registry,&key,dir.path(),&policy).await;
    }

    #[tokio::test]
    async fn registry_live_follows_open_respawn_and_close() {
        // O canal do hub nasce com a sessão aberta no Rust, passa pela religação e esvazia no `close`.
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",std::time::Duration::from_millis(300)).await;
        let rx = registry.live("cx");
        until("estado da sessão aberta no canal",||rx.borrow().as_ref().is_some_and(|s|s.public_state["state"] == "idle")).await;
        process::kill(&recorded(&policy)[0],&key,dir.path()).await.unwrap();
        until("religou sozinho",||recorded(&policy).len() >= 2).await;
        until_ready(&registry,&key).await;
        until("vida nova no mesmo canal",||rx.borrow().as_ref().is_some_and(|s|s.public_state["state"] == "idle" && s.error.is_none())).await;
        cleanup(&registry,&key,dir.path(),&policy).await;
        until("fechar esvazia o canal",||rx.borrow().is_none()).await;
    }

    #[tokio::test]
    async fn close_with_kill_ends_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let key = unique_key();
        let (registry,policy) = lifecycle_session(dir.path(),&key,"Full Access",std::time::Duration::from_secs(5)).await;
        let cano = recorded(&policy)[0].clone();
        let traces = ||std::fs::read_dir(dir.path()).unwrap().flatten()
            .filter(|entry|entry.file_name().to_string_lossy().starts_with(&format!("cano-{}",&key[..16]))).count();
        assert!(traces() > 0,"o cano vivo tem socket e log na pasta");
        let closed = registry.close_with_kill(&key,1).await.unwrap();
        assert_eq!(closed,json!({"closed":true,"killed":true}));
        assert!(matches!(process::liveness(cano.pid,&key),process::Liveness::Dead));
        assert_eq!(traces(),0,"socket e log do cano apagados");
        assert!(registry.handle(&key,1).await.is_err(),"a sessão saiu do Rust");
    }
}
