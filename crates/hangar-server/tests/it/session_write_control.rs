//! `/interrupt`, `/keys`, `/term-input`, `/select`, `/select/submit` e o descarte da fila servidos pelo
//! Rust: o corpo e o código de cada resposta vêm do golden que o Python gera (`gen_golden.py`), e o
//! resto prova a rota inteira com um cano falso e o plugin falso do Python.
use crate::fake;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fake::*;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, Disposition, RuntimeError, RuntimeReply, RuntimeTarget};
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::session_write::control::*;
use hangar_server::terminal_input::TerminalBinding;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const OP: &str = "OP";

fn golden() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/session_write/control.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A resposta do ator no golden: `{disposition,payload}`, `!unknown` (resultado incerto), ou
/// `!erro: codigo: mensagem` (falha do runtime antes da entrega).
fn reply(v: &Value) -> Result<RuntimeReply, RuntimeError> {
    let text = v.as_str();
    if text == Some("!no_pending") { return Err(RuntimeError::new("no_pending_permission", "nenhuma permissão pendente")); }
    if text == Some("!unknown") {
        return Ok(RuntimeReply { operation_id: OP.into(), disposition: Disposition::Unknown, payload: json!({}) });
    }
    if let Some(text) = text {
        let (code, message) = text.trim_start_matches("!erro: ").split_once(": ").unwrap();
        return Err(RuntimeError::new(code, message));
    }
    Ok(RuntimeReply { operation_id: OP.into(), disposition: serde_json::from_value(v["disposition"].clone()).unwrap(), payload: v["payload"].clone() })
}

/// O que o Rust recebe do ator: o `rust_reply` do caso, quando o Python decidiu antes de perguntar.
fn rust_reply(case: &Value) -> Result<RuntimeReply, RuntimeError> {
    reply(if case["rust_reply"].is_null() { &case["reply"] } else { &case["rust_reply"] })
}

/// O `detalhe` é o texto da falha de cada lado (Python e Rust escrevem diferente).
fn plain(mut body: Value) -> Value {
    if let Some(params) = body.pointer_mut("/detail/params").and_then(Value::as_object_mut) { params.remove("detalhe"); }
    body
}

fn check(name: &str, got: (StatusCode, Value), case: &Value) {
    let want = (case["expect"]["status"].as_u64().unwrap() as u16, plain(case["expect"]["body"].clone()));
    assert_eq!((got.0.as_u16(), plain(got.1)), want, "{name}");
}

fn events(case: &Value) -> Vec<&str> { case["diary"].as_array().unwrap().iter().map(|e| e.as_str().unwrap()).collect() }

fn check_diary(name: &str, diary: &Option<(&'static str, String)>, case: &Value) {
    assert_eq!(diary.iter().map(|d| d.0).collect::<Vec<_>>(), events(case), "{name}: diário");
}

#[test]
fn select_answers_match_the_python_golden() {
    let mut seen = 0;
    for case in golden().into_iter().filter(|c| c["route"] == "select") {
        seen += 1;
        let name = case["name"].as_str().unwrap();
        let option = case["args"]["option"].as_u64().unwrap();
        let sent = case["sent"].as_array().unwrap();
        if !case["terminal"].as_bool().unwrap() {
            check(name, select_headless_answer(&rust_reply(&case)), &case);
            continue;
        }
        let pending = (!case["pending"].is_null()).then(|| case["pending"].clone());
        match select_plan(pending.as_ref(), option, case["panel_open"].as_bool().unwrap()) {
            Err(refused) => { check(name, refused, &case); assert!(sent.is_empty(), "{name}: nada vai ao ator"); }
            Ok(payload) => {
                // Sem controle no golden o Python foi ao driver direto (recusa do ator): não há o que comparar.
                if let Some(first) = sent.first() { assert_eq!(payload, first["payload"], "{name}: o controle que o ator recebe"); }
                let (answer, diary) = select_terminal_answer(&rust_reply(&case));
                check(name, answer, &case);
                check_diary(name, &diary, &case);
            }
        }
    }
    assert!(seen >= 16);
}

#[test]
fn select_that_the_actor_refused_is_the_drive_error_of_python() {
    let case = golden().into_iter().find(|c| c["name"] == "select_refused").unwrap();
    let (answer, diary) = select_terminal_answer(&rust_reply(&case));
    check("select_refused", answer, &case);
    assert_eq!(diary.map(|d| d.0), Some("opcao.nao_convergiu"));
}

#[test]
fn submit_answers_match_the_python_golden() {
    for case in golden().into_iter().filter(|c| c["route"] == "select_submit") {
        let name = case["name"].as_str().unwrap();
        if case["panel_open"].as_bool().unwrap() { check(name, panel_open_refusal(), &case); continue; }
        let (answer, diary) = submit_answer(&rust_reply(&case));
        check(name, answer, &case);
        check_diary(name, &diary, &case);
    }
}

#[test]
fn interrupt_answers_match_the_python_golden() {
    for case in golden().into_iter().filter(|c| c["route"] == "interrupt") {
        let name = case["name"].as_str().unwrap();
        let answer = if case["terminal"].as_bool().unwrap() { interrupt_terminal_answer(&rust_reply(&case)) } else { interrupt_headless_answer(case["provider"] == "codex", &rust_reply(&case)) };
        // O aviso ao plugin só sai quando o Esc foi dado.
        if case["terminal"].as_bool().unwrap() { assert_eq!(answer.0 == StatusCode::OK, !case["notified"].as_array().unwrap().is_empty(), "{name}: aviso"); }
        check(name, answer, &case);
    }
}

#[test]
fn keys_and_term_input_answers_match_the_python_golden() {
    for case in golden().into_iter().filter(|c| c["route"] == "keys" || c["route"] == "term_input") {
        let name = case["name"].as_str().unwrap();
        let sent = case["sent"].as_array().unwrap();
        let steps = if case["route"] == "keys" {
            Ok(vec![("navigation_key", json!({"key": case["args"]["key"]}))])
        } else {
            term_input_steps(case["args"]["text"].as_str(), case["args"]["key"].as_str())
        };
        let steps = match steps { Ok(steps) => steps, Err(refused) => { check(name, refused, &case); assert!(sent.is_empty(), "{name}"); continue; } };
        let (mut got, mut done) = (Vec::new(), (StatusCode::OK, json!({"ok": true})));
        let last = steps.len().saturating_sub(1);
        for (index, (control, payload)) in steps.into_iter().enumerate() {
            got.push(json!({"control": control, "payload": payload}));
            // A recusa do ator vale só para o último controle; os de antes foram aceitos.
            let answered = if index == last { rust_reply(&case) } else { reply(&case["reply"]) };
            if let Err(refused) = control_step_answer(&answered) { done = refused; break; }
        }
        // O Python recusa a tecla antes de chegar ao ator; o Rust deixa o ator recusar.
        let prefix = if case["rust_reply"].is_null() { got.len() } else { got.len() - 1 };
        assert_eq!(&got[..prefix], &sent[..], "{name}: controles enviados");
        check(name, done, &case);
    }
}

#[test]
fn queue_answers_match_the_python_golden() {
    for case in golden().into_iter().filter(|c| c["route"] == "queue_remove") {
        let removed = match case["args"]["error"].as_str() {
            Some(error) => reply(&json!(format!("!erro: {error}"))).map(|_| json!(null)),
            None => Ok(case["args"]["removed"].clone()),
        };
        check(case["name"].as_str().unwrap(), queue_answer(&removed), &case);
    }
}

#[test]
fn a_failed_runtime_has_a_code_and_never_a_forward() {
    let error: Result<RuntimeReply, RuntimeError> = Err(RuntimeError::new("runtime_closed", "ator saiu"));
    let (status, body) = control_step_answer(&error).unwrap_err();
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::BAD_GATEWAY, Some("erro_envio_falhou")));
    assert_eq!(queue_answer(&Err(RuntimeError::new("runtime_closed", "ator saiu"))).0, StatusCode::BAD_GATEWAY);
}

// ── a rota inteira ──────────────────────────────────────────────────────────────────────────────

async fn cano(sink: Arc<std::sync::Mutex<Vec<Value>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let sink = sink.clone();
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
                    sink.lock().unwrap().push(envelope.clone());
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                    // A CLI de verdade responde ao pedido de controle; o cano falso faz o mesmo.
                    let frame: Value = serde_json::from_str(envelope["frame"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
                    if frame["type"] == "control_request" {
                        let response = json!({"type":"control_response","response":{"subtype":"success",
                            "request_id":frame["request_id"],"response":{}}});
                        let output = json!({"type":"cano_output","frame":response.to_string()});
                        let _ = write.write_all(format!("{output}\n").as_bytes()).await;
                    }
                }
            });
        }
    });
    format!("tcp:{address}")
}

/// `calls` conta quantas vezes o ator pediu o serviço de plugin ao Python (a permissão segurada vai
/// por ele); um contador por política, para os testes em paralelo não se contarem.
async fn policy(calls: Arc<std::sync::atomic::AtomicUsize>) -> std::net::SocketAddr {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let calls = calls.clone();
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
                    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let data = match request["kind"].as_str() {
                        // O pane confere com o vínculo da própria entrada: devolve o que veio.
                        Some("terminal_facts") => json!({"binding": request["payload"]["binding"], "ready": true, "idle": true,
                            "open_question": false, "plugin_live": false, "plugin_user": false, "native": null}),
                        Some("terminal_plugin_control") => {
                            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            json!({"disposition": "accepted"})
                        }
                        _ => json!({}),
                    };
                    let reply = json!({"ok":true,"data":data}).to_string();
                    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}", reply.len());
                    reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                }
            });
        }
    });
    address
}

async fn registry() -> Arc<RuntimeRegistry> { registry_counting().await.0 }

async fn registry_counting() -> (Arc<RuntimeRegistry>, Arc<std::sync::atomic::AtomicUsize>) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    (Arc::new(RuntimeRegistry::new(policy(calls.clone()).await, "secret-test".into(), "instance-test".into())), calls)
}

async fn open_headless(registry: &RuntimeRegistry, dir: &Path, name: &str) -> Arc<std::sync::Mutex<Vec<Value>>> {
    let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(sink.clone()).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
    sink
}

/// tmux falso: aceita tudo e anota cada chamada, para contar as teclas que o ator mandou.
#[cfg(unix)]
fn fake_tmux(dir: &Path, name: &str) -> (String, std::path::PathBuf) {
    
    let (script, log) = (dir.join("tmux"), dir.join("tmux.log"));
    crate::write_executable(&script, format!(
        "#!/bin/sh\nif [ \"$1\" = display-message ]; then printf '{name}\\t%%1\\t1\\n'; exit 0; fi\necho \"$@\" >> '{}'\n", log.display()));
    (script.to_str().unwrap().to_owned(), log)
}

#[cfg(unix)]
fn keys_sent(log: &Path) -> usize {
    std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.contains("send-keys")).count()
}

/// Entrada com terminal sobre o tmux falso: o que chega ao ator vira `send-keys` no registro.
#[cfg(unix)]
async fn open_terminal(registry: &RuntimeRegistry, dir: &Path, name: &str) -> std::path::PathBuf {
    let (tmux, log) = fake_tmux(dir, name);
    open_terminal_with(registry, dir, name, vec![tmux]).await;
    log
}

/// Entrada com terminal cujo tmux não existe: qualquer controle que chegue ao ator falha.
async fn open_terminal_without_tmux(registry: &RuntimeRegistry, dir: &Path, name: &str) {
    open_terminal_with(registry, dir, name, vec!["/does-not-exist/hangar-test-tmux".into()]).await
}

async fn open_terminal_with(registry: &RuntimeRegistry, dir: &Path, name: &str, mux_argv: Vec<String>) {
    let (state_path, projection_dir) = (dir.join(format!("{name}.state")), dir.join(format!("{name}.projection")));
    let binding = TerminalBinding { name: name.into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv, windows: false, clipboard_lock_path: None };
    let transcript = dir.join(format!("{name}.jsonl"));
    std::fs::write(&transcript, "").unwrap();
    registry.open_terminal(TerminalTarget { key: format!("k-{name}"), generation: 1, name: name.into(), binding,
        lease_path: dir.join(format!("{name}.lease")), state_path, projection_dir, transcript, created: 0.0, plugin_key: None }).await.unwrap();
    let target = registry.writable(name).await.unwrap();
    assert!(target.terminal && target.healthy, "entrada de terminal saudável");
}

async fn serve(registry: Arc<RuntimeRegistry>) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.write_gate_wait = Duration::from_secs(5);
    assert!(state.state.runtime.set(registry).is_ok());
    (python, spawn_state(state).await)
}

async fn send(server: std::net::SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
    let url = format!("http://{server}/api/sessions/{path}");
    let request = match method { "DELETE" => client().delete(url), _ => client().post(url) };
    let response = request.header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}"))
        .body(body.to_owned()).send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test]
async fn headless_interrupt_is_served_in_rust() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    assert_eq!(send(server, "POST", "s/interrupt", "").await, (200, json!({"ok": true})));
    assert_eq!(python.hits_to("/api/sessions/s/interrupt"), 0);
    assert!(sink.lock().unwrap().iter().any(|e| e.to_string().contains("interrupt")), "o cano recebeu a interrupção");
}

#[tokio::test]
async fn headless_select_without_a_pending_permission_is_409() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    let (status, body) = send(server, "POST", "s/select", r#"{"option":1}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_opcao_nao_convergiu"), Some("nenhum pedido de permissão pendente")), "{body}");
    assert_eq!(python.hits_to("/api/sessions/s/select"), 0);
}

#[tokio::test]
async fn select_with_a_body_python_would_refuse_reaches_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    for body in [r#"{"option":"1"}"#, r#"{"option":0}"#, r#"{"option":51}"#, r#"{"option":1,"x":1}"#, r#"{"option":1.5}"#] {
        assert_eq!(send(server, "POST", "s/select", body).await, (200, "from-python".into()), "{body}");
    }
    assert_eq!(python.hits_to("/api/sessions/s/select"), 5);
}

#[tokio::test]
async fn interrupt_with_a_clear_python_cannot_parse_reaches_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    assert_eq!(send(server, "POST", "s/interrupt?clear=talvez", "").await, (200, "from-python".into()));
    assert_ne!(send(server, "POST", "s/interrupt?clear=1", "").await.1, json!("from-python"));
    assert_eq!(python.hits_to("/api/sessions/s/interrupt"), 1, "só o valor que o FastAPI recusa vai a ele");
}

#[tokio::test]
async fn queue_discard_of_an_entry_that_is_not_there_is_404() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    let (status, body) = send(server, "DELETE", "s/queue/nao-existe", "").await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (404, Some("erro_fila_entrada_nao_encontrada"), Some("entrada não está na fila")));
    assert_eq!(python.hits_to("/api/sessions/s/queue/nao-existe"), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn permission_held_by_the_plugin_refuses_option_3_and_the_actor_receives_nothing() {
    let (dir, (registry, counter)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.set_plugin_pending(json!({"id": "perm:1", "questions": []}));
    let calls = || counter.load(std::sync::atomic::Ordering::SeqCst);
    let (keys_before, policy_before) = (keys_sent(&log), calls());
    let (status, body) = send(server, "POST", "t/select", r#"{"option":3}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_opcao_nao_convergiu"), Some("opção fora do pedido de permissão")), "{body}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!((keys_sent(&log), calls()), (keys_before, policy_before), "nenhum controle chegou ao ator");
    // Contraprova: a opção 1 da mesma permissão chega ao ator (pelo serviço de plugin do Python).
    send(server, "POST", "t/select", r#"{"option":1}"#).await;
    assert_eq!(calls(), policy_before + 1, "o contador enxerga um controle que chega");
    assert_eq!(python.hits_to("/api/sessions/t/select"), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn select_whose_plugin_lookup_fails_is_a_503_with_a_code_and_never_a_forward() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.fail_plugin(Some(StatusCode::INTERNAL_SERVER_ERROR));
    let before = keys_sent(&log);
    let (status, body) = send(server, "POST", "t/select", r#"{"option":1}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["params"]["detalhe"].as_str()),
        (503, Some("erro_opcao_nao_convergiu"), Some("plugin_unavailable")), "{body}");
    assert_eq!(python.hits_to("/api/sessions/t/select"), 0);
    assert_eq!(keys_sent(&log), before);
}

#[cfg(unix)]
#[tokio::test]
async fn a_plugin_answer_without_the_pending_key_is_a_failure_not_no_question() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.set_plugin_reply(Some(json!({"ok": true})));
    let before = keys_sent(&log);
    let (status, body) = send(server, "POST", "t/select", r#"{"option":1}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("erro_opcao_nao_convergiu")), "{body}");
    assert_eq!(keys_sent(&log), before);
}

#[cfg(unix)]
#[tokio::test]
async fn interrupt_whose_plugin_lookup_fails_never_presses_escape() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.fail_plugin(Some(StatusCode::INTERNAL_SERVER_ERROR));
    let before = keys_sent(&log);
    let (status, body) = send(server, "POST", "t/interrupt", "").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("erro_envio_falhou")), "{body}");
    assert!(python.plugin_posts().is_empty());
    assert_eq!(python.hits_to("/api/sessions/t/interrupt"), 0);
    assert_eq!(keys_sent(&log), before, "sem ler a pergunta, nenhum Esc");
}

#[cfg(unix)]
#[tokio::test]
async fn an_accepted_escape_tells_the_plugin_which_question_it_closed() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.set_plugin_pending(json!({"id": "ask:9", "questions": []}));
    let before = keys_sent(&log);
    assert_eq!(send(server, "POST", "t/interrupt", "").await, (200, json!({"ok": true})));
    assert!(keys_sent(&log) > before, "o Esc chegou ao pane");
    assert_eq!(python.plugin_posts(), vec![json!({"interrupted": "ask:9"})]);
    assert_eq!(python.hits_to("/api/sessions/t/interrupt"), 0);
}

#[tokio::test]
async fn interrupt_the_terminal_refused_does_not_tell_the_plugin() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_terminal_without_tmux(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.set_plugin_pending(json!({"id": "ask:9", "questions": []}));
    let (status, _) = send(server, "POST", "t/interrupt", "").await;
    assert_ne!(status, 200, "sem tmux o Esc não sai");
    assert_eq!(python.plugin_gets(), 1);
    assert!(python.plugin_posts().is_empty(), "pergunta não interrompida segue valendo");
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_keys_and_term_input_with_bodies_python_would_refuse_reach_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    for (route, body) in [("keys", r#"{"key":5}"#), ("keys", r#"{}"#), ("term-input", r#"{"text":5}"#), ("term-input", r#"{"x":1}"#)] {
        assert_eq!(send(server, "POST", &format!("t/{route}"), body).await, (200, "from-python".into()), "{route} {body}");
    }
    assert_eq!(python.hits_to("/api/sessions/t/keys") + python.hits_to("/api/sessions/t/term-input"), 4);
}

#[cfg(unix)]
#[tokio::test]
async fn keys_reach_the_pane_through_the_actor() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry).await;
    let before = keys_sent(&log);
    assert_eq!(send(server, "POST", "t/keys", r#"{"key":"Down"}"#).await, (200, json!({"ok": true})));
    assert_eq!(keys_sent(&log), before + 1);
    let (status, body) = send(server, "POST", "t/keys", r#"{"key":"Nope"}"#).await;
    assert_eq!((status, body), (400, json!({"detail": "tecla não permitida"})));
    assert_eq!(keys_sent(&log), before + 1, "tecla fora da lista não chega ao pane");
    assert_eq!(python.hits_to("/api/sessions/t/keys"), 0);
}

#[tokio::test]
async fn select_submit_without_a_terminal_goes_to_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    assert_eq!(send(server, "POST", "s/select/submit", "").await, (200, "from-python".into()));
    assert_eq!(python.hits_to("/api/sessions/s/select/submit"), 1);
}
