//! `/interrupt`, `/keys`, `/term-input`, `/select`, `/select/submit` e as rotas da fila servidos pelo
//! Rust: o corpo e o código de cada resposta vêm do golden congelado (sem gerador), e o
//! resto prova a rota inteira com um cano falso e o plugin falso do Python.
use crate::fake;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fake::*;
use hangar_server::routes::AppState;
use hangar_server::interactions::{ExpectedInteraction, InteractionBinding, InteractionKind, InteractionStore, plugin_target};
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
fn selection_plan_uses_the_supplied_target_not_the_first_pending_question() {
    let mut expected = ExpectedInteraction { binding:InteractionBinding {key:"k".into(),conversation:"sid".into(),generation:1},
        target:plugin_target(InteractionKind::Approval,"perm:a"),interaction_id:"shown-a".into(),semantic:json!({}),source_revision:Some("a".into()) };
    assert_eq!(select_plan(&expected,1,true).unwrap(),json!({"option":1,"interaction_id":"shown-a"}));
    assert!(select_plan(&expected,3,false).is_err());
    expected.target = plugin_target(InteractionKind::Question,"ask:a");
    assert_eq!(select_plan(&expected,1,false).unwrap_err().1["detail"]["code"],"erro_interacao_desatualizada");
    expected.target.kind = InteractionKind::Menu;
    assert_eq!(select_plan(&expected,1,true).unwrap_err().1["detail"]["code"],"erro_terminal_aberto");
    assert_eq!(select_plan(&expected,2,false).unwrap()["interaction_id"],"shown-a");
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
        let sent = rust_reply(&case);
        let (answer, diary) = submit_answer(&sent);
        if sent.as_ref().is_ok_and(|reply| reply.disposition == Disposition::Unknown) {
            assert_eq!(answer.1["detail"]["code"], "erro_sem_confirmacao_resposta");
            continue;
        }
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

#[test]
fn headless_steer_that_fell_back_to_promote_during_the_turn_is_not_delivered() {
    let accepted = |ids: Value| Ok(RuntimeReply { operation_id: "op".into(), disposition: Disposition::Accepted, payload: json!({"ids": ids}) });
    assert_eq!(steer_in_turn(&accepted(json!(["a"])), "a").unwrap(), (StatusCode::OK, json!({"ok": true, "steered": true})));
    // Aceita, mas a entrada voltou à fila: segue ao Promote marcada como devolvida.
    assert_eq!(steer_in_turn(&accepted(json!(["outra"])), "a"), Err(true));
    let no_turn = Ok(RuntimeReply { operation_id: "op".into(), disposition: Disposition::Rejected, payload: json!({"code": "steer_no_turn"}) });
    assert_eq!(steer_in_turn(&no_turn, "a"), Err(false));
    // Devolvida e posta na frente: não entrou no turno.
    let (status, body) = steer_after_promote(&Ok(json!("moved")), true).unwrap();
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("queue_steer_not_delivered")));
    // Sem turno (ou ele acabou no meio): vai para a frente e sai na entrega comum.
    assert_eq!(steer_after_promote(&Ok(json!("moved")), false).unwrap(), (StatusCode::OK, json!({"ok": true, "steered": false})));
    assert_eq!(steer_after_promote(&Ok(json!("absent")), true).unwrap_err().1["detail"]["code"], "queue_entry_gone");
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

async fn registry() -> Arc<RuntimeRegistry> {
    Arc::new(RuntimeRegistry::new("instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))))
}

pub(crate) async fn open_headless(registry: &RuntimeRegistry, dir: &Path, name: &str) -> Arc<std::sync::Mutex<Vec<Value>>> {
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
        // `#{pane_pid}` responde o pid de quem chamou (o próprio teste, que é o "agente" da sessão).
        "#!/bin/sh\nif [ \"$1\" = display-message ]; then for last; do :; done; case \"$last\" in '#{{pane_pid}}'*) printf \"$PPID\\t\";; esac; printf '{name}\\t%%1\\t1\\n'; exit 0; fi\nif [ \"$1\" = capture-pane ]; then printf '────────────────────────────────\\n❯ \\n────────────────────────────────\\n'; exit 0; fi\necho \"$@\" >> '{}'\n", log.display()));
    (script.to_str().unwrap().to_owned(), log)
}

#[cfg(unix)]
pub(crate) fn keys_sent(log: &Path) -> usize {
    std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.contains("send-keys")).count()
}

/// Entrada com terminal sobre o tmux falso: o que chega ao ator vira `send-keys` no registro.
#[cfg(unix)]
pub(crate) async fn open_terminal(registry: &RuntimeRegistry, dir: &Path, name: &str) -> std::path::PathBuf {
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
    // Registro nativo do Claude para o agente (o próprio teste): prova a conversa sem transcript aberto.
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::write(dir.join("sessions").join(format!("{}.json", std::process::id())), json!({"pid": std::process::id(), "sessionId": "sid"}).to_string()).unwrap();
    registry.open_terminal(TerminalTarget { key: format!("k-{name}"), generation: 1, name: name.into(), binding,
        lease_path: dir.join(format!("{name}.lease")), state_path, projection_dir, transcript, created: 0.0, plugin_key: None,
        metadata: hangar_server::terminal_facts::TerminalMetadata { config_dir:dir.into(),agent_pid:std::process::id(),agent_birth:hangar_server::terminal_facts::agent_birth(std::process::id()).unwrap(),legacy_import_after:None,fingerprint:"fixture".into(),origin:"uds:fixture".into() } }).await.unwrap();
    let target = registry.writable(name).await.unwrap();
    assert!(target.terminal && target.healthy, "entrada de terminal saudável");
}

async fn serve(registry: Arc<RuntimeRegistry>) -> (Arc<Fake>, std::net::SocketAddr) {
    serve_with_interactions(registry,Arc::new(InteractionStore::default())).await
}

pub(crate) async fn serve_with_interactions(registry: Arc<RuntimeRegistry>, interactions: Arc<InteractionStore>) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, _) = spawn_fake().await;
    let mut state = AppState::new(config("127.0.0.1"));
    state.interactions = interactions;
    state.write_gate_wait = Duration::from_secs(5);
    assert!(state.state.runtime.set(registry).is_ok());
    let addr = spawn_state_for(&python, state).await;
    (python, addr)
}

pub(crate) async fn send(server: std::net::SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
    let url = format!("http://{server}/api/sessions/{path}");
    let request = client().request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url);
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
    let (_, server) = serve(registry).await;
    // Sessão parada: não há turno a interromper, e dizer "ok" seria fingir (como o Python).
    let (status, body) = send(server, "POST", "s/interrupt", "").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_sem_turno")), "{body}");
    assert!(!sink.lock().unwrap().iter().any(|e| e.to_string().contains("interrupt")), "sem turno nada vai ao cano");
}

#[tokio::test]
async fn headless_select_without_identity_is_409_and_never_reaches_the_writer() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let (_, server) = serve(registry).await;
    let (status, body) = send(server, "POST", "s/select", r#"{"option":1}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_interacao_sem_identidade"), Some("Atualize o app ou recarregue a página antes de responder.")), "{body}");
    assert!(!sink.lock().unwrap().iter().any(|event|event.to_string().contains("control_response")));
}

#[tokio::test]
async fn select_with_a_body_the_route_refuses_is_a_rust_422() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let (_, server) = serve(registry).await;
    for body in [r#"{"option":"1","interaction_id":"old"}"#, r#"{"option":0}"#, r#"{"option":51}"#,
        r#"{"option":1,"interaction_id":"old","x":1}"#, r#"{"option":1.5}"#] {
        let (status, reply) = send(server, "POST", "s/select", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}");
    }
    let (status,body) = send(server,"POST","s/select",r#"{"option":1,"interaction_id":"old"}"#).await;
    assert_eq!((status,body["detail"]["code"].as_str()),(409,Some("erro_interacao_desatualizada")));
    assert!(!sink.lock().unwrap().iter().any(|event|event.to_string().contains("control_response")));
}

#[tokio::test]
async fn interrupt_with_a_clear_the_route_cannot_parse_is_a_rust_422() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (_, server) = serve(registry).await;
    let (status, body) = send(server, "POST", "s/interrupt?clear=talvez", "").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")));
}

#[tokio::test]
async fn queue_discard_of_an_entry_that_is_not_there_is_404() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (_, server) = serve(registry).await;
    let (status, body) = send(server, "DELETE", "s/queue/nao-existe", "").await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (404, Some("erro_fila_entrada_nao_encontrada"), Some("entrada não está na fila")));
}

#[tokio::test]
async fn queue_panel_routes_are_served_in_rust_and_an_absent_entry_is_409_gone() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (_, server) = serve(registry).await;
    for (method, path, body) in [("PATCH", "s/queue/nada", r#"{"text":"x"}"#), ("DELETE", "s/queue/nada?only_waiting=true", ""),
        ("POST", "s/queue/reorder", r#"{"id":"nada","before_id":null}"#), ("POST", "s/queue/nada/steer", "")] {
        let (status, reply) = send(server, method, path, body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("queue_entry_gone")), "{method} {path}: {reply}");
    }
}

#[tokio::test]
async fn queue_edit_with_an_empty_or_slash_text_is_422() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s").await;
    let (_python, server) = serve(registry).await;
    for body in [r#"{"text":"  "}"#, r#"{"text":"/clear"}"#] {
        let (status, reply) = send(server, "PATCH", "s/queue/a", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("queue_text_invalid")), "{body}");
    }
    for body in [r#"{"text":"x","extra":1}"#, r#"{"texto":"x"}"#, "[]"] {
        assert_eq!(send(server, "PATCH", "s/queue/a", body).await.1["detail"]["code"], "erro_corpo_invalido", "{body}");
    }
    assert_eq!(send(server, "DELETE", "s/queue/a?only_waiting=talvez", "").await.1["detail"]["code"], "erro_corpo_invalido");
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_steer_of_an_absent_entry_is_409_and_sends_no_keys() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (_python, server) = serve(registry).await;
    let before = keys_sent(&log);
    let (status, reply) = send(server, "POST", "t/queue/nada/steer", "").await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("queue_entry_gone")), "{reply}");
    assert_eq!(keys_sent(&log), before);
}

/// Ator ocupado lendo a tela (psmux lento no Windows) não é entrada doente: a escrita espera a vez.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_steer_while_the_actor_reads_a_slow_screen_is_not_unhealthy() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let _log = open_terminal(&registry, dir.path(), "t").await;
    let tmux = dir.path().join("tmux");
    let script = std::fs::read_to_string(&tmux).unwrap();
    let reading = dir.path().join("reading");
    crate::write_executable(&tmux, script.replacen("= capture-pane ]; then",
        &format!("= capture-pane ]; then touch '{}'; sleep 3;", reading.display()), 1));
    let (_python, server) = serve(registry).await;
    // O interromper lê a tela uma vez antes do Esc: segura o ator nessa leitura lenta.
    let interrupt = tokio::spawn(send(server, "POST", "t/interrupt", ""));
    let started = std::time::Instant::now();
    while !reading.exists() {
        assert!(started.elapsed() < Duration::from_secs(30), "o ator não começou a ler a tela");
        tokio::time::sleep(Duration::from_millis(10)).await; // estavel: espera pelo marcador que o tmux falso cria, teto de 30 s
    }
    let asked = std::time::Instant::now();
    let (status, reply) = send(server, "POST", "t/queue/nada/steer", "").await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("queue_entry_gone")), "o Orientar chega ao ator: {reply}");
    assert!(asked.elapsed() >= Duration::from_secs(1), "o Orientar achou o ator ocupado e esperou a vez dele");
    tokio::time::timeout(Duration::from_secs(30), interrupt).await.expect("o interromper terminou").unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn permission_held_by_the_plugin_refuses_option_3_and_the_actor_receives_nothing() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let store = Arc::new(InteractionStore::default());
    let binding = registry.interaction_binding("t").await.unwrap();
    store.bind(&binding).unwrap();
    let id = store.observe(&binding,&plugin_target(InteractionKind::Approval,"perm:1"),Some(json!({}))).unwrap().unwrap();
    let (_, server) = serve_with_interactions(registry,store).await;
    let keys_before = keys_sent(&log);
    let (status, body) = send(server, "POST", "t/select", &json!({"option":3,"interaction_id":id}).to_string()).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_opcao_nao_convergiu"), Some("opção fora do pedido de permissão")), "{body}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(keys_sent(&log), keys_before, "nenhum controle chegou ao ator");
    // Sem a ligação do dono plugin não há reserva por tecla nem resposta fingida.
    let (status, body) = send(server, "POST", "t/select", &json!({"option":1,"interaction_id":id}).to_string()).await;
    assert_eq!((status,body["detail"]["code"].as_str()),(503,Some("erro_interacao_indisponivel")));
    assert_eq!(keys_sent(&log), keys_before);
}

#[cfg(unix)]
#[tokio::test]
async fn missing_identity_is_rejected_before_any_plugin_lookup() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (_, server) = serve(registry).await;
    let before = keys_sent(&log);
    let (status, body) = send(server, "POST", "t/select", r#"{"option":1}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_interacao_sem_identidade")), "{body}");
    assert_eq!(keys_sent(&log), before);
}

#[cfg(unix)]
#[tokio::test]
async fn submit_and_select_reject_stale_ids_without_any_writer_command() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (_, server) = serve(registry).await;
    let before = keys_sent(&log);
    for (route,payload) in [("select",json!({"option":1,"interaction_id":"old"})),
        ("select/submit",json!({"interaction_id":"old"}))] {
        let (status, body) = send(server,"POST",&format!("t/{route}"),&payload.to_string()).await;
        assert_eq!((status,body["detail"]["code"].as_str()),(409,Some("erro_interacao_desatualizada")),"{body}");
    }
    assert_eq!(keys_sent(&log), before);
}

/// Registro com o `plugin::Store` do Rust ligado, como o servidor monta: a pergunta segurada é dele.
pub(crate) async fn registry_with_plugin(interactions: &Arc<InteractionStore>) -> (Arc<RuntimeRegistry>, Arc<hangar_server::plugin::Store>) {
    let plugin = Arc::new(hangar_server::plugin::Store::new(interactions.clone()));
    let registry = RuntimeRegistry::new("instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1")))
        .with_interactions(interactions.clone()).with_plugin(plugin.clone());
    (Arc::new(registry), plugin)
}

fn held_question() -> hangar_server::plugin::Ask {
    hangar_server::plugin::Ask { id: "ask:9".into(), questions: Some(vec![json!({"question": "Pasta?", "options": [{"label": "a"}]})]),
        tool: None, janela_ms: None, summary: None }
}

#[cfg(unix)]
#[tokio::test]
async fn an_accepted_escape_closes_the_question_in_the_rust_store_without_asking_python() {
    let (dir, interactions) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, plugin) = registry_with_plugin(&interactions).await;
    let log = open_terminal(&registry, dir.path(), "t").await;
    let binding = registry.interaction_binding("t").await.unwrap();
    let (_, server) = serve_with_interactions(registry, interactions).await;
    let gate = hangar_server::plugin::PermissionGate { app_present: true, terminal_attached: false, mode_without_dialog: false };
    let poll = plugin.begin_ask(&binding, held_question(), gate, std::time::Instant::now()).unwrap();
    let before = keys_sent(&log);
    let (hook, interrupted) = tokio::join!(poll.wait(), send(server, "POST", "t/interrupt", ""));
    assert_eq!(interrupted, (200, json!({"ok": true})));
    assert!(keys_sent(&log) > before, "o Esc chegou ao pane");
    assert_eq!(hook, json!({"answers": null}), "o hook do plugin soube pelo Store do Rust");
    assert!(plugin.pending_all(&binding, std::time::Instant::now()).unwrap().is_empty());
}

/// Sem ler a pergunta segurada no Store, nenhum Esc: o erro aparece na resposta, e o Python não é a reserva.
#[cfg(unix)]
#[tokio::test]
async fn interrupt_whose_store_read_fails_never_presses_escape() {
    let (dir, interactions) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, plugin) = registry_with_plugin(&interactions).await;
    let log = open_terminal(&registry, dir.path(), "t").await;
    let binding = registry.interaction_binding("t").await.unwrap();
    // A sessão sai do Store: `pending_all` recusa a leitura antes do Esc.
    plugin.forget(&binding.key).unwrap();
    let (_, server) = serve_with_interactions(registry, interactions).await;
    let before = keys_sent(&log);
    let (status, body) = send(server, "POST", "t/interrupt", "").await;
    assert_ne!(status, 200, "{body}");
    assert!(body["detail"]["code"].as_str().is_some_and(|code| !code.is_empty()), "erro com código: {body}");
    assert_eq!(keys_sent(&log), before, "sem ler a pergunta, nenhum Esc");
}

#[tokio::test]
async fn interrupt_the_terminal_refused_keeps_the_question_open() {
    let (dir, interactions) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, plugin) = registry_with_plugin(&interactions).await;
    open_terminal_without_tmux(&registry, dir.path(), "t").await;
    let binding = registry.interaction_binding("t").await.unwrap();
    let (_, server) = serve_with_interactions(registry, interactions).await;
    let gate = hangar_server::plugin::PermissionGate { app_present: true, terminal_attached: false, mode_without_dialog: false };
    let _poll = plugin.begin_ask(&binding, held_question(), gate, std::time::Instant::now()).unwrap();
    let (status, _) = send(server, "POST", "t/interrupt", "").await;
    assert_ne!(status, 200, "sem tmux o Esc não sai");
    assert_eq!(plugin.pending_all(&binding, std::time::Instant::now()).unwrap().len(), 1, "pergunta não interrompida segue valendo");
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_keys_and_term_input_with_bodies_the_route_refuses_are_a_rust_422() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (_, server) = serve(registry).await;
    let before = keys_sent(&log);
    for (route, body) in [("keys", r#"{"key":5}"#), ("keys", r#"{}"#), ("term-input", r#"{"text":5}"#), ("term-input", r#"{"x":1}"#)] {
        let (status, reply) = send(server, "POST", &format!("t/{route}"), body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{route} {body}");
    }
    assert_eq!(keys_sent(&log), before);
}

#[cfg(unix)]
#[tokio::test]
async fn keys_reach_the_pane_through_the_actor() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (_, server) = serve(registry).await;
    let before = keys_sent(&log);
    assert_eq!(send(server, "POST", "t/keys", r#"{"key":"Down"}"#).await, (200, json!({"ok": true})));
    assert_eq!(keys_sent(&log), before + 1);
    let (status, body) = send(server, "POST", "t/keys", r#"{"key":"Nope"}"#).await;
    assert_eq!((status, body), (400, json!({"detail": "tecla não permitida"})));
    assert_eq!(keys_sent(&log), before + 1, "tecla fora da lista não chega ao pane");
}

const MSG_NO_TERMINAL: &str = "esta sessão não tem terminal; nada foi enviado";

fn no_terminal() -> (u16, Value) {
    (502, json!({"detail": {"code": "erro_envio_falhou", "params": {"erro": MSG_NO_TERMINAL}, "msg": MSG_NO_TERMINAL}}))
}

/// Claude (`s`, cano que anota cada envelope) e Codex (`c`) sem terminal no mesmo servidor.
async fn headless_pair() -> (tempfile::TempDir, Arc<std::sync::Mutex<Vec<Value>>>, Arc<std::sync::Mutex<Vec<String>>>, std::net::SocketAddr) {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let methods: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    crate::groups_deliver::open_codex_headless(&registry, dir.path(), "c", methods.clone()).await;
    let (_, server) = serve(registry).await;
    (dir, sink, methods, server)
}

/// Nenhuma mensagem chegou aos canos: nem frame `user` no Claude, nem turno pedido ao Codex. A conversa
/// de abertura dos dois segue à parte, por isso não se conta envelope.
fn nothing_sent(sink: &std::sync::Mutex<Vec<Value>>, methods: &std::sync::Mutex<Vec<String>>) {
    let frames: Vec<Value> = sink.lock().unwrap().iter().filter_map(|e| serde_json::from_str(e["frame"].as_str()?).ok()).collect();
    assert!(!frames.iter().any(|frame| frame["type"] == "user"), "{frames:?}");
    let methods = methods.lock().unwrap();
    assert!(!methods.iter().any(|method| method.starts_with("turn/")), "{methods:?}");
}

// R2-01, R2-02 (`api.py:2746`, Python 502 `erro_envio_falhou`): tecla em sessão sem terminal não vai a pane
// nenhum nem ao Python; a recusa é do Rust, igual para Claude e Codex.
#[tokio::test]
async fn keys_without_a_terminal_are_a_rust_502_and_nothing_is_sent() {
    let (_dir, sink, methods, server) = headless_pair().await;
    for name in ["s", "c"] {
        assert_eq!(send(server, "POST", &format!("{name}/keys"), r#"{"key":"Down"}"#).await, no_terminal(), "{name}");
        assert_eq!(send(server, "POST", &format!("{name}/keys"), r#"{"key":5}"#).await.1["detail"]["code"], "erro_corpo_invalido", "{name}");
    }
    nothing_sent(&sink, &methods);
}

// R2-03 (`api.py:2760`): corpo sem texto nem tecla é 200 sem tocar em nada; com texto ou tecla, a recusa de R2-01.
#[tokio::test]
async fn term_input_without_a_terminal_is_ok_when_empty_and_a_502_otherwise() {
    let (_dir, sink, methods, server) = headless_pair().await;
    for name in ["s", "c"] {
        for body in ["{}", r#"{"text":null,"key":null}"#, r#"{"text":"","key":""}"#] {
            assert_eq!(send(server, "POST", &format!("{name}/term-input"), body).await, (200, json!({"ok": true})), "{name} {body}");
        }
        for body in [r#"{"text":"ls"}"#, r#"{"key":"Enter"}"#, r#"{"text":"ls","key":"Enter"}"#] {
            assert_eq!(send(server, "POST", &format!("{name}/term-input"), body).await, no_terminal(), "{name} {body}");
        }
    }
    nothing_sent(&sink, &methods);
}

// R2-01: com a porta presa pela troca de conversa, tecla e texto de terminal da sessão sem terminal são o
// 409 de sessão ocupada, na hora, e nada chega aos canos.
#[tokio::test]
async fn keys_and_term_input_without_a_terminal_while_a_transfer_holds_the_gate_are_409_busy() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let methods: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    crate::groups_deliver::open_codex_headless(&registry, dir.path(), "c", methods.clone()).await;
    for name in ["s", "c"] { registry.ingress().hold(name, Duration::from_secs(1)).await.unwrap(); }
    let (_, server) = serve(registry).await;
    for name in ["s", "c"] {
        for (route, body) in [("keys", r#"{"key":"Down"}"#), ("term-input", r#"{"text":"ls","key":"Enter"}"#)] {
            let (status, reply) = send(server, "POST", &format!("{name}/{route}"), body).await;
            assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("session_transfer_busy")), "{name} {route}: {reply}");
        }
    }
    nothing_sent(&sink, &methods);
}

// R2-03 (`api.py:2746,2760`, `key not allowed`): tecla fora da lista é o 400 da validação antes da recusa
// por falta de terminal, em `keys` e em `term-input`; o Backspace do `term-input` está na lista dele.
#[tokio::test]
async fn a_key_outside_the_list_without_a_terminal_is_400_before_the_missing_terminal() {
    let (_dir, sink, methods, server) = headless_pair().await;
    let refused = (400, json!({"detail": "tecla não permitida"}));
    for name in ["s", "c"] {
        assert_eq!(send(server, "POST", &format!("{name}/keys"), r#"{"key":"Nope"}"#).await, refused, "{name} keys");
        assert_eq!(send(server, "POST", &format!("{name}/keys"), r#"{"key":"Backspace"}"#).await, refused, "{name}: Backspace só no term-input");
        assert_eq!(send(server, "POST", &format!("{name}/term-input"), r#"{"text":"ls","key":"Nope"}"#).await, refused, "{name} term-input");
        assert_eq!(send(server, "POST", &format!("{name}/term-input"), r#"{"key":"Backspace"}"#).await, no_terminal(), "{name}");
    }
    nothing_sent(&sink, &methods);
}

// R2-04 (`api.py:2408`, Python 409 `erro_opcao_nao_convergiu` com o detalhe do picker ausente) e R2-05
// (Codex, Python 502 `erro_envio_falhou`); decisão 7: sem `interaction_id`, 409 sem identidade antes.
#[tokio::test]
async fn select_submit_without_a_terminal_is_refused_in_rust() {
    let (_dir, sink, methods, server) = headless_pair().await;
    for name in ["s", "c"] {
        for body in ["", "{}", r#"{"interaction_id":null}"#, r#"{"interaction_id":""}"#] {
            let (status, reply) = send(server, "POST", &format!("{name}/select/submit"), body).await;
            assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_interacao_sem_identidade")), "{name} {body:?}");
        }
    }
    assert_eq!(send(server, "POST", "s/select/submit", r#"{"interaction_id":"i-1"}"#).await, (409, json!({"detail": {"code": "erro_opcao_nao_convergiu",
        "params": {"detalhe": "nao ha picker de multipla escolha na tela — nada enviado"}, "msg": "não consegui enviar as opções marcadas — tente de novo"}})));
    assert_eq!(send(server, "POST", "c/select/submit", r#"{"interaction_id":"i-1"}"#).await, no_terminal());
    nothing_sent(&sink, &methods);
}

// R2-06, R2-10, R2-16 (`test_codex_models_claude_rejected_with_400`, `test_limits_claude_rejected_with_400`,
// `api.py:2692`): sessão Claude do Rust nas rotas do Codex recebe o 400 da rota, sem ir ao Python.
#[tokio::test]
async fn codex_routes_on_a_rust_claude_session_are_the_python_400s() {
    let (_dir, sink, methods, server) = headless_pair().await;
    assert_eq!(send(server, "GET", "s/models", "").await, (400, json!({"detail": {"code": "erro_models_so_codex", "params": {}, "msg": "models so existe pra sessoes Codex"}})));
    assert_eq!(send(server, "GET", "s/limits", "").await, (400, json!({"detail": {"code": "erro_limits_so_codex", "params": {}, "msg": "limits so existe pra sessoes Codex"}})));
    assert_eq!(send(server, "POST", "s/codex/plan/implement", "").await,
        (400, json!({"detail": {"code": "erro_model_so_codex", "params": {}, "msg": "esta ação só existe para sessões Codex"}})));
    nothing_sent(&sink, &methods);
}

/// `/compact` do Codex com terminal servido pelo motor: o mesmo corpo do `_send_managed` do Python.
#[test]
fn codex_terminal_compact_answers_like_the_python_send() {
    use hangar_server::session_write::codex::compact_answer;
    let reply = |disposition: Disposition, payload: Value| -> Result<RuntimeReply, RuntimeError> {
        Ok(RuntimeReply { operation_id: OP.into(), disposition, payload })
    };
    assert_eq!(compact_answer(&reply(Disposition::Accepted, json!({}))),
        (StatusCode::OK, json!({"ok": true, "delivered": true, "steered": false, "native": false})));
    assert_eq!(compact_answer(&reply(Disposition::Deferred, json!({}))).1["delivered"], false);
    let refused = |sent: Result<RuntimeReply, RuntimeError>, msg: &str| {
        let (status, body) = compact_answer(&sent);
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["detail"], json!({"code": "erro_envio_falhou", "params": {"erro": msg}, "msg": msg}));
    };
    refused(Err(RuntimeError::new("codex_command", "espere o Codex terminar")), "espere o Codex terminar");
    refused(reply(Disposition::Unknown, json!({})), "resultado incerto; entrada conservada sem reenvio");
    refused(reply(Disposition::Rejected, json!({"error": "recusado"})), "recusado");
    refused(reply(Disposition::Rejected, json!({})), "entrada recusada pelo runtime");
}

#[test]
fn invalid_codex_option_is_a_bad_answer_not_a_failure_to_answer() {
    use hangar_server::session_write::control::select_headless_answer;
    let invalid = Err(RuntimeError::new(hangar_server::runtime::codex::INVALID_ANSWER, "opção inválida"));
    let (status, body) = select_headless_answer(&invalid);
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_codex_resposta_invalida")));
    let (status, _) = select_headless_answer(&Err(RuntimeError::new("codex_command", "runtime sem terminal indisponível")));
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "falha de envio continua 503");
}

// ── parar uma tarefa (subagente ou shell em fundo) ───────────────────────────────────────────────

#[tokio::test]
async fn headless_stop_task_writes_the_stop_task_control_to_the_cano() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let (_, server) = serve(registry).await;
    let (status, body) = send(server, "POST", "s/tasks/a85258e41e374c2ea/stop", "").await;
    assert_eq!((status, &body), (202, &json!({"pending": true})));
    let frames: Vec<Value> = sink.lock().unwrap().iter().filter_map(|e| serde_json::from_str(e["frame"].as_str()?).ok()).collect();
    let stop = frames.iter().find(|frame| frame["request"]["subtype"] == "stop_task").expect("stop_task no cano");
    assert_eq!((&stop["type"], &stop["request"]["task_id"]), (&json!("control_request"), &json!("a85258e41e374c2ea")));
}

#[tokio::test]
async fn stop_task_with_an_invalid_id_is_422_and_reaches_nothing() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    let sink = open_headless(&registry, dir.path(), "s").await;
    let (_python, server) = serve(registry).await;
    for id in ["a.b", "a%20b", &"x".repeat(65)] {
        let (status, body) = send(server, "POST", &format!("s/tasks/{id}/stop"), "").await;
        assert_eq!((status, body["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{id}: {body}");
    }
    assert!(!sink.lock().unwrap().iter().any(|e| e.to_string().contains("stop_task")));
}

#[tokio::test]
async fn stop_task_on_codex_is_409_only_claude() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    crate::groups_deliver::open_codex_headless(&registry, dir.path(), "c", Default::default()).await;
    let (_, server) = serve(registry).await;
    let (status, body) = send(server, "POST", "c/tasks/bserqfwvd/stop", "").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_parar_so_claude")), "{body}");
}

#[tokio::test]
async fn stop_task_on_a_name_without_rust_owner_is_409_only_claude_not_python() {
    let (_, server) = serve(registry().await).await;
    let (status, body) = send(server, "POST", "shell-cru/tasks/bserqfwvd/stop", "").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_parar_so_claude")), "{body}");
}

/// Pai `p` e filhas ligadas `a` (sem terminal, parada) e `b` (nenhuma sessão com esse nome), pelos arquivos de grupo.
/// Volta também o diário do servidor, para esperar as gravações dele.
async fn linked_family(registry: Arc<RuntimeRegistry>, dir: &Path) -> (Arc<Fake>, std::net::SocketAddr, hangar_server::diag::DiagClient) {
    use hangar_server::groups::{orq::OrqVault, service::GroupService, store::PairDir};
    let pair = dir.join("pair");
    std::fs::create_dir_all(&pair).unwrap();
    for child in ["a", "b"] {
        let sidecar = json!({"peers": ["p"], "task": "", "gid": "g1", "harness": {child: "claude", "p": "claude"}, "linked_by": "p"});
        std::fs::write(pair.join(format!("{child}.json")), sidecar.to_string()).unwrap();
    }
    let (python, _) = spawn_fake().await;
    let mut state = AppState::new(config("127.0.0.1"));
    state.write_gate_wait = Duration::from_secs(5);
    state.diag = python.bind_diary(state.diag.clone());
    state.groups = Some(Arc::new(GroupService::new(PairDir::new(pair, dir.join("arquivo")), Arc::new(OrqVault::from_state(&state)), "casa".into())));
    assert!(state.state.runtime.set(registry).is_ok());
    let diag = state.diag.clone();
    let addr = spawn_state_for(&python, state).await;
    (python, addr, diag)
}

/// O Parar para só a sessão: as filhas ligadas não recebem nada, nem com o `cascade=1` de um cliente antigo, que é
/// ignorado. Quem interrompe as filhas é só o fechamento do pai.
#[tokio::test]
async fn interrupting_a_parent_does_not_touch_its_linked_children() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "p").await;
    let child = open_headless(&registry, dir.path(), "a").await;
    let (python, server, diag) = linked_family(registry, dir.path()).await;
    for path in ["p/interrupt", "p/interrupt?cascade=1"] {
        let (status, body) = send(server, "POST", path, "").await;
        assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_sem_turno")), "{path}: {body}");
        assert!(body.get("cascade").is_none(), "{path}: {body}");
    }
    assert!(!child.lock().unwrap().iter().any(|e| e.to_string().contains("interrupt")), "nada foi à filha");
    diag.flush().await;
    let tried = python.diag().into_iter().filter(|d| d["evento"] == "rust.link_cascade_failed").count();
    assert_eq!(tried, 0, "nenhuma filha foi procurada: {:?}", python.diag());
}

/// Pai em turno: o Parar chega a ele e só a ele, também com o `cascade=1` de um cliente antigo.
#[tokio::test]
async fn interrupting_a_parent_in_a_turn_stops_only_the_parent() {
    let interrupted = |sink: &Arc<std::sync::Mutex<Vec<Value>>>| sink.lock().unwrap().iter()
        .filter_map(|e| serde_json::from_str::<Value>(e["frame"].as_str()?).ok())
        .any(|frame| frame["request"]["subtype"] == "interrupt");
    for path in ["p/interrupt", "p/interrupt?cascade=1"] {
        let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
        let parent = open_headless(&registry, dir.path(), "p").await;
        let child = open_headless(&registry, dir.path(), "a").await;
        let (_python, server, _diag) = linked_family(registry, dir.path()).await;
        // O cano falso nunca termina o turno que o envio abre.
        let (status, body) = send(server, "POST", "p/input", r#"{"text":"um"}"#).await;
        assert_eq!((status, &body["delivered"]), (200, &json!(true)), "{path}: o envio abre o turno: {body}");
        let (status, body) = send(server, "POST", path, "").await;
        assert_eq!((status, &body), (200, &json!({"ok": true})), "{path}: {body}");
        assert!(interrupted(&parent), "{path}: o Parar chegou ao pai");
        assert!(!interrupted(&child), "{path}: nada foi à filha");
    }
}

/// Pai Claude sem terminal estacionado (só o arquivo dele): a recusa sem turno vem antes da entrada, sem subir o
/// processo só para o Esc.
#[tokio::test]
async fn a_parked_session_answers_no_turn_without_launching_it() {
    let dir = tempfile::tempdir().unwrap();
    let sidecars = dir.path().join(".hangar/claude-headless");
    std::fs::create_dir_all(&sidecars).unwrap();
    let parked = json!({"name": "p", "key": "cd".repeat(16), "cwd": dir.path(), "session_id": "sid-p", "headless": true, "permission_mode": "manual"});
    std::fs::write(sidecars.join("p.json"), parked.to_string()).unwrap();
    let env = hangar_server::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(), dir.path().to_string_lossy().into_owned())].into());
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_codex_environment(env));
    let (_, server) = serve(registry.clone()).await;
    let (status, body) = send(server, "POST", "p/interrupt", "").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_sem_turno")), "{body}");
    assert!(registry.writable("p").await.is_none(), "o estacionado não subiu só para o Esc");
}
