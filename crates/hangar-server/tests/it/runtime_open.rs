//! `open`/`close` do runtime sem terminal: o Rust abre a sessão sozinho, sem o Python adotar antes.
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::*;
use hangar_server::runtime::queue::{Action,State,Store,acquire_lease};
use serde_json::{Value,json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize,Ordering};
use std::time::{Duration,Instant};
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};

/// Cano Claude falso: responde o `initialize` quando `init_gate` libera (na hora, sem ele) e conta os
/// `user` escritos.
async fn cano(init_gate:Option<Arc<tokio::sync::Notify>>,init:Option<&str>) -> (String,Arc<AtomicUsize>,tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let users = Arc::new(AtomicUsize::new(0));
    let counter = users.clone();
    let init = init.map(str::to_owned);
    let task = tokio::spawn(async move {
        // Conexão que não começa pelo token (há quem sonde portas efêmeras nesta máquina) não é o Rust.
        let (mut reader,write) = loop {
            let (stream,_) = listener.accept().await.unwrap();
            let (read,write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut header = String::new();
            if reader.read_line(&mut header).await.is_ok() && header == "secret-test\n" { break (reader,write); }
        };
        let write = Arc::new(tokio::sync::Mutex::new(write));
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":init,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        write.lock().await.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let frame:Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            write.lock().await.write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            if frame["type"] == "user" { counter.fetch_add(1,Ordering::SeqCst); }
            if frame["type"] == "control_request" && frame["request"]["subtype"] == "initialize" {
                let (write,init_gate) = (write.clone(),init_gate.clone());
                tokio::spawn(async move {
                    if let Some(gate) = init_gate { gate.notified().await; }
                    let reply = json!({"type":"control_response","response":{"subtype":"success",
                        "request_id":frame["request_id"],"response":{"commands":[]}}});
                    let line = json!({"type":"cano_output","frame":reply.to_string()});
                    let _ = write.lock().await.write_all(format!("{line}\n").as_bytes()).await;
                });
            }
        }
    });
    (format!("tcp:{address}"),users,task)
}

fn target(dir:&std::path::Path,escuta:String,initialized:bool) -> RuntimeTarget {
    RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":initialized}),
        binding:CanoBinding { pid:42,escuta,token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 }
}

/// Python falso da política: responde `ok` a tudo; `prepare_prompt` devolve o texto como está.
async fn policy() -> std::net::SocketAddr {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
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
                    let data = if kind == "prepare_prompt" { json!({"content":"Olá","notices":[],"native_candidate":false}) } else { json!({}) };
                    let reply = json!({"ok":true,"data":data}).to_string();
                    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}",reply.len());
                    reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                }
            });
        }
    });
    address
}

async fn registry() -> RuntimeRegistry { RuntimeRegistry::new(policy().await,"secret-test".into(),"instance-test".into()) }

fn clock() -> ClockSample { ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 } }

#[tokio::test]
async fn open_runs_queue_recover() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path(),String::new(),true);
    {
        let lease = acquire_lease(&target.lease_path).unwrap();
        let mut store = Store::open(&target.state_path,&target.projection_dir,State::new("key",1,"session",vec![])).unwrap();
        store.exec(1,"prepare",clock(),Action::Prepare { id:"op".into(),payload:json!({"kind":"input"}),entry_id:None }).unwrap();
        store.exec(1,"begin",clock(),Action::BeginDispatch { id:"op".into(),wire_id:"wire".into(),staged:false }).unwrap();
        drop(lease);
    }
    let (escuta,_,server) = cano(None,None).await;
    let registry = registry().await;
    registry.open(RuntimeTarget { binding:CanoBinding { escuta,..target.binding.clone() },..target.clone() }).await.unwrap();
    let state:Value = serde_json::from_slice(&std::fs::read(&target.state_path).unwrap()).unwrap();
    assert_eq!(state["operations"]["op"]["status"],"unknown","entrada em despacho vira incerta ao abrir, antes de o ator olhar");
    registry.close("key",1).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn open_answers_before_initialize() {
    let dir = tempfile::tempdir().unwrap();
    // O `initialize` só sai quando o teste soltar: open que o esperasse não voltaria.
    let gate = Arc::new(tokio::sync::Notify::new());
    let (escuta,users,server) = cano(Some(gate.clone()),None).await;
    let registry = registry().await;
    let opened = tokio::time::timeout(Duration::from_secs(30),registry.open(target(dir.path(),escuta,false))).await
        .expect("open esperou o initialize").unwrap();
    assert_eq!(opened["opened"],true);
    let handle = registry.handle("key",1).await.unwrap();
    let reply = handle.command(RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,
        payload:json!({"text":"Olá","entry_id":"msg"}) }).await.unwrap();
    assert_eq!(reply.disposition,Disposition::Deferred);
    assert_eq!(users.load(Ordering::SeqCst),0);
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(10),async {
        while users.load(Ordering::SeqCst) == 0 { tokio::time::sleep(Duration::from_millis(20)).await; }
    }).await.expect("a entrada sai depois do initialize");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(users.load(Ordering::SeqCst),1,"a entrada sai uma vez");
    registry.close("key",1).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn open_waits_for_lease_then_refuses() {
    for (held,opens) in [(Duration::from_secs(1),true),(Duration::from_secs(4),false)] {
        let dir = tempfile::tempdir().unwrap();
        let (escuta,_,server) = cano(None,None).await;
        let target = target(dir.path(),escuta,true);
        let lease = acquire_lease(&target.lease_path).unwrap();
        let holder = tokio::spawn(async move { tokio::time::sleep(held).await; drop(lease); });
        let registry = registry().await;
        match registry.open(target).await {
            Ok(_)=>{ assert!(opens,"abriu com a trava presa por {held:?}"); registry.close("key",1).await.unwrap(); }
            Err(error)=>{ assert!(!opens,"recusou com a trava presa por {held:?}: {} {}",error.code,error.message); assert_eq!(error.code,"runtime_lease"); }
        }
        holder.await.unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn close_releases_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,_,server) = cano(None,None).await;
    let target = target(dir.path(),escuta,true);
    let registry = registry().await;
    registry.open(target.clone()).await.unwrap();
    assert!(acquire_lease(&target.lease_path).is_err());
    assert_eq!(registry.close("key",1).await.unwrap()["closed"],true);
    drop(crate::lease_when_free(&target.lease_path));
    assert_eq!(registry.close("key",1).await.unwrap()["closed"],true,"fechar o que já fechou não é erro");
    server.abort();
}

#[tokio::test]
async fn reopen_of_initialized_cano_becomes_deliverable() {
    // O cano já passou pelo `initialize` (o snapshot traz o `system/init`), mas a vista nova não sabe:
    // o Rust reenvia e o Claude real responde `success` de novo (medição em harnesses.md).
    let dir = tempfile::tempdir().unwrap();
    let init = json!({"type":"system","subtype":"init","session_id":"sid-1","model":"claude-haiku-4-5"}).to_string();
    let (escuta,users,server) = cano(None,Some(&init)).await;
    let registry = registry().await;
    registry.open(target(dir.path(),escuta,false)).await.unwrap();
    let handle = registry.handle("key",1).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let snapshot = handle.snapshot().await.unwrap();
            if snapshot["view"]["initialized"] == true { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("a sessão fica inicializada");
    let snapshot = handle.snapshot().await.unwrap();
    assert!(snapshot["view"]["problema"].is_null(),"sem headless_nao_subiu: {}",snapshot["view"]["problema"]);
    let reply = handle.command(RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,
        payload:json!({"text":"Olá","entry_id":"msg"}) }).await.unwrap();
    assert_ne!(reply.disposition,Disposition::Rejected);
    tokio::time::timeout(Duration::from_secs(5),async {
        while users.load(Ordering::SeqCst) == 0 { tokio::time::sleep(Duration::from_millis(20)).await; }
    }).await.expect("a sessão reaberta entrega");
    registry.close("key",1).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn failed_open_leaves_no_entry_and_frees_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (_reserved, closed) = crate::refused_address();
    let escuta = format!("tcp:{closed}");
    let target = target(dir.path(),escuta,true);
    let registry = registry().await;
    assert_eq!(registry.open(target.clone()).await.unwrap_err().code,"cano_connect");
    assert!(registry.handle("key",1).await.is_err());
    drop(crate::lease_when_free(&target.lease_path)); // a trava sai junto com a falha
}

/// Cano recém-lançado que só cria o socket depois de um tempo (escopo do systemd + exec).
#[cfg(unix)]
#[tokio::test]
async fn open_waits_for_launched_cano_to_listen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cano.sock");
    let socket = path.clone();
    let server = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (stream,_) = listener.accept().await.unwrap();
        let (read,mut write) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        let mut header = String::new();
        reader.read_line(&mut header).await.unwrap();
        assert_eq!(header,"secret-test\n");
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        write.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { break; }
            let envelope:Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            write.write_all(format!("{ack}\n").as_bytes()).await.unwrap();
        }
    });
    let target = target(dir.path(),format!("unix:{}",path.display()),true);
    let registry = registry().await;
    let started = Instant::now();
    let opened = registry.open(target).await.expect("o open espera o cano começar a escutar");
    assert_eq!(opened["opened"],true);
    assert!(started.elapsed() >= Duration::from_millis(250));
    registry.close("key",1).await.unwrap();
    server.abort();
}
