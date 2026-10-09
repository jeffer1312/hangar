//! Aviso de grupo pelo mesmo caminho do `/input` (`deliver_text`), texto do protocolo e fatos da
//! orquestração pedidos ao Python falso.
mod fake;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fake::*;
use hangar_server::groups::deliver::{ProtocolArgs, protocol_text};
use hangar_server::groups::orq::{PythonOrq, is_orchestrator};
use hangar_server::groups::service::{OrqFacts, OrqPhase, PromoteError};
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeTarget};
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::session_write::input::deliver_text;
use hangar_server::terminal_input::TerminalBinding;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Cano Claude falso que confirma toda entrada como escrita.
async fn cano() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
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
                    if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
                    let envelope: Value = serde_json::from_str(&raw).unwrap();
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                }
            });
        }
    });
    format!("tcp:{address}")
}

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

async fn registry() -> Arc<RuntimeRegistry> {
    Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()))
}

async fn open_headless(registry: &RuntimeRegistry, dir: &Path, name: &str) {
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano().await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
}

/// Cano Codex falso: confirma cada escrita, responde cada pedido (o `turn/start` abre o turno) e anota
/// o método de cada um.
async fn codex_cano(methods: Arc<std::sync::Mutex<Vec<String>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else { return };
        let mut reader = BufReader::new(stream);
        let mut header = String::new();
        if reader.read_line(&mut header).await.is_err() || header != "secret-test\n" { return; }
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
            let envelope: Value = serde_json::from_str(&raw).unwrap();
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            let frame: Value = serde_json::from_str(envelope["frame"].as_str().unwrap_or("{}")).unwrap();
            methods.lock().unwrap().push(frame["method"].as_str().unwrap_or("").to_owned());
            if frame["id"].is_null() { continue; }
            let result = if frame["method"] == "turn/start" { json!({"turn":{"id":"turn-1","status":"inProgress"}}) } else { json!({"data":[]}) };
            let out = json!({"type":"cano_output","frame":json!({"id":frame["id"],"result":result}).to_string()});
            reader.get_mut().write_all(format!("{out}\n").as_bytes()).await.unwrap();
        }
    });
    format!("tcp:{address}")
}

async fn open_codex_headless(registry: &RuntimeRegistry, dir: &Path, name: &str, methods: Arc<std::sync::Mutex<Vec<String>>>) {
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "codex".into(),
        metadata: json!({"name": name, "headless": true, "thread_id": "thread-1", "initialized": true, "ready": true}),
        binding: CanoBinding { pid: 42, escuta: codex_cano(methods).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
}

/// Entrada com terminal sobre um tmux falso que aceita tudo e anota cada chamada.
#[cfg(unix)]
async fn open_terminal(registry: &RuntimeRegistry, dir: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let (script, log) = (dir.join("tmux"), dir.join("tmux.log"));
    std::fs::write(&script, format!(
        "#!/bin/sh\nif [ \"$1\" = display-message ]; then printf '{name}\\t%%1\\t1\\n'; exit 0; fi\necho \"$@\" >> '{}'\n", log.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (state_path, projection_dir) = (dir.join(format!("{name}.state")), dir.join(format!("{name}.projection")));
    let binding = TerminalBinding { name: name.into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec![script.to_str().unwrap().to_owned()], windows: false, clipboard_lock_path: None };
    let transcript = dir.join(format!("{name}.jsonl"));
    std::fs::write(&transcript, "").unwrap();
    registry.open_terminal(TerminalTarget { key: format!("k-{name}"), generation: 1, name: name.into(), binding,
        lease_path: dir.join(format!("{name}.lease")), state_path, projection_dir, transcript, created: 0.0, plugin_key: None }).await.unwrap();
    let target = registry.writable(name).await.unwrap();
    assert!(target.terminal && target.healthy, "entrada de terminal saudável");
}

async fn state(registry: Option<Arc<RuntimeRegistry>>, wait: Duration) -> (Arc<Fake>, AppState) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.write_gate_wait = wait;
    if let Some(registry) = registry { assert!(state.state.runtime.set(registry).is_ok()); }
    (python, state)
}

#[tokio::test]
async fn notice_to_a_headless_claude_session_goes_through_rust() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, st) = state(Some(registry), Duration::from_secs(5)).await;
    assert_eq!(deliver_text(&st, "s", "aviso").await, Ok(()));
    assert_eq!(python.input_calls(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn notice_to_a_claude_terminal_session_goes_through_rust() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_terminal(&registry, dir.path(), "t").await;
    let (python, st) = state(Some(registry), Duration::from_secs(5)).await;
    assert_eq!(deliver_text(&st, "t", "aviso").await, Ok(()));
    assert_eq!(python.input_calls(), 0);
    // Sem pane de verdade o ator deixa o recado na fila dele: a prova de que foi o Rust que recebeu.
    let queued = std::fs::read_to_string(dir.path().join("t.projection").join("t.jsonl")).unwrap();
    assert!(queued.contains("\"aviso\""), "{queued}");
}

#[tokio::test]
async fn notice_to_a_headless_codex_session_goes_through_rust() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let methods = Arc::new(std::sync::Mutex::new(Vec::new()));
    open_codex_headless(&registry, dir.path(), "c", methods.clone()).await;
    let (python, st) = state(Some(registry), Duration::from_secs(5)).await;
    assert_eq!(deliver_text(&st, "c", "aviso").await, Ok(()));
    assert_eq!(python.input_calls(), 0);
    assert!(methods.lock().unwrap().iter().any(|m| m == "turn/start"), "{:?}", methods.lock().unwrap());
}

/// O `/compact` do Codex é controle do Python, como no handler: vai ao `/input` dele sem o passe na mão.
#[tokio::test]
async fn codex_compact_notice_goes_to_python_without_the_pass() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let methods = Arc::new(std::sync::Mutex::new(Vec::new()));
    open_codex_headless(&registry, dir.path(), "c", methods.clone()).await;
    let (python, st) = state(Some(registry.clone()), Duration::from_secs(5)).await;
    python.set_input_reply(Some((StatusCode::OK, json!({"ok": true}))));
    python.hold_input(true);
    let st = Arc::new(st);
    let sending = tokio::spawn({ let st = st.clone(); async move { deliver_text(&st, "c", "/compact").await } });
    for _ in 0..100 { if python.hits_to("/api/sessions/c/input") == 1 { break; } tokio::time::sleep(Duration::from_millis(20)).await; }
    assert_eq!(python.hits_to("/api/sessions/c/input"), 1, "o aviso chegou ao Python");
    assert!(registry.ingress().close("c", Duration::from_millis(300)).await.is_ok(), "passe na mão durante o repasse");
    registry.ingress().open("c");
    python.hold_input(false);
    python.release.notify_one();
    assert_eq!(sending.await.unwrap(), Ok(()));
    assert!(!methods.lock().unwrap().iter().any(|m| m == "turn/start"), "{:?}", methods.lock().unwrap());
}

#[tokio::test]
async fn notice_to_a_session_rust_does_not_own_goes_to_python_input_as_owner() {
    let (python, st) = state(Some(registry().await), Duration::from_secs(5)).await;
    python.set_input_reply(Some((StatusCode::OK, json!({"ok": true, "delivered": true, "steered": false, "native": false}))));
    assert_eq!(deliver_text(&st, "codex-1", "aviso").await, Ok(()));
    assert_eq!(python.hits_to("/api/sessions/codex-1/input"), 1);
    assert_eq!(serde_json::from_slice::<Value>(&python.last_body()).unwrap(), json!({"text": "aviso", "steer": false}));
    let (_, headers) = python.last_hit();
    assert_eq!(headers["authorization"], format!("Bearer {OWNER}").as_str());
    assert!(headers.get("x-hangar-internal").is_none());
}

#[tokio::test]
async fn notice_without_runtime_goes_to_python() {
    let (python, st) = state(None, Duration::from_secs(5)).await;
    python.set_input_reply(Some((StatusCode::OK, json!({"ok": true}))));
    assert_eq!(deliver_text(&st, "s", "aviso").await, Ok(()));
    assert_eq!(python.input_calls(), 1);
}

#[tokio::test]
async fn python_refusal_comes_back_as_its_envelope() {
    let (python, st) = state(Some(registry().await), Duration::from_secs(5)).await;
    let detail = json!({"code": "erro_sessao_recado_nao_enfileirado", "params": {}, "msg": "sessão não encontrada"});
    python.set_input_reply(Some((StatusCode::NOT_FOUND, json!({"detail": detail.clone()}))));
    assert_eq!(deliver_text(&st, "x", "aviso").await, Err(detail));
    // Corpo que não é o envelope: o mesmo genérico do `_deliver` do Python.
    python.set_input_reply(Some((StatusCode::BAD_GATEWAY, json!("o backend não respondeu"))));
    let err = deliver_text(&st, "x", "aviso").await.unwrap_err();
    assert_eq!(err["code"], "erro_envio_falhou_desconhecida");
}

#[tokio::test]
async fn notice_waits_the_ingress_gate_and_answers_busy() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, st) = state(Some(registry), Duration::from_millis(100)).await;
    let err = deliver_text(&st, "s", "aviso").await.unwrap_err();
    assert_eq!(err["code"], "session_transfer_busy");
    assert_eq!(python.input_calls(), 0);
}

#[tokio::test]
async fn protocol_text_asks_python_with_every_field() {
    let (python, st) = state(None, Duration::from_secs(5)).await;
    python.set_internal("pair/text", StatusCode::OK, json!({"text": "protocolo do grupo"}));
    let args = ProtocolArgs { me: "a".into(), others: vec!["b".into()], task: "t".into(), contract: Some("/c.md".into()),
        harness: [("a".to_owned(), "claude".to_owned())].into(), ..Default::default() };
    assert_eq!(protocol_text(&st, "group", &args).await, Ok("protocolo do grupo".to_owned()));
    assert_eq!(python.internal_bodies("pair/text"), vec![json!({"kind": "group", "me": "a", "others": ["b"], "task": "t",
        "contract": "/c.md", "contract_remote": false, "harness": {"a": "claude"}, "peer": "", "owner": ""})]);
    python.set_internal("pair/text", StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    assert_eq!(protocol_text(&st, "orq", &args).await.unwrap_err()["code"], "erro_envio_falhou");
}

#[tokio::test]
async fn orq_phase_maps_every_answer_and_failure_is_unknown() {
    let (python, st) = state(None, Duration::from_secs(5)).await;
    let orq = PythonOrq::from_state(&st);
    for (phase, want) in [(json!("live"), OrqPhase::Live), (json!("ended"), OrqPhase::Ended), (Value::Null, OrqPhase::NotStarted),
                          (json!("unknown"), OrqPhase::Unknown), (json!("other"), OrqPhase::Unknown)] {
        python.set_internal("orq/group-phase", StatusCode::OK, json!({"phase": phase}));
        assert_eq!(orq.phase("g1").await, want, "{phase}");
    }
    assert_eq!(python.internal_bodies("orq/group-phase")[0], json!({"gid": "g1"}));
    python.set_internal("orq/group-phase", StatusCode::OK, json!({}));
    assert_eq!(orq.phase("g1").await, OrqPhase::Unknown, "sem a chave não é 'sem execução'");
    python.set_internal("orq/group-phase", StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    assert_eq!(orq.phase("g1").await, OrqPhase::Unknown);
}

#[tokio::test]
async fn orq_promote_ok_and_conflict_text() {
    let (python, st) = state(None, Duration::from_secs(5)).await;
    let orq = PythonOrq::from_state(&st);
    python.set_internal("orq/promote", StatusCode::OK, json!({}));
    assert_eq!(orq.promote("a", "g1").await, Ok(()));
    assert_eq!(python.internal_bodies("orq/promote"), vec![json!({"name": "a", "gid": "g1"})]);
    python.set_internal("orq/promote", StatusCode::CONFLICT,
        json!({"detail": {"code": "erro_orq_arquivo_mudou", "params": {}, "msg": "o time já pertence a outro grupo"}}));
    assert_eq!(orq.promote("a", "g1").await, Err(PromoteError::Conflict("o time já pertence a outro grupo".to_owned())));
    // Só o 409 é conflito; o resto é o Python indisponível, com o código.
    for (status, code) in [(StatusCode::INTERNAL_SERVER_ERROR, "groups_orq_promote_status_500"), (StatusCode::NOT_FOUND, "groups_orq_promote_status_404")] {
        python.set_internal("orq/promote", status, json!({}));
        assert_eq!(orq.promote("a", "g1").await, Err(PromoteError::Unavailable(code.to_owned())));
    }
}

#[tokio::test]
async fn orchestrator_names_come_from_python() {
    let (python, st) = state(None, Duration::from_secs(5)).await;
    python.set_internal("orq/is-orchestrator", StatusCode::OK, json!({"names": ["g1-orq"]}));
    let names = vec!["a".to_owned(), "g1-orq".to_owned()];
    assert_eq!(is_orchestrator(&st, &names).await, Ok(vec!["g1-orq".to_owned()]));
    assert_eq!(python.internal_bodies("orq/is-orchestrator"), vec![json!({"names": ["a", "g1-orq"]})]);
    python.set_internal("orq/is-orchestrator", StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    assert!(is_orchestrator(&st, &names).await.is_err());
}
