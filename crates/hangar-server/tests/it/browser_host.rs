//! Navegador emprestado: o app de outra máquina abre `/api/browser-host` e o CLI daqui pede pelo
//! `browser-cmd`. O app é um cliente WebSocket falso.
use crate::fake;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn server(home: &std::path::Path) -> SocketAddr {
    let mut state = hangar_server::routes::AppState::new(fake::config("127.0.0.1"));
    state.hangar_dir = home.join(".hangar");
    fake::spawn_state(state).await
}

async fn host(addr: SocketAddr) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}/api/browser-host?token={}", fake::OWNER)).await.expect("canal anfitrião").0
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn next_request(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("prazo").expect("fim").unwrap();
        if let Message::Text(t) = msg { return serde_json::from_str(&t).unwrap(); }
    }
}

fn cmd(addr: SocketAddr, session: &str, body: Value) -> reqwest::RequestBuilder {
    reqwest::Client::new().post(format!("http://{addr}/api/sessions/{session}/browser-cmd")).bearer_auth(fake::OWNER).json(&body)
}

/// O foco chega por mensagem: o `browser-cmd` só vale depois que o servidor a leu.
async fn focused(addr: SocketAddr, ws: &mut Ws, session: &str) {
    send(ws, json!({"type": "focus", "session": session})).await;
    send(ws, json!({"type": "state", "session": session, "url": "about:blank"})).await;
    let ceiling = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < ceiling {
        let list = cmd(addr, session, json!({"verbo": "list", "args": []})).send().await.unwrap().text().await.unwrap();
        if list.contains(&format!("{session}  about:blank  vivo")) { return; }
        // estavel: intervalo da consulta até o servidor confirmar o foco; o teto é o prazo de 10 s acima
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("o foco não chegou ao servidor");
}

#[tokio::test]
async fn browser_host_without_focus_is_409() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let r = cmd(addr, "s", json!({"verbo": "url", "args": [], "deadline_ms": 1000})).send().await.unwrap();
    assert_eq!(r.status(), 409);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["detail"]["code"], "browser_host_none");
    assert_eq!(body["detail"]["params"], json!({}));
}

#[tokio::test]
async fn browser_host_round_trip_with_shot_written_here() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let mut app = host(addr).await;
    focused(addr, &mut app, "minha sessao").await;

    let call = tokio::spawn(cmd(addr, "minha sessao", json!({"verbo": "goto", "args": ["https://example.com/"], "aba": 2, "deadline_ms": 5000})).send());
    let asked = next_request(&mut app).await;
    assert_eq!((asked["session"].as_str(), asked["verbo"].as_str(), asked["aba"].as_i64()), (Some("minha sessao"), Some("goto"), Some(2)));
    assert_eq!(asked["args"], json!(["https://example.com/"]));
    assert_eq!(asked["deadline_ms"], 5000);
    send(&mut app, json!({"id": asked["id"], "ok": true, "text": "ok: https://example.com/"})).await;
    let r = call.await.unwrap().unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok: https://example.com/");

    let list = cmd(addr, "minha sessao", json!({"verbo": "list"})).send().await.unwrap().text().await.unwrap();
    assert_eq!(list, "minha sessao  https://example.com/  vivo");

    let call = tokio::spawn(cmd(addr, "minha sessao", json!({"verbo": "shot", "args": ["--max", "800"], "deadline_ms": 5000})).send());
    let asked = next_request(&mut app).await;
    // O caminho desta máquina vai na frente, como no `/cmd` local; o `--max` segue para o app.
    assert_eq!(asked["args"].as_array().unwrap()[1..], [json!("--max"), json!("800")]);
    send(&mut app, json!({"id": asked["id"], "bin": "png"})).await;
    app.send(Message::Binary(vec![0x89, b'P', b'N', b'G'].into())).await.unwrap();
    let text = call.await.unwrap().unwrap().text().await.unwrap();
    let path = text.strip_prefix("ok: shot ").expect("caminho do print");
    let shots = home.path().join(".hangar/nav/shots/minha-sessao");
    assert!(std::path::Path::new(path).starts_with(&shots) && path.ends_with(".png"), "{path}");
    assert_eq!(std::fs::read(path).unwrap(), vec![0x89, b'P', b'N', b'G']);
}

#[tokio::test]
async fn browser_host_refusals_follow_the_contract() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let mut app = host(addr).await;
    focused(addr, &mut app, "s").await;

    let r = cmd(addr, "s", json!({"verbo": "objetivo", "args": ["x"]})).send().await.unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"], "browser_host_verb_unsupported");

    let call = tokio::spawn(cmd(addr, "s", json!({"verbo": "goto", "args": ["https://example.com/"]})).send());
    let asked = next_request(&mut app).await;
    send(&mut app, json!({"id": asked["id"], "ok": false, "error": "browser_host_not_allowed"})).await;
    let r = call.await.unwrap().unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"], "browser_host_not_allowed");

    let r = cmd(addr, "s", json!({"verbo": "click", "args": ["@e1"], "deadline_ms": 100})).send().await.unwrap();
    assert_eq!(r.status(), 504);
    assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"], "browser_host_timeout");

    let call = tokio::spawn(cmd(addr, "s", json!({"verbo": "shot", "args": [], "deadline_ms": 5000})).send());
    // O primeiro é o `click` que esgotou o prazo; o segundo, o `shot`.
    assert_eq!(next_request(&mut app).await["verbo"], "click");
    assert_eq!(next_request(&mut app).await["verbo"], "shot");
    drop(app);
    let r = call.await.unwrap().unwrap();
    assert_eq!(r.status(), 502);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["detail"]["code"], "browser_host_gone");
    assert!(body["detail"]["msg"].as_str().unwrap().contains("pode ter sido executado"));
}

#[tokio::test]
async fn browser_host_route_never_carries_a_vault_value() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let mut app = host(addr).await;
    focused(addr, &mut app, "s").await;
    for body in [json!({"verbo": "fill-secret", "args": ["@e1", "password"]}), json!({"verbo": "fill", "args": ["@e1", "x"], "valor": "segredo-7Qz"})] {
        let r = cmd(addr, "s", body.clone()).send().await.unwrap();
        assert_eq!(r.status(), 422, "{body}");
        let text = r.text().await.unwrap();
        assert!(text.contains("browser_host_secret_refused") && !text.contains("segredo-7Qz"), "{text}");
    }
    // Nada chegou ao app: o primeiro pedido que ele vê é o `url`.
    let call = tokio::spawn(cmd(addr, "s", json!({"verbo": "url", "args": [], "deadline_ms": 5000})).send());
    let asked = next_request(&mut app).await;
    assert_eq!(asked["verbo"], "url");
    send(&mut app, json!({"id": asked["id"], "ok": true, "text": "ok: \"about:blank\""})).await;
    assert_eq!(call.await.unwrap().unwrap().status(), 200);
}

/// O CLI pergunta `host`: com o app de lá só com foco (sem clique), a hora 0; sem app nenhum, 409.
#[tokio::test]
async fn browser_host_host_verb_answers_zero_for_focus_alone() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let none = cmd(addr, "s", json!({"verbo": "host", "args": []})).send().await.unwrap();
    assert_eq!(none.status(), 409);
    let mut app = host(addr).await;
    focused(addr, &mut app, "s").await;
    let r = cmd(addr, "s", json!({"verbo": "host", "args": []})).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "ok: host 0");
}

#[tokio::test]
async fn browser_host_bad_bodies_are_422() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let r = cmd(addr, "s", json!({"verbo": "url", "args": [], "cdp": 9222})).send().await.unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"], "browser_host_verb_unsupported");
    for raw in ["{", r#"{"args": []}"#, r#"{"verbo": "url", "args": "x"}"#] {
        let r = reqwest::Client::new().post(format!("http://{addr}/api/sessions/s/browser-cmd")).bearer_auth(fake::OWNER)
            .header("content-type", "application/json").body(raw).send().await.unwrap();
        assert_eq!(r.status(), 422, "{raw}");
        assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"], "erro_corpo_invalido", "{raw}");
    }
}

#[tokio::test]
async fn revoking_the_device_closes_the_host_with_4410() {
    let home = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config("127.0.0.1"));
    state.hangar_dir = home.path().join(".hangar");
    let store = std::sync::Arc::new(hangar_server::connect::devices::Store::new(home.path().join("connect"), None));
    state.connect_devices = store.clone();
    let addr = fake::spawn_state(state).await;
    let (id, token) = store.enroll("notebook").unwrap();
    let (mut app, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/browser-host?token={token}")).await.expect("canal anfitrião");
    store.remove(&id).unwrap();
    // O vigia do aparelho confere a cada 5 s; o teto cobre duas voltas.
    let code = tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(Ok(message)) = app.next().await {
            if let Message::Close(frame) = message { return frame.map(|f| u16::from(f.code)); }
        }
        None
    }).await.expect("o canal fecha com o vigia");
    assert_eq!(code, Some(4410));
}

#[tokio::test]
async fn browser_host_refuses_the_login_cookie() {
    let home = tempfile::tempdir().unwrap();
    let addr = server(home.path()).await;
    let cookie = format!("cp_token={}", fake::OWNER);
    let mut req = format!("ws://{addr}/api/browser-host").into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status(), 403),
        other => panic!("cookie sozinho abriu o canal: {:?}", other.map(|_| ())),
    }
    let r = reqwest::Client::new().post(format!("http://{addr}/api/sessions/s/browser-cmd")).header("cookie", cookie)
        .json(&json!({"verbo": "url"})).send().await.unwrap();
    assert_eq!(r.status(), 403);
}

#[cfg(unix)]
#[tokio::test]
async fn browser_host_routes_do_not_exist_on_the_guest_port() {
    use hangar_server::share::{gate::GuestConn, store::Kind};
    let home = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config("127.0.0.1"));
    let share = crate::share_pair_routes::share_state(&home.path().join("shares.json"), home.path(), std::sync::Arc::new(std::sync::Mutex::new(1_000.0)));
    let (_, invite) = share.store.create_redeemed("s0", Some("t:1".into()), Kind::Share).unwrap();
    state.share = Some(share);
    let app = hangar_server::routes::guest_router(std::sync::Arc::new(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<GuestConn>()).await.unwrap() });
    let r = reqwest::Client::new().post(format!("http://{addr}/api/sessions/s0/browser-cmd")).bearer_auth(&invite)
        .json(&json!({"verbo": "url"})).send().await.unwrap();
    // A recusa do caminho sem rota (404) ou do porteiro (403); nunca uma resposta do handler.
    let status = r.status().as_u16();
    let body = r.text().await.unwrap();
    assert!(matches!(status, 403 | 404) && !body.contains("browser_host"), "{status} {body}");
    // Caminho fora de sessão: o porteiro da 8766 recusa antes da rota, como em toda rota global do dono.
    let r = reqwest::Client::new().get(format!("http://{addr}/api/browser-host")).bearer_auth(&invite).send().await.unwrap();
    assert_eq!(r.status(), 403);
}
