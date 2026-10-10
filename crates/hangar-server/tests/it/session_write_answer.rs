//! `/answer` servido pelo Rust: o corpo e o código de cada resposta vêm do golden que o Python gera
//! (`gen_golden.py`); o resto prova a rota inteira com um tmux falso e o plugin falso do Python.
use crate::fake;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fake::*;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeError, RuntimeReply, RuntimeTarget};
#[cfg(unix)]
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::session_write::answer::*;
use hangar_server::session_write::control::control_step_answer;
#[cfg(unix)]
use hangar_server::terminal_input::TerminalBinding;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const OP: &str = "OP";

fn golden(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/session_write").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A resposta do ator no golden: `{disposition,payload}` ou `!erro: codigo: mensagem`.
fn reply(v: &Value) -> Result<RuntimeReply, RuntimeError> {
    if let Some(text) = v.as_str() {
        let (code, message) = text.trim_start_matches("!erro: ").split_once(": ").unwrap();
        return Err(RuntimeError::new(code, message));
    }
    Ok(RuntimeReply { operation_id: OP.into(), disposition: serde_json::from_value(v["disposition"].clone()).unwrap(), payload: v["payload"].clone() })
}

/// O `detalhe` e o `erro` são o texto da falha de cada lado.
fn plain(mut body: Value) -> Value {
    if let Some(params) = body.pointer_mut("/detail/params").and_then(Value::as_object_mut) { params.remove("detalhe"); }
    body
}

fn check(name: &str, got: (StatusCode, Value), case: &Value) {
    let want = (case["expect"]["status"].as_u64().unwrap() as u16, plain(case["expect"]["body"].clone()));
    assert_eq!((got.0.as_u16(), plain(got.1)), want, "{name}");
}

fn body_of(case: &Value) -> Vec<u8> {
    json!({"answers": case["answers"], "request_id": case["request_id"]}).to_string().into_bytes()
}

#[test]
fn chat_text_matches_the_python_golden() {
    let rows = golden("askq_chat_text.json");
    assert!(rows.len() >= 12);
    for row in rows {
        let questions: Vec<String> = row["questions"].as_array().map(|q| q.iter().map(|q| q.as_str().unwrap().to_owned()).collect()).unwrap_or_default();
        let answers = row["answers"].as_array().unwrap();
        assert_eq!(chat_text(answers, &questions), row["expect"].as_str().unwrap(), "{}", row["name"]);
    }
}

#[test]
fn terminal_answers_match_the_python_golden() {
    let mut seen = 0;
    for case in golden("answer.json").into_iter().filter(|c| c["terminal"] == true) {
        seen += 1;
        let name = case["name"].as_str().unwrap();
        let body = parse_body(&body_of(&case).into()).unwrap_or_else(|| panic!("{name}: o corpo é válido"));
        let pending = (!case["pending"].is_null()).then(|| case["pending"].clone());
        let sent = case["sent"].as_array().unwrap();
        let plan = match terminal_plan(&body, pending.as_ref(), case["panel_open"].as_bool().unwrap()) {
            Err(refused) => { check(name, refused, &case); assert!(sent.is_empty(), "{name}: nada vai ao ator"); continue; }
            Ok(plan) => plan,
        };
        let done = match plan {
            Plan::Control(payload) => {
                assert_eq!(json!({"control": "answer_questions", "payload": payload}), sent[0], "{name}: o controle que o ator recebe");
                control_answer(&reply(&case["reply"]))
            }
            Plan::Chat => {
                let questions: Vec<String> = case["sidecar"].as_array().map(|q| q.iter().map(|q| q.as_str().unwrap().to_owned()).collect()).unwrap_or_default();
                let text = chat_text(&body.answers_json(), &questions);
                match chat_refusal(&text) {
                    Some(refused) => refused,
                    None => match control_step_answer(&reply(&case["interrupt_reply"])) {
                        // O Esc não foi aceito: nada é digitado.
                        Err(not_closed) => {
                            assert_eq!(sent, &[json!({"control": "interrupt", "payload": {}})], "{name}: só o Esc");
                            not_closed
                        }
                        Ok(()) => {
                            assert_eq!(sent, &[json!({"control": "interrupt", "payload": {}}), json!({"submit": text})], "{name}: Esc e depois o texto");
                            chat_submit_answer(&reply(&case["submit_reply"]))
                        }
                    },
                }
            }
        };
        assert_eq!(case["cleared"].as_bool().unwrap(), done.0 == StatusCode::OK, "{name}: sidecar");
        check(name, done, &case);
    }
    assert!(seen >= 40);
}

#[test]
fn headless_answers_match_the_python_golden() {
    let mut seen = 0;
    for case in golden("answer.json").into_iter().filter(|c| c["terminal"] == false) {
        seen += 1;
        let name = case["name"].as_str().unwrap();
        let body = parse_body(&body_of(&case).into()).unwrap();
        // Sem id o Rust não responde: repassa ao Python (que aceita, como o golden mostra).
        if relays_headless(&body) {
            assert!(case["request_id"].is_null() && case["expect"]["status"] == 200, "{name}: só o pedido sem id é repassado");
            continue;
        }
        assert!(!case["request_id"].is_null(), "{name}");
        let command = headless_command(&body);
        // O Python manda `indices: null` nas respostas que não são de opção; o ator lê igual.
        let mut want = case["sent"][0]["payload"].clone();
        for answer in want["answers"].as_array_mut().unwrap() { if answer["indices"].is_null() { answer["indices"] = json!([]); } }
        assert_eq!(command, want, "{name}: o controle que o ator recebe");
        check(name, headless_answer(case["provider"] == "codex", &reply(&case["reply"])), &case);
    }
    assert!(seen >= 6);
}

#[test]
fn bodies_python_would_coerce_or_refuse_are_not_parsed() {
    let bad = [
        json!({"answers": [{"kind": "option", "indices": ["1"], "labels": ["A"]}]}),
        json!({"answers": [{"kind": "option", "indices": [1], "labels": ["A"], "x": 1}]}),
        json!({"answers": [{"kind": "option", "indices": [1], "labels": ["A"], "multi": "true"}]}),
        json!({"answers": [{"kind": "text", "value": 5, "type_index": 1}]}),
        json!({"answers": [{"kind": "text", "value": "a", "type_index": 1.5}]}),
        json!({"answers": [{"labels": ["A"]}]}),
        json!({"answers": [{"kind": "chat", "chat_index": 1, "labels": null}]}),
        json!({"answers": [], "request_id": 1.5}),
        json!({"answers": [], "request_id": true}),
        json!({"answers": [], "request_id": [1]}),
        json!({"answers": [], "extra": 1}),
        json!({"answers": {}}),
        json!({}),
    ];
    for body in bad { assert!(parse_body(&body.to_string().into()).is_none(), "{body}"); }
    let good = json!({"answers": [{"kind": "chat", "chat_index": 1, "question_id": null, "value": null, "indices": null, "multi": false, "labels": [], "type_index": null}], "request_id": 7});
    assert!(parse_body(&good.to_string().into()).is_some());
}

// ── a espera do rodapé ──────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_wait_ends_when_the_footer_is_gone_and_gives_up_at_the_limit() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let reads = AtomicUsize::new(0);
    let gone = wait_footer_gone(|| async { Some(reads.fetch_add(1, SeqCst) < 2) }, Duration::from_secs(5), Duration::from_millis(1)).await;
    assert_eq!((gone, reads.load(SeqCst)), (true, 3), "três leituras: duas com o rodapé, uma sem");
    let stuck = wait_footer_gone(|| async { Some(true) }, Duration::from_millis(50), Duration::from_millis(5)).await;
    assert!(!stuck, "o rodapé que não sai estoura o prazo e quem chama envia assim mesmo");
    assert!(wait_footer_gone(|| async { None }, Duration::from_secs(5), Duration::from_millis(1)).await, "captura que falha não segura o texto");
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
                }
            });
        }
    });
    format!("tcp:{address}")
}

/// `calls` conta os controles que chegaram ao serviço de plugin do Python (a pergunta segurada vai por ele).
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

async fn registry_counting() -> (Arc<RuntimeRegistry>, Arc<std::sync::atomic::AtomicUsize>) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    (Arc::new(RuntimeRegistry::new(policy(calls.clone()).await, "secret-test".into(), "instance-test".into())), calls)
}

async fn open_headless(registry: &RuntimeRegistry, dir: &Path, name: &str) {
    let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(sink).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
}

/// tmux falso: aceita tudo e anota cada chamada com o texto que ela leva.
#[cfg(unix)]
fn fake_tmux(dir: &Path, name: &str, pane: &str) -> (String, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (script, log) = (dir.join("tmux"), dir.join("tmux.log"));
    std::fs::write(&script, format!(
        "#!/bin/sh\nif [ \"$1\" = display-message ]; then printf '{name}\\t{}\\t1\\n'; exit 0; fi\necho \"$@\" >> '{}'\n", pane.replace('%', "%%"), log.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script.to_str().unwrap().to_owned(), log)
}

#[cfg(unix)]
fn log_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_owned).collect()
}

#[cfg(unix)]
fn keys_sent(log: &Path) -> usize { log_lines(log).iter().filter(|l| l.contains("send-keys")).count() }

/// Entrada com terminal; a transcrição mora em `<conta>/projects/p/<name>.jsonl`, de onde sai o sidecar.
#[cfg(unix)]
async fn open_terminal(registry: &RuntimeRegistry, config: &Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    open_terminal_on(registry, config, name, "%1").await
}

#[cfg(unix)]
async fn open_terminal_on(registry: &RuntimeRegistry, config: &Path, name: &str, pane: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = config.join("projects/p");
    std::fs::create_dir_all(&dir).unwrap();
    let (tmux, log) = fake_tmux(config, name, pane);
    let binding = TerminalBinding { name: name.into(), pane: pane.into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec![tmux], windows: false, clipboard_lock_path: None };
    let transcript = dir.join(format!("{name}.jsonl"));
    std::fs::write(&transcript, "").unwrap();
    registry.open_terminal(TerminalTarget { key: format!("k-{name}"), generation: 1, name: name.into(), binding,
        lease_path: dir.join(format!("{name}.lease")), state_path: dir.join(format!("{name}.state")), projection_dir: dir.join(format!("{name}.projection")),
        transcript: transcript.clone(), created: 0.0, plugin_key: None }).await.unwrap();
    assert!(registry.writable(name).await.unwrap().healthy);
    (log, transcript)
}

#[cfg(unix)]
fn write_sidecar(config: &Path, name: &str) -> std::path::PathBuf {
    let path = config.join(".hangar-askq").join(format!("{name}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, json!({"tool_input": {"questions": [
        {"header": "h", "question": "Cor?", "multiSelect": false, "options": [{"label": "A"}]},
        {"header": "h", "question": "Tamanho?", "multiSelect": false, "options": [{"label": "G"}]}]}}).to_string()).unwrap();
    path
}

async fn serve(registry: Arc<RuntimeRegistry>) -> (Arc<Fake>, std::net::SocketAddr) {
    serve_with(registry, hangar_server::terminal_control::TerminalPool::new(), None).await
}

/// `panel`: a sessão com painel de terminal aberto na tela.
async fn serve_with(registry: Arc<RuntimeRegistry>, pool: hangar_server::terminal_control::TerminalPool, panel: Option<&str>) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::with_terminal_pool(config(upstream, "127.0.0.1"), pool);
    state.write_gate_wait = Duration::from_secs(5);
    assert!(state.state.runtime.set(registry).is_ok());
    if let Some(name) = panel { state.term.mark_open_for_test(name, &state.diag).await; }
    (python, spawn_state(state).await)
}

async fn answer(server: std::net::SocketAddr, name: &str, body: &Value) -> (u16, Value) {
    let response = client().post(format!("http://{server}/api/sessions/{name}/answer"))
        .header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}"))
        .body(body.to_string()).send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

const OPT: fn() -> Value = || json!({"kind": "option", "indices": [0], "labels": ["A"]});
const CHAT: fn() -> Value = || json!({"kind": "chat", "chat_index": 1});

#[cfg(unix)]
#[tokio::test]
async fn chat_answer_interrupts_then_submits_and_never_borrows_the_keyboard() {
    let (config, (registry, counter)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    let (log, _) = open_terminal(&registry, config.path(), "t").await;
    let sidecar = write_sidecar(config.path(), "t");
    let (python, server) = serve(registry.clone()).await;
    let (status, body) = answer(server, "t", &json!({"answers": [OPT(), CHAT()]})).await;
    assert_eq!((status, body), (200, json!({"ok": true, "fallback": false})));
    let lines = log_lines(&log);
    let escape = lines.iter().position(|l| l.contains("send-keys") && l.contains("Escape")).expect("o Esc chegou ao pane");
    // O pane falso não tem composer: o texto fica na fila durável do ator (`deferred`) e sai quando o terminal ficar livre.
    let queued = std::fs::read_to_string(config.path().join("projects/p/t.state")).unwrap();
    assert!(queued.contains("Sobre «Tamanho?» prefiro conversar antes de responder"), "o texto está na fila do ator: {queued}");
    let view = registry.terminal_view("t").await.unwrap().1.to_string();
    assert!(lines[escape..].iter().any(|l| l.starts_with("capture-pane")), "esperou o menu sair pelo pane depois do Esc: {lines:?}");
    assert!(!view.contains("keyboard_loan"), "o teclado não foi emprestado: {view}");
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0, "a pergunta do plugin não foi consultada pelo ator");
    assert!(!sidecar.exists(), "o sidecar da pergunta fechada saiu");
    assert_eq!(python.hits_to("/api/sessions/t/answer"), 0);
    assert_eq!(python.plugin_gets(), 1, "só a leitura da pergunta pendente foi ao Python");
}

#[cfg(unix)]
#[tokio::test]
async fn chat_answer_with_nothing_to_preserve_is_409_and_presses_nothing() {
    let (config, (registry, _)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    let (log, _) = open_terminal(&registry, config.path(), "t").await;
    let (_, server) = serve(registry).await;
    let before = keys_sent(&log);
    let (status, body) = answer(server, "t", &json!({"answers": [CHAT()]})).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_sem_resposta"), Some("resposta sem texto para conversar")), "{body}");
    assert_eq!(keys_sent(&log), before);
}

#[cfg(unix)]
#[tokio::test]
async fn held_question_is_answered_through_the_actor_and_clears_the_sidecar() {
    let (config, (registry, counter)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    open_terminal(&registry, config.path(), "t").await;
    let sidecar = write_sidecar(config.path(), "t");
    let (python, server) = serve(registry).await;
    python.set_plugin_pending(json!({"id": "ask:1", "questions": []}));
    let (status, body) = answer(server, "t", &json!({"answers": [OPT()], "request_id": "ask:1"})).await;
    assert_eq!((status, body), (200, json!({"ok": true, "fallback": false})));
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1, "o controle chegou ao ator, que o entregou ao plugin");
    assert!(!sidecar.exists());
    assert_eq!(python.hits_to("/api/sessions/t/answer"), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_changed_question_is_409_and_the_actor_receives_nothing() {
    let (config, (registry, counter)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    let (log, _) = open_terminal(&registry, config.path(), "t").await;
    let sidecar = write_sidecar(config.path(), "t");
    let (python, server) = serve(registry).await;
    python.set_plugin_pending(json!({"id": "ask:2", "questions": []}));
    let before = keys_sent(&log);
    let (status, body) = answer(server, "t", &json!({"answers": [OPT()], "request_id": "ask:1"})).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_sem_resposta"), Some("a pergunta mudou; resposta conservada")), "{body}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!((keys_sent(&log), counter.load(std::sync::atomic::Ordering::SeqCst)), (before, 0));
    assert!(sidecar.exists(), "pergunta não respondida segue valendo");
}

#[cfg(unix)]
#[tokio::test]
async fn a_plugin_lookup_that_fails_is_a_503_with_a_code_and_sends_nothing() {
    let (config, (registry, _)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    let (log, _) = open_terminal(&registry, config.path(), "t").await;
    let (python, server) = serve(registry).await;
    python.fail_plugin(Some(StatusCode::INTERNAL_SERVER_ERROR));
    let before = keys_sent(&log);
    let (status, body) = answer(server, "t", &json!({"answers": [OPT(), CHAT()]})).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["params"]["detalhe"].as_str()),
        (503, Some("erro_sem_resposta"), Some("plugin_unavailable")), "{body}");
    assert_eq!(python.hits_to("/api/sessions/t/answer"), 0);
    assert_eq!(keys_sent(&log), before);
}

#[cfg(unix)]
#[tokio::test]
async fn bodies_python_would_coerce_or_refuse_reach_python() {
    let (config, (registry, _)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    open_terminal(&registry, config.path(), "t").await;
    let (python, server) = serve(registry).await;
    for body in [json!({"answers": [{"kind": "option", "indices": ["0"], "labels": ["A"]}]}), json!({"answers": [OPT()], "x": 1}),
        json!({"answers": [OPT()], "request_id": 1.5})] {
        assert_eq!(answer(server, "t", &body).await, (200, "from-python".into()), "{body}");
    }
    assert_eq!(python.hits_to("/api/sessions/t/answer"), 3);
}

#[tokio::test]
async fn headless_without_a_pending_question_is_a_409_with_a_code_and_never_a_forward() {
    let (dir, (registry, _)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    let (status, body) = answer(server, "s", &json!({"answers": [OPT()], "request_id": "r1"})).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_codex_resposta_invalida")), "{body}");
    assert_eq!(python.hits_to("/api/sessions/s/answer"), 0);
}

#[tokio::test]
async fn headless_without_a_request_id_goes_to_python() {
    let (dir, (registry, _)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry).await;
    assert_eq!(answer(server, "s", &json!({"answers": [OPT()]})).await, (200, "from-python".into()));
    assert_eq!(python.hits_to("/api/sessions/s/answer"), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn an_open_terminal_panel_refuses_the_answer_when_no_question_is_held() {
    let (config, (registry, counter)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    let (log, _) = open_terminal(&registry, config.path(), "t").await;
    let sidecar = write_sidecar(config.path(), "t");
    let (python, server) = serve_with(registry, hangar_server::terminal_control::TerminalPool::new(), Some("t")).await;
    let before = keys_sent(&log);
    let (status, body) = answer(server, "t", &json!({"answers": [OPT(), CHAT()]})).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_terminal_aberto")), "{body}");
    assert_eq!((keys_sent(&log), counter.load(std::sync::atomic::Ordering::SeqCst)), (before, 0), "nada chegou ao ator");
    assert!(sidecar.exists());
    assert_eq!(python.hits_to("/api/sessions/t/answer"), 0);
    // Com a pergunta segurada pelo plugin a resposta entra sem teclado, então o painel não a impede.
    python.set_plugin_pending(json!({"id": "ask:1", "questions": []}));
    assert_eq!(answer(server, "t", &json!({"answers": [OPT()]})).await.0, 200);
}

/// O Esc que fecha a pergunta e a espera do menu usam o observador do pane; o do `Monitor` da sessão
/// (`monitor:<nome>`) divide esse observador e tem de sobreviver à resposta.
#[cfg(unix)]
#[tokio::test]
async fn the_footer_wait_never_releases_the_live_monitor_observer() {
    use hangar_server::terminal_control::{CaptureRequest, Limits, TerminalPool};
    let dir = tempfile::tempdir().unwrap();
    let label = format!("hangar-answer-{}", dir.path().file_name().unwrap().to_string_lossy());
    let _server = IsolatedTmux(label.clone());
    let tmux = |args: &[&str]| {
        let out = std::process::Command::new("tmux").arg("-u").arg("-L").arg(&label).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "120", "-y", "30", "cat"]);
    let socket = std::path::PathBuf::from(tmux(&["display-message", "-p", "#{socket_path}"]).trim());
    let pane = tmux(&["display-message", "-p", "-t", "=t:", "#{pane_id}"]).trim().to_owned();
    let pool = TerminalPool::with_program("tmux", Some(socket), Limits::default());
    let monitor = CaptureRequest { consumer: "monitor:t".into(), name: "t".into(), provider: "claude".into(), binding: "sid".into(),
        target: pane.clone(), started: 0.0, lines: 200, colors: false, join: false };
    pool.capture(monitor.clone()).await.unwrap();
    let clients = || tmux(&["list-clients", "-F", "#{client_pid}"]);
    let before = clients();
    assert_eq!(before.lines().count(), 1, "um cliente de controle do monitor");

    let (config, (registry, _)) = (tempfile::tempdir().unwrap(), registry_counting().await);
    open_terminal_on(&registry, config.path(), "t", &pane).await;
    write_sidecar(config.path(), "t");
    let (_, server) = serve_with(registry, pool.clone(), None).await;
    assert_eq!(answer(server, "t", &json!({"answers": [OPT(), CHAT()]})).await.0, 200);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(clients(), before, "o observador do monitor segue o mesmo");
    pool.capture(monitor).await.unwrap();
}

/// Derruba o servidor tmux isolado (só o `-L` do teste, nunca o padrão) mesmo quando uma asserção falha.
#[cfg(unix)]
struct IsolatedTmux(String);
#[cfg(unix)]
impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        let tmux = |args: &[&str]| std::process::Command::new("tmux").arg("-L").arg(&self.0).args(args).output().ok();
        let socket = tmux(&["display-message", "-p", "#{socket_path}"]).filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|s| !s.is_empty());
        tmux(&["kill-server"]);
        if let Some(socket) = socket { let _ = std::fs::remove_file(socket); }
    }
}
