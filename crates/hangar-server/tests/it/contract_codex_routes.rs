//! Rotas só do Codex sem terminal servidas pelo Rust: cada caso de `golden/codex_routes.json` (gravado
//! pela rota Python com respostas fixas do app-server) roda aqui pelo roteador de verdade, com um cano
//! falso que devolve as MESMAS respostas, e o corpo e o código têm de ser iguais.
use crate::fake;

use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::interactions::{InteractionBinding, InteractionKind, InteractionStore, native_target};
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeError, RuntimeTarget};
use hangar_server::runtime::terminal::TerminalTarget;
use hangar_server::terminal_input::TerminalBinding;
use hangar_server::session_write::codex::{permission_answer, skip_answer};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[test]
fn clear_with_an_open_terminal_panel_has_the_same_409_as_other_pane_controls() {
    let (status,body) = hangar_server::session_write::codex::compact_answer(&Err(RuntimeError::new("terminal_panel_open","painel aberto")));
    assert_eq!(status.as_u16(),409);
    assert_eq!(body["detail"]["code"],"erro_terminal_aberto");
}

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

/// Registro como o do servidor, sem Python atrás (nada do Codex vai a ele) e sem `codex` no PATH: uma
/// subida pedida falha no processo, nunca abre um app-server de verdade.
fn registry(dir: &std::path::Path) -> RuntimeRegistry {
    RuntimeRegistry::new(crate::refused_address(), "secret-test".into(), "instance-test".into())
        .with_codex_environment(hangar_server::accounts::environment::AccountEnvironment::from_map(
            [("HOME".to_owned(), dir.to_string_lossy().into_owned()), ("PATH".to_owned(), dir.join("sem-bin").to_string_lossy().into_owned())].into()))
}

/// O arquivo da sessão que o Python grava ao criar o Codex sem terminal; o Rust grava nele.
fn sidecar(dir: &std::path::Path, extra: Value) {
    let mut meta = json!({"name": "s", "key": "k", "provider": "codex", "headless": true, "thread_id": "thread-1", "cwd": "/tmp/projeto"});
    for (field, value) in extra.as_object().unwrap() { meta[field] = value.clone(); }
    std::fs::write(dir.join("s.json"), meta.to_string()).unwrap();
}

async fn open(registry: &RuntimeRegistry, dir: &std::path::Path, case: &Value) {
    let s = &case["session"];
    let questions: Vec<Value> = s["async"].as_array().unwrap().iter().map(|id| json!([id, {"request_id": id, "questions": [{"question": "?"}]}])).collect();
    let metadata = json!({"name": "s", "headless": true, "thread_id": "thread-1", "initialized": true, "ready": true, "cwd": "/tmp/projeto",
        "model": s["model"], "effort": s["effort"], "service_tier": s["service_tier"], "mode": s["mode"],
        "permission_mode": s["permission_mode"], "async_questions": questions});
    sidecar(dir, json!({"permission_mode": s["permission_mode"]}));
    registry.open_managed(RuntimeTarget { key: "k".into(), generation: 1, name: "s".into(), provider: "codex".into(), metadata,
        binding: CanoBinding { pid: 42, escuta: cano(case["rpc"].clone(), s["busy"] == true).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join("k.lock"), state_path: dir.join("k.queue-state.json"), projection_dir: dir.join("k-projection"),
        transcript: dir.join("k.jsonl"), created: 0.0 }, dir.into()).await.unwrap();
}

async fn serve(registry: Arc<RuntimeRegistry>, home: &std::path::Path) -> (Arc<Fake>, std::net::SocketAddr) {
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    state.accounts = hangar_server::accounts::AccountService::new(
        hangar_server::accounts::environment::AccountEnvironment::from_map([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), String::new()),
        ].into()));
    let mut list_env = hangar_server::list::bridge::ListEnv::from_env();
    list_env.dirs = None;
    state.list = Arc::new(hangar_server::list::bridge::ListBridge::new(list_env,
        hangar_server::list::facts::FactsClient::new(upstream, SECRET.into())));
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

/// Nome sem sessão Codex do Rust: o Rust responde com o código da rota Python (`expect` do golden quando
/// ele existe), e nada vai ao Python.
async fn check_unowned_route(server: std::net::SocketAddr, python: &Fake, case: &Value) {
    let (status, body) = call(server, case, OWNER).await;
    let path = format!("/api/sessions/s/{}", case["route"].as_str().unwrap());
    if case["route"] == "commands" {
        assert_eq!(status, 200, "{}", case["name"]);
        let commands = body.as_array().expect("catálogo Claude no Rust");
        assert!(commands.iter().any(|command| command["name"] == "reload-plugins"));
    } else if case["expect"].is_null() {
        assert_eq!((status, body), (400, json!({"detail": {"code": "erro_model_so_codex", "params": {}, "msg": "model so existe pra sessoes Codex"}})), "{}", case["name"]);
    } else {
        assert_eq!((status, &body), (case["expect"]["status"].as_u64().unwrap() as u16, &case["expect"]["body"]), "{}", case["name"]);
    }
    assert_eq!(python.hits_to(&path), 0, "{}: nunca ao Python", case["name"]);
}

/// ID que a pergunta assíncrona da sessão recebe ao ser exibida (o `<shown>` do golden).
fn show(store: &InteractionStore, case: &Value) -> Value {
    let request = &case["session"]["async"][0];
    let binding = InteractionBinding { key: "k".into(), conversation: "thread-1".into(), generation: 1 };
    let shown = json!({"request_id": request, "questions": [{"question": "?"}]});
    json!(store.observe(&binding, &native_target(InteractionKind::Question, "thread-1", request), Some(shown)).unwrap().unwrap())
}

#[tokio::test]
async fn headless_codex_routes_match_the_python_golden() {
    for case in golden().iter().filter(|c| c["relay"].is_null() && c["fault"].is_null()) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InteractionStore::default());
        let registry = registry(dir.path());
        let registry = Arc::new(if case["route"] == "question/skip" { registry.with_interactions(store.clone()) } else { registry });
        open(&registry, dir.path(), case).await;
        let (python, server) = serve(registry.clone(), dir.path()).await;
        let mut sent = case.clone();
        if sent["body"]["interaction_id"] == "<shown>" { sent["body"]["interaction_id"] = show(&store, case); }
        let (status, body) = call(server, &sent, OWNER).await;
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{}", case["route"].as_str().unwrap())), 0, "{}: foi ao Python", case["name"]);
        let mut expected = case["expect"]["body"].clone();
        // O modo Python conserva seu catálogo antigo; a conversa nova é controle exclusivo do Rust.
        if case["route"] == "commands" && case["expect"]["status"] == 200 {
            expected.as_array_mut().unwrap().insert(0,json!({"name":"clear","display":"/clear","source":"builtin",
                "description":"Limpa o histórico da conversa","destructive":true}));
        }
        assert_eq!((status, &body), (case["expect"]["status"].as_u64().unwrap() as u16, &expected), "{}", case["name"]);
        registry.close("k", 1).await.unwrap();
    }
}

/// Dispensar pergunta assíncrona pela rota de verdade: o corpo dos clientes (`request_id` +
/// `interaction_id`) é aceito e o ID chega ao motor; sem ele, 409 com o código de identidade.
#[tokio::test]
async fn skip_route_accepts_and_forwards_the_displayed_interaction_id() {
    let case = golden().into_iter().find(|c| c["name"] == "skip_async").unwrap();
    let request = case["body"]["request_id"].as_str().unwrap().to_owned();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InteractionStore::default());
    let registry = Arc::new(registry(dir.path()).with_interactions(store.clone()));
    open(&registry, dir.path(), &case).await;
    let (python, server) = serve(registry, dir.path()).await;
    let mut bare = case.clone();
    bare["body"] = json!({"request_id": request});
    let missing = call(server, &bare, OWNER).await;
    assert_eq!((missing.0, missing.1["detail"]["code"].as_str()), (409, Some("erro_interacao_sem_identidade")));
    let binding = InteractionBinding { key: "k".into(), conversation: "thread-1".into(), generation: 1 };
    let shown = json!({"request_id": request, "questions": [{"question": "?"}]});
    let token = store.observe(&binding, &native_target(InteractionKind::Question, "thread-1", &json!(request)), Some(shown)).unwrap().unwrap();
    let mut identified = case.clone();
    identified["body"] = json!({"request_id": request, "interaction_id": token});
    assert_eq!(call(server, &identified, OWNER).await, (200, json!({"ok": true})));
    assert_eq!(python.hits_to("/api/sessions/s/question/skip"), 0);
}

#[tokio::test]
async fn unowned_controls_answer_in_rust_with_the_python_codes() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(registry(dir.path()));
    let (python, server) = serve(registry, dir.path()).await;
    for case in golden().iter().filter(|c| !c["relay"].is_null()) {
        check_unowned_route(server, &python, case).await;
    }
}

#[tokio::test]
async fn stranger_gets_the_rust_401_and_bodies_the_route_refuses_are_a_rust_422() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(registry(dir.path()));
    let case = golden().into_iter().find(|c| c["name"] == "model_ok").unwrap();
    open(&registry, dir.path(), &case).await;
    let (python, server) = serve(registry, dir.path()).await;
    // Decisão 5: quem não pode na sessão leva o 401 do Rust; o Python não tem mais a rota.
    assert_eq!(call(server, &case, "token-errado").await, (401, json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}})));
    for (method, route) in [("GET", "models"), ("GET", "limits"), ("GET", "commands"), ("POST", "codex/plan/implement")] {
        let probe = json!({"name": route, "method": method, "route": route, "body": {}});
        assert_eq!(call(server, &probe, "token-errado").await.0, 401, "{route}");
    }
    let tier = json!({"service_tier": "turbo"});
    for (route, body) in [("model", json!({"model": "gpt-5", "x": 1})), ("model", json!({"model": 5})), ("model", json!({"effort": "low"})), ("service-tier", tier)] {
        let refused = json!({"name": "corpo", "method": "POST", "route": route, "body": body});
        let (status, reply) = call(server, &refused, OWNER).await;
        assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{body}");
    }
    for route in ["model", "service-tier", "models", "limits", "commands", "codex/plan/implement"] {
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{route}")), 0, "{route}: nada ao Python");
    }
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
    let registry = Arc::new(registry(dir.path()));
    let case = golden().into_iter().find(|c| c["name"] == "limits_ok").unwrap();
    open(&registry, dir.path(), &case).await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve(registry, dir.path()).await;
    let started = std::time::Instant::now();
    assert_eq!(call(server, &case, OWNER).await, (200, json!({"primary": null, "secondary": null, "planType": null})));
    assert!(started.elapsed() < Duration::from_secs(1), "esperou a porta: {:?}", started.elapsed());
    assert_eq!(python.hits_to("/api/sessions/s/limits"), 0);
}

#[tokio::test]
async fn codex_controls_on_a_claude_terminal_and_its_commands_skip_the_write_gate() {
    // A entrada Claude exercita a recusa dos controles Codex; seu catálogo é uma leitura do Rust.
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(registry(dir.path()));
    let binding = TerminalBinding { name: "s".into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec!["/does-not-exist/hangar-test-tmux".into()], windows: false, clipboard_lock_path: None };
    let transcript = dir.path().join("s.jsonl");
    std::fs::write(&transcript, "").unwrap();
    registry.open_terminal(TerminalTarget { key: "k-s".into(), generation: 1, name: "s".into(), binding,
        lease_path: dir.path().join("s.lease"), state_path: dir.path().join("s.state"), projection_dir: dir.path().join("s.projection"),
        transcript, created: 0.0, plugin_key: None,
        metadata: hangar_server::terminal_facts::TerminalMetadata { config_dir:dir.path().into(),agent_pid:std::process::id(),agent_birth:hangar_server::terminal_facts::agent_birth(std::process::id()).unwrap(),legacy_import_after:None,fingerprint:"fixture".into(),origin:"uds:fixture".into() } }).await.unwrap();
    assert!(registry.writable("s").await.unwrap().terminal);
    // A porta fechada prova que a recusa vem antes dela: responde sem esperar.
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let (python, server) = serve(registry, dir.path()).await;
    for case in golden().iter().filter(|c| c["relay"] == "terminal") {
        check_unowned_route(server, &python, case).await;
    }
}

#[test]
fn skip_that_was_not_saved_is_a_send_failure_and_a_bad_answer_is_invalid() {
    use hangar_server::runtime::protocol::{Disposition, RuntimeReply};
    // O Python gravava antes de esconder: falha de gravação é 503 e a pergunta continua.
    let unsaved = Ok(RuntimeReply { operation_id: "op".into(), disposition: Disposition::Rejected, payload: json!({"skip_not_saved": true}) });
    let (status, body) = skip_answer(&unsaved);
    assert_eq!((status.as_u16(), body["detail"]["code"].as_str()), (503, Some("erro_codex_resposta_envio")));
    let invalid = Err(RuntimeError::new(hangar_server::runtime::codex::INVALID_ANSWER, "a pergunta já está sendo pulada"));
    let (status, body) = skip_answer(&invalid);
    assert_eq!((status.as_u16(), body["detail"]["code"].as_str()), (409, Some("erro_codex_resposta_invalida")));
}

/// Cano que cai logo depois de abrir e não volta (sem quem o religue): a entrada fica doente.
async fn dying_cano() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else { return };
        let mut reader = BufReader::new(stream);
        let mut header = String::new();
        if reader.read_line(&mut header).await.is_err() { return; }
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = reader.get_mut().write_all(format!("{}\n", json!({"type":"cano_saiu","rc":1})).as_bytes()).await;
        let mut rest = String::new();
        while reader.read_line(&mut rest).await.unwrap_or(0) > 0 { rest.clear(); }
    });
    format!("tcp:{address}")
}

#[tokio::test]
async fn permissions_of_a_dead_headless_codex_answer_from_the_session_file_not_503() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(registry(dir.path()));
    let metadata = json!({"name": "s", "headless": true, "thread_id": "thread-1", "initialized": true, "ready": true, "cwd": "/tmp/projeto",
        "permission_mode": "Ask for approval"});
    sidecar(dir.path(), json!({"permission_mode": "Ask for approval"}));
    registry.open_managed(RuntimeTarget { key: "k".into(), generation: 1, name: "s".into(), provider: "codex".into(), metadata,
        binding: CanoBinding { pid: 42, escuta: dying_cano().await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.path().join("k.lock"), state_path: dir.path().join("k.queue-state.json"), projection_dir: dir.path().join("k-projection"),
        transcript: dir.path().join("k.jsonl"), created: 0.0 }, dir.path().into()).await.unwrap();
    let handle = registry.handle("k", 1).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async { while handle.view().await.unwrap()["alive"] != false { tokio::time::sleep(Duration::from_millis(20)).await; } })
        .await.expect("o cano caiu");
    let (python, server) = serve(registry.clone(), dir.path()).await;
    let get = |server| async move { client().get(format!("http://{server}/api/sessions/s/codex-permissions"))
        .header("authorization", format!("Bearer {OWNER}")).send().await.unwrap() };
    let post = |server, mode: &'static str| async move { client().post(format!("http://{server}/api/sessions/s/codex-permissions"))
        .header("authorization", format!("Bearer {OWNER}")).header("content-type", "application/json")
        .body(json!({"mode": mode}).to_string()).send().await.unwrap() };
    let read = get(server).await;
    assert_eq!(read.status().as_u16(), 200);
    assert_eq!(serde_json::from_str::<Value>(&read.text().await.unwrap()).unwrap()["current"], "Ask for approval");
    let written = post(server, "Approve for me").await;
    assert_eq!((written.status().as_u16(), written.text().await.unwrap()), (200, json!({"current": "Approve for me"}).to_string()));
    assert_eq!(serde_json::from_str::<Value>(&get(server).await.text().await.unwrap()).unwrap()["current"], "Approve for me");
    let unknown = post(server, "Read Only").await;
    assert_eq!(unknown.status().as_u16(), 400, "modo desconhecido continua recusado");
    assert_eq!(python.hits_to("/api/sessions/s/codex-permissions"), 0);
    // Outra rota da mesma sessão doente continua 503 com o código.
    let models = client().get(format!("http://{server}/api/sessions/s/models")).header("authorization", format!("Bearer {OWNER}")).send().await.unwrap();
    assert_eq!(models.status().as_u16(), 503);
}

/// Pedido cru a uma rota de `s`: método, rota e corpo, com o token do dono.
async fn raw(server: std::net::SocketAddr, method: &str, route: &str, body: Value) -> (u16, Value) {
    call(server, &json!({"name": route, "method": method, "route": route, "body": body}), OWNER).await
}

fn refusal(code: &str, msg: &str) -> Value { json!({"detail": {"code": code, "params": {}, "msg": msg}}) }

// R2-07, R2-08, R2-09, R2-11, R2-12, R2-14, R2-15, R2-16 (`api.py:2542-2745`): nome sem sessão Codex do Rust.
// Corpo fora do modelo é o 422 do Rust (decisão 11); o resto, o status e o código da rota Python. O
// `service-tier` (B3) responde o 400 que o Python tinha e não alcançava; `codex/terminate` nunca existiu nele.
#[tokio::test]
async fn codex_routes_on_a_name_without_codex_answer_like_the_python_routes() {
    let dir = tempfile::tempdir().unwrap();
    let (python, server) = serve(Arc::new(registry(dir.path())), dir.path()).await;
    let invalid = |(status, body): (u16, Value)| (status, body["detail"]["code"].clone());
    for (route, body) in [("model", json!({"model": 5})), ("service-tier", json!({"service_tier": "turbo"})), ("codex/mode", json!({"mode": "bypassPermissions"})),
        ("question/skip", json!({"interaction_id": "i"})), ("codex-permissions", json!({}))] {
        assert_eq!(invalid(raw(server, "POST", route, body.clone()).await), (422, json!("erro_corpo_invalido")), "{route} {body}");
    }
    assert_eq!(raw(server, "POST", "model", json!({"model": "gpt-5", "effort": null})).await, (400, refusal("erro_model_so_codex", "model so existe pra sessoes Codex")));
    assert_eq!(raw(server, "POST", "service-tier", json!({"service_tier": "priority"})).await,
        (400, refusal("erro_fast_indisponivel", "Fast exige Codex ou Claude com GPT no CLIProxyAPI local")));
    assert_eq!(raw(server, "POST", "codex/mode", json!({"mode": "plan"})).await, (400, refusal("erro_model_so_codex", "model só existe para sessões Codex")));
    assert_eq!(raw(server, "POST", "question/skip", json!({"request_id": "r1", "interaction_id": null})).await,
        (409, refusal("erro_codex_resposta_invalida", "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.")));
    assert_eq!(raw(server, "POST", "codex/terminate", json!({"process_id": "p1"})).await, (404, json!({"detail": "Not Found"})));
    let permission = refusal("erro_permissao_so_codex", "este modo de permissao so vale para sessoes Codex");
    assert_eq!(raw(server, "GET", "codex-permissions", Value::Null).await, (400, permission.clone()));
    assert_eq!(raw(server, "POST", "codex-permissions", json!({"mode": "Full Access"})).await, (400, permission));
    assert_eq!(raw(server, "POST", "codex/plan/implement", Value::Null).await,
        (404, refusal("erro_sessao_opcao_nao_enviada", "sessão não encontrada — plano NÃO iniciado")));
    for route in ["model", "service-tier", "codex/mode", "question/skip", "codex/terminate", "codex-permissions", "codex/plan/implement"] {
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{route}")), 0, "{route}");
    }
}

// R2-13, R2-17 (`adapter.py:917-932`): arquivo de sessão Codex que o Rust não adota. As rotas que o Python
// respondia com `erro_codex_controle` ficam; `/limits` é a resposta neutra; as que digitavam no pane
// (`codex-permissions`, `plan/implement`) passam a recusar com o mesmo código (decisão 6).
#[tokio::test]
async fn codex_routes_on_a_sidecar_rust_does_not_adopt_never_type_and_answer_with_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let sidecars = dir.path().join(".hangar/codex-sessions");
    std::fs::create_dir_all(&sidecars).unwrap();
    std::fs::write(sidecars.join("s.json"), "{}").unwrap();
    let (python, server) = serve(Arc::new(registry(dir.path())), dir.path()).await;
    let control = (409, refusal("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente."));
    for (method, route, body) in [("GET", "models", Value::Null), ("POST", "model", json!({"model": "gpt-5"})), ("POST", "service-tier", json!({"service_tier": "priority"})),
        ("POST", "codex/mode", json!({"mode": "plan"})), ("GET", "commands", Value::Null), ("GET", "codex-permissions", Value::Null),
        ("POST", "codex-permissions", json!({"mode": "Full Access"})), ("POST", "codex/plan/implement", Value::Null)] {
        assert_eq!(raw(server, method, route, body).await, control, "{method} {route}");
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{route}")), 0, "{route}");
    }
    assert_eq!(raw(server, "GET", "limits", Value::Null).await, (200, json!({"primary": null, "secondary": null, "planType": null})));
    assert_eq!(python.hits_to("/api/sessions/s/limits"), 0);
}
