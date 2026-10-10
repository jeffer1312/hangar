use hangar_server::runtime::gateway::{self,RuntimeRegistry};
use std::sync::Arc;

#[tokio::test]
async fn wrong_secret_instance_or_generation_denied() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    let server = tokio::spawn(gateway::serve(listener,registry,"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let client = reqwest::Client::new();
    for (secret,instance) in [("wrong","instance-test"),("secret-test","wrong")] {
        let response = client.post(format!("http://{address}/runtime/op"))
            .header("x-hangar-internal",secret).header("x-hangar-runtime-instance",instance)
            .body("not-json").send().await.unwrap();
        assert_eq!(response.status(),404);
    }
    server.abort();
}

#[tokio::test]
async fn private_port_loopback_only() {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    assert!(gateway::serve(listener,registry,"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL).await.is_err());
}

#[test]
fn startup_one_line_no_secret() {
    let line = gateway::startup_line(hangar_server::INTERNAL_PROTOCOL,"instance-test",1234);
    assert!(!line.contains("secret-test"));
    let value:serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["type"],"runtime_ready");
    assert_eq!(value["port"],1234);
    assert_eq!(value.as_object().unwrap().len(),4);
}

#[tokio::test]
async fn unknown_command_fields_are_rejected() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    let server = tokio::spawn(gateway::serve(listener,registry,"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let response = reqwest::Client::new().post(format!("http://{address}/runtime/op"))
        .header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test")
        .header("content-type","application/json")
        .body(serde_json::json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key",
            "generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},
            "command":{"kind":"close","unexpected":true}}).to_string()).send().await.unwrap();
    assert!(!response.status().is_success());
    server.abort();
}

#[tokio::test]
async fn open_refuses_carry() {
    let dir=tempfile::tempdir().unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let address=listener.local_addr().unwrap();
    let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    let server=tokio::spawn(gateway::serve(listener,registry,"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let binding=serde_json::json!({"name":"session","pane":"%1","conversation":"sid","generation":1,"created":1,"mux_argv":["fake"],"windows":false,"clipboard_lock_path":null});
    let descriptor=serde_json::json!({"name":"session","key":"key","provider":"claude","headless":false,"meta":{"key":"key","terminal":binding},"jsonl":dir.path().join("chat.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
    // Trava presa por outro dono: se o `carry` passasse, a resposta seria `runtime_lease` depois de 3 s.
    let _held=hangar_server::runtime::queue::acquire_lease(&dir.path().join("lease")).unwrap();
    let response=reqwest::Client::new().post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test").header("content-type","application/json")
        .body(serde_json::json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key","generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},
            "command":{"kind":"open","descriptor":descriptor,"carry":{"runtime_state":{}}}}).to_string()).send().await.unwrap();
    assert_eq!(response.status(),503);
    let value:serde_json::Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(value["error_code"],"command_fields","recusado antes de pegar a trava");
    server.abort();
}

#[tokio::test]
async fn terminal_runtime_gateway_opens_without_cano_and_fences_generation() {
    let dir=tempfile::tempdir().unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let address=listener.local_addr().unwrap();
    let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    let server=tokio::spawn(gateway::serve(listener,registry.clone(),"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let binding=serde_json::json!({"name":"session","pane":"%1","conversation":"sid","generation":1,"created":1,"mux_argv":["fake"],"windows":false,"clipboard_lock_path":null});
    let descriptor=serde_json::json!({"name":"session","key":"key","provider":"claude","headless":false,"meta":{"key":"key","terminal":binding},"jsonl":dir.path().join("chat.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
    let client=reqwest::Client::new();
    let send=|generation,command|client.post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test").header("content-type","application/json").body(serde_json::json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key","generation":generation,"operation_id":"gateway","clock":{"monotonic_s":0.0,"epoch_s":0.0},"command":command}).to_string());
    let response=send(1,serde_json::json!({"kind":"open","descriptor":descriptor})).send().await.unwrap(); assert!(response.status().is_success());
    let value:serde_json::Value=serde_json::from_str(&response.text().await.unwrap()).unwrap(); assert_eq!(value["result"]["state"]["view"]["terminal"],true); assert!(value["result"]["state"]["view"].get("public_state").is_none());
    assert!(registry.handle("key",1).await.is_err());
    assert_eq!(send(2,serde_json::json!({"kind":"snapshot"})).send().await.unwrap().status(),503);
    assert!(hangar_server::runtime::queue::acquire_lease(&dir.path().join("lease")).is_err());
    assert!(send(1,serde_json::json!({"kind":"close"})).send().await.unwrap().status().is_success());
    drop(crate::lease_when_free(&dir.path().join("lease"))); server.abort();
}

/// Registro com um cano Claude falso que só aceita entradas; a política aponta para uma porta fechada.
async fn opened(dir:&std::path::Path) -> (RuntimeRegistry,serde_json::Value,tokio::task::JoinHandle<()>) {
    use hangar_server::runtime::protocol::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cano = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        let mut raw = String::new();
        while reader.read_line(&mut raw).await.unwrap_or(0) > 0 { raw.clear(); }
    });
    let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into());
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 };
    let ready = registry.open(target).await.unwrap();
    (registry,ready,cano)
}

#[tokio::test]
async fn a_failing_status_service_does_not_stop_the_session() {
    // O carimbo e a linha de status vêm do Python; sem ele a sessão perde só isso, não a posse.
    let dir = tempfile::tempdir().unwrap();
    let (registry,ready,cano) = opened(dir.path()).await;
    assert_eq!(ready["opened"],true);
    registry.close("key",1).await.unwrap();
    cano.abort();
}

#[tokio::test]
async fn an_actor_that_ended_with_an_error_is_released_and_leaves_the_others_alone() {
    // Antes, a entrada morta ficava registrada: o fechamento falhava para sempre, a sessão não
    // reabria e o retrato inicial de eventos (de todas as sessões) respondia erro.
    let dir = tempfile::tempdir().unwrap();
    let (registry,_,cano) = opened(dir.path()).await;
    // A parada falha ao reparar a projeção: o ator termina com erro.
    std::fs::remove_dir_all(dir.path().join("projection")).unwrap();
    std::fs::write(dir.path().join("projection"),"").unwrap();
    let closed = registry.close("key",1).await.unwrap();
    assert_eq!(closed["closed"],true);
    assert!(registry.snapshots().await.unwrap().is_empty());
    let _lease = hangar_server::runtime::queue::acquire_lease(&dir.path().join("key.lock")).unwrap();
    cano.abort();
}

#[tokio::test]
async fn one_bad_message_from_the_cli_does_not_end_the_actor() {
    use hangar_server::runtime::protocol::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cano = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        let bad = json!({"type":"cano_output","frame":json!({"type":"control_response","response":{"request_id":null}}).to_string()});
        reader.get_mut().write_all(format!("{snapshot}\n{bad}\n").as_bytes()).await.unwrap();
        let mut raw = String::new();
        while reader.read_line(&mut raw).await.unwrap_or(0) > 0 { raw.clear(); }
    });
    let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into());
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript:dir.path().join("chat.jsonl"),created:0.0 };
    registry.open(target).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(registry.snapshots().await.unwrap().len(),1,"o ator segue vivo depois da mensagem ruim");
    registry.close("key",1).await.unwrap();
    cano.abort();
}

#[tokio::test]
async fn opening_refused_by_the_queue_says_why() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("key.queue-state.json"),"{}").unwrap();
    let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into());
    let target = hangar_server::runtime::protocol::RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:serde_json::json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:hangar_server::runtime::protocol::CanoBinding { pid:42,escuta:"tcp:127.0.0.1:9".into(),token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("key.lock"),state_path:dir.path().join("key.queue-state.json"),projection_dir:dir.path().join("projection"),
        transcript:dir.path().join("chat.jsonl"),created:0.0 };
    let error = registry.open(target).await.unwrap_err();
    assert!(error.message.contains("estado da fila inválido"),"{}",error.message);
    drop(crate::lease_when_free(&dir.path().join("key.lock"))); // a trava sai junto com a recusa
}

#[tokio::test]
async fn close_with_kill_that_cannot_kill_releases_and_says_not_killed() {
    // Sessão que o Rust não subiu: o `close` solta a posse e diz que não matou, para quem pediu decidir.
    let dir = tempfile::tempdir().unwrap();
    let (registry,_,cano) = opened(dir.path()).await;
    let closed = registry.close_with_kill("key",1).await.unwrap();
    assert_eq!(closed,serde_json::json!({"closed":true,"killed":false}));
    assert!(registry.snapshots().await.unwrap().is_empty(),"a sessão saiu do Rust");
    cano.abort();
}

#[tokio::test]
async fn codex_cano_without_version_reopens_without_launch() {
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
    // Cano subido pelo Python grava o `cano` sem `versao`; reabrir sem subir conecta mesmo assim.
    let dir = tempfile::tempdir().unwrap();
    let cano_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cano_address = cano_listener.local_addr().unwrap();
    let cano = tokio::spawn(async move {
        let (stream,_) = cano_listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        let mut raw = String::new();
        while reader.read_line(&mut raw).await.unwrap_or(0) > 0 { raw.clear(); }
    });
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let address=listener.local_addr().unwrap();
    let registry=Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    let server=tokio::spawn(gateway::serve(listener,registry.clone(),"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let meta=json!({"name":"cx","key":"key","headless":true,"thread_id":"thread-1",
        "cano":{"pid":42,"escuta":format!("tcp:{cano_address}"),"token":"secret-test","ts":1.0}});
    let descriptor=json!({"name":"cx","key":"key","provider":"codex","headless":true,"meta":meta,"jsonl":dir.path().join("rollout.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
    let response=reqwest::Client::new().post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test").header("content-type","application/json")
        .body(json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key","generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},
            "command":{"kind":"open","descriptor":descriptor}}).to_string()).send().await.unwrap();
    let status=response.status(); let text=response.text().await.unwrap();
    assert!(status.is_success(),"{status} {text}");
    registry.close("key",1).await.unwrap();
    server.abort(); cano.abort();
}
