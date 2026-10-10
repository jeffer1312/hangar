use crate::fake;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake::*;
use hangar_server::mods::model::*;
use hangar_server::mods::state::*;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use serde_json::{Value, json};

/// Os apps desistem do pedido em 8 s: a resposta da rota, recusa incluída, tem de chegar antes.
const APP_GIVES_UP: Duration = Duration::from_secs(8);

#[derive(Default)]
struct FakeInner {
    calls: Mutex<Vec<ModsCall>>,
    replies: Mutex<VecDeque<Result<Value, ModsError>>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    delay: Duration,
    copy: Option<(Mods, String)>,
}

/// Tipo local (A2): `impl SurfaceLink for Arc<…>` seria trait de fora em tipo de fora (E0117).
#[derive(Clone, Default)]
struct FakeLink(Arc<FakeInner>);

impl FakeLink {
    fn with(inner: FakeInner) -> Self { Self(Arc::new(inner)) }
}

impl std::ops::Deref for FakeLink {
    type Target = FakeInner;
    fn deref(&self) -> &FakeInner { &self.0 }
}

impl SurfaceLink for FakeLink {
    fn call(&self, call: ModsCall, _: Instant) -> CallFuture {
        let link = self.0.clone();
        Box::pin(async move {
            let now = link.active.fetch_add(1, SeqCst) + 1;
            link.peak.fetch_max(now, SeqCst);
            link.calls.lock().unwrap().push(call);
            tokio::time::sleep(link.delay).await;
            if let Some((mods, text)) = &link.copy { mods.copied("s", 1, "vitrine", text); }
            link.active.fetch_sub(1, SeqCst);
            link.replies.lock().unwrap().pop_front().unwrap_or(Ok(json!({"element": "ok", "shown_id": "painel", "value": "olá"})))
        })
    }
}

async fn setup(link: FakeLink) -> (Arc<Fake>, std::net::SocketAddr, Mods, FakeLink) {
    let (python, server, mods) = serve_mods("s", Arc::new(link.clone())).await;
    (python, server, mods, link)
}

async fn setup_gated(link: FakeLink) -> (std::net::SocketAddr, Arc<RuntimeRegistry>, FakeLink) {
    let (_python, server, _mods, registry) = serve_mods_gated("s", Arc::new(link.clone())).await;
    (server, registry, link)
}

fn abrir() -> Value { json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}) }

async fn post(server: std::net::SocketAddr, name: &str, route: &str, body: Value, token: Option<&str>) -> (u16, Value) {
    // O `reqwest` do crate não tem a função `json`: o corpo vai pronto, com o tipo.
    let mut request = client().post(format!("http://{server}/api/sessions/{name}/plugin/{route}"))
        .header("content-type", "application/json").body(body.to_string());
    if let Some(token) = token { request = request.header("authorization", format!("Bearer {token}")); }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test]
async fn press_and_close_go_to_the_surface() {
    let (python, server, _mods, link) = setup(FakeLink::default()).await;
    assert_eq!(post(server, "s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), Some(OWNER)).await, (200, json!({"ok": true})));
    assert_eq!(post(server, "s", "close", json!({"site": "painel"}), Some(OWNER)).await.0, 200);
    assert_eq!(*link.calls.lock().unwrap(), vec![
        ModsCall::Press { site: "above-prompt".into(), plugin: "vitrine".into(), key: "abrir".into() },
        ModsCall::Close { site: "painel".into() }]);
    assert!(python.hits_to("/internal/sessions/s/transfer") == 0, "a guarda da troca de agente não pergunta ao Python");
}

#[tokio::test]
async fn refusal_becomes_409_with_code() {
    let link = FakeLink::default();
    link.replies.lock().unwrap().push_back(Err(stale()));
    let (_python, server, _mods, _link) = setup(link).await;
    let (status, body) = post(server, "s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), Some(OWNER)).await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"]["code"], "erro_mod_desenho_vencido");
}

#[tokio::test]
async fn other_session_or_guest_goes_to_python() {
    let (python, server, _mods, link) = setup(FakeLink::default()).await;
    assert_eq!(post(server, "outra", "press", json!({"site": "x", "plugin": "vitrine", "key": "y"}), Some(OWNER)).await.1, "from-python");
    assert_eq!(python.hits_to("/api/sessions/outra/plugin/press"), 1);
    assert_eq!(post(server, "outra", "show", json!({"site": "x"}), Some(OWNER)).await.1, "from-python");
    assert_eq!(post(server, "outra", "close", json!({"site": "x"}), Some(OWNER)).await.1, "from-python");
    assert_eq!(python.hits_to("/api/sessions/outra/plugin/close"), 1);
    assert_eq!(post(server, "s", "input", json!({"site": "x", "plugin": "vitrine", "key": "y", "kind": "change", "value": ""}), Some("errado")).await.1, "from-python");
    assert!(link.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn input_outside_the_surface_is_refused_in_rust() {
    // Sem superfície no Rust não há por onde digitar no campo do mod, e o Python não tem a rota: a recusa
    // sai do Rust, com o código que o app traduz.
    let (python, server, _mods, _link) = setup(FakeLink::default()).await;
    let (status, body) = post(server, "outra", "input", json!({"site": "p", "plugin": "vitrine", "key": "k", "kind": "submit", "value": "olá"}), Some(OWNER)).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_sem_digitacao")));
    // B4: a `msg` é a frase nova, que vale para a sessão com terminal e para a sem terminal fora da superfície.
    assert_eq!(body["detail"]["msg"], "Nesta sessão, o campo do mod só aceita digitação no terminal ou não está ligado ao app.");
    assert_eq!(python.hits_to("/api/sessions/outra/plugin/input"), 0);
}

#[tokio::test]
async fn transfer_in_progress_is_refused_before_the_mod() {
    // A troca retém a porta da sessão: toda operação, inclusive cada `change` do campo, é recusada na hora.
    let (server, registry, link) = setup_gated(FakeLink::default()).await;
    registry.ingress().hold("s", Duration::from_secs(1)).await.unwrap();
    for (route, body) in [("press", abrir()), ("show", json!({"site": "painel"})),
                          ("input", json!({"site": "painel", "plugin": "vitrine", "key": "V18-campo", "kind": "change", "value": "o"}))] {
        let start = Instant::now();
        let (status, answer) = post(server, "s", route, body, Some(OWNER)).await;
        assert_eq!((status, answer["detail"]["code"].as_str()), (409, Some("session_transfer_busy")), "{route}");
        assert!(start.elapsed() < Duration::from_secs(1), "{route}: {:?}", start.elapsed());
    }
    assert!(link.calls.lock().unwrap().is_empty(), "nada chega ao mod durante a troca");
    registry.ingress().release("s");
    assert_eq!(post(server, "s", "press", abrir(), Some(OWNER)).await.0, 200);
}

#[tokio::test]
async fn transfer_waits_for_the_whole_operation() {
    // A troca que começa com a operação em curso só fecha a porta depois da resposta: o mod nunca roda com
    // a troca já começada.
    let (server, registry, link) = setup_gated(FakeLink::with(FakeInner { delay: Duration::from_millis(400), ..Default::default() })).await;
    let request = tokio::spawn(post(server, "s", "press", abrir(), Some(OWNER)));
    while link.active.load(SeqCst) == 0 { tokio::time::sleep(Duration::from_millis(5)).await; }
    registry.ingress().hold("s", Duration::from_secs(5)).await.unwrap();
    assert_eq!(link.active.load(SeqCst), 0, "a porta fechou com o mod ainda rodando");
    assert_eq!(request.await.unwrap().0, 200);
    assert_eq!(post(server, "s", "show", json!({"site": "painel"}), Some(OWNER)).await.1["detail"]["code"], "session_transfer_busy");
}

#[tokio::test]
async fn short_freeze_is_waited_inside_the_budget() {
    // Fechamento comum (relançar, renomear) não é a troca: a operação espera reabrir; sem reabrir no
    // orçamento, é a recusa do mod sem resposta, antes de o app desistir.
    let (server, registry, link) = setup_gated(FakeLink::default()).await;
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let gates = registry.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(200)).await; gates.ingress().open("s") });
    assert_eq!(post(server, "s", "press", abrir(), Some(OWNER)).await.0, 200);
    registry.ingress().close("s", Duration::from_secs(1)).await.unwrap();
    let start = Instant::now();
    let (status, body) = post(server, "s", "press", abrir(), Some(OWNER)).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_clique_sem_resposta")));
    assert!(start.elapsed() < APP_GIVES_UP, "{:?}", start.elapsed());
    assert_eq!(link.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn server_without_runtime_refuses_with_code() {
    // Sem o registro do runtime não há porta para conferir: recusa com código, sem chamar o mod.
    let link = FakeLink::default();
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream, "127.0.0.1"));
    state.mods.attach("s", 1, Arc::new(link.clone()));
    let server = spawn_state(state).await;
    let (status, body) = post(server, "s", "press", abrir(), Some(OWNER)).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("erro_mod_guarda_indisponivel")));
    assert!(link.calls.lock().unwrap().is_empty());
    assert_eq!(python.hits_to("/api/sessions/s/plugin/press"), 0);
}

#[tokio::test]
async fn busy_turn_answers_before_the_app_gives_up_without_the_mod() {
    // Um pedido preso segura a vez da sessão: o seguinte não passa do prazo do app e não chega ao mod.
    let (_python, server, mods, link) = setup(FakeLink::default()).await;
    let turn = mods.link("s").unwrap().lock;
    let held = turn.lock().await;
    let start = Instant::now();
    let (status, body) = post(server, "s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), Some(OWNER)).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_clique_sem_resposta")));
    assert!(start.elapsed() < APP_GIVES_UP, "{:?}", start.elapsed());
    assert!(link.calls.lock().unwrap().is_empty());
    // Solta a vez: o pedido seguinte passa.
    drop(held);
    assert_eq!(post(server, "s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), Some(OWNER)).await.0, 200);
}

#[tokio::test]
async fn stuck_mod_is_cut_before_the_app_gives_up_and_frees_the_turn() {
    // O que sobra do prazo limita a chamada ao mod: preso, ele não segura o app nem a vez da sessão.
    let (_python, server, mods, link) = setup(FakeLink::with(FakeInner { delay: Duration::from_secs(30), ..Default::default() })).await;
    let start = Instant::now();
    let (status, body) = post(server, "s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), Some(OWNER)).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_clique_sem_resposta")));
    assert!(start.elapsed() < APP_GIVES_UP, "{:?}", start.elapsed());
    assert_eq!(link.calls.lock().unwrap().len(), 1);
    let turn = mods.link("s").unwrap().lock;
    assert!(turn.try_lock().is_ok(), "a vez da sessão ficou livre");
}

#[tokio::test]
async fn show_and_input_validate_and_answer() {
    let (_python, server, _mods, link) = setup(FakeLink::default()).await;
    assert_eq!(post(server, "s", "show", json!({"site": "painel"}), Some(OWNER)).await, (200, json!({"ok": true, "shown_id": "painel"})));
    assert_eq!(post(server, "s", "input", json!({"site": "painel", "plugin": "vitrine", "key": "V18-campo", "kind": "submit", "value": "olá"}), Some(OWNER)).await,
        (200, json!({"ok": true, "value": "olá"})));
    for bad in [json!({"site": "p", "plugin": "vitrine", "key": "k", "kind": "blur", "value": ""}),
                json!({"site": "p", "plugin": "vitrine", "key": "k", "kind": "change", "value": "x".repeat(16385)}),
                json!({"site": "p", "plugin": "vitrine", "key": "k", "kind": "change", "value": "", "extra": 1}),
                json!({"site": "", "plugin": "vitrine", "key": "k", "kind": "change", "value": ""}),
                json!({"site": "p", "plugin": "", "key": "k", "kind": "change", "value": ""})] {
        assert_eq!(post(server, "s", "input", bad, Some(OWNER)).await.0, 422);
    }
    for bad in [json!({"site": "p", "plugin": "", "key": "k"}), json!({"site": "p", "key": "k", "extra": 1})] {
        assert_eq!(post(server, "s", "press", bad, Some(OWNER)).await.0, 422);
    }
    for bad in [json!({"site": ""}), json!({"site": "p", "key": "k"})] {
        assert_eq!(post(server, "s", "close", bad, Some(OWNER)).await.0, 422);
    }
    assert_eq!(*link.calls.lock().unwrap(), vec![
        ModsCall::Show { site: "painel".into() },
        ModsCall::Input { site: "painel".into(), plugin: "vitrine".into(), key: "V18-campo".into(), submit: true, value: "olá".into() }]);
}

#[tokio::test]
async fn an_app_without_the_mod_is_still_served() {
    // O app de antes desta versão não manda o mod: o servidor acha o único mod com a `key` no lugar, recusa a
    // `key` de dois mods como antes, e o `press` com `__close__` continua fechando o painel.
    let (_python, server, mods, link) = setup(FakeLink::default()).await;
    let button = |key: &str, plugin: &str| json!({"type": "Button", "props": {"key": key, "label": key}, "press": {"plugin": plugin, "handle": 1}});
    let field = |key: &str, plugin: &str| json!({"type": "Input", "props": {"key": key}, "press": {"plugin": plugin, "handle": 2}});
    mods.publish_ui("s", 1, json!({"above": {"type": "Box", "children": [button("so-um", "vitrine"), button("dois", "vitrine"), button("dois", "outro")]},
        "panes": [{"id": "painel", "tree": {"type": "Box", "children": [field("campo", "vitrine")]}}],
        "shown_id": "painel", "columns": 110, "source": "surface"}));
    assert_eq!(post(server, "s", "press", json!({"site": "above-prompt", "key": "so-um"}), Some(OWNER)).await, (200, json!({"ok": true})));
    let (status, body) = post(server, "s", "press", json!({"site": "above-prompt", "key": "dois"}), Some(OWNER)).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_botao_inexistente")));
    assert_eq!(post(server, "s", "press", json!({"site": "painel", "key": "__close__"}), Some(OWNER)).await.0, 200);
    assert_eq!(post(server, "s", "input", json!({"site": "painel", "key": "campo", "kind": "change", "value": "a"}), Some(OWNER)).await.0, 200);
    assert_eq!(*link.calls.lock().unwrap(), vec![
        ModsCall::Press { site: "above-prompt".into(), plugin: "vitrine".into(), key: "so-um".into() },
        ModsCall::Close { site: "painel".into() },
        ModsCall::Input { site: "painel".into(), plugin: "vitrine".into(), key: "campo".into(), submit: false, value: "a".into() }]);
}

#[tokio::test]
async fn concurrent_presses_run_one_at_a_time() {
    let (_python, server, _mods, link) = setup(FakeLink::with(FakeInner { delay: Duration::from_millis(200), ..Default::default() })).await;
    let body = json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"});
    let (a, b) = tokio::join!(post(server, "s", "press", body.clone(), Some(OWNER)), post(server, "s", "press", body, Some(OWNER)));
    assert_eq!((a.0, b.0), (200, 200));
    assert_eq!(link.peak.load(SeqCst), 1, "dois aparelhos não se cruzam no mod");
    assert_eq!(link.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn copy_during_the_click_goes_back_to_the_app() {
    let (_python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream, "127.0.0.1"));
    let mods = state.mods.clone();
    let _ = state.state.runtime.set(Arc::new(RuntimeRegistry::new(upstream, "secret-test".into(), "instance-test".into())));
    let server = spawn_state(state).await;
    let link = FakeLink::with(FakeInner { copy: Some((mods.clone(), "Texto copiado pela vitrine (V44)".into())), ..Default::default() });
    mods.attach("s", 1, Arc::new(link.clone()));
    // A cópia só é do clique quando vem do mod do botão (A11): o botão tem de estar no último `plugin_ui`.
    mods.publish_ui("s", 1, json!({"above": null, "shown_id": "vitrine-botoes", "columns": 110, "source": "surface",
        "panes": [{"id": "vitrine-botoes", "title": "Botões", "placement": "dock", "columns": 58,
            "tree": {"type": "Button", "props": {"key": "V44-copiar", "label": "Copiar"}, "press": {"plugin": "vitrine", "handle": 7}}}]}));
    let (status, body) = post(server, "s", "press", json!({"site": "vitrine-botoes", "plugin": "vitrine", "key": "V44-copiar"}), Some(OWNER)).await;
    assert_eq!((status, body["copied"].as_str()), (200, Some("Texto copiado pela vitrine (V44)")));
}

#[tokio::test]
async fn bridge_runs_what_python_authenticated() {
    // O Python autentica o convidado (inclusive o convite da porta 8766) e devolve o pedido pela porta
    // privada: o mod é acionado como pelo dono. Só a sessão fora do Rust é 404 (o Python a trata).
    let link = FakeLink::default();
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream, "127.0.0.1"));
    let _ = state.state.runtime.set(Arc::new(RuntimeRegistry::new(upstream, "secret-test".into(), "instance-test".into())));
    state.mods.attach("s", 1, Arc::new(link.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let app = hangar_server::routes::terminal_router(Arc::new(state));
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let call = |name: &'static str, op: &'static str, body: Value, secret: &'static str| async move {
        let response = client().post(format!("http://{private}/__hangar_server/mods/{name}/{op}"))
            .header("content-type", "application/json").header("x-hangar-internal", secret).body(body.to_string()).send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)))
    };
    assert_eq!(call("s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), SECRET).await, (200, json!({"ok": true})));
    assert_eq!(call("s", "show", json!({"site": "painel"}), SECRET).await.0, 200);
    assert_eq!(call("s", "input", json!({"site": "painel", "plugin": "vitrine", "key": "campo", "kind": "submit", "value": "olá"}), SECRET).await.0, 200);
    assert_eq!(call("s", "close", json!({"site": "painel"}), SECRET).await.0, 200);
    assert_eq!(link.calls.lock().unwrap().len(), 4);
    assert_eq!(call("s", "press", json!({"site": "above-prompt", "plugin": "vitrine", "key": "abrir"}), "errado").await.0, 403);
    assert_eq!(call("s", "outra-op", json!({}), SECRET).await.0, 400);
    assert_eq!(call("outra", "press", json!({"site": "x", "plugin": "vitrine", "key": "y"}), SECRET).await.0, 404);
    assert_eq!(link.calls.lock().unwrap().len(), 4, "segredo errado e sessão fora do Rust não acionam nada");
    assert_eq!(python.hits_to("/api/sessions/outra/plugin/press"), 0, "a ponte nunca volta ao Python");
}
