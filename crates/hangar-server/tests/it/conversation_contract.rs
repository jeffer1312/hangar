//! Matriz do corte de posse (Task 12): Claude e Codex, com e sem terminal, são do Rust. Seleção e
//! resposta com ID atual, antigo e ausente, corpo torto e dono doente recebem a resposta do Rust e nunca
//! chegam ao Python. A ponte privada do Claude (`/__hangar_server/claude/<nome>/<rota>`) faz o mesmo
//! para o que entra pelas portas do Python, e o `DELETE` do Codex com terminal fecha no Rust antes da
//! limpeza do Python.
use std::sync::Arc;
use std::time::Duration;

use hangar_server::interactions::{InteractionBinding, InteractionKind, InteractionStore, native_target, plugin_target};
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeTarget};
use serde_json::{Value, json};

use crate::fake::{self, Fake, OWNER, SECRET};
use crate::session_write_control::{open_headless, registry_with_plugin, send, serve_with_interactions};

const MISSING: &str = "erro_interacao_sem_identidade";
const STALE: &str = "erro_interacao_desatualizada";

fn option() -> Value { json!({"kind": "option", "indices": [0], "labels": ["first"]}) }

/// Dois retratos do mesmo alvo: o primeiro ID fica velho, o segundo é o exibido agora.
fn shown(store: &InteractionStore, binding: &InteractionBinding, kind: InteractionKind) -> (String, String) {
    store.bind(binding).unwrap();
    let target = if kind == InteractionKind::Approval { plugin_target(kind, "perm:1") } else { native_target(kind, &binding.conversation, &json!(0)) };
    let old = store.observe(binding, &target, Some(json!({"question": "A"}))).unwrap().unwrap();
    let current = store.observe(binding, &target, Some(json!({"question": "B"}))).unwrap().unwrap();
    assert_ne!(old, current);
    (old, current)
}

async fn code(server: std::net::SocketAddr, name: &str, route: &str, body: &str) -> (u16, Value) {
    let (status, reply) = send(server, "POST", &format!("{name}/{route}"), body).await;
    (status, reply["detail"]["code"].clone())
}

/// A matriz de uma sessão aberta: `/answer` e `/select` com ID ausente, antigo e atual e corpo torto.
/// `open`: o ator tem a pergunta aberta. Sem terminal o motor confere o ID contra a pergunta que ele
/// mesmo segura; sem nenhuma, o ID atual do retrato fabricado é de pergunta encerrada (desatualizada).
async fn identity_matrix(server: std::net::SocketAddr, python: &Fake, store: &InteractionStore, binding: &InteractionBinding, name: &str, open: bool) {
    let (old_q, current_q) = shown(store, binding, InteractionKind::Question);
    let (old_s, current_s) = shown(store, binding, InteractionKind::Approval);
    for (route, body, want) in [
        ("answer", json!({"answers": [option()]}), MISSING),
        ("answer", json!({"answers": [option()], "interaction_id": old_q}), STALE),
        ("select", json!({"option": 1}), MISSING),
        ("select", json!({"option": 1, "interaction_id": old_s}), STALE),
    ] {
        assert_eq!(code(server, name, route, &body.to_string()).await, (409, json!(want)), "{name} {route} {body}");
    }
    // O ID atual passa da identidade: o Rust escreve ou recusa por outro motivo, nunca como ausente/velho.
    for (route, body) in [("answer", json!({"answers": [option()], "interaction_id": current_q})),
        ("select", json!({"option": 1, "interaction_id": current_s}))] {
        let (status, reply) = send(server, "POST", &format!("{name}/{route}"), &body.to_string()).await;
        assert_ne!(reply, json!("from-python"), "{name} {route}");
        if open {
            assert!(![MISSING, STALE].contains(&reply["detail"]["code"].as_str().unwrap_or("")), "{name} {route}: {status} {reply}");
        } else {
            assert_eq!((status, reply["detail"]["code"].clone()), (409, json!(STALE)), "{name} {route}: sem pergunta aberta no motor");
        }
    }
    for (route, body) in [("answer", r#"{"answers":"x"}"#), ("answer", r#"{"answers":[],"interaction_id":5}"#), ("select", r#"{"option":"1"}"#)] {
        assert_eq!(code(server, name, route, body).await, (422, json!("erro_corpo_invalido")), "{name} {route} {body}");
    }
    for route in ["answer", "select"] {
        assert_eq!(python.hits_to(&format!("/api/sessions/{name}/{route}")), 0, "{name} {route}: nunca ao Python");
    }
}

/// Dono doente: a escrita é a recusa do Rust, nunca o Python relançando.
async fn unhealthy(server: std::net::SocketAddr, python: &Fake, name: &str) {
    for (route, body) in [("answer", json!({"answers": [option()], "interaction_id": "x"})), ("select", json!({"option": 1, "interaction_id": "x"}))] {
        assert_eq!(code(server, name, route, &body.to_string()).await, (503, json!("runtime_unhealthy")), "{name} {route}");
        assert_eq!(python.hits_to(&format!("/api/sessions/{name}/{route}")), 0);
    }
}

async fn until_sick(registry: &RuntimeRegistry, name: &str) {
    for _ in 0..100 {
        if !registry.writable(name).await.unwrap().healthy { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{name}: a entrada não ficou doente");
}

/// Entrada sem terminal cujo cano cai quando avisado.
async fn open_dying(registry: &RuntimeRegistry, dir: &std::path::Path, name: &str, provider: &str) -> Arc<tokio::sync::Notify> {
    let kill = Arc::new(tokio::sync::Notify::new());
    let metadata = if provider == "codex" { json!({"name": name, "headless": true, "thread_id": "thread-1", "initialized": true, "ready": true}) }
        else { json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}) };
    registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: provider.into(), metadata,
        binding: CanoBinding { pid: 42, escuta: crate::session_write_routes::cano_with(Some(kill.clone())).await, token: "secret-test".into(), versao: 2 },
        lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
        projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
    kill
}

#[tokio::test]
async fn claude_headless_answers_and_refuses_in_rust() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    open_headless(&registry, dir.path(), "s").await;
    let binding = registry.interaction_binding("s").await.unwrap();
    open_dying(&registry, dir.path(), "d", "claude").await.notify_one();
    until_sick(&registry, "d").await;
    let (python, server) = serve_with_interactions(registry, store.clone()).await;
    identity_matrix(server, &python, &store, &binding, "s", false).await;
    unhealthy(server, &python, "d").await;
}

#[tokio::test]
async fn codex_headless_answers_and_refuses_in_rust() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    crate::groups_deliver::open_codex_headless(&registry, dir.path(), "c", Arc::default()).await;
    let binding = registry.interaction_binding("c").await.unwrap();
    open_dying(&registry, dir.path(), "d", "codex").await.notify_one();
    until_sick(&registry, "d").await;
    let (python, server) = serve_with_interactions(registry, store.clone()).await;
    identity_matrix(server, &python, &store, &binding, "c", false).await;
    unhealthy(server, &python, "d").await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_terminal_answers_and_refuses_in_rust() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    crate::session_write_control::open_terminal(&registry, dir.path(), "t").await;
    let binding = registry.interaction_binding("t").await.unwrap();
    let (python, server) = serve_with_interactions(registry, store.clone()).await;
    identity_matrix(server, &python, &store, &binding, "t", true).await;
    // Entrada com o vínculo trocado: doente, e a recusa é do Rust.
    let sick_dir = tempfile::tempdir().unwrap();
    let sick = crate::session_write_input::registry().await;
    crate::session_write_input::open_sick_terminal(&sick, sick_dir.path(), "t").await;
    let (python, server) = serve_with_interactions(sick, Arc::new(InteractionStore::default())).await;
    unhealthy(server, &python, "t").await;
}

/// Codex com terminal adotado pela vigia: ID ausente, desconhecido e corpo torto são recusas do Rust.
/// O dono doente segue o mesmo ramo de `route_write` dos outros três (sem fake que derrube só a vista).
#[cfg(unix)]
#[tokio::test]
async fn codex_terminal_answers_and_refuses_in_rust() {
    use crate::runtime_codex_watch::{finish, fixture, serve_owner, tui};
    use crate::runtime_open::{app_server_process, fake_app_server, registry};
    let fx = fixture();
    let app_server = fake_app_server(true).await;
    let (mut app, mut tui) = (app_server_process(&app_server.endpoint), tui());
    fx.put_sidecar("cx-proof", fx.meta("cx-proof", &app_server.endpoint, Some(app.id()), &tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));
    let (python, server) = serve_owner(registry.clone()).await;
    for route in ["answer", "select"] {
        let missing = if route == "answer" { json!({"answers": [option()]}) } else { json!({"option": 1}) };
        assert_eq!(code(server, "cx-proof", route, &missing.to_string()).await, (409, json!(MISSING)), "{route}");
        let mut unknown = missing.clone();
        unknown["interaction_id"] = json!("nunca-exibido");
        let (status, reply) = code(server, "cx-proof", route, &unknown.to_string()).await;
        assert!(matches!((status, reply.as_str()), (409, Some(STALE)) | (503, Some("erro_interacao_indisponivel"))), "{route}: {status} {reply}");
        assert_eq!(code(server, "cx-proof", route, r#"{"x":1}"#).await, (422, json!("erro_corpo_invalido")), "{route}");
        assert_eq!(python.hits_to(&format!("/api/sessions/cx-proof/{route}")), 0, "{route}: nunca ao Python");
    }
    // O relançamento é do Rust: corpo torto recusado aqui, nunca o 422 do FastAPI pelo repasse.
    let (status, reply) = send(server, "POST", "cx-proof/modo-execucao", r#"{"terminal":"sim"}"#).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")));
    assert_eq!(python.hits_to("/api/sessions/cx-proof/modo-execucao"), 0);
    // DELETE: fecha no Rust; o repetido (sessão que já acabou) também fecha no Rust, pelo nome (D17).
    let delete = || fake::client().delete(format!("http://{server}/api/sessions/cx-proof")).header("authorization", format!("Bearer {OWNER}")).send();
    assert_eq!(delete().await.unwrap().status().as_u16(), 200);
    assert!(!registry.owns_codex_terminal("cx-proof"));
    assert_eq!(delete().await.unwrap().status().as_u16(), 200);
    assert_eq!(python.hits_to("/api/sessions/cx-proof"), 0, "o fechamento inteiro é do Rust");
    finish(&mut [&mut app, &mut tui]);
}

/// A ponte privada do Claude: corpo intacto (o `interaction_id` chega à conferência do Rust), resposta e
/// código do Rust, 421 para o que ele não atende, e a rota de personalizações intacta.
#[tokio::test]
async fn claude_bridge_serves_input_answers_and_queue_of_a_rust_claude() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    open_headless(&registry, dir.path(), "s").await;
    let binding = registry.interaction_binding("s").await.unwrap();
    let (old_s, _) = shown(&store, &binding, InteractionKind::Approval);
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    state.interactions = store.clone();
    assert!(state.state.runtime.set(registry).is_ok());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let router = hangar_server::routes::terminal_router(Arc::new(state));
    tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let call = |method: reqwest::Method, path: &str, body: &str, secret: bool| {
        let request = fake::client().request(method, format!("http://{private}/__hangar_server/claude/{path}"))
            .header("content-type", "application/json").body(body.to_owned());
        let request = if secret { request.header("x-hangar-internal", SECRET) } else { request };
        async move {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            (status, serde_json::from_str::<Value>(&response.text().await.unwrap()).unwrap_or(Value::Null))
        }
    };
    let post = |path: &'static str, body: String| call(reqwest::Method::POST, path, &body, true);
    let (status, reply) = post("s/answer", json!({"answers": [option()]}).to_string()).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some(MISSING)));
    let (status, reply) = post("s/select", json!({"option": 1, "interaction_id": old_s}).to_string()).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some(STALE)), "o ID do corpo chegou intacto");
    let (status, reply) = post("s/answer", r#"{"answers":"x"}"#.into()).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")));
    let (status, reply) = post("s/input", json!({"text":"oi","jev_task":"","jev_message":"oi"}).to_string()).await;
    assert_eq!((status, reply["ok"].as_bool(), reply["timing_note"].as_str()), (200, Some(true), Some("delivered")), "{reply}");
    assert_eq!(reply["steered"], false, "sem contexto não há orientação automática");
    let (status, reply) = post("s/input", json!({"text":"oi","jev_task":"t"}).to_string()).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")));
    assert_eq!(call(reqwest::Method::POST, "s/input", r#"{"text":"oi"}"#, false).await.0, 404);
    // Sessão que o Rust não tem não entra pela ponte.
    assert_eq!(post("nada/input", json!({"text": "oi"}).to_string()).await.0, 421);
    // Aba Submit sem pane: o Python não tem mais a rota, e a recusa é do Rust (decisões 6 e 7), sem
    // `interaction_id` primeiro e, com ele, a do picker ausente.
    let (status, reply) = post("s/select/submit", "{}".into()).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some(MISSING)), "{reply}");
    let (status, reply) = post("s/select/submit", json!({"interaction_id": "i-1"}).to_string()).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_opcao_nao_convergiu")), "{reply}");
    // Fora do recorte: outra rota, outro método, sessão desconhecida.
    assert_eq!(post("s/not-a-route", "{}".into()).await.0, 421);
    assert_eq!(call(reqwest::Method::GET, "s/answer", "", true).await.0, 421);
    assert_eq!(post("nada/answer", json!({"answers": [option()]}).to_string()).await.0, 421);
    // O painel da fila: corpo fora de forma é do Rust (422); id com barra e leitura não são rotas dele.
    let (status, reply) = call(reqwest::Method::PATCH, "s/queue/a", r#"{"text":1}"#, true).await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{reply}");
    assert_eq!(post("s/queue/a/b/steer", String::new()).await.0, 421);
    assert_eq!(call(reqwest::Method::GET, "s/queue/a", "", true).await.0, 421);
    assert_eq!(call(reqwest::Method::POST, "s/answer", "{}", false).await.0, 404, "sem o segredo a ponte não existe");
    // O modo de permissão do convidado é atendido na rota do dono (decisão 1): a ponte não o tem mais.
    assert_eq!(call(reqwest::Method::GET, "s/permission-modes", "", true).await.0, 421);
    assert_eq!(post("s/permission-mode", json!({"mode": "plan"}).to_string()).await.0, 421);
    assert_eq!(python.hits_to("/api/sessions/s/permission-modes"), 0);
    // Btw: a pergunta lateral do Connect chega à rota do Rust, que responde até o corpo errado.
    let (status, reply) = post("s/btw", r#"{"question":"q","x":1}"#.into()).await;
    assert_eq!((status, reply["detail"][0]["type"].as_str()), (422, Some("extra_forbidden")), "{reply}");
    assert_eq!(python.hits_to("/api/sessions/s/btw"), 0);
    // A rota irmã das personalizações não é engolida pela ponte.
    assert_ne!(post("customizations", "{}".into()).await.0, 421);
    for route in ["answer", "select", "select/submit", "input"] {
        assert_eq!(python.hits_to(&format!("/api/sessions/s/{route}")), 0, "{route}: nunca volta ao Python");
    }
}

/// Servidor do dono com a lista, o multiplexador e a conta numa pasta da prova: é o mesmo `DELETE` que o
/// Connect usa (o aparelho troca a credencial pela do dono), sem a ponte `POST close`, que saiu.
#[cfg(unix)]
async fn owner_with_mux(registry: Arc<RuntimeRegistry>, home: &std::path::Path) -> (Arc<Fake>, std::net::SocketAddr) {
    use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
    let mux = home.join("tmux");
    // Nenhuma sessão no multiplexador: o `has-session` diz que ela não existe.
    crate::write_executable(&mux, "#!/bin/sh\n[ \"$1\" = has-session ] && { echo \"can't find session: x\" >&2; exit 1; }\nexit 0\n".to_owned());
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    let text = home.to_string_lossy().into_owned();
    state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
        [("HOME".to_owned(), text.clone()), ("USERPROFILE".to_owned(), text)].into()));
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"), "pi_sessions": home.join(".pi/agent/sessions"),
        "omp_config": home.join(".omp"), "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    assert!(dirs.is_some());
    state.list = Arc::new(ListBridge::new(ListEnv { mux: hangar_server::list::mux::Mux::with_program(&mux, Duration::from_secs(5)),
        capture_program: mux.clone().into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        hangar_server::list::facts::FactsClient::new()));
    assert!(state.state.runtime.set(registry).is_ok());
    (python, fake::spawn_state(state).await)
}

#[cfg(unix)]
async fn delete(server: std::net::SocketAddr, name: &str) -> (u16, Value) {
    let response = fake::client().delete(format!("http://{server}/api/sessions/{name}")).bearer_auth(OWNER).send().await.unwrap();
    let status = response.status().as_u16();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
}

/// Decisão 4: o fechamento do Claude do Rust pelo Connect é o `DELETE` da 8765, com todas as limpezas: a
/// entrada desta vida sai do Rust, a resposta é a do fechamento e nada vai ao Python; sessão que o Rust não
/// tem fecha pelo nome (já acabou: 200).
#[cfg(unix)]
#[tokio::test]
async fn connect_delete_of_a_rust_claude_closes_through_the_bridge() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    open_headless(&registry, dir.path(), "s").await;
    let home = tempfile::tempdir().unwrap();
    let (python, server) = owner_with_mux(registry.clone(), home.path()).await;
    assert_eq!(delete(server, "nada").await, (200, json!({"ok": true, "warning": null})));
    let (status, body) = delete(server, "s").await;
    assert_eq!(status, 200, "{body}");
    assert!(!registry.has_entry(None, "s").await, "o encerramento passou pelo Rust: a entrada desta vida saiu");
    assert_eq!(python.hits_to("/api/sessions/s") + python.hits_to("/api/sessions/nada"), 0, "nunca vai ao Python");
}

/// O fechamento recusado (409, troca de conversa em curso) deixa a sessão viva, e ela continua voltando no
/// reboot; o que fecha de fato a tira da fotografia.
#[cfg(unix)]
#[tokio::test]
async fn refused_connect_close_keeps_the_session_in_the_restore_snapshot() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    open_headless(&registry, dir.path(), "s").await;
    let home = tempfile::tempdir().unwrap();
    let snapshot = home.path().join(".hangar/session-restore.json");
    std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
    std::fs::write(&snapshot, json!({"sessions": {"k-s": {"name": "s", "provider": "claude", "conversation": null},
        "k-nada": {"name": "nada", "provider": "claude", "conversation": null}}}).to_string()).unwrap();
    let names = || serde_json::from_slice::<Value>(&std::fs::read(&snapshot).unwrap()).unwrap()["sessions"].as_object().unwrap()
        .values().map(|entry| entry["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    let (_python, server) = owner_with_mux(registry.clone(), home.path()).await;
    registry.ingress().hold("s", Duration::from_secs(1)).await.unwrap();
    assert_eq!(delete(server, "s").await.0, 409);
    assert!(names().contains(&"s".to_owned()), "recusado: a sessão viva segue na fotografia");
    registry.ingress().release("s");
    assert_eq!(delete(server, "nada").await.0, 200);
    assert!(!names().contains(&"nada".to_owned()), "fechada de fato, ela não volta no reboot");
}

/// Achado 4 da revisão: a ponte `POST /__hangar_server/claude/<n>/close` não tem chamador (o Connect saiu
/// do Python em 6bdd33994) e saiu. Antes ela tirava a entrada da vida do Claude do Rust; agora é 421 e
/// nada fecha: o fechamento é só o `DELETE`.
#[tokio::test]
async fn the_claude_close_bridge_is_gone() {
    let (dir, store) = (tempfile::tempdir().unwrap(), Arc::new(InteractionStore::default()));
    let (registry, _plugin) = registry_with_plugin(&store).await;
    open_headless(&registry, dir.path(), "s").await;
    let (python, upstream) = fake::spawn_fake().await;
    let state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let router = hangar_server::routes::terminal_router(Arc::new(state));
    tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let response = fake::client().post(format!("http://{private}/__hangar_server/claude/s/close"))
        .header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 421);
    assert!(registry.has_entry(None, "s").await, "nada foi encerrado: a entrada desta vida segue");
    assert_eq!(python.hits_to("/api/sessions/s"), 0);
}

/// O Reabrir (`/recarregar`) do Claude é atendido pela rota da 8765, que o Connect também usa: a recusa da
/// reabertura (aqui o arquivo da sessão ilegível) volta com o código do Rust, sem nunca ir ao Python. A
/// ponte privada não tem mais a recarga: quem a chamava era a rota Python que saiu.
#[tokio::test]
async fn reload_of_a_claude_answers_with_the_rust_code() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".hangar/claude-headless");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.json"), "não é json").unwrap();
    let env = hangar_server::accounts::environment::AccountEnvironment::from_map([("HOME".to_owned(), home.path().to_string_lossy().into_owned())].into());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_codex_environment(env));
    let (python, upstream) = fake::spawn_fake().await;
    let state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    assert!(state.state.runtime.set(registry).is_ok());
    let server = fake::spawn_state(state).await;
    let response = fake::client().post(format!("http://{server}/api/sessions/s/recarregar")).bearer_auth(fake::OWNER).send().await.unwrap();
    let status = response.status().as_u16();
    let reply = serde_json::from_str::<Value>(&response.text().await.unwrap()).unwrap_or(Value::Null);
    assert_eq!((status, reply["detail"]["code"].as_str(), reply["detail"]["params"]["code"].as_str()),
        (409, Some("erro_recarregar"), Some("claude_sidecar_read")), "{reply}");
    assert_eq!(python.hits_to("/api/sessions/s/recarregar"), 0);
}
