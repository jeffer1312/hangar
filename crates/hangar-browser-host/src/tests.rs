use super::*;

fn body(raw: Value) -> Map<String, Value> {
    raw.as_object().cloned().unwrap()
}

fn spawn(registry: &Arc<Registry>, session: &str, raw: Value) -> tokio::task::JoinHandle<Result<Reply, Refusal>> {
    let (registry, session) = (registry.clone(), session.to_owned());
    tokio::spawn(async move { registry.command(&session, body(raw)).await })
}

async fn request(rx: &mut mpsc::UnboundedReceiver<String>) -> Value {
    serde_json::from_str(&rx.recv().await.expect("pedido ao anfitrião")).unwrap()
}

#[tokio::test]
async fn last_focus_on_the_session_wins() {
    let registry = Arc::new(Registry::default());
    let (mut a, mut a_rx) = registry.attach();
    let (mut b, mut b_rx) = registry.attach();
    a.text(r#"{"type":"focus","session":"s"}"#);
    b.text(r#"{"type":"focus","session":"s"}"#);
    // Foco em outra sessão não tira a de `s` de B.
    a.text(r#"{"type":"focus","session":"outra"}"#);

    let call = spawn(&registry, "s", json!({"verbo": "url", "args": [], "deadline_ms": 5000}));
    let sent = request(&mut b_rx).await;
    assert_eq!(sent["session"], "s");
    assert_eq!(sent["verbo"], "url");
    assert_eq!(sent["deadline_ms"], 5000);
    b.text(&json!({"id": sent["id"], "ok": true, "text": "ok: \"http://x\""}).to_string());
    assert_eq!(call.await.unwrap(), Ok(Reply::Text("ok: \"http://x\"".into())));
    assert!(a_rx.try_recv().is_err());

    a.text(r#"{"type":"focus","session":"s"}"#);
    let call = spawn(&registry, "s", json!({"verbo": "text", "args": [], "aba": 2}));
    let sent = request(&mut a_rx).await;
    assert_eq!(sent["aba"], 2);
    a.text(&json!({"id": sent["id"], "ok": false, "error": "pagina caiu"}).to_string());
    assert_eq!(call.await.unwrap(), Ok(Reply::Text("erro: pagina caiu".into())));
}

#[tokio::test]
async fn host_that_drops_mid_shot_answers_gone() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    let call = spawn(&registry, "s", json!({"verbo": "shot", "args": ["/tmp/x.png"]}));
    let sent = request(&mut rx).await;
    host.text(&json!({"id": sent["id"], "bin": "png"}).to_string());
    drop(host);
    assert_eq!(call.await.unwrap(), Err(Refusal::Gone));
    assert!(Refusal::Gone.message("shot").contains("pode ter sido executado"));
    // O anfitrião saiu: o próximo pedido não tem a quem ir.
    assert_eq!(registry.command("s", body(json!({"verbo": "url"}))).await, Err(Refusal::NoHost));
}

#[tokio::test]
async fn shot_png_comes_in_the_binary_frame() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    let call = spawn(&registry, "s", json!({"verbo": "shot", "args": []}));
    let sent = request(&mut rx).await;
    host.text(&json!({"id": sent["id"], "bin": "png"}).to_string());
    host.binary(vec![0x89, b'P', b'N', b'G']);
    assert_eq!(call.await.unwrap(), Ok(Reply::Png(vec![0x89, b'P', b'N', b'G'])));
}

#[tokio::test(start_paused = true)]
async fn deadline_answers_timeout() {
    let registry = Arc::new(Registry::default());
    let (mut host, _rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    assert_eq!(registry.command("s", body(json!({"verbo": "click", "args": ["@e1"], "deadline_ms": 50}))).await, Err(Refusal::Timeout));
}

#[tokio::test]
async fn request_dropped_by_the_cli_leaves_no_pending_entry() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    let pending = |registry: &Registry| registry.lock().hosts.values().map(|h| h.pending.len()).sum::<usize>();
    let mut call = Box::pin(registry.command("s", body(json!({"verbo": "click", "args": ["@e1"]}))));
    tokio::select! {
        _ = &mut call => panic!("respondeu sem o anfitrião"),
        sent = request(&mut rx) => assert_eq!(sent["verbo"], "click"),
    }
    assert_eq!(pending(&registry), 1);
    drop(call);
    assert_eq!(pending(&registry), 0);
}

#[tokio::test]
async fn session_names_from_the_host_are_capped() {
    let registry = Arc::new(Registry::default());
    let (mut host, _rx) = registry.attach();
    for n in 0..MAX_SESSIONS + 10 {
        host.text(&json!({"type": "focus", "session": format!("s{n}")}).to_string());
        host.text(&json!({"type": "state", "session": format!("s{n}"), "url": "about:blank"}).to_string());
    }
    let inner = registry.lock();
    assert_eq!((inner.focus.len(), inner.state.len()), (MAX_SESSIONS, MAX_SESSIONS));
    // O foco mais antigo é o que sai; o último fica.
    assert!(!inner.focus.contains_key("s0") && inner.focus.contains_key(&format!("s{}", MAX_SESSIONS + 9)));
}

#[tokio::test]
async fn objetivo_and_confere_never_reach_the_host() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    for verb in ["objetivo", "confere"] {
        assert_eq!(registry.command("s", body(json!({"verbo": verb, "args": ["x"]}))).await, Err(Refusal::Unsupported));
    }
    assert_eq!(Refusal::Unsupported.status(), 422);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn without_focus_is_no_host_and_not_allowed_is_its_own_refusal() {
    let registry = Arc::new(Registry::default());
    assert_eq!(registry.command("s", body(json!({"verbo": "url"}))).await, Err(Refusal::NoHost));
    assert_eq!((Refusal::NoHost.status(), Refusal::NoHost.code()), (409, "browser_host_none"));

    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    let call = spawn(&registry, "s", json!({"verbo": "goto", "args": ["https://example.com"]}));
    let sent = request(&mut rx).await;
    host.text(&json!({"id": sent["id"], "ok": false, "error": "browser_host_not_allowed"}).to_string());
    assert_eq!(call.await.unwrap(), Err(Refusal::NotAllowed));
    assert_eq!(Refusal::NotAllowed.status(), 403);
    // Recusado, o `goto` não vira o endereço do navegador.
    assert_eq!(registry.list(), "nenhum navegador registrado");
}

#[tokio::test]
async fn state_updates_and_confirmed_open_feed_list() {
    let registry = Arc::new(Registry::default());
    assert_eq!(registry.command("s", body(json!({"verbo": "list"}))).await, Ok(Reply::Text("nenhum navegador registrado".into())));

    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    host.text(r#"{"type":"state","session":"s","url":"http://localhost:3000/","aba":1}"#);
    assert_eq!(registry.list(), "s  http://localhost:3000/  vivo  (aba 1)");

    let call = spawn(&registry, "s", json!({"verbo": "open", "args": ["http://localhost:5173/"]}));
    let sent = request(&mut rx).await;
    host.text(&json!({"id": sent["id"], "ok": true, "text": "ok"}).to_string());
    call.await.unwrap().unwrap();
    assert_eq!(registry.list(), "s  http://localhost:5173/  vivo  (aba 1)");

    drop(host);
    assert_eq!(registry.list(), "s  http://localhost:5173/  MORTO (reabra com: hangar-preview open http://localhost:5173/)  (aba 1)");
}

#[tokio::test]
async fn host_answers_when_another_machine_last_showed_the_session() {
    let registry = Arc::new(Registry::default());
    assert_eq!(registry.command("s", body(json!({"verbo": "host"}))).await, Err(Refusal::NoHost));
    let before = now_ms();
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    let seen = registry.last_seen("s").expect("foco registrado");
    assert!(seen >= before);
    assert_eq!(registry.command("s", body(json!({"verbo": "host"}))).await, Ok(Reply::Text(format!("ok: host {seen}"))));
    // A pergunta não chega ao app.
    assert!(rx.try_recv().is_err());
    // O app desligou: a hora fica, e o verbo seguinte recusa em vez de ir a outro navegador.
    drop(host);
    assert_eq!(registry.last_seen("s"), Some(seen));
    assert_eq!(registry.command("s", body(json!({"verbo": "url"}))).await, Err(Refusal::NoHost));
}
