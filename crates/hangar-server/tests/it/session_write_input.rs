//! `/input` e `/steer` de sessão Claude servidos pelo Rust: o corpo e o código de cada resposta do ator
//! vêm do golden que o Python gera (`gen_golden.py`), e o resto prova a rota inteira com um cano falso.
use crate::fake;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, ClockSample, RuntimeError, RuntimeReply, RuntimeTarget};
use hangar_server::runtime::queue::{Action, State, Store};
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::session_write::input::*;
use hangar_server::terminal_input::TerminalBinding;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const OP: &str = "OP";

fn golden(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/session_write").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A resposta do ator no golden: `{disposition,payload}`, ou o texto `codigo: mensagem` da falha.
fn reply(v: &Value) -> Result<RuntimeReply, RuntimeError> {
    if let Some(text) = v.as_str() {
        let (code, message) = text.split_once(": ").unwrap();
        return Err(RuntimeError::new(code, message));
    }
    Ok(RuntimeReply { operation_id: OP.into(), disposition: serde_json::from_value(v["disposition"].clone()).unwrap(), payload: v["payload"].clone() })
}

#[test]
fn input_answers_match_the_python_golden() {
    let cases = golden("input.json");
    assert!(cases.len() >= 25, "o golden tem os casos do brief");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let params = Params { text: case["text"].as_str().unwrap(), steer: case["steer"].as_bool().unwrap(),
            terminal: case["terminal"].as_bool().unwrap(), codex: case["provider"] == "codex", operation_id: OP };
        let (sent, diary) = classify(&params, &reply(&case["reply"]));
        // O golden antigo não fornece o resultado do controle terminal por entrada.
        let steered = wants_steer_queue(&params, &sent) && !case["queue"].is_null() && was_steered(OP, &reply(&case["queue"]));
        let (status, body) = input_answer(&sent, steered);
        assert_eq!((status.as_u16(), body), (case["expect"]["status"].as_u64().unwrap() as u16, case["expect"]["body"].clone()), "{name}");
        let want: Vec<(&str, Option<&str>)> = case["diary"].as_array().unwrap().iter()
            .map(|d| (d["event"].as_str().unwrap(), d["code"].as_str())).collect();
        match diary {
            None => assert!(want.is_empty(), "{name}: faltou o diário {want:?}"),
            Some((event, code)) => {
                assert_eq!(want.len(), 1, "{name}: diário a mais {event}");
                assert_eq!(event, want[0].0, "{name}");
                // O código do Python para falha é o nome da classe da exceção; só os de aviso são comparáveis.
                if event != "runtime.send_failed" { assert_eq!(Some(code.as_str()), want[0].1, "{name}"); }
            }
        }
    }
}

#[test]
fn codex_slash_command_rides_the_queue_and_a_sibling_message_steers_the_turn() {
    // No Codex a barra entra na fila como qualquer texto (`_send_one_codex_locked`): adiada, ela sai depois
    // sozinha, então "mande de novo" faria a skill rodar duas vezes.
    let codex = |text: &'static str, steer: bool| Params { text, steer, terminal: true, codex: true, operation_id: OP };
    let reply = |disposition: &str| Ok(RuntimeReply { operation_id: OP.into(), disposition: serde_json::from_value(json!(disposition)).unwrap(), payload: json!({}) });
    for disposition in ["deferred", "unknown"] {
        let (sent, _) = classify(&codex("/skill revisar", false), &reply(disposition));
        let (status, body) = input_answer(&sent, false);
        assert_eq!((status.as_u16(), body["ok"].clone(), body["delivered"].clone()), (200, json!(true), json!(false)), "{disposition}");
    }
    // Recado de sessão-irmã (`steer: true`) orienta o turno em curso do Codex com terminal, como o `steer_queue` do Python.
    let (sent, _) = classify(&codex("olha isto", true), &reply("deferred"));
    assert!(wants_steer_queue(&codex("olha isto", true), &sent));
}

#[test]
fn steer_answers_match_the_python_golden() {
    let cases = golden("steer.json");
    assert!(cases.len() >= 19);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let control = reply(&case["reply"]);
        let (status, body) = if case["terminal"].as_bool().unwrap() {
            match steer_terminal_control(&control) {
                Err(refused) => refused,
                Ok(promoted) => steer_terminal_done(promoted, &match &case["confirm"] {
                    Value::Null => Ok(json!({})),
                    text if text.is_string() => reply(text).map(|_| json!({})),
                    done => Ok(done.clone()),
                }),
            }
        } else {
            steer_headless(!case["text"].is_null(), case["provider"] == "codex", &control)
        };
        assert_eq!((status.as_u16(), body), (case["expect"]["status"].as_u64().unwrap() as u16, case["expect"]["body"].clone()), "{name}");
    }
}

/// Cano Claude falso: confirma o que recebe com o `outcome` dado (`written`, `unknown`, `not_written`).
async fn cano(outcome: &'static str) -> String {
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
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":outcome});
                    let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                }
            });
        }
    });
    format!("tcp:{address}")
}

async fn policy() -> std::net::SocketAddr {
    hangar_server::diag::disable_diary();
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

pub(crate) async fn registry() -> Arc<RuntimeRegistry> {
    Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))))
}

async fn open_headless(registry: &RuntimeRegistry, dir: &Path, name: &str, outcome: &'static str) {
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(outcome).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
}

/// Entrada com terminal e uma linha parada na fila, cujos fatos o Python falso não sabe dar: a
/// manutenção falha e o retrato dela passa a dizer `terminal_facts` (o vínculo não se confirma).
pub(crate) async fn open_sick_terminal(registry: &RuntimeRegistry, dir: &Path, name: &str) {
    let (state_path, projection_dir) = (dir.join(format!("{name}.state")), dir.join(format!("{name}.projection")));
    let mut store = Store::open(&state_path, &projection_dir, State::new(&format!("k-{name}"), 1, name, vec![])).unwrap();
    let clock = ClockSample { monotonic_s: 0.0, epoch_s: 1.0 };
    store.exec(1, "append", clock, Action::Append { text: "parada".into(), delivered: false, ts: None, pre_transcript: false, entry_id: Some("e1".into()) }).unwrap();
    drop(store);
    let binding = TerminalBinding { name: name.into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec!["/does-not-exist/hangar-test-tmux".into()], windows: false, clipboard_lock_path: None };
    let transcript = dir.join(format!("{name}.jsonl"));
    std::fs::write(&transcript, "").unwrap();
    registry.open_terminal(TerminalTarget { key: format!("k-{name}"), generation: 1, name: name.into(), binding,
        lease_path: dir.join(format!("{name}.lease")), state_path, projection_dir, transcript, created: 0.0, plugin_key: None,
        metadata: hangar_server::terminal_facts::TerminalMetadata { config_dir:dir.into(),agent_pid:std::process::id(),agent_birth:hangar_server::terminal_facts::agent_birth(std::process::id()).unwrap(),legacy_import_after:None,fingerprint:"fixture".into(),origin:"uds:fixture".into() } }).await.unwrap();
    for _ in 0..250 {
        if !registry.writable(name).await.unwrap().healthy { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("a entrada de terminal não ficou doente");
}

async fn serve(registry: Arc<RuntimeRegistry>, wait: Duration) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.diag = python.bind_diary(state.diag.clone());
    state.write_gate_wait = wait;
    assert!(state.state.runtime.set(registry).is_ok());
    (python, spawn_state(state).await)
}

async fn post(server: std::net::SocketAddr, name: &str, route: &str, body: &str) -> (u16, Value) {
    let response = client().post(format!("http://{server}/api/sessions/{name}/{route}"))
        .header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}"))
        .body(body.to_owned()).send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn diary_of(python: &Fake, event: &str) -> Vec<Value> {
    crate::fake::wait_diary(python.diary_dir(), |row| row["evento"] == event).await.into_iter().filter(|row| row["evento"] == event).collect()
}

#[tokio::test]
async fn headless_input_is_served_in_rust_without_reaching_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, body) = post(server, "s", "input", r#"{"text":"oi"}"#).await;
    assert_eq!((status, body), (200, json!({"ok": true, "delivered": true, "steered": false, "native": false})));
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0);
}

/// Recado `[de: …]` para sessão sem terminal: o Rust o escreve no socket de mensagens do próprio Claude,
/// achado pelo registro do CLI na pasta da conta, sem passar pelo Python nem pelo cano.
#[cfg(unix)]
#[tokio::test]
async fn headless_native_message_goes_to_the_claude_socket_without_python() {
    use tokio::io::AsyncReadExt;
    // A classe do recado sai da memória de modo: a conversa confirmada em bypass.
    let memory = Arc::new(std::sync::Mutex::new(hangar_server::state::permission::PermissionMemory::default()));
    memory.lock().unwrap().observe("sid-1", "bypassPermissions");
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_permissions(memory)
        .with_native_inbox_dir(dir.path().into()));
    registry.bind_lifecycle();
    let socket = dir.path().join("claude.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let frames = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).await.unwrap();
        serde_json::from_str::<Value>(raw.trim()).unwrap()
    });
    let sessions = dir.path().join("conta/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("1.json"), json!({"pid": std::process::id(), "sessionId": "sid-1", "messagingSocketPath": socket}).to_string()).unwrap();
    registry.open(RuntimeTarget { key: "k-s".into(), generation: 1, name: "s".into(), provider: "claude".into(),
        metadata: json!({"name": "s", "headless": true, "session_id": "sid-1", "initialized": true, "config_dir": dir.path().join("conta")}),
        binding: CanoBinding { pid: 42, escuta: cano("not_written").await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.path().join("s.lock"), state_path: dir.path().join("s.queue-state.json"),
        projection_dir: dir.path().join("s-projection"), transcript: dir.path().join("s.jsonl"), created: 0.0 }).await.unwrap();
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, body) = post(server, "s", "input", r#"{"text":"[de: par] oi"}"#).await;
    assert_eq!((status, body["native"].as_bool()), (200, Some(true)), "{body}");
    let frame = tokio::time::timeout(Duration::from_secs(5), frames).await.unwrap().unwrap();
    let content = frame["message"]["content"].as_str().unwrap();
    assert!(content.contains("from-name=\"par\"") && content.contains("from-mode=\"bypass\"") && content.contains("[de: par] oi"), "{content}");
    assert!(frame["msg_id"].as_str().is_some_and(|id| id.len() == 36), "{frame}");
    assert!(python.diag().iter().all(|d| d["evento"] != "runtime.send_failed"), "{:?}", python.diag());
    // O receptor recusa: o recibo volta à caixa do Rust (o `from` do recado) e encerra a operação dele.
    let inbox = frame["from"].as_str().and_then(|from| from.strip_prefix("uds:")).expect("from é a caixa do Rust").to_owned();
    assert!(inbox.starts_with(&*dir.path().to_string_lossy()), "{inbox}");
    let receipt = json!({"type": "peer_message_status", "orig_msg_id": frame["msg_id"], "state": "refused"});
    let mut stream = tokio::net::UnixStream::connect(&inbox).await.unwrap();
    stream.write_all(format!("{receipt}\n").as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    let state = dir.path().join("s.queue-state.json");
    for _ in 0..250 {
        let raw = std::fs::read(&state).unwrap_or_default();
        if serde_json::from_slice::<Value>(&raw).is_ok_and(|state| state["operations"].as_object().is_some_and(|ops| ops.values()
            .any(|op| op["status"] == "rejected" && op["result"]["payload"]["native_status"] == "refused"))) { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("o recibo de recusa não encerrou a operação do recado: {}", String::from_utf8_lossy(&std::fs::read(&state).unwrap_or_default()));
}

/// A classe do recado não se lê (sem memória de modo): nada foi escrito no socket, então o recado sai pelo cano
/// como texto comum, em vez de ficar incerto e preso na fila.
#[tokio::test]
async fn headless_native_message_without_a_readable_class_goes_through_the_cano() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry().await;
    registry.bind_lifecycle();
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, body) = post(server, "s", "input", r#"{"text":"[de: par] oi"}"#).await;
    assert_eq!((status, body["delivered"].as_bool(), body["native"].as_bool()), (200, Some(true), Some(false)), "{body}");
    assert!(python.diag().iter().all(|d| d["evento"] != "runtime.send_failed"), "{:?}", python.diag());
}

#[tokio::test]
async fn headless_input_the_cano_never_confirmed_is_an_error_with_a_code_and_a_diary_line() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "unknown").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, body) = post(server, "s", "input", r#"{"text":"oi"}"#).await;
    assert_eq!(status, 400);
    assert_eq!(body["detail"]["code"], "erro_envio_falhou");
    assert_eq!(body["detail"]["msg"], "resultado incerto; entrada conservada sem reenvio");
    let diary = diary_of(&python, "runtime.send_failed").await;
    assert_eq!(diary.len(), 1, "{:?}", python.diag());
    assert_eq!(diary[0]["sessao"], "s");
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0, "dono único: a falha não vira repasse");
}

#[tokio::test]
async fn headless_input_the_cano_refused_is_a_400() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "not_written").await;
    let (_python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, body) = post(server, "s", "input", r#"{"text":"oi"}"#).await;
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (400, Some("erro_envio_falhou"), Some("entrada recusada pelo runtime")));
}

#[tokio::test]
async fn bodies_the_route_refuses_are_a_rust_422_never_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    for body in [r#"{"text":"oi","extra":1}"#, r#"{"steer":true}"#, r#"{"text":5}"#, r#"{"text":"oi","steer":"sim"}"#] {
        let (status, reply) = post(server, "s", "input", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}");
    }
    // Sem `application/json` o corpo não é lido como JSON: a recusa também é daqui.
    let response = client().post(format!("http://{server}/api/sessions/s/input")).header("content-type", "text/plain")
        .header("authorization", format!("Bearer {OWNER}")).body(r#"{"text":"oi"}"#).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 422);
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0);
}

#[tokio::test]
async fn deferred_submit_with_steer_promotes_that_very_entry() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    // O primeiro envio abre o turno e o cano nunca o termina: o segundo fica adiado na fila.
    assert_eq!(post(server, "s", "input", r#"{"text":"um"}"#).await.1["delivered"], true);
    let (status, body) = post(server, "s", "input", r#"{"text":"dois","steer":true}"#).await;
    assert_eq!((status, &body), (200, &json!({"ok": true, "delivered": true, "steered": true, "native": false})), "{body}");
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0);
}

#[test]
fn steer_without_body_that_steered_an_entry_keeps_the_fields_old_clients_read() {
    use hangar_server::runtime::protocol::Disposition;
    let r = |disposition, payload: Value| Ok(RuntimeReply { operation_id: OP.into(), disposition, payload });
    // Sem `entry_id`: nada esperava no Hangar, e a rota segue o caminho de antes.
    assert!(steer_first_answer(&r(Disposition::Accepted, json!({"promoted": true}))).is_none());
    assert!(steer_first_answer(&Err(RuntimeError::new("terminal_io", "x"))).is_none());
    let (status, body) = steer_first_answer(&r(Disposition::Accepted, json!({"steered": true, "drained": 1, "entry_id": "a"}))).unwrap();
    assert_eq!((status.as_u16(), body),
        (200, json!({"ok": true, "steered": true, "promoted": false, "confirmed": 1, "queued_ids": ["a"]})));
    // Parado e sem entregar agora: ficou na frente, nada confirmado; nunca `promoted: true`.
    let (_, body) = steer_first_answer(&r(Disposition::Accepted, json!({"steered": false, "drained": 0, "entry_id": "a"}))).unwrap();
    assert_eq!((body["promoted"].clone(), body["confirmed"].clone(), body["queued_ids"].clone()), (json!(false), json!(0), json!(["a"])));
    // Parado e entregue na hora: conta como confirmada.
    let (_, body) = steer_first_answer(&r(Disposition::Accepted, json!({"steered": false, "drained": 1, "entry_id": "a"}))).unwrap();
    assert_eq!(body["confirmed"], 1);
    let (status, body) = steer_first_answer(&r(Disposition::Deferred, json!({"code": "queue_steer_not_delivered", "entry_id": "a"}))).unwrap();
    assert_eq!((status.as_u16(), body["detail"]["code"].as_str()), (409, Some("queue_steer_not_delivered")));
}

#[tokio::test]
async fn headless_steer_with_text_is_served_in_rust() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    assert_eq!(post(server, "s", "steer", r#"{"text":"vira"}"#).await, (200, json!({"ok": true, "promoted": false})));
    assert_eq!(python.hits_to("/api/sessions/s/steer"), 0);
}

#[tokio::test]
async fn headless_steer_of_the_queue_without_a_turn_is_409_erro_sem_turno() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, body) = post(server, "s", "steer", "").await;
    // O ator recusa (sem turno em voo): o Python respondia 500; aqui é o 409 que a rota passou a ter.
    assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["msg"].as_str()),
        (409, Some("erro_sem_turno"), Some("Não há turno em andamento para orientar")), "{body}");
    assert_eq!(python.hits_to("/api/sessions/s/steer"), 0);
}

#[tokio::test]
async fn steer_with_a_body_the_route_refuses_is_a_rust_422() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    for body in [r#"{}"#, r#"{"text":"a","x":1}"#] {
        let (status, reply) = post(server, "s", "steer", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}");
    }
    assert_eq!(python.hits_to("/api/sessions/s/steer"), 0);
}

#[tokio::test]
async fn unhealthy_terminal_entry_is_a_rust_error_never_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_sick_terminal(&registry, dir.path(), "t").await;
    let target = registry.writable("t").await.unwrap();
    assert!(target.terminal && !target.healthy, "a entrada com o vínculo trocado está doente");
    let snapshot = registry.snapshots().await.unwrap().into_iter().find(|e| e.key == "k-t").unwrap();
    assert_eq!(snapshot.data["error"], "terminal_facts", "doente pelo motivo do vínculo, não por outro");
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    let (status, reply) = post(server, "t", "input", r#"{"text":"oi"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (503, Some("runtime_unhealthy")));
    assert_eq!(python.hits_to("/api/sessions/t/input"), 0, "o Python não relança nem responde pela entrada doente");
}

#[tokio::test]
#[cfg(unix)]
async fn clear_on_a_terminal_session_goes_to_the_rust_actor_never_python() {
    let (dir, interactions) = (tempfile::tempdir().unwrap(), Arc::new(hangar_server::interactions::InteractionStore::default()));
    let (registry, _plugin) = crate::session_write_control::registry_with_plugin(&interactions).await;
    let log = crate::session_write_control::open_terminal(&registry, dir.path(), "t").await;
    open_headless(&registry, dir.path(), "h", "written").await;
    for name in ["t", "h"] { registry.ingress().close(name, Duration::from_secs(1)).await.unwrap(); }
    let (python, server) = serve(registry.clone(), Duration::from_millis(100)).await;
    // Porta fechada: o /clear espera como qualquer texto, com e sem terminal; nenhum atalho ao Python.
    for name in ["t", "h"] {
        assert_eq!(post(server, name, "input", r#"{"text":"  /clear tudo"}"#).await.0, 409, "{name}");
    }
    // Porta aberta: o ator do terminal digita o /clear (a barreira e a conversa nova são dele).
    registry.ingress().open("t");
    let (status, reply) = post(server, "t", "input", r#"{"text":"/clear"}"#).await;
    assert_ne!(reply, Value::String("from-python".into()), "{status}");
    assert!(std::fs::read_to_string(&log).unwrap_or_default().contains("/clear"), "o /clear chegou ao pane pelo ator");
    assert_eq!(python.hits_to("/api/sessions/t/input") + python.hits_to("/api/sessions/h/input"), 0, "nada passa pelo Python");
}

#[test]
fn codex_compact_is_a_control_of_the_engine() {
    // `/compact` do Codex é controle do motor (argumento é recusado com 400); texto nunca vira turno.
    assert!(is_codex_compact("codex", "/compact") && is_codex_compact("codex", "  /compact agora"));
    assert!(!is_codex_compact("claude", "/compact") && !is_codex_compact("codex", "oi /compact"));
}
