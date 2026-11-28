use super::*;

fn body(raw: Value) -> Map<String, Value> {
    raw.as_object().cloned().unwrap()
}

fn spawn(registry: &Arc<Registry>, session: &str, raw: Value) -> tokio::task::JoinHandle<Result<Reply, Refusal>> {
    let (registry, session) = (registry.clone(), session.to_owned());
    tokio::spawn(async move { registry.command(&session, body(raw)).await })
}

async fn request(rx: &mut mpsc::UnboundedReceiver<Zeroizing<String>>) -> Value {
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
    // Foco sozinho (o app reconectando) não diz que a pessoa está lá.
    host.text(r#"{"type":"focus","session":"s"}"#);
    assert_eq!(registry.last_seen("s"), None);
    host.text(r#"{"type":"seen","session":"s"}"#);
    let seen = registry.last_seen("s").expect("uso registrado");
    assert!(seen >= before);
    assert_eq!(registry.command("s", body(json!({"verbo": "host"}))).await, Ok(Reply::Text(format!("ok: host {seen}"))));
    // A pergunta não chega ao app.
    assert!(rx.try_recv().is_err());
    // O app desligou: a hora fica, e o verbo seguinte recusa em vez de ir a outro navegador.
    drop(host);
    assert_eq!(registry.last_seen("s"), Some(seen));
    assert_eq!(registry.command("s", body(json!({"verbo": "url"}))).await, Err(Refusal::NoHost));
    // A hora segue para o CLI, marcada: sem app ligado, o app daqui caído não passa a sessão adiante.
    assert_eq!(registry.command("s", body(json!({"verbo": "host"}))).await, Ok(Reply::Text(format!("ok: host {seen} detached"))));
}

#[tokio::test]
async fn host_answers_zero_while_an_app_has_the_session_in_focus_without_use() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    // Sem `seen`: o app de lá serve quem perdeu o app daqui, e a hora 0 perde para o foco de um app vivo daqui.
    assert_eq!(registry.command("s", body(json!({"verbo": "host"}))).await, Ok(Reply::Text("ok: host 0".into())));
    assert!(registry.has_host("s") && !registry.has_host("outra"));
    assert_eq!(registry.command("outra", body(json!({"verbo": "host"}))).await, Err(Refusal::NoHost));
    assert!(rx.try_recv().is_err(), "a pergunta não chega ao app");
    drop(host);
    assert!(!registry.has_host("s"));
    assert_eq!(registry.command("s", body(json!({"verbo": "host"}))).await, Err(Refusal::NoHost));
}

#[tokio::test]
async fn fill_secret_and_valor_never_come_from_the_route() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    for raw in [json!({"verbo": "fill-secret", "args": ["@e1", "password"]}), json!({"verbo": "fill", "args": ["@e1", "x"], "valor": "senha"})] {
        assert_eq!(registry.command("s", body(raw)).await, Err(Refusal::Secret));
    }
    assert_eq!((Refusal::Secret.status(), Refusal::Secret.code()), (422, "browser_host_secret_refused"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn fill_secret_puts_the_value_only_in_the_frame() {
    const VALUE: &str = "S3nh@ \"x\"/é";
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s","caps":["fill_secret"]}"#);
    host.text(r#"{"type":"state","session":"s","url":"https://github.com/login"}"#);
    let call = {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry.fill_secret("s", &["@e3".to_owned(), "password".to_owned()], "github.com", Zeroizing::new(VALUE.to_owned())).await
        })
    };
    let sent = request(&mut rx).await;
    assert_eq!((sent["verbo"].as_str(), sent["valor"].as_str(), sent["expected_host"].as_str()), (Some("fill-secret"), Some(VALUE), Some("github.com")));
    assert_eq!(sent["args"], json!(["@e3", "password"]));
    assert_eq!(sent["session"], "s");
    // Em curso, o pedido guarda só a sessão: nada do valor fica no registro.
    {
        let inner = registry.lock();
        let pending: Vec<&Pending> = inner.hosts.values().flat_map(|h| h.pending.values()).collect();
        assert!(pending.len() == 1 && pending[0].url.is_none() && pending[0].session == "s");
    }
    host.text(&json!({"id": sent["id"], "ok": true, "text": "ok: fill-secret @e3"}).to_string());
    assert_eq!(call.await.unwrap(), Ok(Reply::Text("ok: fill-secret @e3".into())));
    let list = registry.list();
    assert_eq!(list, "s  https://github.com/login  vivo");
    assert!(!list.contains("S3nh"));
}

/// App de uma versão sem o `fill-secret`: o valor nunca entra num quadro para ele, e a recusa diz para atualizar.
#[tokio::test]
async fn fill_secret_is_refused_to_an_app_that_did_not_announce_it() {
    const VALUE: &str = "S3nh@-do-cofre";
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    let args = ["@e1".to_owned(), "password".to_owned()];
    assert_eq!(registry.fill_secret("s", &args, "github.com", Zeroizing::new(VALUE.to_owned())).await, Err(Refusal::Outdated));
    assert_eq!((Refusal::Outdated.status(), Refusal::Outdated.code()), (409, "browser_host_app_outdated"));
    assert!(Refusal::Outdated.message("fill-secret").contains("atualize o app desktop"));
    while let Ok(frame) = rx.try_recv() { assert!(!frame.contains(VALUE), "nenhum quadro leva o valor"); }
    // Outras capacidades não bastam, e os outros verbos seguem indo ao mesmo app.
    host.text(r#"{"type":"focus","session":"s","caps":["outra"]}"#);
    assert_eq!(registry.fill_secret("s", &args, "github.com", Zeroizing::new(VALUE.to_owned())).await, Err(Refusal::Outdated));
    assert!(rx.try_recv().is_err());
    let call = spawn(&registry, "s", json!({"verbo": "url"}));
    let sent = request(&mut rx).await;
    host.text(&json!({"id": sent["id"], "ok": true, "text": "ok"}).to_string());
    assert_eq!(call.await.unwrap(), Ok(Reply::Text("ok".into())));
    // Anunciada, o quadro sai com o valor.
    host.text(r#"{"type":"focus","session":"s","caps":["fill_secret"]}"#);
    let call = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.fill_secret("s", &["@e1".to_owned(), "password".to_owned()], "github.com", Zeroizing::new(VALUE.to_owned())).await })
    };
    let sent = request(&mut rx).await;
    assert_eq!((sent["verbo"].as_str(), sent["valor"].as_str()), (Some("fill-secret"), Some(VALUE)));
    host.text(&json!({"id": sent["id"], "ok": true, "text": "ok: fill-secret @e1"}).to_string());
    assert_eq!(call.await.unwrap(), Ok(Reply::Text("ok: fill-secret @e1".into())));
}

#[tokio::test]
async fn can_fill_secret_follows_the_app_that_would_receive_the_value() {
    let registry = Arc::new(Registry::default());
    assert_eq!(registry.can_fill_secret("s"), Err(Refusal::NoHost));
    let (mut old, _old_rx) = registry.attach();
    old.text(r#"{"type":"focus","session":"s"}"#);
    assert_eq!(registry.can_fill_secret("s"), Err(Refusal::Outdated));
    let (mut new, _new_rx) = registry.attach();
    new.text(r#"{"type":"focus","session":"s","caps":["fill_secret"]}"#);
    assert_eq!(registry.can_fill_secret("s"), Ok(()));
    // O foco volta ao app antigo: é ele que receberia o valor.
    old.text(r#"{"type":"focus","session":"s"}"#);
    assert_eq!(registry.can_fill_secret("s"), Err(Refusal::Outdated));
}

/// O quadro com o valor é o JSON de sempre, e o buffer reservado antes não cresceu (nenhuma cópia ficou para trás).
#[test]
fn the_secret_frame_keeps_its_text_and_never_grows() {
    for secret in ["S3nh@ \"x\"/é", "\u{1}\u{1f}\\", ""] {
        let map = body(json!({"verbo": "fill-secret", "id": 7}));
        let base = Value::Object(map.clone()).to_string();
        let out = frame(map, Some(secret));
        let expected = format!("{},\"valor\":{}}}", &base[..base.len() - 1], serde_json::to_string(secret).unwrap());
        assert_eq!(*out, expected);
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["valor"], secret);
        assert_eq!(out.capacity(), frame_capacity(base.len(), secret), "{secret:?}");
    }
    assert_eq!(*frame(Map::new(), None), "{}");
}

/// O app diz por que o verbo deu `erro:`; código fora do formato de nome não atravessa (vai num cabeçalho).
#[tokio::test]
async fn the_app_refusal_keeps_its_cause() {
    let registry = Arc::new(Registry::default());
    let (mut host, mut rx) = registry.attach();
    host.text(r#"{"type":"focus","session":"s"}"#);
    for (code, expected) in [("page_gone", Reply::Error("page_gone".into(), "erro: a pagina caiu".into())),
        ("x\r\nset-cookie: a", Reply::Text("erro: a pagina caiu".into()))] {
        let call = spawn(&registry, "s", json!({"verbo": "click", "args": ["@e1"]}));
        let sent = request(&mut rx).await;
        host.text(&json!({"id": sent["id"], "ok": true, "text": "erro: a pagina caiu", "code": code}).to_string());
        assert_eq!(call.await.unwrap(), Ok(expected), "{code}");
    }
}
