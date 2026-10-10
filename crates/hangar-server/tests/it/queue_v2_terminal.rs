use hangar_server::runtime::{protocol::ClockSample, queue::*};
use serde_json::{json,Value};

fn clock()->ClockSample {ClockSample {monotonic_s:10.0,epoch_s:1_800_000_000.0}}
fn intent()->Value {json!({"operation_id":"root","kind":"input","payload":{"text":"fixture-input","_terminal_generation":1}})}
fn payload()->Value {json!({"native":true,"message_id":"native-id","native_status":"delivered","cleanup":"not_needed",
    "code":"native_write","stage":"native","preserve_binding":true,"queued":true,"tool_result":"x".repeat(100_000)})}

#[test]
fn terminal_v2_keeps_native_metadata_without_bulk() {
    let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("state");
    let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare {id:"root".into(),payload:intent(),entry_id:Some("entry".into())}).unwrap();
    store.exec(1,"append",clock(),Action::Append {text:"fixture-input".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into())}).unwrap();
    let full=store.exec(1,"finish",clock(),Action::Finish {id:"root".into(),status:Status::Accepted,
        result:json!({"operation_id":"root","disposition":"accepted","payload":payload()})}).unwrap();
    assert_eq!(full["result"]["payload"]["tool_result"].as_str().unwrap().len(),100_000);
    for n in 0..270 {store.exec(1,&format!("fill:{n}"),clock(),Action::SetRuntimeState {state:json!({})}).unwrap();}
    let mut expected=payload();expected.as_object_mut().unwrap().remove("tool_result");
    assert_eq!(store.state().operations["root"].result["payload"],expected);
    assert!(store.state().operations["root"].entry_materialized);
    assert!(!std::fs::read_to_string(path).unwrap().contains(&"x".repeat(1000)));
    assert_eq!(store.state().operations.keys().filter(|id|id.starts_with("call::")).count(),256);
}

fn legacy(case:&str)->Value {
    let operation=|id:&str,body:Value,entry:Value,status:&str,result:Value|json!({"id":id,"payload":body,"entry_id":entry,
        "status":status,"result":result,"dispatch_cursor":null,"wire_attempts":{}});
    let mut state=json!({"version":1,"owner_key":"key","generation":1,"name":"session","rows":[],"operations":{},"used_occurrences":{},"runtime_state":{}});
    if case!="claim" {
        state["operations"]["root"]=operation("root",intent(),json!("entry"),"prepared",Value::Null);
        state["operations"]["call::terminal:queue:1"]=operation("call::terminal:queue:1",json!({"kind":"prepare","id":"root","entry_id":"entry","payload":intent()}),Value::Null,"accepted",Value::Null);
    }
    if case!="before_append" {state["rows"]=json!([{"id":"entry","text":"fixture-input","ts":1_800_000_000.0,"delivered":true}]);}
    if case=="removed" {
        state["rows"]=json!([]);
        state["operations"]["call::append"]=operation("call::append",json!({"kind":"append","text":"fixture-input","entry_id":"entry"}),Value::Null,"accepted",json!({"id":"entry"}));
        state["operations"]["call::remove"]=operation("call::remove",json!({"kind":"remove","entry_id":"entry"}),Value::Null,"accepted",json!(true));
    }
    if case=="claim" {state["operations"]["call::terminal:queue:1"]=operation("call::terminal:queue:1",json!({"kind":"claim"}),Value::Null,"accepted",state["rows"].clone());}
    if case=="finish_bump" {
        let result=json!({"operation_id":"root","disposition":"deferred","payload":{"cleanup":"proved"}});
        state["rows"][0]["attempts"]=json!(1);state["operations"]["root"]["status"]=json!("deferred");
        state["operations"]["root"]["result"]=result.clone();
        state["operations"]["root"]["wire_attempts"]=json!({"terminal:1:root":{"status":"unknown","result":null}});
        state["operations"]["call::terminal:queue:2"]=operation("call::terminal:queue:2",json!({"kind":"finish","id":"root","status":"deferred","result":result}),Value::Null,"accepted",Value::Null);
        state["operations"]["call::terminal:queue:3"]=operation("call::terminal:queue:3",json!({"kind":"bump_attempts","entry_id":"entry"}),Value::Null,"accepted",json!(1));
    }
    state
}

#[test]
fn terminal_v1_preserves_proofs_before_leased_recover_then_compacts() {
    for case in ["before_append","removed","claim","finish_bump"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");
        std::fs::write(&path,serde_json::to_vec(&legacy(case)).unwrap()).unwrap();
        let _lease=acquire_lease(&dir.path().join("lease")).unwrap();
        let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
        assert!(store.state().operations.keys().any(|id|id.starts_with("call::")),"{case}");
        store.exec(1,"recover",clock(),Action::Recover).unwrap();
        assert_eq!(store.state().version,2);
        if case=="removed" {assert!(store.state().rows.is_empty());assert!(store.state().operations["root"].entry_materialized);}
        else {
            assert_eq!(store.state().rows.len(),1);assert_eq!(store.state().rows[0]["delivered"],false);
            if case=="before_append" {assert!(store.state().operations["root"].entry_materialized);}
            if case=="finish_bump" {assert_eq!(store.state().rows[0]["attempts"],1);assert!(store.state().operations["root"].terminal_finalized);}
        }
        assert!(store.state().operations.keys().filter(|id|id.starts_with("call::")).all(|id|id=="call::recover"));
        let rows=store.state().rows.clone();store.exec(1,"again",clock(),Action::Recover).unwrap();assert_eq!(store.state().rows,rows);
    }
}

#[test]
fn terminal_v2_normalized_finish_proof_does_not_count_again() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let mut frozen=legacy("finish_bump");
    frozen["version"]=json!(2);frozen["next_seq"]=json!(100);
    for op in frozen["operations"].as_object_mut().unwrap().values_mut() {op["seq"]=json!(90);}
    frozen["operations"]["root"]["result"]["payload"]["bulk"]=json!("x".repeat(1000));
    std::fs::write(&path,serde_json::to_vec(&frozen).unwrap()).unwrap();
    let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"recover",clock(),Action::Recover).unwrap();
    assert_eq!(store.state().rows[0]["attempts"],1);assert_eq!(store.state().rows[0]["delivered"],false);
    assert!(store.state().operations["root"].terminal_finalized);
}

#[test]
fn terminal_v1_prepare_receipt_becomes_durable_generation_proof() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let mut frozen=legacy("before_append");
    frozen["operations"]["root"]["payload"]["payload"].as_object_mut().unwrap().remove("_terminal_generation");
    frozen["operations"]["call::terminal:queue:1"]["payload"]["payload"]["payload"].as_object_mut().unwrap().remove("_terminal_generation");
    std::fs::write(&path,serde_json::to_vec(&frozen).unwrap()).unwrap();
    let _lease=acquire_lease(&dir.path().join("lease")).unwrap();
    let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"recover",clock(),Action::Recover).unwrap();
    assert!(!store.state().operations.contains_key("call::terminal:queue:1"));
    assert_eq!(store.state().operations["root"].payload["payload"]["_terminal_generation"],1);
}

#[test]
#[ignore = "exige HANGAR_TEST_PYTHON apontando ao Python do backend"]
fn terminal_v2_metadata_and_migration_roundtrip_rust_python_rust() {
    let python=std::env::var_os("HANGAR_TEST_PYTHON").expect("HANGAR_TEST_PYTHON ausente");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let projection=dir.path().join("projection");let lease_path=dir.path().join("lease");
    std::fs::write(&path,serde_json::to_vec(&legacy("finish_bump")).unwrap()).unwrap();
    let lease=acquire_lease(&lease_path).unwrap();
    let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"rust-recover",clock(),Action::Recover).unwrap();
    store.exec(1,"native-prepare",clock(),Action::Prepare {id:"native".into(),payload:intent(),entry_id:Some("native-entry".into())}).unwrap();
    store.exec(1,"native-append",clock(),Action::Append {text:"fixture-input".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("native-entry".into())}).unwrap();
    store.exec(1,"native-dispatch",clock(),Action::BeginDispatch {id:"native".into(),wire_id:"terminal:1:native".into(),staged:false}).unwrap();
    store.exec(1,"native-finish",clock(),Action::Finish {id:"native".into(),status:Status::Accepted,
        result:json!({"operation_id":"native","disposition":"accepted","payload":payload()})}).unwrap();
    drop(store);drop(lease);
    let script=r#"import sys
from pathlib import Path
from app.runtime_queue import QueueStore,initial_state
from app.runtime_coordinator import WriterLease
lease=WriterLease(Path(sys.argv[3]))
try:
 s=QueueStore(Path(sys.argv[1]),Path(sys.argv[2]),initial_state('key',1,'session',[]))
 c={'monotonic_s':10.0,'epoch_s':1800000000.0}
 assert s.state['version']==2
 assert s.state['operations']['root']['entry_materialized'] is True
 assert s.state['operations']['root']['terminal_finalized'] is True
 assert s.state['operations']['native']['result']['payload']['message_id']=='native-id'
 assert 'tool_result' not in s.state['operations']['native']['result']['payload']
 s.exec(1,'python-recover',c,{'kind':'recover'})
 assert s.state['rows'][0]['attempts']==1
 for i in range(270):s.exec(1,f'fill:{i}',c,{'kind':'set_runtime_state','state':{}})
 s.exec(1,'abandon',c,{'kind':'abandon','entry_id':'entry'})
 assert s.exec(1,'remove',c,{'kind':'remove','entry_id':'entry'}) is True
 s.exec(1,'python-recover-again',c,{'kind':'recover'})
 assert [r['id'] for r in s.state['rows']]==['native-entry']
 assert len([k for k in s.state['operations'] if k.startswith('call::')])==256
finally:lease.close()
"#;
    let backend=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend");
    let output=std::process::Command::new(python).current_dir(backend).args(["-c",script])
        .args([&path,&projection,&lease_path]).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let _lease=acquire_lease(&lease_path).unwrap();
    let mut store=Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"rust-recover-again",clock(),Action::Recover).unwrap();
    assert!(store.state().operations["root"].terminal_finalized && store.state().operations["root"].entry_materialized);
    assert_eq!(store.state().rows.len(),1);assert_eq!(store.state().rows[0]["id"],"native-entry");
    assert_eq!(store.state().operations["native"].result["payload"]["message_id"],"native-id");
    assert!(store.state().next_seq>270);
}

#[test]
fn absent_transcript_one_echo_cannot_confirm_two_equal_inputs_after_compact_reopen() {
    use hangar_server::runtime::receipt::ReceiptIndex;
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let transcript=dir.path().join("absent.jsonl");
    let mut index=ReceiptIndex::new("claude","sid");let cursor=index.capture(&transcript).unwrap();assert!(cursor.file_identity.is_none());
    let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    for number in [1,2] {
        let id=format!("input-{number}");
        store.exec(1,&format!("append:{number}"),clock(),Action::Append {text:"same-input".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some(id.clone())}).unwrap();
        store.exec(1,&format!("prepare:{number}"),clock(),Action::Prepare {id:id.clone(),payload:json!({"kind":"input"}),entry_id:Some(id.clone())}).unwrap();
        store.exec(1,&format!("cursor:{number}"),clock(),Action::BindDispatch {id:id.clone(),cursor:serde_json::to_value(&cursor).unwrap()}).unwrap();
        store.exec(1,&format!("dispatch:{number}"),clock(),Action::BeginDispatch {id:id.clone(),wire_id:id,staged:false}).unwrap();
    }
    let millis=((cursor.absent_since.unwrap()+1.0)*1000.0) as i64;
    let timestamp=chrono::DateTime::from_timestamp_millis(millis).unwrap().to_rfc3339();
    std::fs::write(&transcript,format!("{}\n",json!({"type":"user","uuid":"one-echo","sessionId":"sid","timestamp":timestamp,"message":{"content":"same-input"}}))).unwrap();
    index.scan(&transcript).unwrap();let proof=index.match_after(&transcript,&cursor,&store.state().rows[0],&store.state().used_occurrences).unwrap().unwrap();
    assert_eq!(store.exec(1,"confirm-first",clock(),Action::ConfirmOccurrence {id:"input-1".into(),proof:proof.clone()}).unwrap(),true);
    for n in 0..270 {store.exec(1,&format!("fill:{n}"),clock(),Action::SetRuntimeState {state:json!({})}).unwrap();}
    drop(store);let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    assert_eq!(store.state().used_occurrences.len(),1);
    assert!(index.match_after(&transcript,&cursor,&store.state().rows[1],&store.state().used_occurrences).unwrap().is_none());
    assert_eq!(store.exec(1,"confirm-second",clock(),Action::ConfirmOccurrence {id:"input-2".into(),proof}).unwrap(),false);
    assert_eq!(store.state().rows[0]["confirmed"],true);assert_ne!(store.state().rows[1]["confirmed"],true);
}

#[test]
fn terminal_write_barrier_follows_remaining_uncertain_input_and_lifts_after_last() {
    use hangar_server::runtime::receipt::ReceiptIndex;
    for resolution in ["confirm","late_accepted"] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");let transcript=dir.path().join("sid.jsonl");
        let mut index=ReceiptIndex::new("claude","sid");let cursor=index.capture(&transcript).unwrap();
        let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
        for text in ["A","B"] {
            store.exec(1,&format!("append:{text}"),clock(),Action::Append {text:text.into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some(text.into())}).unwrap();
            store.exec(1,&format!("prepare:{text}"),clock(),Action::Prepare {id:text.into(),payload:json!({"kind":"input","payload":{"text":text,"_terminal_generation":1}}),entry_id:Some(text.into())}).unwrap();
            store.exec(1,&format!("cursor:{text}"),clock(),Action::BindDispatch {id:text.into(),cursor:serde_json::to_value(&cursor).unwrap()}).unwrap();
            store.exec(1,&format!("dispatch:{text}"),clock(),Action::BeginDispatch {id:text.into(),wire_id:format!("terminal:1:{text}"),staged:false}).unwrap();
            store.exec(1,&format!("finish:{text}"),clock(),Action::Finish {id:text.into(),status:Status::Unknown,
                result:json!({"operation_id":text,"disposition":"unknown","payload":{"cleanup":"uncertain"}})}).unwrap();
        }
        let millis=((cursor.absent_since.unwrap()+1.0)*1000.0) as i64;
        let timestamp=chrono::DateTime::from_timestamp_millis(millis).unwrap().to_rfc3339();
        std::fs::write(&transcript,["A","B"].iter().map(|text|format!("{}\n",json!({"type":"user","uuid":format!("echo-{text}"),"sessionId":"sid","timestamp":timestamp,"message":{"content":text}}))).collect::<String>()).unwrap();
        index.scan(&transcript).unwrap();
        let confirm=|store:&mut Store,index:&ReceiptIndex,text:&str| {
            let row=store.state().rows.iter().find(|row|row["id"]==text).unwrap().clone();
            let proof=index.match_after(&transcript,&cursor,&row,&store.state().used_occurrences).unwrap().unwrap();
            store.exec(1,&format!("confirm:{text}"),clock(),Action::ConfirmOccurrence {id:text.into(),proof}).unwrap()
        };
        assert_eq!(store.state().runtime_state["terminal_write_barrier"]["operation_id"],"A");
        assert!(store.state().terminal_write_blocked("sid"));
        if resolution=="confirm" {assert_eq!(confirm(&mut store,&index,"A"),true);}
        else {store.exec(1,"late:A",clock(),Action::Finish {id:"A".into(),status:Status::Accepted,result:json!({"operation_id":"A","disposition":"accepted","payload":{}})}).unwrap();}
        // B continua incerta na mesma conversa: a trava passa para ela.
        assert_eq!(store.state().runtime_state["terminal_write_barrier"]["operation_id"],"B","{resolution}");
        assert!(store.state().terminal_write_blocked("sid"));
        assert_eq!(confirm(&mut store,&index,"B"),true);
        assert!(store.state().runtime_state.get("terminal_write_barrier").is_none(),"{resolution}");
        assert!(!store.state().terminal_write_blocked("sid"));
        drop(store);let store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
        assert!(!store.state().terminal_write_blocked("sid"));
    }
}

#[test]
fn uncertain_input_of_a_second_conversation_blocks_writes_while_old_barrier_stays() {
    use hangar_server::runtime::receipt::ReceiptIndex;
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("state");
    let mut store=Store::open(&path,dir.path(),State::new("key",1,"session",vec![])).unwrap();
    for (text,conversation,generation) in [("A","sid",1),("C","new-sid",2)] {
        let cursor=ReceiptIndex::new("claude",conversation).capture(&dir.path().join(format!("{conversation}.jsonl"))).unwrap();
        store.exec(1,&format!("append:{text}"),clock(),Action::Append {text:text.into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some(text.into())}).unwrap();
        store.exec(1,&format!("prepare:{text}"),clock(),Action::Prepare {id:text.into(),payload:json!({"kind":"input","payload":{"text":text,"_terminal_generation":generation}}),entry_id:Some(text.into())}).unwrap();
        store.exec(1,&format!("cursor:{text}"),clock(),Action::BindDispatch {id:text.into(),cursor:serde_json::to_value(&cursor).unwrap()}).unwrap();
        store.exec(1,&format!("dispatch:{text}"),clock(),Action::BeginDispatch {id:text.into(),wire_id:format!("terminal:{generation}:{text}"),staged:false}).unwrap();
        store.exec(1,&format!("finish:{text}"),clock(),Action::Finish {id:text.into(),status:Status::Unknown,
            result:json!({"operation_id":text,"disposition":"unknown","payload":{"cleanup":"uncertain"}})}).unwrap();
    }
    assert_eq!(store.state().runtime_state["terminal_write_barrier"]["conversation"],"sid");
    assert!(store.state().terminal_write_blocked("new-sid"));
    store.exec(1,"late:C",clock(),Action::Finish {id:"C".into(),status:Status::Accepted,result:json!({"operation_id":"C","disposition":"accepted","payload":{}})}).unwrap();
    assert!(!store.state().terminal_write_blocked("new-sid"));
    assert!(store.state().terminal_write_blocked("sid"));
}

#[test]
fn terminal_recover_requeues_only_attempts_that_never_started_writing() {
    for writing in [false,true] {
        let dir=tempfile::tempdir().unwrap();
        let mut store=Store::open(&dir.path().join("state"),dir.path(),State::new("key",1,"session",vec![])).unwrap();
        store.exec(1,"prepare",clock(),Action::Prepare {id:"root".into(),payload:intent(),entry_id:Some("entry".into())}).unwrap();
        store.exec(1,"append",clock(),Action::Append {text:"fixture-input".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into())}).unwrap();
        store.exec(1,"dispatch",clock(),Action::BeginDispatch {id:"root".into(),wire_id:"terminal:1:root".into(),staged:true}).unwrap();
        if writing {store.exec(1,"writing",clock(),Action::MarkWriting {id:"root".into(),wire_id:"terminal:1:root".into()}).unwrap();}
        store.exec(1,"recover",clock(),Action::Recover).unwrap();
        let state=store.state();
        if writing {
            assert_eq!(serde_json::to_value(state.operations["root"].status).unwrap(),"unknown");
            assert_eq!(state.runtime_state["terminal_write_barrier"]["operation_id"],"root");
        } else {
            assert_eq!(serde_json::to_value(state.operations["root"].status).unwrap(),"deferred");
            assert_eq!(state.operations["root"].result["payload"]["code"],"interrupted_before_write");
            assert_eq!(state.rows[0]["delivered"],false,"volta à fila");
            assert!(state.runtime_state["terminal_write_barrier"].is_null());
        }
    }
}
