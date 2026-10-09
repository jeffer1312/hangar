//! `/input` e `/steer` de sessão Claude servidos pelo Rust: o corpo e o código de cada resposta do ator
//! vêm do golden que o Python gera (`gen_golden.py`), e o resto prova a rota inteira com um cano falso.
mod fake;

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
            terminal: case["terminal"].as_bool().unwrap(), operation_id: OP };
        let (sent, diary) = classify(&params, &reply(&case["reply"]));
        let steered = wants_steer_queue(&params, &sent) && was_steered(OP, &reply(&case["queue"]));
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

async fn open_headless(registry: &RuntimeRegistry, dir: &Path, name: &str, outcome: &'static str) {
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta: cano(outcome).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
}

/// Entrada com terminal e uma linha parada na fila, cujos fatos o Python falso não sabe dar: a
/// manutenção falha e o retrato dela passa a dizer `terminal_facts` (o vínculo não se confirma).
async fn open_sick_terminal(registry: &RuntimeRegistry, dir: &Path, name: &str) {
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
        lease_path: dir.join(format!("{name}.lease")), state_path, projection_dir, transcript, created: 0.0, plugin_key: None }).await.unwrap();
    for _ in 0..250 {
        if !registry.writable(name).await.unwrap().healthy { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("a entrada de terminal não ficou doente");
}

async fn serve(registry: Arc<RuntimeRegistry>, wait: Duration) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
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
    for _ in 0..100 {
        let found: Vec<Value> = python.diag().into_iter().filter(|d| d["evento"] == event).collect();
        if !found.is_empty() { return found; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Vec::new()
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
async fn bodies_python_would_refuse_still_reach_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    for body in [r#"{"text":"oi","extra":1}"#, r#"{"steer":true}"#, r#"{"text":5}"#, r#"{"text":"oi","steer":"sim"}"#] {
        assert_eq!(post(server, "s", "input", body).await, (200, "from-python".into()), "{body}");
        assert_eq!(python.last_body(), body.as_bytes());
    }
    // Sem `application/json` o FastAPI não lê o corpo como JSON: quem recusa é ele.
    let response = client().post(format!("http://{server}/api/sessions/s/input")).header("content-type", "text/plain")
        .header("authorization", format!("Bearer {OWNER}")).body(r#"{"text":"oi"}"#).send().await.unwrap();
    assert_eq!(response.text().await.unwrap(), "from-python");
    assert_eq!(python.hits_to("/api/sessions/s/input"), 5);
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
async fn steer_with_a_body_python_would_refuse_reaches_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_headless(&registry, dir.path(), "s", "written").await;
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    for body in [r#"{}"#, r#"{"text":"a","x":1}"#] {
        assert_eq!(post(server, "s", "steer", body).await, (200, "from-python".into()), "{body}");
    }
    assert_eq!(python.hits_to("/api/sessions/s/steer"), 2);
}

#[tokio::test]
async fn unhealthy_terminal_entry_sends_input_to_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_sick_terminal(&registry, dir.path(), "t").await;
    let target = registry.writable("t").await.unwrap();
    assert!(target.terminal && !target.healthy, "a entrada com o vínculo trocado está doente");
    let snapshot = registry.snapshots().await.unwrap().into_iter().find(|e| e.key == "k-t").unwrap();
    assert_eq!(snapshot.data["error"], "terminal_facts", "doente pelo motivo do vínculo, não por outro");
    let (python, server) = serve(registry, Duration::from_secs(5)).await;
    assert_eq!(post(server, "t", "input", r#"{"text":"oi"}"#).await, (200, "from-python".into()));
    assert_eq!(python.hits_to("/api/sessions/t/input"), 1);
}

#[tokio::test]
async fn clear_on_a_terminal_session_is_forwarded_before_the_gate() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_sick_terminal(&registry, dir.path(), "t").await;
    open_headless(&registry, dir.path(), "h", "written").await;
    for name in ["t", "h"] { registry.ingress().close(name, Duration::from_secs(1)).await.unwrap(); }
    let (python, server) = serve(registry, Duration::from_millis(100)).await;
    // Terminal: o /clear segue ao Python com a porta fechada, sem esperá-la nem responder 409.
    assert_eq!(post(server, "t", "input", r#"{"text":"  /clear tudo"}"#).await, (200, "from-python".into()));
    assert_eq!(python.hits_to("/api/sessions/t/input"), 1);
    // Texto comum e /clear de sessão sem terminal esperam a porta, que segue fechada: 409.
    assert_eq!(post(server, "t", "input", r#"{"text":"oi"}"#).await.0, 409);
    assert_eq!(post(server, "h", "input", r#"{"text":"/clear"}"#).await.0, 409);
    assert_eq!(python.hits_to("/api/sessions/h/input"), 0);
}

#[test]
fn codex_compact_goes_to_the_python_control() {
    // O Python manda `/compact` do Codex como controle (e recusa argumento com 400); texto nunca vira turno.
    assert!(relays_codex_input("codex", "/compact") && relays_codex_input("codex", "  /compact agora"));
    assert!(!relays_codex_input("claude", "/compact") && !relays_codex_input("codex", "oi /compact"));
}
