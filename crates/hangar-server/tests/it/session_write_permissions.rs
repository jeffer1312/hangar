//! `GET permission-modes` e `POST permission-mode` do dono quando o Rust não tem a entrada da sessão:
//! a recusa sai daqui, com código, pela linha da lista (decisões 2 e 11). Nada chega ao Python, que
//! apertava Shift+Tab no tmux de uma sessão que o Rust conduz (D4).
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use serde_json::{Value, json};

use crate::fake::{self, Fake, SECRET};
use crate::session_write_control::{open_headless, policy, send};

/// `model_args.MODOS_PERMISSAO_CLAUDE`, na ordem do Python.
const MODES: [&str; 6] = ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];

const GUEST: &str = "convidado-ana";
const BLIND: &str = "convidado-bia";

/// O sidecar que o registro e o ator leem para o Claude sem terminal de `open_headless` (chave `k-<nome>`,
/// conversa `sid-1`, cano 42): é onde a troca de modo fica gravada e conferida.
pub(crate) fn headless_sidecar(home: &Path, name: &str) {
    let dir = home.join(".hangar/claude-headless");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.json")), json!({"name": name, "provider": "claude", "headless": true, "key": format!("k-{name}"),
        "session_id": "sid-1", "cwd": home, "permission_mode": "manual", "cano": {"pid": 42}}).to_string()).unwrap();
}

/// Lista no tmux falso com `s0` (Claude com terminal) e `p0` (Pi, pela escolha da criação). O registro
/// não tem vigia: não adota ninguém. `headless`: um Claude sem terminal `h` aberto no Rust.
async fn serve(root: &Path, headless: bool) -> (Arc<Fake>, std::net::SocketAddr) {
    let script = crate::list_support::sessions(root, 1);
    let pi = root.join("wp");
    std::fs::create_dir_all(&pi).unwrap();
    crate::write_executable(&script, format!(
        "#!/bin/sh\necho \"$1\" >> '{}'\n[ \"$1\" = list-panes ] || {{ printf '● pronto\\n❯\\n'; exit 0; }}\nprintf 's0\\t1\\t\\t{}\\t%%0\\t\\t\\t\\t0\\t0\\np0\\t1\\t\\t{}\\t%%1\\t\\tpi\\t\\t0\\t0\\n'\n",
        root.join("calls.log").display(), root.join("w0").display(), pi.display()));
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"), "omp_agent": home.join(".omp/agent"),
        "kimi_home": home.join(".kimi-code")}).to_string());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // A pasta da prova: os sidecars e specs que o registro lê são os dela, nunca os da máquina.
    let env = hangar_server::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(), home.to_string_lossy().into_owned())].into());
    let registry = Arc::new(RuntimeRegistry::new(policy(calls).await, "secret-test".into(), "instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))).with_codex_environment(env));
    if headless {
        headless_sidecar(&home, "h");
        open_headless(&registry, root, "h").await;
    }
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    // Convidados com conta: a Ana vê as sessões do dono, a Bia não.
    let guests = hangar_server::login_guests::Service::new(hangar_server::login_guests::Settings { guests: root.join("guests.json"),
        vault: root.join("vault.json"), runtime_config: root.join("runtime-config.json"), sync_enabled: false, bootstrap: String::new(),
        secret: vec![1], secure: false, rate_max: 10.into(), rate_window: 300.into() });
    let text = root.to_string_lossy().into_owned();
    guests.store.create("Ana", &text, true, true, "ana".into(), GUEST.into()).unwrap();
    guests.store.create("Bia", &text, false, true, "bia".into(), BLIND.into()).unwrap();
    state.login_guests = Arc::new(guests);
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    state.write_gate_wait = Duration::from_secs(5);
    assert!(state.state.runtime.set(registry).is_ok());
    (python, fake::spawn_state(state).await)
}

async fn code(server: std::net::SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
    let (status, reply) = send(server, method, path, body).await;
    (status, reply["detail"]["code"].clone())
}

fn python_hits(python: &Fake, name: &str) -> usize {
    python.hits_to(&format!("/api/sessions/{name}/permission-modes")) + python.hits_to(&format!("/api/sessions/{name}/permission-mode"))
}

/// R4-01 (`_cached_info` → 404): sessão que ninguém tem é 404 `erro_sessao_inexistente`, lida e trocada.
/// Decisão 11: a sessão é conferida antes do modo, e o modo torto numa sessão inexistente também é 404.
#[tokio::test(flavor = "multi_thread")]
async fn permissions_r4_01_session_nobody_has_is_404_from_rust() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve(dir.path(), false).await;
    let missing = json!("erro_sessao_inexistente");
    assert_eq!(code(server, "GET", "nada/permission-modes", "").await, (404, missing.clone()));
    assert_eq!(code(server, "POST", "nada/permission-mode", r#"{"mode":"plan"}"#).await, (404, missing.clone()));
    assert_eq!(code(server, "POST", "nada/permission-mode", r#"{"mode":"wrong"}"#).await, (404, missing));
    assert_eq!(python_hits(&python, "nada"), 0);
}

/// R4-02 (`test_perm_get_nao_claude_409`, `test_perm_post_nao_claude_409`): sessão de outro agente é 409
/// `erro_permissao_so_claude`, sem tecla e sem repasse.
#[tokio::test(flavor = "multi_thread")]
async fn permissions_r4_02_session_of_another_agent_is_409_only_claude() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve(dir.path(), false).await;
    let only = json!("erro_permissao_so_claude");
    assert_eq!(code(server, "GET", "p0/permission-modes", "").await, (409, only.clone()));
    assert_eq!(code(server, "POST", "p0/permission-mode", r#"{"mode":"plan"}"#).await, (409, only));
    assert_eq!(python_hits(&python, "p0"), 0);
}

/// D4 e contrato (c): Claude com terminal que o Rust não conduz nem adota é 503 `runtime_unhealthy`;
/// o Python não aperta Shift+Tab no pane por fora da porta de entrada do Rust.
#[tokio::test(flavor = "multi_thread")]
async fn permissions_d4_claude_without_actor_is_503_never_the_python() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve(dir.path(), false).await;
    let unhealthy = json!("runtime_unhealthy");
    assert_eq!(code(server, "GET", "s0/permission-modes", "").await, (503, unhealthy.clone()));
    assert_eq!(code(server, "GET", "s0/permission-modes?sondar=1", "").await, (503, unhealthy.clone()));
    assert_eq!(code(server, "POST", "s0/permission-mode", r#"{"mode":"plan"}"#).await, (503, unhealthy));
    assert_eq!(python_hits(&python, "s0"), 0);
    assert_eq!(crate::list_support::calls(dir.path(), "send-keys"), 0, "nenhuma tecla no pane");
}

/// R4-08, contrato (c): Claude sem terminal parado, sem entrada no Rust, é lido pelo registro dele como a
/// adormecida (o `headless_sessions.load` do Python): o modo gravado, sem sonda, e nada sobe o processo.
#[tokio::test(flavor = "multi_thread")]
async fn permissions_r4_08_parked_headless_claude_is_read_from_its_record() {
    let dir = tempfile::tempdir().unwrap();
    let (cwd, sidecars) = (dir.path().join("wq"), dir.path().join("home/.hangar/claude-headless"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&sidecars).unwrap();
    // `q0` guarda o último modo fora do plano; `q1` não: o Python devolvia `meta.get("previous_non_plan")`.
    for (name, key, sid, previous) in [("q0", "ef", "aa", Some("auto")), ("q1", "fe", "bb", None)] {
        let mut meta = json!({"name": name, "provider": "claude", "headless": true, "key": key.repeat(16), "cwd": cwd,
            "session_id": format!("00000000-0000-0000-0000-0000000000{sid}"), "permission_mode": "plan", "cano": null});
        if let Some(previous) = previous { meta["previous_non_plan"] = json!(previous); }
        std::fs::write(sidecars.join(format!("{name}.json")), meta.to_string()).unwrap();
    }
    let (python, server) = serve(dir.path(), false).await;
    for (name, previous) in [("q0", json!("auto")), ("q1", Value::Null)] {
        let (status, reply) = send(server, "GET", &format!("{name}/permission-modes"), "").await;
        assert_eq!((status, reply), (200, json!({"current": "plan", "modes": MODES, "sondavel": false, "previous_non_plan": previous})), "{name}");
        assert_eq!(python_hits(&python, name), 0);
    }
    assert_eq!(crate::list_support::calls(dir.path(), "new-session"), 0, "ler não sobe o processo");
}

/// Contrato (c): Claude sem terminal parado que a escrita não consegue religar (registro sem a chave da
/// vida, que a lista ainda mostra) é 409 `erro_permissao_sessao_parada`: repetir não adianta, só reabrir.
/// Nunca o 503 `runtime_unhealthy` de "tente de novo".
#[tokio::test(flavor = "multi_thread")]
async fn permissions_parked_headless_that_cannot_relaunch_is_409_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let (cwd, sidecars) = (dir.path().join("wq"), dir.path().join("home/.hangar/claude-headless"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&sidecars).unwrap();
    std::fs::write(sidecars.join("q2.json"), json!({"name": "q2", "provider": "claude", "headless": true, "cwd": cwd,
        "session_id": "00000000-0000-0000-0000-0000000000cc", "permission_mode": "plan", "cano": null}).to_string()).unwrap();
    let (python, server) = serve(dir.path(), false).await;
    let (status, reply) = send(server, "POST", "q2/permission-mode", r#"{"mode":"auto"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_permissao_sessao_parada")), "{reply}");
    assert_eq!(python_hits(&python, "q2"), 0);
    assert_eq!(crate::list_support::calls(dir.path(), "new-session"), 0, "ler não sobe o processo");
}

/// R4-03, R4-07 e R4-08 (`test_perm_post_invalida_409`, `test_perm_post_aceita_permission_mode_alias`): o
/// Claude sem terminal lê os seis modos sem sonda (`?sondar=xyz` vale falso) e troca pelo pedido de
/// controle; vazio é 422, modo fora da lista 409, campo a mais 422, e `previous_non_plan` vem sempre.
#[tokio::test(flavor = "multi_thread")]
async fn permissions_r4_08_headless_claude_reads_and_sets_through_the_actor() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve(dir.path(), true).await;
    for query in ["", "?sondar=xyz"] {
        let (status, reply) = send(server, "GET", &format!("h/permission-modes{query}"), "").await;
        assert_eq!((status, reply["modes"].clone(), reply["sondavel"].clone()), (200, json!(MODES), json!(false)), "{query}: {reply}");
        assert!(reply.as_object().unwrap().contains_key("previous_non_plan"), "{reply}");
    }
    let invalid = json!("erro_permissao_invalida");
    assert_eq!(code(server, "POST", "h/permission-mode", "{}").await, (422, invalid.clone()));
    assert_eq!(code(server, "POST", "h/permission-mode", r#"{"mode":""}"#).await, (422, invalid.clone()));
    assert_eq!(code(server, "POST", "h/permission-mode", r#"{"mode":"wrong"}"#).await, (409, invalid.clone()));
    assert_eq!(code(server, "POST", "h/permission-mode", r#"{"mode":"plan","x":1}"#).await, (422, json!("erro_corpo_invalido")), "decisão 11");
    let (status, reply) = send(server, "POST", "h/permission-mode", r#"{"permission_mode":"plan"}"#).await;
    assert_eq!((status, reply["mode"].clone(), reply["current"].clone()), (200, json!("plan"), json!("plan")), "{reply}");
    assert!(reply.as_object().unwrap().contains_key("previous_non_plan"), "{reply}");
    assert_eq!(python_hits(&python, "h"), 0);
}

/// R4-19, decisões 1 e 2 (D2): o convidado com conta chega a modos e modelo da sessão que vê pela rota do
/// dono e recebe a resposta do Rust (antes, o 401 do Python, que o deslogava); a que não vê a sessão
/// leva o 403 do porteiro. Nada vai ao Python.
#[tokio::test(flavor = "multi_thread")]
async fn r4_19_login_guest_reaches_mode_and_model_in_rust() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve(dir.path(), true).await;
    let send = |method: reqwest::Method, route: &'static str, token: &'static str| {
        let request = fake::client().request(method, format!("http://{server}/api/sessions/h/{route}")).bearer_auth(token)
            .header("content-type", "application/json");
        async move {
            let body = match route { "permission-mode" => r#"{"mode":"plan"}"#, "model-effort" => r#"{"model":"sonnet"}"#, _ => "" };
            let response = request.body(body).send().await.unwrap();
            let status = response.status().as_u16();
            (status, serde_json::from_str::<Value>(&response.text().await.unwrap()).unwrap_or(Value::Null))
        }
    };
    for (method, route) in [(reqwest::Method::GET, "permission-modes"), (reqwest::Method::POST, "permission-mode"), (reqwest::Method::POST, "model-effort")] {
        let (status, body) = send(method.clone(), route, BLIND).await;
        assert_eq!((status, body["detail"]["code"].as_str()), (403, Some("erro_fora_do_convidado")), "{route}: {body}");
        let (status, body) = send(method, route, GUEST).await;
        assert_ne!(body["detail"]["code"].as_str(), Some("erro_fora_do_convidado"), "{route}: a sessão é visível para ela");
        assert_eq!(status, 200, "{route}: a rota do dono atende o convidado: {body}");
        assert_eq!(python.hits_to(&format!("/api/sessions/h/{route}")), 0, "{route}");
    }
}
