use hangar_server::runtime::{protocol::ClockSample,queue::*};
use serde_json::json;

fn clock() -> ClockSample { ClockSample { monotonic_s:1.0,epoch_s:1_800_000_000.0 } }

#[test]
fn recovery_matrix_preserves_input_without_resending_unknown() {
    for point in ["before_prepare","after_prepare","before_write","partial_write","after_write","before_reply","after_detach"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.json");
        let projection = dir.path().join("projection");
        let mut store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
        let lease_path = dir.path().join("key.lock");
        let lease = acquire_lease(&lease_path).unwrap();
        assert!(acquire_lease(&lease_path).is_err());
        store.exec(1,"append",clock(),Action::Append { text:"Olá\r\nação 🚀".into(),delivered:false,
            ts:None,pre_transcript:false,entry_id:Some("entry".into()) }).unwrap();
        if point != "before_prepare" {
            store.exec(1,"prepare",clock(),Action::Prepare { id:"entry".into(),entry_id:Some("entry".into()),payload:json!({"kind":"input"}) }).unwrap();
        }
        let dispatched = ["partial_write","after_write","before_reply","after_detach"].contains(&point);
        if dispatched { store.exec(1,"dispatch",clock(),Action::BeginDispatch { id:"entry".into(),wire_id:"wire".into(),staged:false }).unwrap(); }
        drop(store); drop(lease);
        let _reserve = acquire_lease(&lease_path).unwrap();
        assert!(acquire_lease(&lease_path).is_err());
        let mut store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
        store.exec(1,"recover",clock(),Action::Recover).unwrap();
        assert_eq!(store.state().rows.len(),1,"{point}");
        assert_eq!(store.state().rows[0]["text"],"Olá\r\nação 🚀");
        if dispatched {
            assert!(matches!(store.state().operations["entry"].status,Status::Unknown),"{point}");
            assert!(store.exec(1,"retry",clock(),Action::BeginDispatch { id:"entry".into(),wire_id:"retry".into(),staged:false }).is_err());
        }
    }
}

#[test]
fn projection_failure_preserves_definitive_reply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.json");
    let projection = dir.path().join("projection");
    let mut store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),entry_id:None,payload:json!({}) }).unwrap();
    std::fs::remove_file(projection.join("session.jsonl")).unwrap();
    std::fs::create_dir(projection.join("session.jsonl")).unwrap();
    assert!(store.exec(1,"reply",clock(),Action::Finish { id:"op".into(),status:Status::Accepted,
        result:json!({"disposition":"accepted","payload":{}}) }).is_err());
    std::fs::remove_dir(projection.join("session.jsonl")).unwrap();
    let store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    assert!(matches!(store.state().operations["op"].status,Status::Accepted));
}

#[test]
#[ignore = "exige HANGAR_TEST_PYTHON apontando ao Python do backend; conferir também no Windows"]
fn python_and_rust_share_the_lease() {
    let python = std::env::var_os("HANGAR_TEST_PYTHON").expect("HANGAR_TEST_PYTHON ausente");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lease-á.lock");
    let lease = acquire_lease(&path).unwrap();
    let script = "import sys\nfrom app.runtime_coordinator import WriterLease\ntry:\n WriterLease(sys.argv[1])\nexcept BlockingIOError:\n sys.exit(0)\nsys.exit(1)";
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend");
    let result = std::process::Command::new(python).args(["-c",script]).arg(&path)
        .env("PYTHONPATH",backend).output().unwrap();
    assert!(result.status.success());
    drop(lease);
}

#[test]
fn recover_leaves_the_attempt_of_a_confirmed_operation_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (path, projection) = (dir.path().join("key.json"), dir.path().join("projection"));
    let mut store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"append",clock(),Action::Append { text:"Olá".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("entry".into()) }).unwrap();
    store.exec(1,"prepare",clock(),Action::Prepare { id:"entry".into(),entry_id:Some("entry".into()),payload:json!({"kind":"input"}) }).unwrap();
    store.exec(1,"dispatch",clock(),Action::BeginDispatch { id:"entry".into(),wire_id:"wire".into(),staged:false }).unwrap();
    drop(store);
    // O transcript confirmou a entrada antes da resposta do escritor, e o processo caiu.
    let mut state: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["operations"]["entry"]["status"] = json!("confirmed");
    state["rows"][0]["delivered"] = json!(true); state["rows"][0]["confirmed"] = json!(true);
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let mut store = Store::open(&path,&projection,State::new("key",1,"session",vec![])).unwrap();
    store.exec(1,"recover",clock(),Action::Recover).unwrap();
    let op = &store.state().operations["entry"];
    assert!(matches!(op.status,Status::Confirmed));
    assert_eq!(op.wire_attempts["wire"]["status"],"dispatching");
}
