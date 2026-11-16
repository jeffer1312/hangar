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
pub(crate) async fn cano_with(kill: Option<Arc<tokio::sync::Notify>>) -> String {
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
async fn policy() -> std::net::SocketAddr { policy_seen(Arc::default()).await }

/// O mesmo, guardando a linha de cada pedido (método e caminho).
async fn policy_seen(seen: Arc<std::sync::Mutex<Vec<String>>>) -> std::net::SocketAddr {
    hangar_server::diag::disable_diary();
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut length = 0usize;
                    let mut first = true;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        if std::mem::take(&mut first) { seen.lock().unwrap().push(line.trim_end().to_owned()); }
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

pub(crate) async fn open_entry_with(registry: &RuntimeRegistry, dir: &std::path::Path, key: &str, name: &str, generation: u64, escuta: String) {
    registry.open(RuntimeTarget { key: key.into(), generation, name: name.into(), provider: "claude".into(),
        metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
        binding: CanoBinding { pid: 42, escuta, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{key}.lock")), state_path: dir.join(format!("{key}.queue-state.json")),
        projection_dir: dir.join(format!("{key}-projection")), transcript: dir.join(format!("{key}.jsonl")), created: 0.0 }).await.unwrap();
}

async fn registry() -> Arc<RuntimeRegistry> {
    crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))))
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

// Só as que aceitam POST: `models` é só GET (a leitura vai no laço de baixo), e um POST nela não é escrita.
const CODEX_ROUTES: [&str; 7] = ["model", "codex/mode", "service-tier", "question/skip", "codex-permissions", "codex/plan/implement", "codex/terminate"];

/// Cada escrita do dono (POST das rotas de escrita e do Codex, mais fila e leituras do Codex), com a
/// resposta que voltou; depois confere que nada chegou ao Python.
async fn every_write(python: &Fake, server: std::net::SocketAddr) -> Vec<(String, (u16, Value))> {
    let mut answers = Vec::new();
    let parse = |(status, text): (u16, String)| (status, serde_json::from_str(&text).unwrap_or(Value::String(text)));
    for route in ROUTES.iter().chain(&CODEX_ROUTES) {
        answers.push((format!("POST {route}"), parse(post(server, route, r#"{"text":"oi","x":1}"#, OWNER).await)));
    }
    for (method, route) in [(reqwest::Method::DELETE, "queue/e1"), (reqwest::Method::PATCH, "queue/e1"), (reqwest::Method::GET, "limits"), (reqwest::Method::GET, "models")] {
        let response = client().request(method.clone(), format!("http://{server}/api/sessions/s/{route}"))
            .header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}")).body(r#"{"text":"oi"}"#).send().await.unwrap();
        answers.push((format!("{method} {route}"), parse((response.status().as_u16(), response.text().await.unwrap()))));
    }
    for route in ROUTES.iter().chain(&CODEX_ROUTES).chain(&["queue/e1", "limits", "models"]) {
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{route}")), 0, "{route}: nada chega ao Python");
    }
    answers
}

/// R2-27 e achado 1 da revisão: servidor que subiu sem o registro de sessões (sem `runtime_instance`)
/// nunca o terá; a escrita é 503 `runtime_unavailable`, código que não pede repetição.
#[tokio::test]
async fn without_a_runtime_every_write_is_runtime_unavailable_and_never_python() {
    let (python, server) = serve_with(None).await;
    for (route, (status, body)) in every_write(&python, server).await {
        assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("runtime_unavailable")), "{route}: {body}");
    }
}

/// Guarda da R2-27 (decisão 8): na subida em curso a porta é do `startup::guard`, com o 503
/// `server_starting` que o cliente repete, antes de qualquer rota de escrita e sem ir ao Python. Já valia
/// antes da R2; separa a subida do servidor sem registro (`runtime_unavailable`, o teste acima).
#[tokio::test]
async fn while_starting_every_write_is_server_starting_and_never_python() {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.startup = hangar_server::startup::Readiness::pending();
    assert!(state.state.runtime.set(registry().await).is_ok());
    let server = spawn_state(state).await;
    for (route, answer) in every_write(&python, server).await {
        assert_eq!(answer, (503, json!({"detail": "server_starting"})), "{route}");
    }
}

// R2-27 (`commands/service.rs:175`): a lista de comandos do Codex (arquivo de sessão Codex no disco) é 503
// com código na subida e sem o registro de sessões, nunca o Python.
#[tokio::test]
async fn codex_commands_while_starting_and_without_a_runtime_are_503_with_a_code() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".hangar/codex-sessions")).unwrap();
    std::fs::write(dir.path().join(".hangar/codex-sessions/s.json"), r#"{"name":"s","provider":"codex","headless":true}"#).unwrap();
    for starting in [true, false] {
        let (python, upstream) = spawn_fake().await;
        let mut state = AppState::new(config(upstream, "127.0.0.1"));
        state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
            [("HOME".to_owned(), dir.path().to_string_lossy().into_owned()), ("PATH".to_owned(), String::new())].into()));
        if starting {
            state.startup = hangar_server::startup::Readiness::pending();
            assert!(state.state.runtime.set(registry().await).is_ok());
        }
        let server = spawn_state(state).await;
        let response = client().get(format!("http://{server}/api/sessions/s/commands")).header("authorization", format!("Bearer {OWNER}")).send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        let code = if starting { body["detail"].as_str() } else { body["detail"]["code"].as_str() };
        assert_eq!((status, code), (503, Some(if starting { "server_starting" } else { "runtime_unavailable" })), "{body}");
        assert_eq!(python.hits_to("/api/sessions/s/commands"), 0);
    }
}

#[tokio::test]
async fn open_entry_with_a_body_the_route_refuses_is_a_rust_422_never_python() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    let (python, server) = serve_with(Some(registry)).await;
    // Campo a mais e corpo que não é JSON de objeto: a recusa é do Rust, com código.
    for body in [r#"{"text":"oi","x":1}"#, "não é json"] {
        let (status, text) = post(server, "input", body, OWNER).await;
        let reply: Value = serde_json::from_str(&text).unwrap();
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}");
    }
    assert_eq!(python.hits_to("/api/sessions/s/input"), 0);
}

/// Decisão 5: quem não pode na sessão recebe o 401 do Rust na hora, sem esperar a porta e sem chegar
/// ao Python, que não tem mais as rotas.
#[tokio::test]
async fn stranger_gets_the_rust_401_at_once_even_with_the_gate_closed() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve_with(Some(registry)).await;
    let unauthorized = (401, r#"{"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}"#.to_owned());
    for route in ROUTES {
        let answer = tokio::time::timeout(Duration::from_secs(2), post(server, route, r#"{"text":"oi"}"#, "token-errado")).await;
        assert_eq!(answer.unwrap(), unauthorized, "{route}");
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{route}")), 0, "{route}");
    }
}

#[tokio::test]
async fn gate_reopened_in_time_lets_the_request_through() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve_with(Some(registry.clone())).await;
    let reopen = registry.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(50)).await; reopen.ingress().open("s"); });
    // A porta reabriu e o Rust atendeu: corpo sem `answers` é a recusa dele, não o Python.
    let (status, text) = post(server, "answer", "{}", OWNER).await;
    assert_eq!(status, 422, "{text}");
    assert_eq!(python.hits_to("/api/sessions/s/answer"), 0);
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

/// Btw B2 (`api.py:404`): troca de conta em curso é 409 antes do corpo, e nada chega ao Python.
#[tokio::test]
async fn btw_with_the_gate_closed_is_409_busy_before_the_body() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.write_gate_wait = Duration::from_millis(100);
    assert!(state.state.runtime.set(registry).is_ok());
    let server = spawn_state(state).await;
    let (status, text) = post(server, "btw", "não é json", OWNER).await;
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("session_transfer_busy")));
    assert_eq!(python.hits_to("/api/sessions/s/btw"), 0);
}

/// Btw B6: com a porta aberta, o corpo fora do modelo é o 422 do Rust, sem o Python.
#[tokio::test]
async fn btw_body_outside_the_model_is_a_rust_422() {
    let (dir, registry) = (tempfile::tempdir().unwrap(), registry().await);
    open_entry(&registry, dir.path(), "k", "s").await;
    let (python, server) = serve_with(Some(registry)).await;
    let (status, text) = post(server, "btw", r#"{"question":"q","x":1}"#, OWNER).await;
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!((status, body["detail"][0]["type"].as_str()), (422, Some("extra_forbidden")));
    assert_eq!(python.hits_to("/api/sessions/s/btw"), 0);
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
    // Sem entrada: o envio a sessão sem dono não segura o passe (`session_write_unowned`).
    // Doente e corpo que a rota recusa: o Rust responde com código, nunca repassa, e solta o passe.
    for (name, body, status, code) in [("doente", r#"{"text":"oi"}"#, 503, "runtime_unhealthy"), ("s", r#"{"text":"oi","x":1}"#, 422, "erro_corpo_invalido")] {
        let (got, text) = post_to(server, name, "input", body).await;
        let reply: Value = serde_json::from_str(&text).unwrap();
        assert_eq!((got, reply["detail"]["code"].as_str()), (status, Some(code)), "{name}");
        assert_eq!(python.hits_to(&format!("/api/sessions/{name}/input")), 0, "{name}: nunca ao Python");
        assert!(registry.ingress().close(name, Duration::from_millis(300)).await.is_ok(), "{name}: passe solto");
        registry.ingress().open(name);
    }
}

async fn post_to(server: std::net::SocketAddr, name: &str, route: &str, body: &str) -> (u16, String) {
    let response = client().post(format!("http://{server}/api/sessions/{name}/{route}"))
        .header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}"))
        .body(body.to_owned()).send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

/// Parity A3: escrever para a sessão doente a reabre aqui, sem pedir ao Python (o 503 fica). Sem o
/// sidecar dela na casa do teste, a reabertura falha visível no diário e a vida doente sai.
#[tokio::test]
async fn unhealthy_write_reopens_the_session_in_rust() {
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let diary = dir.path().join("diário");
    std::fs::create_dir_all(&diary).unwrap();
    let registry = crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into())
        .with_diagnostics(hangar_server::diag::DiagClient::new().with_diary(Some(diary.clone())))
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1")))
        .with_codex_environment(hangar_server::accounts::environment::AccountEnvironment::from_map(
            [("HOME".to_owned(), home.path().to_string_lossy().into_owned())].into())));
    let kill = Arc::new(tokio::sync::Notify::new());
    open_entry_with(&registry, dir.path(), "d", "doente", 1, cano_with(Some(kill.clone())).await).await;
    kill.notify_one();
    for _ in 0..100 {
        if !registry.writable("doente").await.unwrap().healthy { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (_python, server) = serve_with(Some(registry.clone())).await;
    let (status, _) = post_to(server, "doente", "input", r#"{"text":"oi"}"#).await;
    assert_eq!(status, 503);
    let recorded = crate::fake::wait_diary(&diary, |row| row["sessao"] == "doente" && row["evento"] == "rust.runtime_reopen_failed").await;
    assert!(registry.writable("doente").await.is_none(), "a vida doente fechou para reabrir");
    let line = recorded.iter().find(|row| row["sessao"] == "doente" && row["evento"] == "rust.runtime_reopen_failed").unwrap();
    assert!(line["codigo"].as_str().is_some_and(|code| !code.is_empty()));
    assert_eq!(line["nivel"], "erro");
    let seen = seen.lock().unwrap();
    assert!(!seen.iter().any(|line| line == "POST /internal/diag HTTP/1.1"));
    assert!(!seen.iter().any(|line| line.contains("/reopen")), "nada vai ao coordenador do Python: {seen:?}");
}

/// R2-28: as combinações que a tabela entregava ao Python e que nenhuma sessão alcança (Pi/omp/Kimi sem
/// terminal; controle do Codex numa sessão Claude) agora são recusa do Rust, e a posse anunciada na
/// saúde (`owns`) não muda.
#[test]
fn table_hands_nothing_to_python_and_the_announced_ownership_stays() {
    use hangar_server::session_write::{Owner, Provider, WriteRoute, decide, table::owned_modes};
    for provider in [Provider::Pi, Provider::Omp, Provider::Kimi] {
        assert_ne!(decide(WriteRoute::Input, provider, false), Owner::Rust, "{provider:?} sem terminal não existe");
        assert_eq!(decide(WriteRoute::Input, provider, true), Owner::Rust, "{provider:?}");
    }
    for terminal in [true, false] {
        assert_ne!(decide(WriteRoute::CodexControl, Provider::Claude, terminal), Owner::Rust);
        for route in [WriteRoute::Keys, WriteRoute::TermInput, WriteRoute::SelectSubmit] {
            assert_eq!(decide(route, Provider::Claude, terminal), Owner::Rust, "{route:?}: a rota recusa sem terminal");
        }
    }
    assert_eq!(owned_modes(), vec![(Provider::Claude, true), (Provider::Claude, false), (Provider::Codex, true), (Provider::Codex, false),
        (Provider::Pi, false), (Provider::Omp, false), (Provider::Kimi, false)]);
}
