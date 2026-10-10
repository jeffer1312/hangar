//! Rotas só do Codex sem terminal servidas pelo Rust: cada caso de `golden/codex_routes.json` (gravado
//! pela rota Python com respostas fixas do app-server) roda aqui pelo roteador de verdade, com um cano
//! falso que devolve as MESMAS respostas, e o corpo e o código têm de ser iguais.
use crate::fake;

use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeError, RuntimeTarget};
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::terminal_input::TerminalBinding;
use hangar_server::session_write::codex::{permission_answer, skip_answer};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn golden() -> Vec<Value> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../backend/tests/fixtures/contract/golden/codex_routes.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Cano Codex falso: retrato (com um turno em voo quando `busy`) e, a cada pedido, a resposta do caso
/// para o método dele. Método fora do caso responde erro com o nome, para o teste falhar dizendo qual.
async fn cano(rpc: Value, busy: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else { return };
        let mut reader = BufReader::new(stream);
        let mut header = String::new();
        if reader.read_line(&mut header).await.is_err() || header != "secret-test\n" { return; }
        let inflight = if busy { json!({"codex": {"thread-1": {"turnId": "turn-1", "text": "", "complete": true}}}) } else { json!({}) };
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":inflight});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
            let envelope: Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            let frame: Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
            if frame["id"].is_null() { continue; }
            let method = frame["method"].as_str().unwrap_or("");
            let mut reply = json!({"id": frame["id"]});
            match &rpc[method] {
                Value::Null => reply["error"] = json!({"code": -32601, "message": format!("método fora do caso: {method}")}),
                answer if answer.get("error").is_some() => reply["error"] = answer["error"].clone(),
                answer => reply["result"] = answer["result"].clone(),
            }
            let out = json!({"type":"cano_output","frame":reply.to_string()});
            reader.get_mut().write_all(format!("{out}\n").as_bytes()).await.unwrap();
        }
    });
    format!("tcp:{address}")
}

/// Python falso da política: `ok` a tudo (o `session.patch_meta` do pular e do modo de permissão).
async fn policy() -> std::net::SocketAddr {
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

async fn open(registry: &RuntimeRegistry, dir: &std::path::Path, case: &Value) {
    let s = &case["session"];
    let questions: Vec<Value> = s["async"].as_array().unwrap().iter().map(|id| json!([id, {"request_id": id, "questions": [{"question": "?"}]}])).collect();
    let metadata = json!({"name": "s", "headless": true, "thread_id": "thread-1", "initialized": true, "ready": true, "cwd": "/tmp/projeto",
        "model": s["model"], "effort": s["effort"], "service_tier": s["service_tier"], "mode": s["mode"],
        "permission_mode": s["permission_mode"], "async_questions": questions});
    registry.open(RuntimeTarget { key: "k".into(), generation: 1, name: "s".into(), provider: "codex".into(), metadata,
        binding: CanoBinding { pid: 42, escuta: cano(case["rpc"].clone(), s["busy"] == true).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join("k.lock"), state_path: dir.join("k.queue-state.json"), projection_dir: dir.join("k-projection"),
        transcript: dir.join("k.jsonl"), created: 0.0 }).await.unwrap();
}

async fn serve(registry: Arc<RuntimeRegistry>) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream, "127.0.0.1"));
    assert!(state.state.runtime.set(registry).is_ok());
    (python, spawn_state(state).await)
}

async fn call(server: std::net::SocketAddr, case: &Value, token: &str) -> (u16, Value) {
    let url = format!("http://{server}/api/sessions/s/{}", case["route"].as_str().unwrap());
    let request = if case["method"] == "GET" { client().get(url) } else {
        client().post(url).header("content-type", "application/json").body(case["body"].to_string())
    };
    let response = tokio::time::timeout(Duration::from_secs(20), request.header("authorization", format!("Bearer {token}")).send())
        .await.unwrap_or_else(|_| panic!("{}: sem resposta", case["name"])).unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test]
async fn headless_codex_routes_match_the_python_golden() {
    for case in golden().iter().filter(|c| c["relay"].is_null() && c["fault"].is_null()) {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()));
        open(&registry, dir.path(), case).await;
        let (python, server) = serve(registry.clone()).await;
        let (status, body) = call(server, case, OWNER).await;
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{}", case["route"].as_str().unwrap())), 0, "{}: foi ao Python", case["name"]);
        assert_eq!((status, &body), (case["expect"]["status"].as_u64().unwrap() as u16, &case["expect"]["body"]), "{}", case["name"]);
        registry.close("k", 1).await.unwrap();
    }
}

#[tokio::test]
async fn sessions_the_rust_does_not_own_reach_python_intact() {
    let registry = Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()));
    let (python, server) = serve(registry).await;
    for case in golden().iter().filter(|c| !c["relay"].is_null()) {
        assert_eq!(call(server, case, OWNER).await, (200, Value::String("from-python".into())), "{}", case["name"]);
        assert_eq!(python.last_hit().0, format!("/api/sessions/s/{}", case["route"].as_str().unwrap()));
        if case["method"] == "POST" { assert_eq!(python.last_body(), case["body"].to_string().as_bytes()); }
    }
}

#[tokio::test]
async fn guest_and_bodies_fastapi_refuses_reach_python() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()));
    let case = golden().into_iter().find(|c| c["name"] == "model_ok").unwrap();
    open(&registry, dir.path(), &case).await;
    let (python, server) = serve(registry).await;
    assert_eq!(call(server, &case, "token-errado").await.1, Value::String("from-python".into()), "convidado segue ao Python");
    for body in [json!({"model": "gpt-5", "x": 1}), json!({"model": 5}), json!({"effort": "low"})] {
        let refused = json!({"name": "corpo", "method": "POST", "route": "model", "body": body});
        assert_eq!(call(server, &refused, OWNER).await.1, Value::String("from-python".into()), "{body}");
        assert_eq!(python.last_body(), body.to_string().as_bytes());
    }
    let tier = json!({"name": "tier", "method": "POST", "route": "service-tier", "body": {"service_tier": "turbo"}});
    assert_eq!(call(server, &tier, OWNER).await.1, Value::String("from-python".into()));
}

/// Falhas do ator que o cano falso não produz: a função de resposta com o erro do caso dá o mesmo corpo.
#[test]
fn actor_failures_match_the_python_golden() {
    for case in golden().iter().filter(|c| !c["fault"].is_null()) {
        let error = &case["fault"]["rust"];
        let sent = Err(RuntimeError::new(error["code"].as_str().unwrap(), error["message"].as_str().unwrap()));
        let (status, body) = match case["route"].as_str().unwrap() {
            "question/skip" => skip_answer(&sent),
            "codex-permissions" => permission_answer(&sent),
            route => panic!("rota sem função de falha: {route}"),
        };
        assert_eq!((status.as_u16(), &body), (case["expect"]["status"].as_u64().unwrap() as u16, &case["expect"]["body"]), "{}", case["name"]);
    }
}

#[tokio::test]
async fn limits_never_errors_nor_waits_with_the_gate_closed() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()));
    let case = golden().into_iter().find(|c| c["name"] == "limits_ok").unwrap();
    open(&registry, dir.path(), &case).await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve(registry).await;
    let started = std::time::Instant::now();
    assert_eq!(call(server, &case, OWNER).await, (200, json!({"primary": null, "secondary": null, "planType": null})));
    assert!(started.elapsed() < Duration::from_secs(1), "esperou a porta: {:?}", started.elapsed());
    assert_eq!(python.hits_to("/api/sessions/s/limits"), 0);
}

#[tokio::test]
async fn terminal_entry_goes_to_python() {
    // O registro do Rust só abre entrada de terminal como `claude` (Codex com terminal é do Python até a
    // 5C): é ela que exercita o desvio antes da porta.
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()));
    let binding = TerminalBinding { name: "s".into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec!["/does-not-exist/hangar-test-tmux".into()], windows: false, clipboard_lock_path: None };
    let transcript = dir.path().join("s.jsonl");
    std::fs::write(&transcript, "").unwrap();
    registry.open_terminal(TerminalTarget { key: "k-s".into(), generation: 1, name: "s".into(), binding,
        lease_path: dir.path().join("s.lease"), state_path: dir.path().join("s.state"), projection_dir: dir.path().join("s.projection"),
        transcript, created: 0.0, plugin_key: None }).await.unwrap();
    assert!(registry.writable("s").await.unwrap().terminal);
    // A porta fechada prova que o desvio vem antes dela: o Python responde sem esperar.
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve(registry).await;
    for case in golden().iter().filter(|c| c["relay"] == "terminal") {
        assert_eq!(call(server, case, OWNER).await, (200, Value::String("from-python".into())), "{}", case["name"]);
        assert_eq!(python.last_hit().0, format!("/api/sessions/s/{}", case["route"].as_str().unwrap()));
    }
}
