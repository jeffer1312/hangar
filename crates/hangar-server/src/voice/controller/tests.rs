use super::*;
use crate::groups::peers::{PeerBook, PeerClient};
use crate::voice::call::CallOptions;
use crate::voice::machines::SelfApi;
use crate::voice::organizer::{ModeModels, SEND_DUPLICATE, tools_for};
use crate::voice::test_support::{fake_app_server, push_speech, push_tool};
use axum::{Router, extract::{Path, State}, http::header::CONTENT_TYPE, response::IntoResponse, routing::{delete, get, post}};
use std::net::SocketAddr;

// O axum daqui é sem a feature `json`.
fn js(v: Value) -> axum::response::Response { ([(CONTENT_TYPE, "application/json")], v.to_string()).into_response() }

#[derive(Default)]
struct Api { sessions: Value, history: HashMap<String, Value>, inputs: Vec<(String, String)>, started: usize, deleted: Vec<String>, input_delay: Duration }
type Shared = Arc<Mutex<Api>>;

async fn fake_api(api: Shared) -> SocketAddr {
    let app = Router::new()
        .route("/api/sessions", get(|State(api): State<Shared>| async move { js(api.lock().unwrap().sessions.clone()) }))
        .route("/api/sessions/{n}", delete(|State(api): State<Shared>, Path(n): Path<String>| async move {
            api.lock().unwrap().deleted.push(n);
            js(json!({}))
        }))
        .route("/api/sessions/{n}/input", post(|State(api): State<Shared>, Path(n): Path<String>, body: String| async move {
            let delay = { let mut a = api.lock().unwrap(); a.started += 1; a.input_delay };
            tokio::time::sleep(delay).await;
            let text = serde_json::from_str::<Value>(&body).unwrap()["text"].as_str().unwrap_or_default().to_owned();
            api.lock().unwrap().inputs.push((n, text));
            js(json!({"ok": true, "delivered": true}))
        }))
        .route("/api/sessions/{n}/history", get(|State(api): State<Shared>, Path(n): Path<String>| async move {
            let history = api.lock().unwrap().history.get(&n).cloned().unwrap_or_else(|| json!([]));
            js(history)
        }))
        .with_state(api);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

struct Rig {
    seen: mpsc::UnboundedReceiver<Value>, log: Vec<Value>, push: mpsc::UnboundedSender<Value>,
    to_ctl: mpsc::UnboundedSender<ToController>, device: mpsc::UnboundedReceiver<ServerMsg>, api: Shared, link: DeviceLink, _dir: tempfile::TempDir,
}

/// Chamada viva com a sessão `hangar` na tela, sobre o app-server e a API falsos.
async fn rig(caps: &[&str], peers: Option<Value>) -> Rig {
    let mut rig = launch(caps, peers).await;
    rig.to_ctl.send(ToController::Device(ClientMsg::Offer { sdp: "v=0 offer".into() })).unwrap();
    loop { if let ServerMsg::Answer { .. } = rig.device.recv().await.unwrap() { break; } }
    rig.to_ctl.send(ToController::Device(ClientMsg::Live)).unwrap();
    rig
}

/// O controlador de pé, antes da primeira oferta.
async fn launch(caps: &[&str], peers: Option<Value>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("runtime-config.json"), r#"{"codex_voice_beta": true}"#).unwrap();
    let peers_path = peers.map(|p| { let path = dir.path().join("peers.json"); std::fs::write(&path, p.to_string()).unwrap(); path });
    let api: Shared = Arc::new(Mutex::new(Api {
        sessions: json!([{"name": "hangar", "provider": "claude", "state": "idle"}, {"name": "web", "provider": "codex", "state": "working"}]),
        ..Api::default()
    }));
    let addr = fake_api(api.clone()).await;
    let machines = Arc::new(Machines { own: SelfApi::new(addr, "t"), peers: Arc::new(PeerClient::new(PeerBook::new(peers_path))), own_label: "casa".into() });
    let caps: Vec<String> = caps.iter().map(|c| (*c).to_owned()).collect();
    let (spawn, seen, push) = fake_app_server();
    let (tx, events) = async_channel::unbounded();
    let options = CallOptions { voice: None, context: "ctx".into(), cwd: None, target: "hangar".into(), organizer: ModeModels::default(),
        tools: tools_for(&caps), handoff_same_thread: true, voice_dir: dir.path().join("voz") };
    let voice = Voice::start(options, spawn, tx);
    let (link, device) = DeviceLink::channel(caps, Some(Screen { server: String::new(), name: "hangar".into() }));
    let gate = GateSource { home: dir.path().to_path_buf(), claude_dir: dir.path().to_path_buf(), every: Duration::from_secs(5) };
    let diag = DiagClient::new("127.0.0.1:9".parse().unwrap(), "x".into());
    let (to_ctl, from_device) = mpsc::unbounded_channel();
    tokio::spawn(Controller::new(voice, machines, None, link.clone(), gate, diag).run(events, from_device));
    Rig { seen, log: Vec::new(), push, to_ctl, device, api, link, _dir: dir }
}

fn text(result: &Value) -> &str { result["contentItems"][0]["text"].as_str().unwrap_or_default() }

fn summary_of(session: &str) -> impl Fn(&Value) -> bool {
    let head = format!("[RESULTADO DA SESSÃO {session}]");
    move |m: &Value| m["method"] == "turn/start" && m["params"]["input"][0]["text"].as_str().is_some_and(|t| t.starts_with(&head))
}

impl Rig {
    fn drain(&mut self) { while let Ok(m) = self.seen.try_recv() { self.log.push(m); } }

    /// O que a chamada escreveu ao app-server e casa com `pred`.
    async fn wait(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(m) = self.log.iter().find(|m| pred(m)) { return m.clone(); }
            match tokio::time::timeout_at(deadline, self.seen.recv()).await {
                Ok(Some(m)) => self.log.push(m),
                _ => panic!("não chegou: {what}"),
            }
        }
    }

    /// Resposta da chamada à ferramenta `id`.
    async fn reply(&mut self, id: &str) -> Value { self.wait(id, |m| m["id"] == id && m.get("result").is_some()).await["result"].clone() }

    fn replied(&mut self, id: &str) -> bool { self.drain(); self.log.iter().any(|m| m["id"] == id && m.get("result").is_some()) }

    async fn tool(&mut self) -> (u64, String, Value) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            match tokio::time::timeout_at(deadline, self.device.recv()).await {
                Ok(Some(ServerMsg::Tool { call, name, args })) => return (call, name, args),
                Ok(Some(_)) => {}
                _ => panic!("o aparelho não recebeu ferramenta"),
            }
        }
    }

    async fn state(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            match tokio::time::timeout_at(deadline, self.device.recv()).await {
                Ok(Some(ServerMsg::State { state })) if pred(&state) => return state,
                Ok(Some(_)) => {}
                _ => panic!("estado não chegou: {what}"),
            }
        }
    }

    fn answer(&self, call: u64, ok: bool, text: &str) {
        self.to_ctl.send(ToController::Device(ClientMsg::ToolResult { call, ok, text: text.into() })).unwrap();
    }
}

#[tokio::test]
async fn list_sessions_names_machines_and_screen() {
    let mut rig = rig(&[], None).await;
    push_tool(&rig.push, "l1", "list_sessions", json!({}), "turn-1");
    let reply = rig.reply("l1").await;
    assert_eq!(reply["success"], true);
    let listed = text(&reply);
    assert!(listed.contains("- hangar") && listed.contains("- web"), "{listed}");
    assert!(listed.lines().any(|l| l.starts_with("- hangar") && l.contains("na tela")), "{listed}");
    assert!(!listed.lines().any(|l| l.starts_with("- web") && l.contains("na tela")), "{listed}");
}

#[tokio::test]
async fn send_to_named_session_posts_input_once_per_turn() {
    let mut rig = rig(&[], None).await;
    let args = json!({"request": "revisar o login da tela", "session": "web"});
    // A fala cita "web" e "casa web": a trava da chamada aceita os dois destinos, e só a do controlador vê que são a mesma sessão.
    push_speech(&rig.push, "turn-1", "manda pra web e pra casa web revisar o login da tela");
    push_tool(&rig.push, "s1", "send_to_session", args.clone(), "turn-1");
    let first = rig.reply("s1").await;
    assert_eq!(first["success"], true, "{first}");
    push_tool(&rig.push, "s2", "send_to_session", args, "turn-1");
    let second = rig.reply("s2").await;
    assert_eq!(second["success"], false);
    assert_eq!(text(&second), SEND_DUPLICATE);
    push_tool(&rig.push, "s3", "send_to_session", json!({"request": "revisar o login da tela", "session": "casa::web"}), "turn-1");
    let third = rig.reply("s3").await;
    assert_eq!(third["success"], false, "{third}");
    assert_eq!(text(&third), SEND_DUPLICATE);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let inputs = rig.api.lock().unwrap().inputs.clone();
    assert_eq!(inputs, vec![("web".to_owned(), "revisar o login da tela".to_owned())]);
}

#[tokio::test]
async fn unreachable_peer_is_named_and_send_fails_loud() {
    let mut rig = rig(&[], Some(json!({"vps": {"base_url": "http://127.0.0.1:9", "token": "x"}}))).await;
    push_tool(&rig.push, "l1", "list_sessions", json!({}), "turn-1");
    let listed = rig.reply("l1").await;
    assert!(text(&listed).contains("Máquina vps sem resposta"), "{listed}");
    push_speech(&rig.push, "turn-2", "manda pra vps x corrigir o login agora");
    push_tool(&rig.push, "s1", "send_to_session", json!({"request": "corrigir o login agora", "session": "vps::x"}), "turn-2");
    let sent = rig.reply("s1").await;
    assert_eq!(sent["success"], false, "{sent}");
    assert!(!text(&sent).trim().is_empty());
    assert!(rig.api.lock().unwrap().inputs.is_empty(), "nada foi para a sessão local");
}

#[tokio::test]
async fn slow_send_does_not_block_screen_tool() {
    let mut rig = rig(&["switch_session"], None).await;
    rig.api.lock().unwrap().input_delay = Duration::from_secs(2);
    push_speech(&rig.push, "turn-1", "manda pra web revisar o login da tela");
    push_tool(&rig.push, "s1", "send_to_session", json!({"request": "revisar o login da tela", "session": "web"}), "turn-1");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while rig.api.lock().unwrap().started == 0 {
        assert!(std::time::Instant::now() < deadline, "o envio não começou");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    push_speech(&rig.push, "turn-2", "troca pra web");
    push_tool(&rig.push, "w1", "switch_session", json!({"name": "web"}), "turn-2");
    let (call, name, _) = rig.tool().await;
    assert_eq!(name, "switch_session");
    rig.answer(call, true, "ok");
    assert_eq!(rig.reply("w1").await["success"], true);
    assert!(!rig.replied("s1"), "a troca respondeu antes de o envio terminar");
    assert!(rig.api.lock().unwrap().inputs.is_empty());
    assert_eq!(rig.reply("s1").await["success"], true);
}

#[tokio::test]
async fn short_turn_reply_is_spoken() {
    let mut rig = rig(&[], None).await;
    let rows = |at: u64| json!([{"name": "hangar", "provider": "claude", "state": "idle", "last_reply_at": at},
        {"name": "web", "provider": "codex", "state": "working"}]);
    rig.api.lock().unwrap().sessions = rows(1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    {
        let mut api = rig.api.lock().unwrap();
        api.history.insert("hangar".into(), json!([{"id": "7", "kind": "user_msg", "text": "oi"}, {"id": "8", "kind": "assistant_msg", "text": "feito"}]));
        // Sem passar por `working`: o turno inteiro coube entre duas leituras.
        api.sessions = rows(2);
    }
    let summary = rig.wait("resultado da hangar", summary_of("hangar")).await;
    assert!(summary["params"]["input"][0]["text"].as_str().unwrap().contains("feito"));
}

#[tokio::test]
async fn switch_session_goes_to_device_and_reply_comes_back() {
    let mut rig = rig(&["switch_session"], None).await;
    push_speech(&rig.push, "turn-1", "troca pra web");
    push_tool(&rig.push, "w1", "switch_session", json!({"name": "web"}), "turn-1");
    let (call, name, args) = rig.tool().await;
    assert_eq!(name, "switch_session");
    assert_eq!(args["name"], "web");
    assert_eq!(args["server"], "");
    assert!(args["base_url"].is_null());
    rig.answer(call, true, "ok");
    let reply = rig.reply("w1").await;
    assert_eq!(reply["success"], true);
    assert!(text(&reply).starts_with("Sessão web aberta; a troca já foi anunciada"), "{reply}");
}

#[tokio::test]
async fn close_session_asks_then_closes() {
    let mut rig = rig(&[], None).await;
    push_speech(&rig.push, "turn-1", "fecha a sessão web");
    push_tool(&rig.push, "c1", "close_session", json!({"name": "web"}), "turn-1");
    let armed = rig.reply("c1").await;
    assert!(text(&armed).starts_with("Nada foi fechado."), "{armed}");
    assert!(rig.api.lock().unwrap().deleted.is_empty());
    push_speech(&rig.push, "turn-2", "sim, pode fechar");
    push_tool(&rig.push, "c2", "close_session", json!({"name": "web", "confirmed": true}), "turn-2");
    let closed = rig.reply("c2").await;
    assert_eq!(closed["success"], true);
    assert_eq!(text(&closed), "Sessão web fechada.");
    assert_eq!(rig.api.lock().unwrap().deleted, vec!["web".to_owned()]);
}

#[tokio::test]
async fn followed_session_reply_is_spoken_when_it_goes_idle() {
    let mut rig = rig(&[], None).await;
    push_tool(&rig.push, "f1", "follow_session", json!({"name": "web"}), "turn-1");
    assert_eq!(rig.reply("f1").await["success"], true);
    tokio::time::sleep(Duration::from_millis(1600)).await;
    {
        let mut api = rig.api.lock().unwrap();
        api.history.insert("web".into(), json!([{"id": "9", "kind": "assistant_msg", "text": "pronto"}]));
        api.sessions = json!([{"name": "hangar", "provider": "claude", "state": "idle"}, {"name": "web", "provider": "codex", "state": "idle"}]);
    }
    rig.wait("resultado da web", summary_of("web")).await;
}

#[tokio::test]
async fn state_snapshot_reaches_device_coalesced() {
    let mut rig = rig(&[], None).await;
    rig.state("modelo efetivo", |s| s["effective"]["model"] == "gpt-test").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    while rig.device.try_recv().is_ok() {}
    for _ in 0..10 {
        rig.push.send(json!({"method": "item/reasoning/summaryTextDelta", "params": {"threadId": "t1", "delta": "x"}})).unwrap();
    }
    let until = tokio::time::Instant::now() + STATE_EVERY;
    let mut states = 0;
    while let Ok(Some(msg)) = tokio::time::timeout_at(until, rig.device.recv()).await {
        if matches!(msg, ServerMsg::State { .. }) { states += 1; }
    }
    assert!(states <= 2, "{states} retratos em 150 ms");
    rig.state("pensamento inteiro", |s| s["thought"] == "xxxxxxxxxx").await;
}

#[tokio::test]
async fn answer_of_a_former_owner_never_reaches_the_new_one() {
    let mut rig = launch(&[], None).await;
    rig.to_ctl.send(ToController::Device(ClientMsg::Offer { sdp: "v=0 a".into() })).unwrap();
    // B assume antes da resposta da oferta de A.
    let (b_tx, mut b) = mpsc::unbounded_channel();
    rig.link.replace_owner(b_tx, vec![], None);
    rig.to_ctl.send(ToController::OwnerChanged).unwrap();
    rig.to_ctl.send(ToController::Device(ClientMsg::Hello { client: "pwa".into(), screen: None, caps: vec![], actions: vec![] })).unwrap();
    rig.to_ctl.send(ToController::Device(ClientMsg::Offer { sdp: "v=0 b".into() })).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let first = loop {
        match tokio::time::timeout_at(deadline, b.recv()).await {
            Ok(Some(ServerMsg::Answer { sdp })) => break sdp,
            Ok(Some(_)) => {}
            _ => panic!("B ficou sem resposta"),
        }
    };
    assert_eq!(first, "v=0 answer 2", "só a resposta da oferta de B");
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(msg) = b.try_recv() { assert!(!matches!(msg, ServerMsg::Answer { .. }), "resposta a mais"); }
    while let Ok(msg) = rig.device.try_recv() { assert!(!matches!(msg, ServerMsg::Answer { .. }), "A já não é o dono"); }
}
