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

/// Cano 42 com o `claude` 43 da conversa dada (`--session-id`).
pub(crate) struct RecipeProcesses(pub &'static str);
impl hangar_server::list::procs::ProcessView for RecipeProcesses {
    fn children(&self,_:Duration)->std::io::Result<Arc<hangar_server::list::procs::ChildrenMap>> {Ok(Arc::new(std::collections::HashMap::from([(42,vec![43])]))) }
    fn argv(&self,_:i64)->Vec<String> {vec![]}
    fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(std::env::temp_dir())}
    fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
    fn start_time(&self,_:i64)->Option<f64> {Some(1000.0)}
    fn recipe(&self,pid:i64)->std::io::Result<hangar_server::list::procs::ProcessRecipe> {
        let argv=match pid {42=>vec!["hangar-cano".into()],43=>vec!["claude".into(),"--session-id".into(),self.0.into()],_=>return Err(std::io::ErrorKind::NotFound.into())};
        Ok(hangar_server::list::procs::ProcessRecipe {pid,birth:1000.0,executable:if pid==42 {"/fixture/hangar-cano".into()} else {"/fixture/claude".into()},cwd:std::env::temp_dir(),argv,environ:vec!["HOME=/fixture".into()],cgroup:None})
    }
}
pub(crate) fn with_hubs(registry:RuntimeRegistry)->Arc<RuntimeRegistry> {
    let registry=Arc::new(registry);let hubs=hangar_server::side::Hubs::default();registry.bind_lifecycle();registry.bind_hubs(hubs.downgrade());
    let mut events=registry.subscribe();
    tokio::spawn(async move {let _hubs=hubs;while events.recv().await.is_ok() {}});
    registry
}
pub(crate) async fn registry() -> Arc<RuntimeRegistry> { registry_with(|registry|registry).await }
/// O ajuste entra antes do `Arc`: depois de ligado aos hubs o registro não muda mais.
pub(crate) async fn registry_with(adjust:impl FnOnce(RuntimeRegistry)->RuntimeRegistry) -> Arc<RuntimeRegistry> {
    with_hubs(adjust(RuntimeRegistry::new(policy().await,"secret-test".into(),"instance-test".into()).with_processes(Arc::new(RecipeProcesses("sid-1")))))
}

/// App-server do Codex falso, em WebSocket de loopback: conta as conexões, guarda os quadros de cada
/// uma e derruba a que o teste pedir. `initialize` responde; `thread/resume` diz que ainda não há
/// rollout; `turn/start` só responde com `answer_turns`.
#[cfg(unix)]
pub(crate) struct FakeAppServer {
    pub endpoint:String,
    pub connections:Arc<AtomicUsize>,
    frames:Arc<std::sync::Mutex<Vec<(usize,Value)>>>,
    drop_connection:tokio::sync::watch::Sender<usize>,
    task:tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl Drop for FakeAppServer { fn drop(&mut self) { self.task.abort(); } }

#[cfg(unix)]
impl FakeAppServer {
    pub fn frames_of(&self,connection:usize) -> Vec<Value> {
        self.frames.lock().unwrap().iter().filter(|(n,_)|*n == connection).map(|(_,frame)|frame.clone()).collect()
    }
    pub async fn wait_method(&self,connection:usize,method:&str) -> Value {
        tokio::time::timeout(Duration::from_secs(10),async {
            loop {
                if let Some(frame) = self.frames_of(connection).into_iter().find(|f|f["method"] == method) { return frame; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap_or_else(|_|panic!("conexão {connection} sem {method}"))
    }
    /// Derruba só o WebSocket `connection` (1 = o primeiro); o processo falso segue vivo.
    pub fn drop_connection(&self,connection:usize) { self.drop_connection.send_replace(connection); }
}

#[cfg(unix)]
pub(crate) async fn fake_app_server(answer_turns:bool) -> FakeAppServer {
    use futures_util::{SinkExt,StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}",listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let frames:Arc<std::sync::Mutex<Vec<(usize,Value)>>> = Arc::default();
    let (drop_connection,_) = tokio::sync::watch::channel(0usize);
    let (counter,seen,dropper) = (connections.clone(),frames.clone(),drop_connection.clone());
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream,_)) = listener.accept().await else { return };
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { continue };
            let index = counter.fetch_add(1,Ordering::SeqCst) + 1;
            let (seen,mut dropped) = (seen.clone(),dropper.subscribe());
            tokio::spawn(async move {
                loop {
                    let message = tokio::select! {
                        message = ws.next() => message,
                        _ = dropped.wait_for(|n|*n == index) => return,
                    };
                    let Some(Ok(message)) = message else { return };
                    let Ok(text) = message.to_text() else { continue };
                    let Ok(frame) = serde_json::from_str::<Value>(text) else { continue };
                    seen.lock().unwrap().push((index,frame.clone()));
                    let reply = match frame["method"].as_str() {
                        Some("initialize") => Some(json!({"id":frame["id"],"result":{"userAgent":"fake"}})),
                        // Conversa que já tem rollout: a assinatura cola (o terminal que acorda espera por ela).
                        Some("thread/resume") if frame["params"]["threadId"] == "thread-resumable" =>
                            Some(json!({"id":frame["id"],"result":{"thread":{"id":"thread-resumable","path":"/tmp/rollout.jsonl","status":{"type":"idle"}}}})),
                        Some("thread/settings/update") if frame["params"]["threadId"] == "thread-resumable" => Some(json!({"id":frame["id"],"result":{}})),
                        Some("thread/resume") => Some(json!({"id":frame["id"],"error":{"code":-32600,"message":"no rollout found for thread id thread-1"}})),
                        Some("turn/start") if answer_turns => Some(json!({"id":frame["id"],"result":{"turn":{"id":"turn-1","status":"inProgress","items":[]}}})),
                        // A configuração que a TUI tem agora (o `/model` digitado nela não avisa o motor).
                        Some("thread/read") if frame["params"]["threadId"] == "thread-unreadable" =>
                            Some(json!({"id":frame["id"],"error":{"code":-32600,"message":"thread not loaded"}})),
                        Some("thread/read") => Some(json!({"id":frame["id"],"result":{"thread":{"id":frame["params"]["threadId"],"status":{"type":"idle"},
                            "turns":[],"model":"gpt-tui","reasoningEffort":"high"}}})),
                        _ => None,
                    };
                    if let Some(reply) = reply {
                        if ws.send(tokio_tungstenite::tungstenite::Message::text(reply.to_string())).await.is_err() { return; }
                    }
                }
            });
        }
    });
    FakeAppServer { endpoint,connections,frames,drop_connection,task }
}

/// Processo cujo argv é o de um app-server deste endereço (`sh -c '…' app-server <endpoint>`): prova o pid
/// sem app-server de verdade. O `read` embutido bloqueia na entrada (presa ao `Child`) sem `fork`: um `sleep`
/// filho criado logo depois da prova deixava, no macOS, o argv do shell ilegível na leitura seguinte.
#[cfg(unix)]
pub(crate) fn app_server_process(endpoint:&str) -> std::process::Child {
    with_app_server_argv(std::process::Command::new("sh").args(["-c","read x","app-server",endpoint])
        .stdin(std::process::Stdio::piped()).spawn().unwrap(),endpoint)
}

/// O `spawn` volta quando o exec troca a memória do filho, antes de o kernel gravar o argv novo: por um
/// instante o `cmdline` vem vazio e a admissão recusa o pid. Devolve o filho só com o argv já legível.
#[cfg(unix)]
pub(crate) fn with_app_server_argv(child:std::process::Child,endpoint:&str) -> std::process::Child {
    use hangar_server::runtime::codex_terminal::{AppServer,app_server};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app_server(child.id(),endpoint) != AppServer::Ours {
        assert!(std::time::Instant::now() < deadline,"o filho não chegou ao argv do app-server");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    child
}

#[cfg(unix)]
pub(crate) fn codex_terminal_target(dir:&std::path::Path,endpoint:&str,app_pid:u32) -> hangar_server::runtime::codex_terminal::CodexTerminalTarget {
    hangar_server::runtime::codex_terminal::CodexTerminalTarget { key:"codex-key".into(),generation:1,name:"cx-proof".into(),
        endpoint:endpoint.into(),app_pid,thread_id:"thread-1".into(),
        terminal:hangar_server::terminal_input::TerminalBinding { name:"cx-proof".into(),pane:"%7".into(),conversation:"thread-1".into(),
            generation:1,created:1,mux_argv:vec!["tmux".into()],windows:false,clipboard_lock_path:None },
        metadata:json!({"key":"codex-key","thread_id":"thread-1","endpoint":endpoint,"app_pid":app_pid,"cwd":dir,"codex_home":dir.join("codex-home")}),
        transcript:None,lease_path:dir.join("codex-key.lock"),state_path:dir.join("codex-key.queue-state.json"),projection_dir:dir.join("projection"),
        sidecar_dir:None }
}

#[cfg(unix)]
#[tokio::test]
async fn codex_terminal_is_adopted_without_rollout_and_sends_before_subscribing() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_app_server(true).await;
    let mut child = app_server_process(&server.endpoint);
    let registry = registry().await;
    let opened = registry.open_codex_terminal(codex_terminal_target(dir.path(),&server.endpoint,child.id())).await.unwrap();
    assert_eq!(opened["opened"],true);
    server.wait_method(1,"initialize").await;
    server.wait_method(1,"thread/resume").await;
    let handle = registry.handle("codex-key",1).await.unwrap();
    let sent = tokio::spawn(async move { handle.command(RuntimeCommand { operation_id:"msg".into(),kind:OperationKind::Input,
        payload:json!({"text":"Olá","entry_id":"msg"}) }).await });
    // Conectado já envia: o primeiro turno é o que cria o rollout que a assinatura exige.
    let start = server.wait_method(1,"turn/start").await;
    assert_eq!(start["params"]["threadId"],"thread-1");
    assert_ne!(sent.await.unwrap().unwrap().disposition,Disposition::Rejected);
    let view = registry.handle("codex-key",1).await.unwrap().view().await.unwrap();
    assert_eq!(view["headless"],false,"o estado terminal não sai como sem terminal");
    assert_eq!(view["public_state"]["headless"],false);
    // Sem a vigia ligada, encerrar recusa; fechar solta a posse e deixa o processo vivo.
    assert_eq!(registry.close_with_kill("codex-key",1).await.unwrap_err().code,"close_kill");
    assert_eq!(registry.close("codex-key",1).await.unwrap()["closed"],true);
    assert!(child.try_wait().unwrap().is_none(),"o app-server adotado segue vivo");
    assert_eq!(server.connections.load(Ordering::SeqCst),1,"um cliente só");
    drop(crate::lease_when_free(&dir.path().join("codex-key.lock")));
    child.kill().unwrap(); child.wait().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn codex_terminal_admission_proves_the_pid_and_fences_another_life() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake_app_server(true).await;
    let mut child = app_server_process(&server.endpoint);
    let registry = registry().await;
    // Pid vivo que não é o app-server deste endereço: recusado antes de pegar a trava e de conectar.
    let error = registry.open_codex_terminal(codex_terminal_target(dir.path(),&server.endpoint,std::process::id())).await.unwrap_err();
    assert_eq!(error.code,"codex_app_pid");
    assert_eq!(server.connections.load(Ordering::SeqCst),0);
    drop(crate::lease_when_free(&dir.path().join("codex-key.lock")));
    let target = codex_terminal_target(dir.path(),&server.endpoint,child.id());
    registry.open_codex_terminal(target.clone()).await.unwrap();
    // A mesma vida reabre sem outro cliente; outra (endereço trocado na mesma geração) é recusada.
    registry.open_codex_terminal(target.clone()).await.unwrap();
    let other = hangar_server::runtime::codex_terminal::CodexTerminalTarget { endpoint:"ws://127.0.0.1:1".into(),..target };
    assert_eq!(registry.open_codex_terminal(other).await.unwrap_err().code,"runtime_generation");
    server.wait_method(1,"initialize").await;
    assert_eq!(server.connections.load(Ordering::SeqCst),1);
    // O app-server morreu: reabrir a mesma vida responde o erro, não `opened` com um ator desconectado.
    child.kill().unwrap(); child.wait().unwrap();
    let again = codex_terminal_target(dir.path(),&server.endpoint,child.id());
    assert_eq!(registry.open_codex_terminal(again).await.unwrap_err().code,"codex_app_pid");
    registry.close("codex-key",1).await.unwrap();
}

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
    let state:Value = serde_json::from_slice(&hangar_server::runtime::queue::read_replaced(&target.state_path).unwrap()).unwrap();
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
    // A linha própria solta no relógio, sem depender de quando o executor do teste a atende; `MAX`
    // segura até o `open` responder.
    for (held,opens) in [(Duration::from_secs(1),true),(Duration::MAX,false)] {
        let dir = tempfile::tempdir().unwrap();
        let (escuta,_,server) = cano(None,None).await;
        let target = target(dir.path(),escuta,true);
        let registry = registry().await;
        let lease = acquire_lease(&target.lease_path).unwrap();
        let (release,released) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || { let _ = released.recv_timeout(held); drop(lease); });
        match registry.open(target).await {
            Ok(_)=>{ assert!(opens,"abriu com a trava presa"); registry.close("key",1).await.unwrap(); }
            Err(error)=>{ assert!(!opens,"recusou com a trava presa por {held:?}: {} {}",error.code,error.message); assert_eq!(error.code,"runtime_lease"); }
        }
        drop(release);
        holder.join().unwrap();
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
    let closed = crate::refused_address();
    let escuta = format!("tcp:{closed}");
    let target = target(dir.path(),escuta,true);
    let registry = registry().await;
    assert_eq!(registry.open(target.clone()).await.unwrap_err().code,"cano_connect");
    assert!(registry.handle("key",1).await.is_err());
    drop(crate::lease_when_free(&target.lease_path)); // a trava sai junto com a falha
}

/// Sessão em motor: o filho do cano é o `hangar-engine` (Python) até o `execvpe` virar o `claude`.
struct EngineBeforeExec(Instant);
impl hangar_server::list::procs::ProcessView for EngineBeforeExec {
    fn children(&self,timeout:Duration)->std::io::Result<Arc<hangar_server::list::procs::ChildrenMap>> {RecipeProcesses("sid-1").children(timeout)}
    fn argv(&self,_:i64)->Vec<String> {vec![]}
    fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(std::env::temp_dir())}
    fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
    fn start_time(&self,_:i64)->Option<f64> {Some(1000.0)}
    fn recipe(&self,pid:i64)->std::io::Result<hangar_server::list::procs::ProcessRecipe> {
        let mut recipe=RecipeProcesses("sid-1").recipe(pid)?;
        if pid==43 && self.0.elapsed()<Duration::from_millis(300) {
            recipe.executable="/usr/bin/python3".into();
            recipe.argv=["python3","/fixture/hangar-engine","--exec","motor","--","claude","--session-id","sid-1"].iter().map(Into::into).collect();
        }
        Ok(recipe)
    }
}

#[tokio::test]
async fn open_waits_for_the_engine_wrapper_to_become_claude() {
    let dir = tempfile::tempdir().unwrap();
    let (escuta,_,server) = cano(None,None).await;
    let registry = registry_with(|registry|registry.with_processes(Arc::new(EngineBeforeExec(Instant::now())))).await;
    let opened = registry.open(target(dir.path(),escuta,true)).await.expect("o open espera o exec do hangar-engine");
    assert_eq!(opened["opened"],true);
    registry.close("key",1).await.unwrap();
    server.abort();
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

/// A sessão sem terminal que não sobe grava `headless_nao_subiu` no arquivo dela, como o
/// `_registrar_problema` do Python: a próxima vida (e o restart do backend) ainda mostram o motivo.
#[tokio::test]
async fn a_failed_open_is_written_to_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let sidecars = dir.path().join("claude-headless");
    std::fs::create_dir_all(&sidecars).unwrap();
    std::fs::write(sidecars.join("session.json"),json!({"name":"session","key":"key","session_id":"sid-1","headless":true}).to_string()).unwrap();
    let target = target(dir.path(),format!("tcp:{}",crate::refused_address()),true);
    let registry = registry().await;
    assert_eq!(registry.open_managed(target,sidecars.clone()).await.unwrap_err().code,"cano_connect");
    let meta:Value = serde_json::from_slice(&std::fs::read(sidecars.join("session.json")).unwrap()).unwrap();
    assert_eq!(meta["problema"][0],"headless_nao_subiu","{meta}");
    assert!(meta["problema"][1].as_str().unwrap_or("").contains("cano_connect"),"{meta}");
}

/// A religação sem terminal toma a guarda da conta como a abertura: com a conta presa por quem
/// a troca ou desloga (guarda exclusiva), recusa antes de encerrar o processo.
#[tokio::test]
async fn the_headless_relaunch_takes_the_account_guard_like_the_open() {
    use hangar_server::accounts::{AccountKey,AccountLocks,GuardMode,Provider};
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join("conta");
    std::fs::create_dir_all(&account).unwrap();
    let (escuta,_,server) = cano(None,None).await;
    let mut target = target(dir.path(),escuta,true);
    target.metadata["config_dir"] = json!(account);
    let registry = registry().await;
    registry.open(target).await.unwrap();
    let held = AccountLocks::system().unwrap().try_acquire(&AccountKey::new(Provider::Claude,&account).unwrap(),GuardMode::Exclusive).unwrap();
    let refused = registry.clone().reopen_claude("session".into(),"key".into(),1,"op".into()).await.unwrap_err();
    assert_eq!(refused.code,"account_busy");
    drop(held);
    registry.close("key",1).await.unwrap();
    server.abort();
}
