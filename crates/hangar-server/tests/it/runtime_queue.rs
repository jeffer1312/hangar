use hangar_server::runtime::{protocol::ClockSample, queue::*};
use serde_json::json;

fn clock() -> ClockSample { ClockSample { monotonic_s: 10.0, epoch_s: 1_800_000_000.0 } }
fn fill(store:&mut Store, count:usize, prefix:&str) {
    for index in 0..count { store.exec(1,&format!("{prefix}:{index}"),clock(),Action::SetRuntimeState { state:json!({}) }).unwrap(); }
}
fn receipts(state:&State) -> usize { state.operations.keys().filter(|key|key.starts_with("call::")).count() }

/// O roteiro e o resultado vêm do Python (backend/tests/test_runtime_queue.py): a poda tem de
/// deixar o mesmo estado nos dois donos.
#[test]
fn compaction_matches_python_oracle() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/runtime_queue/compaction.json");
    let oracle:serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("state"),&dir.path().join("projection"),State::new("key",1,"session",vec![])).unwrap();
    let mut results = Vec::new();
    for step in oracle["steps"].as_array().unwrap() {
        let action:Action = serde_json::from_value(step[1].clone()).unwrap();
        results.push(store.exec(1,step[0].as_str().unwrap(),clock(),action).unwrap());
    }
    fill(&mut store,oracle["fill"].as_u64().unwrap() as usize,"fill");
    assert_eq!(json!(results),oracle["results"]);
    assert_eq!(serde_json::to_value(store.state()).unwrap(),oracle["final"]);
    // O eco da primeira continua usado: a segunda, com cursor antes dele, não o reaproveita.
    let mut proof = oracle["steps"].as_array().unwrap().iter().find(|step|step[0] == "proof-first").unwrap()[1]["proof"].clone();
    proof["cursor"] = store.state().operations["second"].dispatch_cursor.clone();
    let proof = serde_json::from_value(proof).unwrap();
    assert_eq!(store.exec(1,"proof-second",clock(),Action::ConfirmOccurrence { id:"second".into(),proof }).unwrap(),json!(false));
}

#[test]
fn state_stays_bounded_with_100kb_replies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state");
    let mut store = Store::open(&path,&dir.path().join("projection"),State::new("key",1,"session",vec![])).unwrap();
    let big = "x".repeat(100_000);
    // 500 mensagens já passam a janela muitas vezes; as 2000 rodam no teste Python (debug é lento).
    for index in 1..=500u64 {
        let (op,wire) = (format!("op-{index}"),format!("wire:op-{index}:{index}"));
        let cursor = json!({"conversation":"c","file_identity":"1:2","offset":index,"anchor":"a"});
        for (call,action) in [
            ("root",Action::Prepare { id:op.clone(),payload:json!({"operation_id":op,"kind":"input"}),entry_id:None }),
            ("wire",Action::Prepare { id:wire.clone(),payload:json!({"logical_id":op,"frame":{"text":"Olá"}}),entry_id:None }),
            ("cursor",Action::BindDispatch { id:wire.clone(),cursor }),
            ("dispatch",Action::BeginDispatch { id:wire.clone(),wire_id:wire.clone(),staged:false }),
            ("ack",Action::Finish { id:wire.clone(),status:Status::Accepted,result:json!({"write_outcome":"written"}) }),
            ("reply",Action::Finish { id:op.clone(),status:Status::Accepted,
                result:json!({"operation_id":op,"disposition":"accepted","payload":{"tool_result":big}}) }),
        ] { store.exec(1,&format!("{call}:{index}"),clock(),action).unwrap(); }
    }
    let state = store.state();
    assert_eq!(receipts(state),256);
    assert!(state.operations.len() - receipts(state) <= 256);
    assert!(std::fs::metadata(&path).unwrap().len() < 1_000_000);
}

/// A intenção grande (quadro com anexo) ia inteira para o recibo de cada fase: MB por gravação.
#[test]
fn phase_receipts_do_not_copy_a_large_intent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state");
    let mut store = Store::open(&path,&dir.path().join("projection"),State::new("key",1,"session",vec![])).unwrap();
    let big = "x".repeat(100_000);
    store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),payload:json!({"kind":"input","frame":{"text":big}}),entry_id:None }).unwrap();
    store.exec(1,"cursor",clock(),Action::BindDispatch { id:"op".into(),cursor:json!({"conversation":"c","file_identity":"1:2","offset":1,"anchor":"a"}) }).unwrap();
    store.exec(1,"dispatch",clock(),Action::BeginDispatch { id:"op".into(),wire_id:"wire:op:1".into(),staged:false }).unwrap();
    let finish = Action::Finish { id:"op".into(),status:Status::Accepted,
        result:json!({"operation_id":"op","disposition":"accepted","payload":{"tool_result":big}}) };
    let first = store.exec(1,"finish",clock(),finish.clone()).unwrap();
    assert_eq!(first["payload"]["frame"]["text"],big, "quem chama agora recebe a operação inteira");
    for call in ["call::prepare","call::cursor","call::dispatch","call::finish"] {
        assert!(serde_json::to_vec(&store.state().operations[call].result).unwrap().len() < 1_000, "{call}");
    }
    // Sobra a intenção na operação e no recibo do Prepare (que confere reuso do identificador).
    assert!(std::fs::metadata(&path).unwrap().len() < 250_000);
    let replay = store.exec(1,"finish",clock(),finish).unwrap();
    for field in ["id","status","entry_id"] { assert_eq!(replay[field],first[field]); }
    assert_eq!(replay["result"]["disposition"],"accepted");
    let reply_id = |value:&serde_json::Value|serde_json::from_value::<hangar_server::runtime::protocol::RuntimeReply>(value["result"].clone()).unwrap().operation_id;
    assert_eq!(reply_id(&replay),reply_id(&first));
    assert!(store.exec(1,"finish",clock(),Action::Finish { id:"op".into(),status:Status::Rejected,result:json!(null) }).is_err());
}

#[test]
fn kept_reply_still_replays_instead_of_resending() {
    use hangar_server::runtime::protocol::{Disposition,RuntimeReply};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("state"),dir.path(),State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"append",clock(),append()).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),payload:json!({"kind":"input"}),entry_id:Some("entry-1".into()) }).unwrap();
    store.exec(1,"begin",clock(),Action::BeginDispatch { id:"op".into(),wire_id:"wire:op:1".into(),staged:false }).unwrap();
    store.exec(1,"reply",clock(),Action::Finish { id:"op".into(),status:Status::Accepted,
        result:json!({"operation_id":"op","disposition":"accepted","payload":{"tool_result":"x".repeat(1000)}}) }).unwrap();
    fill(&mut store,300,"fill");
    // Linha sem recibo: a raiz fica, e o ator ainda lê a resposta guardada (sem o conteúdo).
    let reply:RuntimeReply = serde_json::from_value(store.state().operations["op"].result.clone()).unwrap();
    assert!(reply.disposition == Disposition::Accepted && reply.payload.is_null());
}

#[test]
fn v1_state_shrinks_on_first_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state");
    let old = json!({"id":"old","payload":{},"entry_id":null,"status":"accepted","result":null,"dispatch_cursor":null,"wire_attempts":{}});
    let mut stuck = old.clone(); stuck["id"] = json!("stuck"); stuck["status"] = json!("unknown");
    std::fs::write(&path,serde_json::to_vec(&json!({"version":1,"owner_key":"key","generation":1,"name":"session","rows":[],
        "operations":{"old":old,"stuck":stuck},"used_occurrences":{"legacy":{"operation_id":"old","generation":1}},"runtime_state":{}})).unwrap()).unwrap();
    let store = Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    // Encolhe já na abertura, antes de qualquer gravação nova.
    let saved:serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((saved["version"].clone(),saved["next_seq"].clone()),(json!(2),json!(257)));
    assert_eq!(saved["operations"].as_object().unwrap().keys().collect::<Vec<_>>(),["stuck"]);
    assert!(store.state().used_occurrences.is_empty());
}
fn append() -> Action { Action::Append { text:"Olá".into(), delivered:false, ts:None,
    pre_transcript:false, entry_id:Some("entry-1".into()) } }

/// A CLI responde `/btw` sozinha e não grava a linha: sem isto a bolha esperava para sempre.
#[test]
fn local_command_answer_confirms_the_command_outside_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("state"),&dir.path().join("projection"),State::new("key",1,"session",vec![])).unwrap();
    for (call,text,entry) in [("a0","btw","e0"),("a1","/btw","e1"),("a2","/context","e2")] {
        store.exec(1,call,clock(),Action::Append { text:text.into(),delivered:true,ts:None,pre_transcript:false,entry_id:Some(entry.into()) }).unwrap();
    }
    store.exec(1,"op",clock(),Action::Prepare { id:"op".into(),payload:json!({"operation_id":"op","kind":"input"}),entry_id:Some("e1".into()) }).unwrap();
    store.exec(1,"local",clock(),Action::AppendLocal { text:"/btw isn't available in this environment.".into(),
        entry_id:None,confirms:Some("/btw".into()) }).unwrap();
    let state = store.state();
    let row = |id:&str|state.rows.iter().find(|r|r["id"] == id).unwrap().clone();
    assert_ne!(row("e0")["confirmed"],true, "texto comum igual ao nome do comando não é o comando");
    assert_eq!(row("e1")["confirmed"],true);
    assert_ne!(row("e2")["confirmed"],true);
    assert!(state.operations["op"].status == Status::Confirmed);
}

/// Fila gravada antes da confirmação pela resposta local: a bolha do `/btw` ficava para sempre.
#[test]
fn reopening_confirms_command_answered_locally_before_the_fix() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state");
    let rows = vec![
        json!({"id":"old","text":"/btw","ts":50.0,"delivered":true}),
        json!({"id":"e1","text":"/btw","ts":100.0,"delivered":true}),
        json!({"id":"local:1","text":"/btw isn't available in this environment.","ts":100.1,"delivered":true,"confirmed":true,"papel":"assistant"}),
        json!({"id":"e2","text":"/context","ts":200.0,"delivered":true}),
        json!({"id":"e3","text":"/compact","ts":300.0,"delivered":true}),
        json!({"id":"local:2","text":"⚙️ A CLI pediu `x`; respondi vazio","ts":301.0,"delivered":true,"confirmed":true,"papel":"assistant"}),
        json!({"id":"e4","text":"/btw","ts":400.0,"delivered":true,"desistiu":true}),
        json!({"id":"local:3","text":"/btw isn't available in this environment.","ts":400.1,"delivered":true,"confirmed":true,"papel":"assistant"}),
    ];
    std::fs::write(&state_path,serde_json::to_vec(&State::new("key",1,"session",rows)).unwrap()).unwrap();
    let store = Store::open(&state_path,&dir.path().join("projection"),State::new("key",1,"session",vec![])).unwrap();
    let row = |state:&State,id:&str|state.rows.iter().find(|r|r["id"] == id).unwrap()["confirmed"].clone();
    assert_eq!(row(store.state(),"e1"),true);
    assert_ne!(row(store.state(),"old"),true, "confirma a linha do par, não a mais antiga com o mesmo texto");
    assert_ne!(row(store.state(),"e2"),true, "sem resposta local logo depois, segue esperando o transcript");
    assert_ne!(row(store.state(),"e3"),true, "aviso local que não nomeia o comando não é a resposta dele");
    assert_ne!(row(store.state(),"e4"),true, "desistência continua visível");
    let disk:serde_json::Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(disk["rows"][1]["confirmed"],true, "a confirmação vai ao disco na abertura");
}

#[test]
fn same_operation_does_not_append_twice() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("key.queue-state.json"), &dir.path().join("projection"),
        State::new("key", 1, "session", vec![])).unwrap();
    let first = store.exec(1, "call-1", clock(), append()).unwrap();
    assert_eq!(store.exec(1, "call-1", clock(), append()).unwrap(), first);
    assert_eq!(store.state().rows.len(), 1);
    assert!(store.exec(1, "call-1", clock(), Action::Append { text:"Outro".into(), delivered:false,
        ts:None, pre_transcript:false, entry_id:Some("entry-1".into()) }).is_err());
}

#[test]
fn state_before_projection() {
    let dir = tempfile::tempdir().unwrap();
    let projection = dir.path().join("projection");
    let path = dir.path().join("key.queue-state.json");
    let mut store = Store::open(&path, &projection, State::new("key",1,"session",vec![])).unwrap();
    std::fs::remove_file(projection.join("session.jsonl")).unwrap();
    std::fs::create_dir(projection.join("session.jsonl")).unwrap();
    assert!(store.exec(1, "append", clock(), append()).is_err());
    let state: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(state.rows.len(), 1);
    std::fs::remove_dir(projection.join("session.jsonl")).unwrap();
    assert_eq!(store.exec(1,"append",clock(),append()).unwrap()["id"], "entry-1");
    assert_eq!(store.state().rows.len(), 1);
}

#[test]
fn corrupt_state_is_not_empty_queue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.queue-state.json");
    std::fs::write(&path, "{").unwrap();
    assert!(Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).is_err());
}

#[test]
fn unknown_never_unclaims() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("state"), dir.path(), State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"append",clock(),append()).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),payload:json!({}),entry_id:Some("entry-1".into()) }).unwrap();
    store.exec(1,"dispatch",clock(),Action::BeginDispatch { id:"op".into(),wire_id:"wire:op:1".into(),staged:false }).unwrap();
    store.exec(1,"unknown",clock(),Action::Finish { id:"op".into(),status:Status::Unknown,result:json!(null) }).unwrap();
    assert!(store.exec(1,"unclaim",clock(),Action::SetDelivered { entry_id:"entry-1".into(),value:false,steered:false }).is_err());
    assert_eq!(store.state().rows[0]["delivered"],true);
}

#[test]
fn cap_keeps_pending() {
    let dir = tempfile::tempdir().unwrap();
    let rows = (0..1000).map(|i|json!({"id":i.to_string(),"text":"pending","ts":1,"delivered":true,"desistiu":true})).collect();
    let mut store = Store::open(&dir.path().join("state"),dir.path(),State::new("key",1,"session",rows)).unwrap();
    assert!(store.exec(1,"append",clock(),append()).is_err());
    assert_eq!(store.state().rows.len(),1000);
}

#[test]
fn rename_keeps_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.queue-state.json");
    let projection = dir.path().join("projection");
    let mut store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"append",clock(),append()).unwrap();
    store.exec(1,"rename",clock(),Action::Rename { name:"renamed".into() }).unwrap();
    assert_eq!(store.state().owner_key,"key");
    assert!(path.exists());
    assert!(!projection.join("session.jsonl").exists());
    assert!(projection.join("renamed.jsonl").exists());
}

#[test]
fn unknown_reply_cannot_downgrade_final() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("state"),dir.path(),State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),payload:json!({}),entry_id:None }).unwrap();
    store.exec(1,"accepted",clock(),Action::Finish { id:"op".into(),status:Status::Accepted,result:json!({"ok":true}) }).unwrap();
    store.exec(1,"late",clock(),Action::Finish { id:"op".into(),status:Status::Unknown,result:json!(null) }).unwrap();
    assert!(store.state().operations["op"].status == Status::Accepted);
    assert_eq!(store.state().operations["op"].result,json!({"ok":true}));
}

#[test]
fn late_reply_matches_generation_and_type() {
    use hangar_server::runtime::protocol::RequestId;
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("state"),dir.path(),State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),payload:json!({"request_id":1}),entry_id:None }).unwrap();
    store.exec(1,"begin",clock(),Action::BeginDispatch { id:"op".into(),wire_id:"wire:1".into(),staged:false }).unwrap();
    for (generation,request_id) in [(2,RequestId::Integer(1)),(1,RequestId::String("1".into()))] {
        assert!(store.exec(1,"late",clock(),Action::LateRpcResolution { id:"op".into(),wire_id:"wire:1".into(),
            request_id,generation,result:json!({"ok":true}) }).is_err());
    }
    assert!(store.state().operations["op"].status == Status::Dispatching);
}

#[test]
fn terminal_runtime_store_recovery_only_unclaims_proved_unsent_terminal_claim() {
    for (case,root_status,side_effect,terminal_claim,expected) in [
        ("safe",None,false,true,false),
        ("prepared",Some(Status::Prepared),false,true,false),
        ("unknown",Some(Status::Unknown),false,true,true),
        ("dispatching",Some(Status::Dispatching),false,true,true),
        ("accepted",Some(Status::Accepted),false,true,true),
        ("confirmed",Some(Status::Confirmed),false,true,true),
        ("side_effect",Some(Status::Prepared),true,true,true),
        ("legacy",None,false,false,true),
    ] {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("state"); let projection=dir.path().join("projection");
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
        store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into())}).unwrap();
        store.exec(1,if terminal_claim{"terminal:queue:999"}else{"legacy-claim"},clock,Action::Claim {min_ts:1.0,limit:Some(1),entry_id:None}).unwrap();
        if let Some(status)=root_status {
            store.exec(1,"root",clock,Action::Prepare {id:"root".into(),entry_id:Some("entry".into()),payload:json!({"kind":"input"})}).unwrap();
            if status==Status::Dispatching {store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"root".into(),wire_id:"wire".into(),staged:false}).unwrap();}
            else if status!=Status::Prepared {store.exec(1,"finish",clock,Action::Finish {id:"root".into(),status,result:json!({})}).unwrap();}
        }
        if side_effect {
            store.exec(1,"phase",clock,Action::Prepare {id:"phase".into(),entry_id:None,payload:json!({"logical_id":"root"})}).unwrap();
            store.exec(1,"phase-dispatch",clock,Action::BeginDispatch {id:"phase".into(),wire_id:"rpc".into(),staged:false}).unwrap();
        }
        drop(store); let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
        store.exec(1,"recover",clock,Action::Recover).unwrap(); assert_eq!(store.state().rows[0]["delivered"],expected,"{case}");
    }
}

#[test]
fn terminal_runtime_finish_is_atomic_and_recover_does_not_repeat_retry_accounting() {
    for claimed in [false,true] {for attempts in [0,1,2] {for rejected in [false,true] {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("state"); let projection=dir.path().join("projection");
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap(); let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
        store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:true,entry_id:Some("entry".into())}).unwrap();
        for n in 0..attempts {store.exec(1,&format!("bump:{n}"),clock,Action::BumpAttempts {entry_id:"entry".into()}).unwrap();}
        if claimed {store.exec(1,"terminal:queue:999",clock,Action::Claim {min_ts:1.0,limit:Some(1),entry_id:None}).unwrap();}
        store.exec(1,"terminal:queue:1000",clock,Action::Prepare {id:"attempt".into(),entry_id:Some("entry".into()),payload:json!({"operation_id":"attempt","kind":"input","payload":{"text":"Olá","pre_transcript":true,"_terminal_generation":1}})}).unwrap();
        store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"attempt".into(),wire_id:"terminal:1:attempt".into(),staged:false}).unwrap();
        let status=if rejected{Status::Rejected}else{Status::Deferred}; let result=json!({"operation_id":"attempt","disposition":if rejected{"rejected"}else{"deferred"},"payload":{"cleanup":"proved"}});
        store.exec(1,"finish",clock,Action::Finish {id:"attempt".into(),status,result:result.clone()}).unwrap();
        let expected_abandoned=rejected||attempts==2; assert_eq!(store.state().rows[0]["delivered"],expected_abandoned,"claimed={claimed} attempts={attempts} rejected={rejected}");
        if expected_abandoned {assert_eq!(store.state().rows[0]["desistiu"],true);} else {assert_eq!(store.state().rows[0]["attempts"],attempts+1);}
        let row=store.state().rows[0].clone(); drop(store);
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap(); store.exec(1,"recover1",clock,Action::Recover).unwrap();
        store.exec(1,"finish-new-id",clock,Action::Finish {id:"attempt".into(),status,result}).unwrap(); store.exec(1,"recover2",clock,Action::Recover).unwrap(); assert_eq!(store.state().rows[0],row);
    }}}
}

#[test]
fn terminal_runtime_recover_finishes_legacy_partial_row_transition_once() {
    for claimed in [false,true] {for attempts in [0,1,2] {for rejected in [false,true] {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("state"); let projection=dir.path().join("projection");
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap(); let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
        store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into())}).unwrap();
        if claimed {store.exec(1,"terminal:queue:1000",clock,Action::Claim {min_ts:1.0,limit:Some(1),entry_id:None}).unwrap();}
        store.exec(1,"terminal:queue:1001",clock,Action::Prepare {id:"attempt".into(),entry_id:Some("entry".into()),payload:json!({"operation_id":"attempt","kind":"input","payload":{"text":"Olá","pre_transcript":false}})}).unwrap();
        store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"attempt".into(),wire_id:"terminal:1:attempt".into(),staged:false}).unwrap();
        let mut frozen=serde_json::to_value(store.state()).unwrap(); drop(store);
        frozen["rows"][0]["attempts"]=json!(attempts);
        frozen["operations"]["attempt"]["status"]=json!(if rejected{"rejected"}else{"deferred"});
        frozen["operations"]["attempt"]["result"]=json!({"operation_id":"attempt","disposition":if rejected{"rejected"}else{"deferred"},"payload":{"cleanup":"proved"}});
        std::fs::write(&path,serde_json::to_vec(&frozen).unwrap()).unwrap();
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap(); store.exec(1,"recover1",clock,Action::Recover).unwrap();
        let row=store.state().rows[0].clone(); assert_eq!(row["delivered"],rejected||attempts==2);
        if rejected||attempts==2 {assert_eq!(row["desistiu"],true);}else{assert_eq!(row["attempts"],attempts+1);}
        store.exec(1,"recover2",clock,Action::Recover).unwrap(); assert_eq!(store.state().rows[0],row);
    }}}
}
#[test]
fn terminal_runtime_missing_row_recovery_preserves_uncertain_final_and_headless_inputs() {
    for status in [Status::Unknown,Status::Dispatching,Status::Accepted,Status::Confirmed,Status::Prepared] {
        for marker in [true,false] {
            let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let projection=dir.path().join("projection");
            let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
            let mut payload=json!({"operation_id":"root","kind":"input","payload":{"text":"Olá","pre_transcript":true}});
            if marker {payload["payload"]["_terminal_generation"]=json!(1);}
            store.exec(1,"headless-prepare",clock,Action::Prepare {id:"root".into(),entry_id:Some("entry".into()),payload}).unwrap();
            if status==Status::Dispatching {store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"root".into(),wire_id:"wire".into(),staged:false}).unwrap();}
            else if status!=Status::Prepared {store.exec(1,"finish",clock,Action::Finish {id:"root".into(),status,result:json!({})}).unwrap();}
            if status==Status::Prepared {
                store.exec(1,"phase",clock,Action::Prepare {id:"phase".into(),entry_id:None,payload:json!({"logical_id":"root"})}).unwrap();
                store.exec(1,"phase-dispatch",clock,Action::BeginDispatch {id:"phase".into(),wire_id:"wire".into(),staged:false}).unwrap();
            }
            store.exec(1,"recover",clock,Action::Recover).unwrap();assert!(store.state().rows.is_empty());
        }
    }
}

#[test]
fn terminal_runtime_legacy_completed_finalize_steps_are_not_repeated() {
    for transition in ["after_bump","after_unclaim","after_abandon"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let projection=dir.path().join("projection");
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
        store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into())}).unwrap();
        store.exec(1,"terminal:queue:1",clock,Action::Prepare {id:"attempt".into(),entry_id:Some("entry".into()),payload:json!({"operation_id":"attempt","kind":"input","payload":{"text":"Olá","pre_transcript":false}})}).unwrap();
        store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"attempt".into(),wire_id:"terminal:1:attempt".into(),staged:false}).unwrap();
        let mut frozen=serde_json::to_value(store.state()).unwrap();drop(store);
        let status=if transition=="after_abandon"{"rejected"}else{"deferred"};
        let result=json!({"operation_id":"attempt","disposition":status,"payload":{"cleanup":"proved"}});
        frozen["operations"]["attempt"]["status"]=json!(status);frozen["operations"]["attempt"]["result"]=result.clone();
        let mut receipt=frozen["operations"]["call::dispatch"].clone();receipt["id"]=json!("call::terminal:queue:2");
        receipt["payload"]=json!({"kind":"finish","id":"attempt","status":status,"result":result});receipt["result"]=json!({});
        frozen["operations"]["call::terminal:queue:2"]=receipt.clone();frozen["rows"][0]["attempts"]=json!(1);
        if transition=="after_unclaim" {frozen["rows"][0]["delivered"]=json!(false);}
        else if transition=="after_abandon" {frozen["rows"][0]["desistiu"]=json!(true);frozen["rows"][0]["desistiu_ts"]=json!(clock.epoch_s-10.0);}
        if transition!="after_abandon" {
            receipt["id"]=json!("call::terminal:queue:3");receipt["payload"]=json!({"kind":"bump_attempts","entry_id":"entry"});receipt["result"]=json!(1);
            frozen["operations"]["call::terminal:queue:3"]=receipt;
        }
        std::fs::write(&path,serde_json::to_vec(&frozen).unwrap()).unwrap();
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();store.exec(1,"recover",clock,Action::Recover).unwrap();
        let row=&store.state().rows[0];assert_eq!(row["attempts"],1,"{transition}");
        if transition=="after_abandon" {assert_eq!(row["desistiu_ts"],clock.epoch_s-10.0);}else{assert_eq!(row["delivered"],false);}
    }
}

#[test]
fn terminal_runtime_rejected_before_dispatch_abandons_existing_pending_entry() {
    let dir=tempfile::tempdir().unwrap();let mut store=Store::open(&dir.path().join("state"),&dir.path().join("projection"),State::new("key",1,"session",vec![])).unwrap();let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
    store.exec(1,"append",clock,Action::Append {text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into())}).unwrap();
    store.exec(1,"prepare",clock,Action::Prepare {id:"op".into(),entry_id:Some("entry".into()),payload:json!({"operation_id":"op","kind":"input","payload":{"text":"Olá","_terminal_generation":1}})}).unwrap();
    store.exec(1,"finish",clock,Action::Finish {id:"op".into(),status:Status::Rejected,result:json!({"operation_id":"op","disposition":"rejected","payload":{"code":"refused"}})}).unwrap();
    assert_eq!(store.state().rows[0]["desistiu"],true);assert_eq!(store.state().rows[0]["delivered"],true);
}

#[test]
#[ignore = "exige HANGAR_TEST_PYTHON apontando ao Python do backend"]
fn terminal_runtime_store_python_rust_interop_preserves_finalization_marker() {
    let python=std::env::var_os("HANGAR_TEST_PYTHON").expect("HANGAR_TEST_PYTHON ausente");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let projection=dir.path().join("projection");
    let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
    store.exec(1,"append",clock,Action::Append {text:"Olá 🌎".into(),delivered:false,ts:None,pre_transcript:true,entry_id:Some("entry".into())}).unwrap();
    let payload=json!({"operation_id":"attempt","kind":"input","payload":{"text":"Olá 🌎","pre_transcript":true,"_terminal_generation":1}});
    store.exec(1,"prepare",clock,Action::Prepare {id:"attempt".into(),entry_id:Some("entry".into()),payload:payload.clone()}).unwrap();
    store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"attempt".into(),wire_id:"terminal:1:attempt".into(),staged:false}).unwrap();
    let result=json!({"operation_id":"attempt","disposition":"deferred","payload":{"cleanup":"proved"}});
    store.exec(1,"finish",clock,Action::Finish {id:"attempt".into(),status:Status::Deferred,result:result.clone()}).unwrap();drop(store);
    let script=r#"import sys
from pathlib import Path
from app.runtime_queue import QueueStore,initial_state
s=QueueStore(Path(sys.argv[1]),Path(sys.argv[2]),initial_state('key',1,'session',[]))
c={'monotonic_s':0.0,'epoch_s':1800000000.0}
assert s.state['operations']['attempt']['terminal_finalized'] is True
assert s.state['operations']['attempt']['entry_materialized'] is True
s.exec(1,'python-recover',c,{'kind':'recover'})
s.exec(1,'python-finish',c,{'kind':'finish','id':'attempt','status':'deferred','result':{'operation_id':'attempt','disposition':'deferred','payload':{'cleanup':'proved'}}})
assert s.state['rows'][0]['attempts']==1
s.exec(1,'python-missing-prepare',c,{'kind':'prepare','id':'missing','entry_id':'missing','payload':{'operation_id':'missing','kind':'input','payload':{'text':'Unicode 🌎','pre_transcript':True,'_terminal_generation':1}}})
s.exec(1,'python-missing-recover',c,{'kind':'recover'})
assert len(s.state['rows'])==2
assert s.state['operations']['missing']['entry_materialized'] is True
assert s.state['rows'][1]['text']=='Unicode 🌎'
s.exec(1,'python-failed-prepare',c,{'kind':'prepare','id':'failed','entry_id':'missing','payload':{'operation_id':'failed','kind':'input','payload':{'text':'Unicode 🌎','_terminal_generation':1}}})
s.exec(1,'python-failed-dispatch',c,{'kind':'begin_dispatch','id':'failed','wire_id':'terminal:1:failed'})
s.exec(1,'python-failed-finish',c,{'kind':'finish','id':'failed','status':'rejected','result':{'operation_id':'failed','disposition':'rejected','payload':{'code':'refused'}}})
assert s.exec(1,'python-remove',c,{'kind':'remove','entry_id':'missing'}) is True
"#;
    let backend=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend");
    let output=std::process::Command::new(python).args(["-c",script]).arg(&path).arg(&projection).env("PYTHONPATH",backend).output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();store.exec(1,"rust-recover",clock,Action::Recover).unwrap();
    assert!(store.state().operations["attempt"].terminal_finalized);assert!(store.state().operations["attempt"].entry_materialized);
    assert!(store.state().operations["missing"].entry_materialized);assert_eq!(store.state().rows.len(),1);assert_eq!(store.state().rows[0]["attempts"],1);
    store.exec(1,"rust-recover-again",clock,Action::Recover).unwrap();assert_eq!(store.state().rows.len(),1);
}

#[test]
fn terminal_runtime_recovered_entry_removed_after_failure_is_not_materialized_again() {
    for exhausted in [false,true] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let projection=dir.path().join("projection");
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();let clock=ClockSample {monotonic_s:0.0,epoch_s:1800000000.0};
        let payload=json!({"operation_id":"root","kind":"input","payload":{"text":"Olá 🌎","pre_transcript":true,"_terminal_generation":1}});
        store.exec(1,"prepare-root",clock,Action::Prepare {id:"root".into(),entry_id:Some("entry".into()),payload}).unwrap();
        store.exec(1,"recover-first-creation",clock,Action::Recover).unwrap();assert_eq!(store.state().rows.len(),1);
        if exhausted {for n in 0..2 {store.exec(1,&format!("bump:{n}"),clock,Action::BumpAttempts {entry_id:"entry".into()}).unwrap();}}
        store.exec(1,"prepare-attempt",clock,Action::Prepare {id:"attempt".into(),entry_id:Some("entry".into()),payload:json!({"operation_id":"attempt","kind":"input","payload":{"text":"Olá 🌎","_terminal_generation":1}})}).unwrap();
        store.exec(1,"dispatch",clock,Action::BeginDispatch {id:"attempt".into(),wire_id:"terminal:1:attempt".into(),staged:false}).unwrap();
        store.exec(1,"finish",clock,Action::Finish {id:"attempt".into(),status:if exhausted {Status::Deferred}else{Status::Rejected},result:json!({"operation_id":"attempt","disposition":if exhausted{"deferred"}else{"rejected"},"payload":{"cleanup":"proved"}})}).unwrap();
        assert_eq!(store.state().rows[0]["desistiu"],true);assert_eq!(store.exec(1,"remove",clock,Action::Remove {entry_id:"entry".into()}).unwrap(),true);drop(store);
        let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
        for n in 0..2 {store.exec(1,&format!("recover-again:{n}"),clock,Action::Recover).unwrap();assert!(store.state().rows.is_empty(),"exhausted={exhausted}");}
    }
}

#[cfg(unix)]
#[test]
fn unchanged_projection_is_not_rewritten() {
    // Ler a fila ou gravar o estado privado não muda as mensagens: o arquivo do nome fica intacto.
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let projection = dir.path().join("projection");
    let mut store = Store::open(&dir.path().join("key.queue-state.json"), &projection, State::new("key",1,"session",vec![])).unwrap();
    store.exec(1, "append", clock(), append()).unwrap();
    let inode = || std::fs::metadata(projection.join("session.jsonl")).unwrap().ino();
    let before = inode();
    store.exec(1, "load", clock(), Action::Load).unwrap();
    fill(&mut store, 3, "view");
    assert_eq!(inode(), before);
    std::fs::write(projection.join("session.jsonl"), "").unwrap();
    store.exec(1, "repair", clock(), Action::EnsureProjection).unwrap();
    assert_eq!(std::fs::read_to_string(projection.join("session.jsonl")).unwrap().lines().count(), 1);
}

#[cfg(unix)]
#[test]
fn write_failure_heals_on_next_operation_once_disk_is_writable() {
    // A pasta da fila travada (chmod) recusa a gravação; destravada, o envio seguinte volta a valer
    // sem precisar de uma ação de reparo que ninguém manda.
    use std::os::unix::fs::PermissionsExt;
    if std::fs::metadata("/proc/self").map(|m|std::os::unix::fs::MetadataExt::uid(&m)==0).unwrap_or(false) {
        return;     // como root o chmod não recusa nada
    }
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("runtime");
    std::fs::create_dir(&state_dir).unwrap();
    let mut store = Store::open(&state_dir.join("key.json"), &dir.path().join("projection"), State::new("key",1,"session",vec![])).unwrap();
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = store.exec(1, "blocked", clock(), append());
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err());
    store.exec(1, "after", clock(), append()).expect("disco destravado: a fila volta sem ação de reparo");
    assert_eq!(store.state().rows.len(), 1, "a entrada que falhou não ficou");
}
