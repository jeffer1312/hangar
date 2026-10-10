//! Servidor de teste da voz: pastas temporárias, Python falso atrás, app-server falso e prazos curtos.
use crate::fake::{self, Fake};
use axum::{Router, http::header::CONTENT_TYPE, response::IntoResponse, routing::get};
use hangar_server::groups::peers::{PeerBook, PeerClient};
use hangar_server::voice::call::Spawn;
use hangar_server::voice::hub::{SpawnFactory, TestParts, VoiceHub};
use hangar_server::voice::rpc::Rpc;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;

fn hub(home: &Path, claude_dir: PathBuf, spawn: Option<SpawnFactory>) -> Arc<VoiceHub> {
    Arc::new(VoiceHub::with_parts(TestParts { home: home.to_path_buf(), claude_dir, grace: Duration::from_millis(400),
        gate_every: Duration::from_millis(100), spawn, accounts: Some(vec!["default".into()]) }))
}

/// Só a configuração (Task 3): nenhuma chamada sobe.
pub async fn server(home: &Path) -> SocketAddr {
    let (_python, upstream) = fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, ""));
    state.voice = hub(home, home.join(".claude"), None);
    fake::spawn_state(state).await
}

type Seen = Arc<Mutex<Vec<Value>>>;
type Push = Arc<Mutex<Option<UnboundedSender<Value>>>>;

pub struct TestVoice {
    pub addr: SocketAddr,
    pub python: Arc<Fake>,
    seen: Seen,
    push: Push,
    claude_dir: PathBuf,
    _home: tempfile::TempDir,
}

fn js(v: Value) -> axum::response::Response { ([(CONTENT_TYPE, "application/json")], v.to_string()).into_response() }

/// A API deste servidor no papel da própria: a lista com a sessão `hangar`, o resto vazio.
async fn fake_api() -> SocketAddr {
    let app = Router::new()
        .route("/api/sessions", get(|| async { js(json!([{"name": "hangar", "provider": "claude", "state": "idle"}])) }))
        .fallback(|| async { js(json!({})) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Um app-server falso por chamada, todos anotando no mesmo `seen`. Cada `thread/realtime/start` devolve `v=0 answer <n>`.
fn fake_app_servers(seen: Seen, push: Push) -> SpawnFactory {
    Arc::new(move || -> Spawn {
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        let (push_tx, mut pushed) = tokio::sync::mpsc::unbounded_channel::<Value>();
        *push.lock().unwrap() = Some(push_tx);
        let seen = seen.clone();
        tokio::spawn(async move {
            let (read, write) = tokio::io::split(theirs);
            let write = Arc::new(tokio::sync::Mutex::new(write));
            let w = write.clone();
            tokio::spawn(async move { while let Some(v) = pushed.recv().await { let _ = w.lock().await.write_all(format!("{v}\n").as_bytes()).await; } });
            let mut lines = BufReader::new(read).lines();
            let mut starts = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = serde_json::from_str(&line).unwrap();
                seen.lock().unwrap().push(msg.clone());
                let result = match msg["method"].as_str() {
                    Some("initialize") => json!({}),
                    Some("config/read") => json!({"config": {"model": "gpt-test"}}),
                    Some("thread/start") => json!({"thread": {"id": "t1"}, "model": "gpt-test"}),
                    Some("thread/realtime/start") => {
                        starts += 1;
                        let sdp = format!("v=0 answer {starts}");
                        let w = write.clone();
                        tokio::spawn(async move {
                            let note = json!({"method": "thread/realtime/sdp", "params": {"threadId": "t1", "sdp": sdp}});
                            let _ = w.lock().await.write_all(format!("{note}\n").as_bytes()).await;
                        });
                        json!({})
                    }
                    Some(_) => json!({}),
                    None => continue,
                };
                if msg.get("id").is_some() {
                    let _ = write.lock().await.write_all(format!("{}\n", json!({"id": msg["id"], "result": result})).as_bytes()).await;
                }
            }
        });
        let (r, w) = tokio::io::split(ours);
        Box::new(move || Box::pin(async move { Ok(Rpc::over_lines(r, w)) }))
    })
}

fn write_gate(claude_dir: &Path, beta: bool) {
    std::fs::write(claude_dir.join("runtime-config.json"), json!({"codex_voice_beta": beta}).to_string()).unwrap();
}

/// `own_api`: `false` = API falsa; `true` = o próprio servidor de teste (o caminho real até o Python falso).
async fn build(beta: bool, account: Option<&str>, own_api: bool) -> TestVoice {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    write_gate(&claude_dir, beta);
    if let Some(account) = account {
        let dir = home.path().join(".hangar/voz");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), json!({"voice": null, "codex_account": account,
            "organizer": {"direct": {"effort": "low"}, "plan": {"effort": "low"}}}).to_string()).unwrap();
    }
    let (seen, push): (Seen, Push) = (Arc::default(), Arc::default());
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, ""));
    state.voice = hub(home.path(), claude_dir.clone(), Some(fake_app_servers(seen.clone(), push.clone())));
    // Nunca os peers reais da máquina.
    state.peers = Arc::new(PeerClient::new(PeerBook::new(None)));
    // O `set_self` do servidor não sobrescreve este (é gravado uma vez só).
    if !own_api { state.voice.set_self(fake_api().await, "t"); }
    let addr = fake::spawn_state(state).await;
    TestVoice { addr, python, seen, push, claude_dir, _home: home }
}

pub async fn voice_server(beta: bool) -> TestVoice { build(beta, None, false).await }

pub async fn voice_server_with_account(beta: bool, account: &str) -> TestVoice { build(beta, Some(account), false).await }

pub async fn voice_server_self_api(beta: bool) -> TestVoice { build(beta, None, true).await }

impl TestVoice {
    pub fn app_server_saw(&self, method: &str) -> bool { self.count(method) > 0 }

    pub fn count(&self, method: &str) -> usize { self.seen.lock().unwrap().iter().filter(|m| m["method"] == method).count() }

    pub fn last(&self, method: &str) -> Value {
        self.seen.lock().unwrap().iter().rev().find(|m| m["method"] == method).cloned().unwrap_or(Value::Null)
    }

    /// Como `app_server_saw`, esperando até 2 s: o pedido sai da chamada em outra tarefa.
    pub async fn app_server_sees(&self, method: &str) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if self.app_server_saw(method) { return true; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    pub fn push(&self, v: Value) {
        let _ = self.push.lock().unwrap().as_ref().expect("nenhum app-server subiu").send(v);
    }

    pub fn set_beta(&self, beta: bool) { write_gate(&self.claude_dir, beta); }

    /// Fala do usuário no turno `turn`, como a voz delega: libera as ferramentas desse turno.
    pub fn spoken_turn(&self, turn: &str, text: &str) {
        let delegation = format!("<realtime_delegation>\n<input>{text}</input>\n<transcript_delta>user: {text}</transcript_delta>\n</realtime_delegation>");
        self.push(json!({"method": "item/started", "params": {"threadId": "t1", "turnId": turn,
            "item": {"type": "userMessage", "id": format!("item-{turn}"), "content": [{"type": "text", "text": delegation}]}}}));
    }

    /// Chamada de ferramenta do organizador; a resposta volta com o mesmo `id`.
    pub fn tool(&self, id: u64, name: &str, args: Value, turn: &str) {
        self.push(json!({"id": id, "method": "item/tool/call", "params": {"tool": name, "arguments": args, "turnId": turn, "threadId": "t1"}}));
    }

    /// O `result` que a chamada mandou ao pedido `id`.
    pub async fn reply_to(&self, id: u64) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let found = self.seen.lock().unwrap().iter().find(|m| m["id"] == id && m.get("result").is_some()).map(|m| m["result"].clone());
            if let Some(result) = found { return result; }
            assert!(tokio::time::Instant::now() < deadline, "a chamada não respondeu a ferramenta {id}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
