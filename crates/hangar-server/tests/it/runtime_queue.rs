use hangar_server::runtime::{protocol::ClockSample, queue::{Action, State, Status, Store}};
use serde_json::json;

fn clock() -> ClockSample { ClockSample { monotonic_s: 10.0, epoch_s: 1_800_000_000.0 } }
fn fill(store:&mut Store, count:usize, prefix:&str) {
    for index in 0..count { store.exec(1,&format!("{prefix}:{index}"),clock(),Action::SetRuntimeState { state:json!({}) }).unwrap(); }
}

fn append() -> Action { Action::Append { text:"Olá".into(), delivered:false, ts:None,
    pre_transcript:false, entry_id:Some("entry-1".into()) } }

/// A CLI responde `/btw` sozinha e não grava a linha: sem isto a bolha esperava para sempre.

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
