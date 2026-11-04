use crate::fake;
use crate::mods_support;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use fake::*;
use hangar_server::mods::bridge::mint;
use hangar_server::mods::state::Mods;
use hangar_server::routes::AppState;
use mods_support::Probe;
use serde_json::{Value, json};
use tokio::net::TcpListener;

struct Server {
    public: SocketAddr,
    bridge: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Drop for Server {
    fn drop(&mut self) { self.task.abort(); }
}

impl Server {
    /// Sobe e devolve None quando a ponte (a mesma porta em 127.0.0.1) já é de outro processo.
    async fn try_start(listener: TcpListener, state: AppState) -> Option<Self> {
        let public = listener.local_addr().unwrap();
        let bridge = SocketAddr::from((Ipv4Addr::LOCALHOST, public.port()));
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let mut task = tokio::spawn(hangar_server::serve_until_with_state(listener, state, async { let _ = stopping.await; }));
        let http = http();
        let up = match tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if task.is_finished() {
                    return false;
                }
                if let Ok(response) = http.get(format!("http://{public}/__hangar_server/health")).send().await {
                    assert_eq!(response.status().as_u16(), 200);
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await {
            Ok(up) => up,
            Err(_) => {
                let _ = stop.send(());
                task.abort();
                panic!("a saúde pública não respondeu em 5 s");
            }
        };
        if !up {
            let error = (&mut task).await.unwrap().expect_err("o servidor saiu sem erro antes da saúde");
            assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error}");
            return None;
        }
        Some(Self { public, bridge, stop: Some(stop), task })
    }


    /// Porta pública nova a cada tentativa: em paralelo, outro processo pode estar na porta da ponte.
    async fn launch(mut state: impl FnMut() -> AppState) -> Self {
        for _ in 0..5 {
            let listener = TcpListener::bind("127.0.0.2:0").await.unwrap();
            if let Some(server) = Self::try_start(listener, state()).await {
                return server;
            }
        }
        panic!("nenhuma porta livre para a ponte");
    }

    async fn stop(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), &mut self.task).await.unwrap().unwrap().is_ok());
        crate::assert_closed(self.public, "a porta pública").await;
        crate::assert_closed(self.bridge, "a ponte").await;
    }
}

fn http() -> reqwest::Client { reqwest::Client::builder().no_proxy().build().unwrap() }

async fn setup() -> (Arc<Fake>, Server, Mods) {
    let (python, upstream) = spawn_fake().await;
    let mut mods = None;
    let server = Server::launch(|| {
        let state = AppState::new(config(upstream, "127.0.0.1"));
        state.mods.attach_terminal("t", "proc-t", 1, Arc::new(Probe::default()));
        mods = Some(state.mods.clone());
        state
    }).await;
    (python, server, mods.unwrap())
}

async fn post(server: SocketAddr, route: &str, body: Value) -> reqwest::Response {
    http().post(format!("http://{server}/api/plugin/{route}"))
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap()
}

#[tokio::test]
async fn specific_bind_mirrors_terminal_ui_through_the_announced_loopback() {
    let (_python, mut server, mods) = setup().await;
    let response = post(server.bridge, "ui", json!({"sessao":"t", "token":mint(OWNER, "t"),
        "above":{"type":"Text", "children":["ponte"]}, "columns":80, "panes":[]})).await;
    assert_eq!(response.status().as_u16(), 200);
    let (_, data) = mods.replay("t").into_iter().find(|(event, _)| *event == "plugin_ui").unwrap();
    let data: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(data["source"], "terminal");
    assert_eq!(data["above"]["children"][0], "ponte");
    server.stop().await;
}

#[tokio::test]
async fn click_url_uses_the_same_attempt_on_public_and_loopback_entries() {
    let (_python, mut server, mods) = setup().await;
    let attempt = mods.begin_click("t", "band", "test", "link");
    let start = post(server.bridge, "press-start", json!({"sessao":"t", "token":mint(OWNER,"t"),
        "requestId":"band", "plugin":"test", "element":"link"})).await;
    assert_eq!(start.status().as_u16(), 200);
    let started: Value = serde_json::from_str(&start.text().await.unwrap()).unwrap();
    assert_eq!(started["fromApp"], true);
    assert_eq!(started["attempt"], attempt);
    let response = post(server.bridge, "opened", json!({"sessao":"t", "token":mint(OWNER,"t"),
        "attempt":attempt, "url":"https://example.com/proof"})).await;
    assert_eq!(response.status().as_u16(), 200);
    let repeated = post(server.public, "press-start", json!({"sessao":"t", "token":mint(OWNER,"t"),
        "requestId":"band", "plugin":"test", "element":"link"})).await;
    let repeated: Value = serde_json::from_str(&repeated.text().await.unwrap()).unwrap();
    assert_eq!(repeated["fromApp"], false);
    assert_eq!(mods.finish_click("t", &attempt, Duration::ZERO).await, (None, Some("https://example.com/proof".into())));
    server.stop().await;
}

#[tokio::test]
async fn bridge_never_promotes_owner_guest_or_internal_credentials_to_plugin_identity() {
    let (python, mut server, mods) = setup().await;
    for extra_header in [("authorization", format!("Bearer {OWNER}")), ("authorization", "Bearer guest".into()),
        ("x-hangar-internal", SECRET.into())] {
        let response = http().post(format!("http://{}/api/plugin/ui", server.bridge))
            .header(extra_header.0, extra_header.1).header("content-type", "application/json")
            .body(json!({"sessao":"t", "token":"wrong", "above":null, "panes":[]}).to_string()).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 403);
    }
    let missing = post(server.bridge, "ui", json!({"sessao":"t", "above":null, "panes":[]})).await;
    assert!(!missing.status().is_success());
    assert!(mods.replay("t").is_empty());
    assert_eq!(python.hits_to("/api/plugin/ui"), 0);
    server.stop().await;
}

async fn echo(request: axum::extract::Request) -> impl axum::response::IntoResponse {
    let path = request.uri().path_and_query().unwrap().as_str().to_owned();
    let method = request.method().to_string();
    let client_ip = request.headers().get("x-forwarded-for").map(|value| value.to_str().unwrap().to_owned());
    let internal = request.headers().contains_key("x-hangar-internal");
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    ([("content-type", "application/json")], json!({"path":path, "method":method, "client_ip":client_ip, "internal":internal, "body":String::from_utf8(body.to_vec()).unwrap()}).to_string())
}

#[tokio::test]
async fn plugin_endpoints_are_rust_and_mods_methods_keep_the_python_path() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async { axum::serve(upstream, axum::Router::new().fallback(echo)).await });
    let mut server = Server::launch(|| AppState::new(config(address, "127.0.0.1"))).await;
    // As nove rotas do plugin são do Rust nas duas portas: pedido sem envelope é recusado aqui, nunca ecoado.
    for base in [server.bridge, server.public] {
        for endpoint in ["whoami", "pull", "suggest", "ask", "ask-fim", "filled", "submitted", "state", "rate"] {
            let response = http().post(format!("http://{base}/api/plugin/{endpoint}?check=1"))
                .header("x-hangar-internal", SECRET).body(json!({"probe":endpoint}).to_string()).send().await.unwrap();
            assert!(response.status().is_client_error(), "{endpoint}: {}", response.status());
            assert!(!response.text().await.unwrap().contains("\"path\""), "{endpoint}: chegou ao Python");
        }
    }
    let response = http().request(reqwest::Method::OPTIONS, format!("http://{}/api/plugin/ui", server.bridge)).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let received: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(received["path"], "/api/plugin/ui");
    assert_eq!(received["method"], "OPTIONS");
    server.stop().await;
    upstream_task.abort();
}

#[tokio::test]
async fn loopback_does_not_publish_the_rest_of_the_public_or_private_api() {
    let (python, mut server, _mods) = setup().await;
    for path in ["/api/sessions", "/internal/list/facts", "/__hangar_server/health", "/__hangar_server/terminal",
        "/api/sessions/t/plugin/press", "/api/plugin-extra/whoami", "/api/plugin/%2e%2e/sessions", "/api/plugin/missing"] {
        let before = python.hits_to(path);
        let response = http().post(format!("http://{}{path}", server.bridge)).header("authorization", format!("Bearer {OWNER}"))
            .header("x-hangar-internal", SECRET).body("{}").send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
        assert_eq!(python.hits_to(path), before, "{path}");
    }
    server.stop().await;
}

#[tokio::test]
async fn occupied_bridge_fails_before_public_health_and_releases_our_sockets() {
    let (_python, upstream) = spawn_fake().await;
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = occupied.local_addr().unwrap();
    let public = SocketAddr::from((Ipv4Addr::new(127,0,0,2), address.port()));
    let listener = TcpListener::bind(public).await.unwrap();
    let result = hangar_server::routes::serve(listener, config(upstream, "127.0.0.1")).await;
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
    crate::assert_closed(public, "a porta pública").await;
    assert!(tokio::net::TcpStream::connect(address).await.is_ok());
}

/// Sobe de novo nas portas que o servidor parado soltou. Um filho de outro teste, entre o fork e o exec,
/// pode carregar por instantes a cópia dos sockets antigos; o exec os solta, e o prazo cobre isso.
async fn restart(address: SocketAddr, upstream: SocketAddr) -> Server {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(listener) = TcpListener::bind(address).await
            && let Some(server) = Server::try_start(listener, AppState::new(config(upstream, "127.0.0.1"))).await
        {
            return server;
        }
        assert!(std::time::Instant::now() < deadline, "o servidor não voltou nas mesmas portas");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn stopping_allows_a_restart_on_the_same_public_and_bridge_port() {
    let (_python, upstream) = spawn_fake().await;
    // A primeira partida sorteia portas livres (a da ponte pode ser de outro teste); a segunda volta nelas.
    let mut server = Server::launch(|| AppState::new(config(upstream, "127.0.0.1"))).await;
    let address = server.public;
    for again in [false, true] {
        if again {
            server = restart(address, upstream).await;
        }
        // A rota do plugin é do Rust: corpo vazio é recusado por ele (422), prova de que a ponte atende.
        let response = post(server.bridge, "state", json!({})).await;
        assert_eq!(response.status().as_u16(), 422);
        server.stop().await;
    }
}

mod held_routes {
    use super::*;
    use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
    use hangar_server::{auth::TrustedHosts, interactions::{InteractionBinding, InteractionStore}, plugin::{self, routes::{Discovery, Identity, MeasuredContext, RouteState, SourceFuture, Sources, Whoami}, PermissionGate, ServiceError}};
    use hangar_server::mods::bridge::{machine_key, mint_keyed};

    struct Facts {
        enabled: AtomicBool,
        identity: Identity,
        rows: Vec<Discovery>,
        rate: Mutex<Option<(u64, f64)>>,
        context: Mutex<Option<(u64, u64)>>,
        turn_errors: Mutex<Vec<String>>,
        adoptions: Mutex<Vec<(String, String, String)>>,
    }
    impl Sources for Facts {
        fn enabled(&self) -> SourceFuture<'_, bool> { Box::pin(async { Ok(self.enabled.load(Ordering::Relaxed)) }) }
        fn resolve<'a>(&'a self, name: &'a str, key: Option<&'a str>) -> SourceFuture<'a, Identity> {
            Box::pin(async move {
                if key.is_some_and(|key| key == self.identity.plugin_key || key == self.identity.binding.key)
                    || key.is_none() && name == self.identity.name { Ok(self.identity.clone()) }
                else { Err(ServiceError("plugin_identity_missing")) }
            })
        }
        fn discovery(&self) -> SourceFuture<'_, Vec<Discovery>> { Box::pin(async { Ok(self.rows.clone()) }) }
        fn permission_gate<'a>(&'a self, _: &'a Identity) -> SourceFuture<'a, PermissionGate> {
            Box::pin(async { Ok(PermissionGate { app_present: true, terminal_attached: false, mode_without_dialog: false }) })
        }
        fn band_present<'a>(&'a self, _: &'a Identity) -> SourceFuture<'a, bool> { Box::pin(async { Ok(false) }) }
        fn context<'a>(&'a self, _: &'a Identity, context: MeasuredContext) -> SourceFuture<'a, ()> {
            Box::pin(async move { *self.context.lock().unwrap() = Some((context.used, context.window)); Ok(()) })
        }
        fn rate<'a>(&'a self, _: &'a Identity, tokens: u64, seconds: f64) -> SourceFuture<'a, ()> {
            Box::pin(async move { *self.rate.lock().unwrap() = Some((tokens, seconds)); Ok(()) })
        }
        fn turn_error<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, ()> {
            Box::pin(async move { self.turn_errors.lock().unwrap().push(identity.name.clone()); Ok(()) })
        }
        fn adopt<'a>(&'a self, name: &'a str, key: &'a str, conversation: &'a str) -> SourceFuture<'a, bool> {
            Box::pin(async move { self.adoptions.lock().unwrap().push((name.into(), key.into(), conversation.into())); Ok(false) })
        }
    }
    struct Running(tokio::task::JoinHandle<()>);
    impl Drop for Running { fn drop(&mut self) { self.0.abort(); } }
    async fn start() -> (Arc<plugin::Store>, Arc<Facts>, SocketAddr, Running) {
        let identity = Identity { name: "renamed".into(), plugin_key: "process-key".into(),
            binding: InteractionBinding { key: "runtime-key".into(), conversation: "sid".into(), generation: 2 } };
        let facts = Arc::new(Facts { enabled: AtomicBool::new(true), rows: vec![Discovery { identity: identity.clone(), pane: Some("%1".into()),
            socket: Some("/tmp/tmux-socket".into()), mux_session: Some("$2".into()), psmux_pid: None }], identity,
            rate: Mutex::new(None), context: Mutex::new(None), turn_errors: Mutex::new(Vec::new()), adoptions: Mutex::new(Vec::new()) });
        let store = Arc::new(plugin::Store::new(Arc::new(InteractionStore::default())));
        store.interactions().bind(&facts.identity.binding).unwrap();
        store.bind(&facts.identity.binding).unwrap();
        let state = Arc::new(RouteState { store: store.clone(), secret: OWNER.into(), trusted: TrustedHosts::parse("127.0.0.1"), sources: facts.clone() });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, plugin::routes::router::<()>(state).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap(); });
        (store, facts, address, Running(task))
    }
    fn envelope() -> Value { json!({"sessao":"original","token":mint_keyed(OWNER,"original","process-key")}) }
    async fn value(response: reqwest::Response) -> Value { serde_json::from_str(&response.text().await.unwrap()).unwrap() }

    // Unix: o agente falso precisa de `argv[0]` próprio (`arg0`).
    #[cfg(unix)]
    #[tokio::test]
    async fn plugin_loopback_held_appstate_and_runtime_share_one_store_and_identity_registry() {
        if std::env::var_os("HANGAR_PLUGIN_BOOT_CHECK").is_none() {
            let home = tempfile::tempdir().unwrap();
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &crate::exact_name(module_path!(), "plugin_loopback_held_appstate_and_runtime_share_one_store_and_identity_registry"), "--nocapture"])
                .env("HANGAR_PLUGIN_BOOT_CHECK", "1").env("HANGAR_RUNTIME_INSTANCE", "plugin-boot")
                .env("HOME", home.path()).env("USERPROFILE", home.path()).env("XDG_CACHE_HOME", home.path().join("cache"))
                .env_remove("CLAUDE_CONFIG_DIR").env_remove("CODEX_HOME")
                .env("CP_CLAUDE_CONFIG_DIRS", "[]").env("HANGAR_LIST_DIRS", "")
                .output().await.unwrap();
            assert!(output.status.success(), "partida isolada falhou: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            crate::assert_ran_one(&output.stdout);
            return;
        }
        let (_python, upstream) = spawn_fake().await;
        let mut shared = None;
        let mut server = Server::launch(|| {
            let state = AppState::new(config(upstream, "127.0.0.1"));
            shared = Some((state.plugin.clone(), state.interactions.clone(), state.state.clone()));
            state
        }).await;
        let (plugin, interactions, live) = shared.unwrap();
        let registry = live.runtime.get().expect("a inicialização de produção precisa ligar o runtime");
        assert!(Arc::ptr_eq(plugin.interactions(), &interactions));
        assert!(Arc::ptr_eq(registry.plugin().unwrap(), &plugin));

        let dir = tempfile::tempdir().unwrap();
        let (socket, _seen, cano) = mods_support::vitrine_cano(&[]).await;
        let _cano = Running(cano);
        // O registro de produção comprova o agente pelos processos reais: um `claude` da conversa, filho
        // deste processo (que faz as vezes do cano).
        let mut agent = {
            use std::os::unix::process::CommandExt;
            std::process::Command::new("/bin/sh").arg0("claude").args(["-c", "sleep 60; :", "--session-id", "sid-1"]).spawn().unwrap()
        };
        let mut target = mods_support::claude_target(dir.path(), socket, true);
        target.binding.pid = std::process::id();
        let opened = registry.open(target).await;
        let binding = opened.and(registry.interaction_binding("session").await);
        let _ = agent.kill(); let _ = agent.wait();
        let binding = binding.unwrap();
        let ask = plugin::Ask { id:"perm:boot".into(), questions:None, tool:Some("Bash".into()), janela_ms:None, summary:None };
        let poll = plugin.begin_ask(&binding, ask, PermissionGate { app_present:true, terminal_attached:false, mode_without_dialog:false }, std::time::Instant::now()).unwrap();
        let pending = plugin.pending(&binding, std::time::Instant::now()).unwrap();
        assert_eq!(interactions.validate(&binding, Some(&pending.interaction_id)).unwrap(),
            hangar_server::interactions::plugin_target(hangar_server::interactions::InteractionKind::Approval, "perm:boot"));
        drop(poll);
        registry.close(&binding.key, binding.generation).await.unwrap();
        assert!(interactions.validate(&binding, Some(&pending.interaction_id)).is_err());
        server.stop().await;
    }

    #[tokio::test]
    async fn plugin_loopback_held_stale_key_is_admitted_by_its_signed_name() {
        let (_store, _facts, address, _server) = start().await;
        let stale = json!({"sessao":"renamed","token":mint_keyed(OWNER,"renamed","python-key"),"estado":"working"});
        assert_eq!(post(address, "state", stale).await.status().as_u16(), 200);
        let ghost = json!({"sessao":"ghost","token":mint_keyed(OWNER,"ghost","python-key"),"estado":"working"});
        assert_eq!(post(address, "state", ghost).await.status().as_u16(), 409);
    }

    #[tokio::test]
    async fn plugin_loopback_pull_of_a_session_without_actor_asks_the_adoption_with_its_conversation() {
        let (_store, facts, address, _server) = start().await;
        let pull = |modos: Value, sid: Value| json!({"sessao":"ghost","token":mint_keyed(OWNER,"ghost","ghost-key"),"instance":"i","modos":modos,"session_id":sid});
        assert_eq!(post(address, "pull", pull(json!(["fill"]), json!("sid-ghost"))).await.status().as_u16(), 409, "a adoção não saiu: segue recusado");
        assert_eq!(*facts.adoptions.lock().unwrap(), [("ghost".to_owned(), "ghost-key".to_owned(), "sid-ghost".to_owned())]);
        assert_eq!(post(address, "pull", pull(json!(["agent_message"]), json!("sid-filho"))).await.status().as_u16(), 409);
        assert_eq!(post(address, "pull", pull(json!(["fill"]), Value::Null)).await.status().as_u16(), 409);
        assert_eq!(facts.adoptions.lock().unwrap().len(), 1, "/pull só de subagente ou sem conversa não pede adoção");
    }

    #[tokio::test]
    async fn plugin_loopback_held_auth_switch_and_rename_use_the_same_binding() {
        let (store, facts, address, _server) = start().await;
        for token in [OWNER.to_owned(), "guest".into(), machine_key(OWNER), mint(OWNER, "other")] {
            let response = post(address, "state", json!({"sessao":"original","token":token,"estado":"working"})).await;
            assert_eq!(response.status().as_u16(), 403);
        }
        let mut body = envelope(); body["estado"] = json!("working");
        assert_eq!(post(address, "state", body.clone()).await.status().as_u16(), 200);
        assert_eq!(store.facts(&facts.identity.binding, std::time::Instant::now()).unwrap()["plugin_state"]["state"], "working");
        body["token"] = json!(mint_keyed(OWNER, "original", "untracked"));
        assert_eq!(post(address, "state", body).await.status().as_u16(), 409);
        // Parity P25: desligar o caminho do plugin vale para quem nasce (`whoami`); a sessão já aberta segue.
        facts.enabled.store(false, Ordering::Relaxed);
        let mut body = envelope(); body["texto"] = json!("sugestão"); body["mostrada"] = json!(true);
        assert_eq!(post(address, "suggest", body).await.status().as_u16(), 200);
        assert_eq!(value(post(address, "whoami", json!({"chave":machine_key(OWNER),"pane":"%1","session_id":"sid"})).await).await, json!({"sessao":null}));
    }

    #[tokio::test]
    async fn plugin_loopback_held_whoami_never_promotes_bearer_or_forwarded_origin() {
        let (_store, _facts, address, _server) = start().await;
        let body = json!({"chave":machine_key(OWNER),"pane":"%1","tmux":"/tmp/tmux-socket,1,2","session_id":"sid"});
        let response = post(address, "whoami", body.clone()).await;
        assert_eq!(response.status().as_u16(), 200);
        let identity = value(response).await;
        assert_eq!(identity["sessao"], "renamed");
        assert_eq!(identity["token"], mint_keyed(OWNER,"renamed","process-key"));
        for key in [OWNER, "guest"] {
            let mut wrong = body.clone(); wrong["chave"] = json!(key);
            let response = http().post(format!("http://{address}/api/plugin/whoami")).header("authorization",format!("Bearer {OWNER}"))
                .header("content-type","application/json").body(wrong.to_string()).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 403);
        }
        let response = http().post(format!("http://{address}/api/plugin/whoami")).header("x-forwarded-for","192.0.2.1")
            .header("content-type","application/json").body(body.to_string()).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 403);
        let mut wrong = body.clone(); wrong["tmux"] = json!("/another/socket,1,2");
        assert_eq!(value(post(address,"whoami",wrong).await).await["sessao"], Value::Null);
        let mut wrong = body; wrong["session_id"] = json!("other");
        assert_eq!(value(post(address,"whoami",wrong).await).await["sessao"], Value::Null);
    }

    #[test]
    fn plugin_loopback_held_discovery_rejects_ambiguity_and_uses_psmux_pid() {
        let identity = |name: &str| Identity { name: name.into(), plugin_key: name.into(), binding: InteractionBinding {key:name.into(),conversation:"sid".into(),generation:1} };
        let mut rows = vec![Discovery {identity:identity("one"),pane:Some("%1".into()),socket:None,mux_session:None,psmux_pid:Some("123".into())},
            Discovery {identity:identity("two"),pane:Some("%1".into()),socket:None,mux_session:None,psmux_pid:Some("456".into())}];
        let mut body = Whoami {chave:String::new(),pane:Some("%1".into()),nome:Some("wrong".into()),tmux:Some("psmux-123,0,0".into()),session_id:Some("sid".into())};
        assert_eq!(plugin::routes::discover(&body,&rows).unwrap().unwrap().0.name,"one");
        for row in &mut rows { row.psmux_pid = None; row.socket = Some("/tmp/socket".into()); }
        body.tmux = Some("/tmp/socket,0,0".into());
        assert_eq!(plugin::routes::discover(&body,&rows).err().unwrap().0,"plugin_identity_ambiguous");
    }

    #[tokio::test]
    async fn plugin_loopback_every_turn_end_is_checked_for_the_usage_limit() {
        // O limite semanal chega num fim de turno que não vem com motivo `error`: todo idle confere.
        let (_store, facts, address, _server) = start().await;
        for motivo in [json!("completed"), json!("api-error"), json!("error"), Value::Null] {
            let mut body = envelope(); body["estado"] = json!("idle"); body["motivo"] = motivo.clone();
            assert_eq!(post(address, "state", body).await.status().as_u16(), 200, "{motivo}");
        }
        let mut working = envelope(); working["estado"] = json!("working");
        assert_eq!(post(address, "state", working).await.status().as_u16(), 200);
        assert_eq!(facts.turn_errors.lock().unwrap().len(), 4, "um por fim de turno, nenhum no working");
    }

    #[tokio::test]
    async fn plugin_loopback_held_python_limits_and_auxiliary_data_are_preserved() {
        let (_store, facts, address, _server) = start().await;
        let mut body = envelope(); body["estado"] = json!("idle"); body["session_id"] = json!("sid");
        for used in [json!(0), json!(true), json!("5")] {
            body["context"] = json!({"used":used,"window":100});
            assert_eq!(post(address,"state",body.clone()).await.status().as_u16(),422);
        }
        body["context"] = json!({"used":10,"window":100});
        assert_eq!(post(address,"state",body).await.status().as_u16(),200);
        assert_eq!(*facts.context.lock().unwrap(),Some((10,100)));
        let mut body = envelope(); body["tokens"] = json!(5); body["seconds"] = json!(0.5); body["session_id"] = json!("sid");
        assert_eq!(post(address,"rate",body.clone()).await.status().as_u16(),200);
        assert_eq!(*facts.rate.lock().unwrap(),Some((5,0.5)));
        for (tokens, seconds) in [(0,1.0),(1_000_001,1.0),(1,0.0),(1,3601.0)] {
            body["tokens"] = json!(tokens); body["seconds"] = json!(seconds);
            assert_eq!(post(address,"rate",body.clone()).await.status().as_u16(),422);
        }
        let mut body = envelope(); body["texto"] = json!("a".repeat(32 * 1024)); body["mostrada"] = json!(true);
        assert_eq!(post(address,"suggest",body).await.status().as_u16(),200);
    }

    #[tokio::test]
    async fn plugin_loopback_held_proxy_disconnect_releases_only_its_waiter() {
        let (store, facts, address, _server) = start().await;
        let mut body = envelope(); body["instance"] = json!("i"); body["session_id"] = json!("sid");
        let request = tokio::spawn(post(address,"pull",body));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !store.waiting(&facts.identity.binding).unwrap() { tokio::task::yield_now().await; }
        }).await.unwrap();
        request.abort();
        let _ = request.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while store.waiting(&facts.identity.binding).unwrap() { tokio::task::yield_now().await; }
        }).await.expect("a conexão encerrada não pode segurar a próxima publicação");
        assert_eq!(store.publish(&facts.identity.binding,plugin::PluginRequest {id:"not-sent".into(),text:"texto".into(),mode:plugin::PluginMode::Fill}).await.unwrap(),plugin::PluginReply::Unavailable);
    }

    #[tokio::test]
    async fn plugin_loopback_held_permission_form_and_deny_reach_the_real_ask_body() {
        use futures_util::FutureExt;
        let (store, facts, address, _server) = start().await;
        let binding = &facts.identity.binding;
        for (request_id, body) in [
            ("perm:allow", json!({"permitir":true})),
            ("perm:deny", json!({"permitir":false})),
            ("ask:form", json!({"answers":{"Escolha":"A"}})),
            ("ask:skip", json!({"deny":"O usuário prefere conversar antes de responder."})),
        ] {
            let mut request = envelope(); request["id"] = json!(request_id);
            if request_id.starts_with("perm:") { request["tool"] = json!("Bash"); }
            else { request["questions"] = json!([{ "question":"Escolha", "options":["A","B"] }]); }
            let poll = tokio::spawn(post(address, "ask", request));
            let pending = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Some(pending) = store.pending(binding, std::time::Instant::now()) { break pending; }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            let receipt = plugin::Receipt { publication_id:request_id.into(), generation:binding.generation, session_id:binding.conversation.clone() };
            let mut answer = Box::pin(store.answer(binding, request_id, &pending.interaction_id, body.clone(), Some(receipt.clone())));
            assert!(answer.as_mut().now_or_never().is_none());
            let response = poll.await.unwrap();
            assert_eq!(response.status().as_u16(), 200);
            let mut expected = body;
            expected["publication_id"] = json!(receipt.publication_id);
            expected["generation"] = json!(receipt.generation);
            expected["session_id"] = json!(receipt.session_id);
            assert_eq!(value(response).await, expected);
            let mut finish = envelope(); finish["id"] = json!(request_id); finish["vencedor"] = json!("app");
            for key in ["publication_id", "generation", "session_id"] { finish[key] = expected[key].clone(); }
            assert_eq!(post(address, "ask-fim", finish).await.status().as_u16(), 200);
            assert_eq!(answer.await.unwrap(), plugin::PluginReply::Accepted);
        }
    }

    #[tokio::test]
    async fn plugin_loopback_only_an_accepted_finish_ends_the_question() {
        let (_store, _facts, address, _server) = start().await;
        // Recibo torto: o fim é recusado e a pergunta segue aberta para o menu e a fila.
        let mut body = envelope(); body["id"] = json!("ask:toolu_fim_recusado"); body["vencedor"] = json!("terminal"); body["publication_id"] = json!("p");
        assert!(post(address,"ask-fim",body).await.status().is_client_error());
        assert!(!hangar_server::state::ask::ended_by_plugin("sid",Some("toolu_fim_recusado"),0.0));
        // O fim aceito (o terminal venceu, sem recibo) encerra a chamada.
        let mut body = envelope(); body["id"] = json!("ask:toolu_fim_aceito"); body["vencedor"] = json!("terminal");
        assert_eq!(post(address,"ask-fim",body).await.status().as_u16(),200);
        assert!(hangar_server::state::ask::ended_by_plugin("sid",Some("toolu_fim_aceito"),0.0));
    }

    #[tokio::test]
    async fn plugin_loopback_explicit_drop_is_not_written_but_failed_submit_stays_unknown() {
        use futures_util::FutureExt;
        for dropped in [Some(true), Some(false), None] {
            let (store, facts, address, _server) = start().await;
            let binding = &facts.identity.binding;
            let poll = store.begin_pull(binding, plugin::Pull {
                instance: "i".into(), modos: vec!["user".into(), "receipt_v2".into()],
                estado: Some("idle".into()), session_id: Some(binding.conversation.clone()),
            }, std::time::Instant::now()).unwrap();
            let mut publish = Box::pin(store.publish(binding, hangar_server::terminal_input::PluginRequest {
                id: "refused".into(), text: "pode continuar".into(), mode: hangar_server::terminal_input::PluginMode::User,
            }));
            assert!(publish.as_mut().now_or_never().is_none());
            let publication = poll.wait().await;
            let mut body = envelope(); body["ok"] = json!(false);
            for key in ["publication_id", "generation", "session_id"] { body[key] = publication[key].clone(); }
            if let Some(dropped) = dropped { body["dropped"] = json!(dropped); }
            if dropped == Some(true) {
                let mut stale = body.clone(); stale["generation"] = json!(binding.generation + 1);
                assert_eq!(post(address, "submitted", stale).await.status().as_u16(), 200);
                assert!(publish.as_mut().now_or_never().is_none(), "recibo de outra vida não autoriza reenvio");
                let mut contradictory = body.clone(); contradictory["ok"] = json!(true);
                assert!(post(address, "submitted", contradictory).await.status().is_client_error());
                assert!(post(address, "filled", body.clone()).await.status().is_client_error());
            }
            assert_eq!(post(address, "submitted", body).await.status().as_u16(), 200);
            assert_eq!(publish.await.unwrap(), if dropped == Some(true) { plugin::PluginReply::NotWritten } else { plugin::PluginReply::Unknown });
        }
    }

    #[tokio::test]
    async fn plugin_loopback_held_legacy_finish_and_receipts_keep_wire_names() {
        use futures_util::FutureExt;
        let (store, facts, address, _server) = start().await;
        let binding = &facts.identity.binding;
        let ask = plugin::Ask {id:"q".into(),questions:Some(vec![]),tool:None,janela_ms:None,summary:None};
        let poll = store.begin_ask(binding,ask,PermissionGate {app_present:true,terminal_attached:false,mode_without_dialog:false},std::time::Instant::now()).unwrap();
        let id = store.pending(binding,std::time::Instant::now()).unwrap().interaction_id;
        let mut answer = Box::pin(store.answer(binding,"q",&id,json!({"answers":{"Escolha":"A"}}),None));
        assert!(answer.as_mut().now_or_never().is_none());
        assert_eq!(poll.wait().await,json!({"answers":{"Escolha":"A"}}));
        let mut body = envelope(); body["id"] = json!("q"); body["vencedor"] = json!("app");
        assert_eq!(post(address,"ask-fim",body).await.status().as_u16(),200);
        assert_eq!(answer.await.unwrap(),plugin::PluginReply::Accepted);
        for suffix in ["filled","submitted"] {
            let mut body = envelope(); body["ok"] = json!(true); body["session_id"] = json!("sid");
            assert_eq!(post(address,suffix,body).await.status().as_u16(),200);
        }
        let mut body = envelope(); body["instance"] = json!("i"); body["session_id"] = json!("other");
        assert_eq!(post(address,"pull",body).await.status().as_u16(),409);
    }
}

#[tokio::test]
async fn start_reports_a_bridge_port_taken_by_someone_else() {
    let (_python, upstream) = spawn_fake().await;
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 2), occupied.local_addr().unwrap().port())).await.unwrap();
    let started = tokio::time::Instant::now();
    assert!(Server::try_start(listener, AppState::new(config(upstream, "127.0.0.1"))).await.is_none());
    assert!(started.elapsed() < Duration::from_secs(2), "esperou a saúde de um servidor que não subiu");
}
