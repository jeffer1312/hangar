//! `POST /api/sessions/{name}/model-effort` do Claude sem conta ChatGPT fixa: o Rust atende com e sem
//! terminal, com as respostas da rota Python, e nada chega ao Python.
use std::sync::Arc;
use std::time::Duration;

use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::term::{TermConfig, Terms};
use serde_json::{Value, json};

use crate::fake::{self, Fake, SECRET};
use crate::session_write_control::{open_headless, policy, send};
#[cfg(unix)]
use crate::session_write_control::{keys_sent, open_terminal};

async fn registry(terms: &Arc<Terms>) -> Arc<RuntimeRegistry> {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    Arc::new(RuntimeRegistry::new(policy(calls).await, "secret-test".into(), "instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_terms(terms.clone()))
}

/// O servidor do dono; `panel`: a sessão com o painel de terminal aberto.
async fn serve(registry: Arc<RuntimeRegistry>, terms: Arc<Terms>, panel: Option<&str>, gate_wait: Duration) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    state.term = terms;
    state.write_gate_wait = gate_wait;
    if let Some(name) = panel { state.term.mark_open_for_test(name, &state.diag).await; }
    assert!(state.state.runtime.set(registry).is_ok());
    (python, fake::spawn_state(state).await)
}

/// O servidor do dono sem vigia, com a lista no tmux falso de `n` sessões Claude (`s0`…) e nas pastas do
/// teste; `panel`: a sessão com o painel de terminal aberto.
#[cfg(unix)]
async fn serve_listed(root: &std::path::Path, n: usize, panel: Option<&str>) -> (Arc<Fake>, std::net::SocketAddr) {
    use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
    let script = crate::list_support::sessions(root, n);
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"), "omp_agent": home.join(".omp/agent"),
        "kimi_home": home.join(".kimi-code")}).to_string());
    let terms = Arc::new(Terms::new(TermConfig::default()));
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: hangar_server::list::mux::Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        hangar_server::list::facts::FactsClient::new(upstream, SECRET.into())));
    state.term = terms.clone();
    if let Some(name) = panel { state.term.mark_open_for_test(name, &state.diag).await; }
    assert!(state.state.runtime.set(registry(&terms).await).is_ok());
    (python, fake::spawn_state(state).await)
}

async fn post(server: std::net::SocketAddr, name: &str, body: &str) -> (u16, Value) { send(server, "POST", &format!("{name}/model-effort"), body).await }

/// B1: corpo estrito (campo a mais, `scope` fora da lista, tipo errado) é 422 do Rust.
#[tokio::test]
async fn model_effort_b1_strict_body_is_a_rust_422() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry, terms, None, Duration::from_secs(5)).await;
    for body in [r#"{"model":"sonnet","x":1}"#, r#"{"model":"sonnet","scope":"global"}"#, r#"{"model":5}"#, "[]", "x"] {
        let (status, reply) = post(server, "s", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}: {reply}");
    }
    assert_eq!(python.hits_to("/api/sessions/s/model-effort"), 0);
}

/// B5: sem terminal, a validação do `model_args` em texto cru e a troca pelo ator, `scope` ignorado.
#[tokio::test]
async fn model_effort_b5_headless_validates_and_sets_through_the_actor() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    let sink = open_headless(&registry, dir.path(), "s").await;
    let (python, server) = serve(registry, terms, None, Duration::from_secs(5)).await;
    assert_eq!(post(server, "s", r#"{"model":"bad id!"}"#).await,
        (422, json!({"detail": "model: use letras, números e . _ : / - ~ [ ] (até 128 caracteres, sem começar com -)"})));
    assert_eq!(post(server, "s", r#"{"effort":"ultracode"}"#).await, (422, json!({"detail": "effort: use um de low, medium, high, xhigh, max"})));
    // Nada pedido passa do `validar` e o ator recusa: o 409 do `except` da rota Python.
    let (status, reply) = post(server, "s", "{}").await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_modelo_indisponivel")), "{reply}");
    assert!(reply["detail"]["msg"].as_str().unwrap().starts_with("não consegui trocar: "), "{reply}");
    assert_eq!(post(server, "s", r#"{"model":"sonnet","scope":"default"}"#).await, (200, json!({"ok": true, "scope": "session", "result": null})));
    let frames: Vec<Value> = sink.lock().unwrap().iter().filter_map(|e| serde_json::from_str(e["frame"].as_str()?).ok()).collect();
    assert!(frames.iter().any(|f| f["request"]["subtype"] == "set_model" && f["request"]["model"] == "sonnet"), "{frames:?}");
    assert_eq!(python.hits_to("/api/sessions/s/model-effort"), 0);
}

/// B6: painel de terminal aberto recusa antes de qualquer tecla.
#[cfg(unix)]
#[tokio::test]
async fn model_effort_b6_open_terminal_panel_refuses() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry, terms, Some("t"), Duration::from_secs(5)).await;
    let (status, reply) = post(server, "t", r#"{"model":"sonnet"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str(), reply["detail"]["msg"].as_str()),
        (409, Some("erro_terminal_aberto"), Some("Terminal aberto nesta sessao. Feche o painel pra responder por aqui.")), "{reply}");
    assert_eq!(keys_sent(&log), 0);
    assert_eq!(python.hits_to("/api/sessions/t/model-effort"), 0);
}

/// R1.4 (D6, D7): sessão que não é Claude (Pi, omp, Kimi, nenhuma) recusa com código, sem tecla e sem
/// repasse; o corpo fora de forma vem antes, como a validação do pydantic antes do handler Python. As
/// capturas `pi_idle_*`, `kimi_idle_model`, `omp_idle_model`, `pi_working`, `pi_dead_session`,
/// `unknown_session_no_info` e `panel_open_409` eram todas 409 sem código; `invalid_scope_422`,
/// `extra_field_422` e `malformed_json_422`, a lista do FastAPI.
#[cfg(unix)]
#[tokio::test]
async fn model_effort_r1_4_other_providers_are_refused_in_rust() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve_listed(dir.path(), 0, Some("pi1")).await;
    for body in [r#"{"scope":"session"}"#, r#"{"model":"sonnet"}"#, r#"{"effort":"turbo"}"#, "{}", r#"{"model":"opus","effort":"high","scope":"default"}"#] {
        let (status, reply) = post(server, "pi1", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_model_effort_so_claude")), "{body}: {reply}");
    }
    for body in [r#"{"scope":"global"}"#, r#"{"model":"sonnet","x":1}"#, "{", "[]"] {
        let (status, reply) = post(server, "pi1", body).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}: {reply}");
    }
    assert_eq!(python.hits_to("/api/sessions/pi1/model-effort"), 0, "nada ao Python");
}

/// R1.4-1: Claude que o Rust não atende (sem vigia, ou com terminal que não se adota) é Claude pela linha da
/// lista: 503 `model_effort_needs_rust_server`, nunca o 409 de "só Claude".
#[cfg(unix)]
#[tokio::test]
async fn model_effort_r1_4_unserved_claude_is_503_by_the_list_row() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve_listed(dir.path(), 1, None).await;
    let (status, reply) = post(server, "s0", r#"{"model":"sonnet"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (503, Some("model_effort_needs_rust_server")), "{reply}");
    assert_eq!(python.hits_to("/api/sessions/s0/model-effort"), 0);
}

/// B9: com terminal, modelo torto é 422 antes do terminal.
#[cfg(unix)]
#[tokio::test]
async fn model_effort_b9_malformed_model_never_types() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (_python, server) = serve(registry, terms, None, Duration::from_secs(5)).await;
    assert_eq!(post(server, "t", r#"{"model":"opus; rm"}"#).await, (422, json!({"detail": "unknown model 'opus; rm'"})));
    assert_eq!(post(server, "t", r#"{"effort":"turbo"}"#).await, (422, json!({"detail": "unknown effort 'turbo'"})));
    assert_eq!(keys_sent(&log), 0);
}

/// B13 e B21 pela rota: o ator do terminal dirige o seletor e a recusa sai em texto cru, como no Python.
#[cfg(unix)]
#[tokio::test]
async fn model_effort_b13_b21_terminal_route_drives_the_actor() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    let log = open_terminal(&registry, dir.path(), "t").await;
    let (python, server) = serve(registry, terms, None, Duration::from_secs(5)).await;
    let (status, reply) = post(server, "t", r#"{"model":"sonnet"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_seletor_modelo_nao_abriu")), "{reply}");
    // `/model`, Enter, o 2º Enter e o Esc.
    assert_eq!(keys_sent(&log), 4);
    assert_eq!(python.hits_to("/api/sessions/t/model-effort"), 0);
}

/// B3: troca de conversa em curso (porta fechada) recusa com o código de sessão ocupada.
#[tokio::test]
async fn model_effort_b3_transfer_in_progress_is_busy() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    open_headless(&registry, dir.path(), "s").await;
    registry.ingress().close("s", Duration::ZERO).await.unwrap();
    let (python, server) = serve(registry, terms, None, Duration::from_millis(100)).await;
    let (status, reply) = post(server, "s", r#"{"model":"sonnet"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("session_transfer_busy")), "{reply}");
    assert_eq!(python.hits_to("/api/sessions/s/model-effort"), 0);
}

/// B2: convidado e Connect chegam pela ponte privada do Claude e recebem a resposta do Rust.
#[tokio::test]
async fn model_effort_b2_private_bridge_serves_the_route() {
    let (dir, terms) = (tempfile::tempdir().unwrap(), Arc::new(Terms::new(TermConfig::default())));
    let registry = registry(&terms).await;
    open_headless(&registry, dir.path(), "s").await;
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    state.term = terms;
    assert!(state.state.runtime.set(registry).is_ok());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let router = hangar_server::routes::terminal_router(Arc::new(state));
    tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let call = |name: &'static str, body: &'static str| async move {
        let response = fake::client().post(format!("http://{private}/__hangar_server/claude/{name}/model-effort"))
            .header("content-type", "application/json").header("x-hangar-internal", SECRET).body(body).send().await.unwrap();
        let status = response.status().as_u16();
        (status, serde_json::from_str::<Value>(&response.text().await.unwrap()).unwrap_or(Value::Null))
    };
    assert_eq!(call("s", r#"{"model":"sonnet"}"#).await, (200, json!({"ok": true, "scope": "session", "result": null})));
    assert_eq!(call("s", r#"{"model":"bad id!"}"#).await.0, 422);
    assert_eq!(call("nada", r#"{"model":"sonnet"}"#).await.0, 421, "sessão que o Rust não tem segue ao Python");
    assert_eq!(python.hits_to("/api/sessions/s/model-effort"), 0);
}
