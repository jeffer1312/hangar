//! Rotas de escrita de sessão Claude (`/input`, `/steer`, ...): nesta etapa o Rust só decide e repassa;
//! o que se prova aqui é que o repasse chega intacto e que a porta de entrada é respeitada.
use crate::fake;

use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeTarget};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Cano Claude falso mínimo: aceita o token, manda o retrato e confirma tudo o que receber.
async fn cano() -> String { cano_with(None).await }

/// `kill`: quando avisado, o cano falso cai e a entrada fica doente (`cano_exited`).
async fn cano_with(kill: Option<Arc<tokio::sync::Notify>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let kill = kill.clone();
            tokio::spawn(async move {
                let (read, mut write) = tokio::io::split(stream);
                let mut reader = BufReader::new(read);
                let mut header = String::new();
                if reader.read_line(&mut header).await.is_err() || header != "secret-test\n" { return; }
                let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
                    "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
                write.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
                loop {
                    let mut raw = String::new();
                    let line = reader.read_line(&mut raw);
                    let read = match &kill {
                        Some(kill) => tokio::select! { n = line => n, () = kill.notified() => return },
                        None => line.await,
                    };
                    if read.unwrap_or(0) == 0 { return; }
                    let envelope: Value = serde_json::from_str(&raw).unwrap();
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                }
            });
        }
    });
    format!("tcp:{address}")
}

/// Python falso da política: responde `ok` a tudo.
async fn policy() -> std::net::SocketAddr {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
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
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).await.unwrap();
                    let reply = json!({"ok":true,"data":{}}).to_string();
                    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}", reply.len());
                    reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                }
            });
        }
    });
    address
}

async fn open_entry(registry: &RuntimeRegistry, dir: &std::path::Path, key: &str, name: &str) {
    open_entry_with(registry, dir, key, name, 1, cano().await).await
}

async fn open_entry_with(registry: &RuntimeRegistry, dir: &std::path::Path, key: &str, name: &str, generation: u64, escuta: String) {
    registry.open(RuntimeTarget { key: key.into(), generation, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{key}.lock")), state_path: dir.join(format!("{key}.queue-state.json")),
        projection_dir: dir.join(format!("{key}-projection")), transcript: dir.join(format!("{key}.jsonl")), created: 0.0 }).await.unwrap();
}

async fn registry() -> Arc<RuntimeRegistry> {
    Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()))
}

/// Servidor com o runtime ligado (como `serve_until_with_state` faz) e o Python falso.
async fn serve_with(registry: Option<Arc<RuntimeRegistry>>) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream, "127.0.0.1"));
    if let Some(registry) = registry { assert!(state.state.runtime.set(registry).is_ok()); }
    (python, spawn_state(state).await)
}

async fn post(server: std::net::SocketAddr, route: &str, body: &str, token: &str) -> (u16, String) {
    let response = client().post(format!("http://{server}/api/sessions/s/{route}"))
        .header("content-type", "application/json").header("authorization", format!("Bearer {token}"))
        .body(body.to_owned()).send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

const ROUTES: &[&str] = &["input", "steer", "interrupt", "select", "select/submit", "answer", "keys", "term-input"];

#[tokio::test]
async fn without_runtime_every_write_route_reaches_python_with_the_same_body() {
    let (python, server) = serve_with(None).await;
    for route in ROUTES {
        let body = r#"{"text":"oi","x":1}"#;
        assert_eq!(post(server, route, body, OWNER).await, (200, "from-python".into()), "{route}");
        assert_eq!(python.last_hit().0, format!("/api/sessions/s/{route}"));
        assert_eq!(python.last_body(), body.as_bytes(), "{route}");
    }
    let response = client().delete(format!("http://{server}/api/sessions/s/queue/e1"))
        .header("authorization", format!("Bearer {OWNER}")).send().await.unwrap();
    assert_eq!(response.text().await.unwrap(), "from-python");
    assert_eq!(python.last_hit().0, "/api/sessions/s/queue/e1");
}

#[tokio::test]
async fn open_entry_with_an_extra_body_field_still_reaches_python_intact() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    let (python, server) = serve_with(Some(registry)).await;
    let body = r#"{"text":"oi","x":1}"#;
    assert_eq!(post(server, "input", body, OWNER).await, (200, "from-python".into()));
    assert_eq!(python.last_body(), body.as_bytes());
    // Corpo que não é JSON de objeto também segue intacto: a recusa é do FastAPI.
    assert_eq!(post(server, "input", "não é json", OWNER).await.0, 200);
    assert_eq!(python.last_body(), "não é json".as_bytes());
}

#[tokio::test]
async fn guest_goes_straight_to_python_even_with_the_gate_closed() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (_python, server) = serve_with(Some(registry)).await;
    let answer = tokio::time::timeout(Duration::from_secs(2), post(server, "input", r#"{"text":"oi"}"#, "token-errado")).await;
    assert_eq!(answer.unwrap(), (200, "from-python".into()));
}

#[tokio::test]
async fn gate_reopened_in_time_lets_the_request_through() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve_with(Some(registry.clone())).await;
    let reopen = registry.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(50)).await; reopen.ingress().open("s"); });
    // `/answer` ainda é o repasse provisório; as outras escritas já são do Rust.
    assert_eq!(post(server, "answer", "{}", OWNER).await, (200, "from-python".into()));
    assert_eq!(python.hits_to("/api/sessions/s/answer"), 1);
}

#[tokio::test]
async fn entry_is_looked_up_after_the_gate_reopens() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "velha", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let swap = registry.clone();
    let path = dir.path().to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        swap.close("velha", 1).await.unwrap();
        open_entry(&swap, &path, "nova", "s").await;
        swap.ingress().open("s");
    });
    let (_pass, target) = hangar_server::session_write::enter_then_find(&registry, "s", Duration::from_secs(5)).await.unwrap().unwrap();
    assert_eq!(target.key, "nova", "a entrada de antes de esperar a porta é a que o relançamento parou");
}

#[tokio::test]
async fn enter_then_find_without_an_entry_still_hands_back_nothing() {
    let registry = registry().await;
    assert!(hangar_server::session_write::enter_then_find(&registry, "s", Duration::from_millis(50)).await.unwrap().is_none());
}

#[tokio::test]
async fn gate_closed_past_the_wait_answers_409_busy() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.write_gate_wait = Duration::from_millis(100);
    assert!(state.state.runtime.set(registry).is_ok());
    let server = spawn_state(state).await;
    let (status, text) = post(server, "input", r#"{"text":"oi"}"#, OWNER).await;
    assert_eq!(status, 409);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["detail"]["code"], "session_transfer_busy");
    assert_eq!(body["detail"]["msg"], "A sessão está trocando de agente; tente novamente quando terminar.");
    assert_eq!(body["detail"]["params"], json!({}));
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0);
}

#[tokio::test]
async fn gate_held_by_a_transfer_answers_409_without_waiting() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().hold("s", Duration::from_secs(1)).await.unwrap();
    // Espera padrão da porta (30 s): a retenção não pode gastá-la.
    let (python, server) = serve_with(Some(registry)).await;
    let start = std::time::Instant::now();
    let (status, text) = tokio::time::timeout(Duration::from_secs(5), post(server, "input", r#"{"text":"oi"}"#, OWNER)).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
    assert_eq!(status, 409);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["detail"]["code"], "session_transfer_busy");
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0);
}

#[tokio::test]
async fn oversized_body_answers_python_413_text() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    let (_python, server) = serve_with(Some(registry)).await;
    let big = vec![b' '; 100 * 1024 * 1024 + 1];
    let response = client().post(format!("http://{server}/api/sessions/s/input"))
        .header("authorization", format!("Bearer {OWNER}")).header("origin", "http://app").body(big).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 413);
    assert_eq!(response.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(response.text().await.unwrap(), "request body too large");
}

#[tokio::test]
async fn writable_picks_the_highest_generation_of_a_repeated_name() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry_with(&registry, dir.path(), "a", "s", 1, cano().await).await;
    open_entry_with(&registry, dir.path(), "b", "s", 2, cano().await).await;
    let target = registry.writable("s").await.unwrap();
    assert_eq!((target.key.as_str(), target.generation), ("b", 2));
}

#[tokio::test]
async fn forwarding_never_holds_the_ingress_pass() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    // Entrada doente: o cano cai depois de aberta e o retrato passa a dizer `cano_exited`.
    let kill = Arc::new(tokio::sync::Notify::new());
    open_entry_with(&registry, dir.path(), "d", "doente", 1, cano_with(Some(kill.clone())).await).await;
    // `notify_one` guarda o aviso se o cano falso ainda não chegou ao `select`.
    kill.notify_one();
    for _ in 0..100 {
        if !registry.writable("doente").await.unwrap().healthy { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!registry.writable("doente").await.unwrap().healthy, "a entrada ficou doente");
    let (python, server) = serve_with(Some(registry.clone())).await;
    // Sem entrada, doente e saudável com corpo que o Rust não atende (o FastAPI recusa): em todos o
    // `close` do Python não espera.
    for name in ["sem-entrada", "doente", "s"] {
        python.hold_input(true);
        let url = format!("http://{server}/api/sessions/{name}/input");
        let sending = tokio::spawn(client().post(url).header("authorization", format!("Bearer {OWNER}")).body(r#"{"text":"oi","x":1}"#).send());
        let path = format!("/api/sessions/{name}/input");
        for _ in 0..100 { if python.hits_to(&path) == 1 { break; } tokio::time::sleep(Duration::from_millis(20)).await; }
        assert_eq!(python.hits_to(&path), 1, "o pedido chegou ao Python");
        assert!(registry.ingress().close(name, Duration::from_millis(300)).await.is_ok(), "{name}: passe na mão durante o repasse");
        registry.ingress().open(name);
        python.hold_input(false);
        python.release.notify_one();
        assert_eq!(sending.await.unwrap().unwrap().status().as_u16(), 200);
    }
}
