use hangar_server::runtime::gateway::{self,RuntimeRegistry};
use std::sync::Arc;

#[tokio::test]
async fn wrong_secret_instance_or_generation_denied() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into()));
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
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into()));
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
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into()));
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
    let registry=Arc::new(RuntimeRegistry::new("instance-test".into()));
    let server=tokio::spawn(gateway::serve(listener,registry,"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let binding=serde_json::json!({"name":"session","pane":"%1","conversation":"sid","generation":1,"created":1,"mux_argv":["fake"],"windows":false,"clipboard_lock_path":null});
    let descriptor=serde_json::json!({"name":"session","key":"key","provider":"claude","headless":false,"meta":{"key":"key","terminal":binding,"config_dir":dir.path(),"agent_pid":std::process::id(),"agent_birth":hangar_server::terminal_facts::agent_birth(std::process::id()).unwrap(),"fingerprint":"fixture-life","message_origin":"uds:fixture"},"jsonl":dir.path().join("chat.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
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
    // Ligado aos hubs como o servidor monta: fechar a vida do terminal esquece a cota pelo hub.
    let registry=crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into()));
    let server=tokio::spawn(gateway::serve(listener,registry.clone(),"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let binding=serde_json::json!({"name":"session","pane":"%1","conversation":"sid","generation":1,"created":1,"mux_argv":["fake"],"windows":false,"clipboard_lock_path":null});
    let descriptor=serde_json::json!({"name":"session","key":"key","provider":"claude","headless":false,"meta":{"key":"key","terminal":binding,"config_dir":dir.path(),"agent_pid":std::process::id(),"agent_birth":hangar_server::terminal_facts::agent_birth(std::process::id()).unwrap(),"fingerprint":"fixture-life","message_origin":"uds:fixture"},"jsonl":dir.path().join("chat.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
    let client=reqwest::Client::new();
    let send=|generation,command|client.post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test").header("content-type","application/json").body(serde_json::json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key","generation":generation,"operation_id":"gateway","clock":{"monotonic_s":0.0,"epoch_s":0.0},"command":command}).to_string());
    let response=send(1,serde_json::json!({"kind":"open","descriptor":descriptor})).send().await.unwrap(); assert!(response.status().is_success());
    let value:serde_json::Value=serde_json::from_str(&response.text().await.unwrap()).unwrap(); assert_eq!(value["result"]["state"]["view"]["terminal"],true); assert!(value["result"]["state"]["view"].get("public_state").is_none());
    assert!(registry.handle("key",1).await.is_err());
    let readonly=registry.terminal_target("session").await.unwrap().unwrap();assert_eq!(readonly.binding.conversation,"sid");
    assert_eq!(registry.interaction_binding_readonly("session").await.unwrap().generation,1);
    assert_eq!(registry.claude_info("session").await.unwrap().session_key,"sid","chave do hub é a conversa, não a do runtime");
    assert_eq!(registry.list_snapshots_readonly().await["key"]["error"],"runtime_live_unavailable","sem Published não inventa estado parado");
    assert_eq!(send(2,serde_json::json!({"kind":"snapshot"})).send().await.unwrap().status(),503);
    assert!(hangar_server::runtime::queue::acquire_lease(&dir.path().join("lease")).is_err());
    assert!(send(1,serde_json::json!({"kind":"close"})).send().await.unwrap().status().is_success());
    drop(crate::lease_when_free(&dir.path().join("lease"))); server.abort();
}

/// Registro com um cano Claude falso que só aceita entradas; a política aponta para uma porta fechada.
async fn opened(dir:&std::path::Path) -> (Arc<RuntimeRegistry>,serde_json::Value,tokio::task::JoinHandle<()>) {
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
    let registry = crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))));
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 };
    let ready = registry.open(target).await.unwrap();
    (registry,ready,cano)
}

/// O mesmo registro de `opened`, com o cano falso devolvendo cada linha que recebe.
async fn opened_recording(dir:&std::path::Path) -> (Arc<RuntimeRegistry>,tokio::sync::mpsc::UnboundedReceiver<String>,tokio::task::JoinHandle<()>) {
    use hangar_server::runtime::protocol::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (lines,received) = tokio::sync::mpsc::unbounded_channel();
    let cano = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut header = String::new(); reader.read_line(&mut header).await.unwrap();
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        let mut raw = String::new();
        while reader.read_line(&mut raw).await.unwrap_or(0) > 0 { let _ = lines.send(std::mem::take(&mut raw)); }
    });
    let registry = crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))));
    let target = RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding { pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 };
    registry.open(target).await.unwrap();
    (registry,received,cano)
}

/// A primeira linha recebida pelo cano que traz um dos textos, até o prazo.
async fn first_written(lines:&mut tokio::sync::mpsc::UnboundedReceiver<String>,texts:&[&str],within:std::time::Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let line = tokio::time::timeout_at(deadline,lines.recv()).await.ok()??;
        // A fila entrega em lote: vários textos na mesma linha, e quem vem antes é o que aparece antes nela.
        if let Some((_,text)) = texts.iter().filter_map(|text|line.find(*text).map(|at|(at,*text))).min() { return Some(text.to_owned()); }
    }
}

#[tokio::test]
async fn a_queue_held_by_the_usage_limit_delivers_nothing_until_let_go() {
    use hangar_server::runtime::{gateway::LimitHit,protocol::UsageLimit,queue::Action};
    let dir = tempfile::tempdir().unwrap();
    let (registry,mut lines,cano) = opened_recording(dir.path()).await;
    let (sink,_hits) = tokio::sync::mpsc::unbounded_channel();
    registry.set_limit_sink(sink);
    registry.usage_limit(LimitHit { name:"session".into(),provider:"claude".into(),account:None,
        limit:UsageLimit { window:"five_hour".into(),resets_at:None } });
    registry.queue_action("session","q1".into(),Action::Append { text:"mensagem-na-fila".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("q1".into()) }).await.unwrap();
    assert_eq!(first_written(&mut lines,&["mensagem-na-fila"],std::time::Duration::from_millis(500)).await,None,"a fila segurada entregou");
    registry.release_limit("session");
    assert_eq!(first_written(&mut lines,&["mensagem-na-fila"],std::time::Duration::from_secs(10)).await.as_deref(),Some("mensagem-na-fila"));
    registry.close("key",1).await.unwrap();
    cano.abort();
}

#[tokio::test]
async fn the_limit_switch_that_did_not_happen_shows_in_the_live_state() {
    let dir = tempfile::tempdir().unwrap();
    let (registry,_,cano) = opened(dir.path()).await;
    let mut live = registry.live("session");
    registry.show_limit_problem("session",Some(("limit_switch_no_room".into(),"17:30".into()))).await;
    let shown = tokio::time::timeout(std::time::Duration::from_secs(10),live.wait_for(|state|state.as_ref()
        .is_some_and(|state|state.public_state["problema"] == "limit_switch_no_room"))).await;
    let state = shown.expect("o estado ao vivo não mostrou o problema").unwrap().clone().unwrap();
    assert_eq!(state.public_state["problema_detalhe"],"17:30");
    registry.close("key",1).await.unwrap();
    cano.abort();
}

#[tokio::test]
async fn readonly_headless_binding_comes_from_the_owner_feed() {
    let dir=tempfile::tempdir().unwrap();let (registry,_,cano)=opened(dir.path()).await;
    let binding=registry.interaction_binding_readonly("session").await.unwrap();assert_eq!(binding.key,"key");assert_eq!(binding.generation,1);assert_eq!(binding.conversation,"sid-1");
    let live=registry.live("session");assert_eq!(live.borrow().as_ref().unwrap().binding.as_ref(),Some(&binding));
    assert!(registry.terminal_target("session").await.unwrap().is_none());
    let snapshots=registry.list_snapshots_readonly().await;assert_eq!(snapshots["key"]["key"],"key");assert_eq!(snapshots["key"]["view"]["headless"],true);
    registry.close("key",1).await.unwrap();assert!(registry.interaction_binding_readonly("session").await.is_err());cano.abort();
}
/// A chave do hub é a conversa (o feed e a taxa a comparam com `conversation`), nunca a chave do runtime.
#[tokio::test]
async fn headless_claude_info_keys_the_hub_by_conversation() {
    let dir=tempfile::tempdir().unwrap();let (registry,_,cano)=opened(dir.path()).await;
    let binding=registry.interaction_binding_readonly("session").await.unwrap();
    let info=registry.claude_info("session").await.unwrap();
    assert_eq!(info.session_key,binding.conversation);
    assert_eq!(info.jsonl.unwrap().file_name().unwrap(),"sid-1.jsonl");
    registry.close("key",1).await.unwrap();cano.abort();
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
    // A parada falha ao reparar a projeção: o ator termina com erro. O ator ainda confere a projeção
    // sozinho depois de abrir, e no Windows o arquivo apagado com o leitor aberto segura a pasta até ele fechar.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while let Err(error) = std::fs::remove_dir_all(dir.path().join("projection")) {
        assert!(std::time::Instant::now() < deadline,"a projeção não foi liberada: {error}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
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
    let registry = crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))));
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
    let registry = RuntimeRegistry::new("instance-test".into());
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
    let registry=Arc::new(RuntimeRegistry::new("instance-test".into()));
    let server=tokio::spawn(gateway::serve(listener,registry.clone(),"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let meta=json!({"name":"cx","key":"key","headless":true,"thread_id":"thread-1",
        "cano":{"pid":42,"escuta":format!("tcp:{cano_address}"),"token":"secret-test","ts":1.0}});
    std::fs::write(dir.path().join("cx.json"),meta.to_string()).unwrap();
    // Codex sem terminal sempre vem com a pasta do sidecar (`_open_codex`); sem ela o gateway recusa.
    let descriptor=json!({"name":"cx","key":"key","provider":"codex","headless":true,"meta":meta,"jsonl":dir.path().join("rollout.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1,"sidecar_dir":dir.path()});
    let mut bare=descriptor.clone(); bare.as_object_mut().unwrap().remove("sidecar_dir");
    let refused=reqwest::Client::new().post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test").header("content-type","application/json")
        .body(json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key","generation":1,"operation_id":"op-bare","clock":{"monotonic_s":0.0,"epoch_s":0.0},
            "command":{"kind":"open","descriptor":bare}}).to_string()).send().await.unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&refused.text().await.unwrap()).unwrap()["error_code"],"launch_sidecar_dir");
    let response=reqwest::Client::new().post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test").header("content-type","application/json")
        .body(json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key","generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},
            "command":{"kind":"open","descriptor":descriptor}}).to_string()).send().await.unwrap();
    let status=response.status(); let text=response.text().await.unwrap();
    assert!(status.is_success(),"{status} {text}");
    registry.close("key",1).await.unwrap();
    server.abort(); cano.abort();
}

/// O Codex com terminal não abre pelo gateway: quem o adota é a vigia do Rust (`runtime_codex_watch`).
#[cfg(unix)]
#[tokio::test]
async fn codex_terminal_never_opens_over_the_gateway() {
    use serde_json::{Value,json};
    let dir=tempfile::tempdir().unwrap();
    let app=crate::runtime_open::fake_app_server(true).await;
    let mut child=crate::runtime_open::app_server_process(&app.endpoint);
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let address=listener.local_addr().unwrap();
    let registry=crate::runtime_open::registry().await;
    let server=tokio::spawn(gateway::serve(listener,registry.clone(),"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL));
    let descriptor=json!({"name":"cx-proof","key":"codex-key","provider":"codex","headless":false,
        "meta":{"key":"codex-key","thread_id":"thread-1","endpoint":app.endpoint.clone(),"app_pid":child.id(),"codex_home":dir.path().join("codex-home"),"cano":null,
            "terminal":{"name":"cx-proof","pane":"%7","conversation":"thread-1","generation":1,"created":1,"mux_argv":["tmux"],"windows":false,"clipboard_lock_path":null}},
        "jsonl":"","projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
    let response=reqwest::Client::new().post(format!("http://{address}/runtime/op")).header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test")
        .header("content-type","application/json").body(json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"codex-key","generation":1,
            "operation_id":"open","clock":{"monotonic_s":0.0,"epoch_s":0.0},"command":{"kind":"open","descriptor":descriptor}}).to_string()).send().await.unwrap();
    assert_eq!(response.status(),503);
    assert_eq!(serde_json::from_str::<Value>(&response.text().await.unwrap()).unwrap()["error_code"],"descriptor_provider");
    assert_eq!(app.connections.load(std::sync::atomic::Ordering::SeqCst),0,"nada conectou");
    assert!(!registry.owns_codex_terminal("cx-proof"));
    server.abort();
    child.kill().unwrap(); child.wait().unwrap();
}
