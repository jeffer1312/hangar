use hangar_server::runtime::{protocol::ClockSample,queue::*};
use serde_json::json;

/// Recuperação do WebSocket do Codex com terminal: só o socket de controle cai; TUI e app-server seguem.
#[cfg(unix)]
mod adopted_socket {
    use crate::runtime_open::{app_server_process,codex_terminal_target,fake_app_server,registry};
    use hangar_server::runtime::codex_terminal::{Adoption,AdoptionCell,Attempt,DISCONNECTED,reconnect};
    use hangar_server::runtime::protocol::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    async fn problem(registry:&hangar_server::runtime::gateway::RuntimeRegistry) -> serde_json::Value {
        registry.handle("codex-key",1).await.unwrap().view().await.unwrap()["public_state"]["problema"].clone()
    }

    #[tokio::test]
    async fn dropped_socket_reconnects_once_to_the_same_process_and_never_resends_the_uncertain_turn() {
        let dir = tempfile::tempdir().unwrap();
        // `turn/start` sem resposta: o turno fica em voo quando o socket cai.
        let server = fake_app_server(false).await;
        let mut child = app_server_process(&server.endpoint);
        let pid = child.id();
        let registry = registry().await;
        registry.open_codex_terminal(codex_terminal_target(dir.path(),&server.endpoint,pid)).await.unwrap();
        server.wait_method(1,"thread/resume").await;
        let handle = registry.handle("codex-key",1).await.unwrap();
        let first = tokio::spawn({ let handle = handle.clone(); async move { handle.command(RuntimeCommand { operation_id:"primeira".into(),
            kind:OperationKind::Input,payload:json!({"text":"primeira","entry_id":"primeira"}) }).await } });
        server.wait_method(1,"turn/start").await;
        server.drop_connection(1);
        // Uma reconexão: inicializa e assina a mesma thread de novo.
        server.wait_method(2,"initialize").await;
        assert_eq!(server.wait_method(2,"thread/resume").await["params"]["threadId"],"thread-1");
        let reply = tokio::time::timeout(Duration::from_secs(10),first).await.unwrap().unwrap().unwrap();
        assert_eq!(reply.disposition,Disposition::Unknown,"turno sem recibo vira incerto, nunca reenviado");
        tokio::time::timeout(Duration::from_secs(5),async { while !problem(&registry).await.is_null() { tokio::time::sleep(Duration::from_millis(20)).await; } })
            .await.expect("o problema sai quando a conexão volta");
        // O estado volta e o texto novo sai pelo socket novo.
        let second = tokio::spawn({ let handle = handle.clone(); async move { handle.command(RuntimeCommand { operation_id:"segunda".into(),
            kind:OperationKind::Input,payload:json!({"text":"segunda","entry_id":"segunda"}) }).await } });
        let start = server.wait_method(2,"turn/start").await;
        assert!(start.to_string().contains("segunda"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!server.frames_of(2).iter().any(|frame|frame.to_string().contains("primeira")),"a operação incerta não sai de novo");
        assert_eq!(server.connections.load(Ordering::SeqCst),2,"sem segundo cliente nem outra tentativa");
        assert!(child.try_wait().unwrap().is_none(),"sem kill no app-server adotado");
        second.abort();
        registry.close("codex-key",1).await.unwrap();
        child.kill().unwrap(); child.wait().unwrap();
    }

    #[tokio::test]
    async fn app_server_gone_during_the_wait_stops_reconnecting_and_stays_visible() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_app_server(true).await;
        let mut child = app_server_process(&server.endpoint);
        let registry = registry().await;
        registry.open_codex_terminal(codex_terminal_target(dir.path(),&server.endpoint,child.id())).await.unwrap();
        server.wait_method(1,"thread/resume").await;
        // Identidade trocada na espera: o pid não é mais o app-server deste endereço.
        child.kill().unwrap(); child.wait().unwrap();
        server.drop_connection(1);
        tokio::time::sleep(Duration::from_millis(3500)).await;
        assert_eq!(server.connections.load(Ordering::SeqCst),1,"processo morto: nenhuma reconexão nem app-server novo");
        assert_eq!(problem(&registry).await,DISCONNECTED);
        registry.close("codex-key",1).await.unwrap();
    }

    #[tokio::test]
    async fn closing_during_the_wait_cancels_the_reconnection() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_app_server(true).await;
        let mut child = app_server_process(&server.endpoint);
        let registry = registry().await;
        registry.open_codex_terminal(codex_terminal_target(dir.path(),&server.endpoint,child.id())).await.unwrap();
        server.wait_method(1,"thread/resume").await;
        server.drop_connection(1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Transferência: a posse sai daqui antes da primeira tentativa (1 s).
        assert_eq!(registry.close("codex-key",1).await.unwrap()["closed"],true);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(server.connections.load(Ordering::SeqCst),1,"timer e tentativa cancelados com a posse");
        assert!(child.try_wait().unwrap().is_none());
        drop(crate::lease_when_free(&dir.path().join("codex-key.lock")));
        child.kill().unwrap(); child.wait().unwrap();
    }

    #[tokio::test]
    async fn an_attempt_revalidates_the_binding_before_connecting() {
        let server = fake_app_server(true).await;
        let mut child = app_server_process(&server.endpoint);
        let cell = AdoptionCell::new(&server.endpoint,child.id());
        let adoption = Adoption { endpoint:server.endpoint.clone(),app_pid:child.id(),cell:cell.clone() };
        // Endereço trocado no registro durante a espera.
        let moved = Adoption { endpoint:"ws://127.0.0.1:1".into(),..adoption.clone() };
        assert!(matches!(reconnect(&moved).await,Attempt::Gone(error) if error.code == "codex_adoption_changed"));
        // Sessão fechada ou transferida.
        cell.revoke();
        assert!(matches!(reconnect(&adoption).await,Attempt::Gone(error) if error.code == "codex_adoption_changed"));
        assert_eq!(server.connections.load(Ordering::SeqCst),0,"nenhuma conexão sem o vínculo atual");
        child.kill().unwrap(); child.wait().unwrap();
    }
}

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
    let script = "import sys\nfrom app.runtime_process import WriterLease\ntry:\n WriterLease(sys.argv[1])\nexcept BlockingIOError:\n sys.exit(0)\nsys.exit(1)";
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
