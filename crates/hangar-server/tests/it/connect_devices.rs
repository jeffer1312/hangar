//! Credencial de dono por aparelho: resgate do bilhete, aprovação, a recusa por padrão da entrada do
//! Connect e o fim do acesso. Listener de verdade contra o Python falso.
#![cfg(unix)]
use crate::connect_port::{GUEST, free_port, get, guest_server, wait_up};
use crate::fake::{Fake, OWNER, client, config, spawn_fake, spawn_state};
use crate::voice_support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::auth::Auth;
use hangar_server::connect::devices::{MAX_TICKET_AHEAD, PENDING_TTL, Poll, Refusal, ServiceIdentity, Store};
use hangar_server::devices::{CONNECT_PEER, Registry};
use hangar_server::login_guests::store::hash;
use hangar_server::login_guests::{Service, Settings};
use hangar_server::routes::AppState;
use hangar_server::term::{TermConfig, Terms};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{self, Message};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

fn vector() -> Value {
    serde_json::from_str(include_str!("../fixtures/connect-ticket-vector.json")).unwrap()
}

fn identity() -> ServiceIdentity {
    let key = STANDARD.decode(vector()["public_key"].as_str().unwrap()).unwrap();
    ServiceIdentity { public_key: key.try_into().unwrap(), machine_id: "pc1".into(), account: "acme".into() }
}

fn sign(text: &str) -> String {
    let seed = vector()["seed_hex"].as_str().unwrap().to_owned();
    let seed: Vec<u8> = (0..seed.len()).step_by(2).map(|i| u8::from_str_radix(&seed[i..i + 2], 16).unwrap()).collect();
    STANDARD.encode(ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap().sign(text.as_bytes()))
}

fn real_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Bilhete assinado com a chave de teste do vetor.
fn ticket_until(machine: &str, account: &str, nonce: &str, secret: &str, expires_at: u64) -> (String, String) {
    let text = json!({"machine_id": machine, "account": account, "device_name": "celular",
        "verifier": hash(secret.as_bytes()), "nonce": nonce, "expires_at": expires_at}).to_string();
    let signature = sign(&text);
    (text, signature)
}

fn ticket(machine: &str, account: &str, nonce: &str, secret: &str) -> (String, String) {
    ticket_until(machine, account, nonce, secret, real_now() + 120)
}

struct Entrance {
    python: Arc<Fake>,
    main: SocketAddr,
    connect: SocketAddr,
    store: Arc<Store>,
    auth: Auth,
    dir: tempfile::TempDir,
}

async fn entrance(tune: impl FnOnce(&mut AppState, &Path)) -> Entrance {
    let dir = crate::physical_tempdir();
    let (python, upstream) = spawn_fake().await;
    let mut state = AppState::new(config(upstream, "127.0.0.1"));
    let store = Arc::new(Store::new(dir.path().join("connect"), Some(identity())));
    state.connect_devices = store.clone();
    let connect = free_port();
    state.connect = Some(connect);
    tune(&mut state, dir.path());
    let auth = state.auth.clone();
    let main = spawn_state(state).await;
    wait_up(connect).await;
    Entrance { python, main, connect, store, auth, dir }
}

async fn plain() -> Entrance {
    entrance(|_, _| {}).await
}

fn post(addr: SocketAddr, path: &str, body: Value) -> reqwest::RequestBuilder {
    client().post(format!("http://{addr}{path}")).header("content-type", "application/json").body(body.to_string())
}

async fn body_of(response: reqwest::Response) -> Value {
    serde_json::from_str(&response.text().await.unwrap()).unwrap()
}

async fn redeem(addr: SocketAddr, text: &str, signature: &str, secret: &str) -> reqwest::Response {
    post(addr, "/api/connect/redeem", json!({"ticket": text, "signature": signature, "secret": secret, "device_name": "nome-do-corpo"}))
        .send().await.unwrap()
}

async fn status(addr: SocketAddr, id: &str, secret: &str) -> Value {
    let response = post(addr, "/api/connect/redeem/status", json!({"device_id": id, "secret": secret})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    body_of(response).await
}

async fn approve(main: SocketAddr, id: &str) -> reqwest::StatusCode {
    client().post(format!("http://{main}/api/connect/devices/{id}/approve")).bearer_auth(OWNER).send().await.unwrap().status()
}

/// Bilhete pela entrada: o aparelho fica esperando aprovação.
async fn requested(e: &Entrance, nonce: &str) -> String {
    let secret = format!("segredo-{nonce}");
    let (text, signature) = ticket("pc1", "acme", nonce, &secret);
    let accepted = redeem(e.connect, &text, &signature, &secret).await;
    assert_eq!(accepted.status(), 202);
    body_of(accepted).await["device_id"].as_str().unwrap().to_owned()
}

async fn collected(e: &Entrance, id: &str, nonce: &str) -> String {
    status(e.connect, id, &format!("segredo-{nonce}")).await["token"].as_str().unwrap().to_owned()
}

/// O caminho inteiro de um aparelho novo: bilhete pela entrada, aprovação na máquina e token.
async fn paired(e: &Entrance, nonce: &str) -> (String, String) {
    let id = requested(e, nonce).await;
    assert_eq!(approve(e.main, &id).await, 200);
    let token = collected(e, &id, nonce).await;
    (id, token)
}

/// O mesmo caminho, com a aprovação vinda de outro aparelho pela internet.
async fn paired_by(e: &Entrance, approver: &str, nonce: &str) -> (String, String) {
    let id = requested(e, nonce).await;
    let approved = client().post(format!("http://{}/api/connect/devices/{id}/approve", e.connect)).bearer_auth(approver).send().await.unwrap();
    assert_eq!(approved.status(), 200);
    let token = collected(e, &id, nonce).await;
    (id, token)
}

/// `Service::from_state` lê `.hangar/config` do `HOME` da conta.
fn with_home(state: &mut AppState, dir: &Path, peers: &str) {
    let config = dir.join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("peers.json"), peers).unwrap();
    std::fs::write(config.join(".env"), "").unwrap();
    let root = dir.to_str().unwrap();
    state.accounts = AccountService::new(AccountEnvironment::from_pairs([("HOME", root), ("USERPROFILE", root)]));
}

async fn api_status(addr: SocketAddr, token: &str) -> reqwest::StatusCode {
    get(addr, "/api/client-api").bearer_auth(token).send().await.unwrap().status()
}

struct Tmux { _dir: tempfile::TempDir, socket: PathBuf }

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux").arg("-S").arg(&self.socket).arg("kill-server").env_remove("TMUX").output();
    }
}

/// Sessão `t` num tmux próprio, para o terminal de verdade.
fn tmux() -> Tmux {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("t.sock");
    let out = std::process::Command::new("tmux").arg("-S").arg(&socket).args(["-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "80", "-y", "24", "cat"])
        .env_remove("TMUX").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Tmux { _dir: dir, socket }
}

fn terms(socket: PathBuf) -> Arc<Terms> {
    Arc::new(Terms::new(TermConfig { socket: Some(socket), ..Default::default() }))
}

async fn open_terminal(addr: SocketAddr, token: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/sessions/t/term?token={token}")).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(first, Message::Binary(_)), "{first:?}");
    ws
}

async fn open_call(addr: SocketAddr, token: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/voice?token={token}")).await.unwrap();
    let hello = json!({"type": "hello", "client": "pwa", "screen": {"server": "", "name": "hangar"}, "caps": ["switch_session"]});
    ws.send(Message::Text(hello.to_string().into())).await.unwrap();
    ws.send(Message::Text(json!({"type": "offer", "sdp": "v=0 pwa"}).to_string().into())).await.unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("prazo").expect("fim").unwrap();
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "answer" { return ws; }
        }
    }
}

/// O código de fechamento; `None` quando o socket caiu sem quadro de fechamento.
async fn closed(ws: &mut Ws) -> Option<u16> {
    tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Close(frame) = message { return frame.map(|f| u16::from(f.code)); }
        }
        None
    }).await.expect("o vigia fecha em até 5 s")
}

#[tokio::test]
async fn redeem_requires_the_secret_behind_the_verifier() {
    let e = plain().await;
    let (text, signature) = ticket("pc1", "acme", "n1", "segredo");
    assert_eq!(redeem(e.connect, &text, &signature, "outro").await.status(), 401);
    let accepted = redeem(e.connect, &text, &signature, "segredo").await;
    assert_eq!(accepted.status(), 202, "errar o segredo não gasta o nonce");
    assert_eq!(body_of(accepted).await["status"], "pending");
}

#[tokio::test]
async fn redeem_refuses_bad_or_replayed_ticket() {
    let e = plain().await;
    let (text, signature) = ticket("pc1", "acme", "n1", "segredo");
    let forged = text.replace("celular", "intruso");
    assert_eq!(redeem(e.connect, &forged, &signature, "segredo").await.status(), 401);
    assert_eq!(redeem(e.connect, &text, "nao-e-base64!", "segredo").await.status(), 401);
    assert_eq!(redeem(e.connect, &text, &signature, "segredo").await.status(), 202);
    assert_eq!(redeem(e.connect, &text, &signature, "segredo").await.status(), 401, "nonce usado");
    let (text, signature) = ticket("pc1", "acme", "n2", "segredo");
    let later = real_now() + 121;
    let late = Store::new(e.dir.path().join("depois"), Some(identity())).with_clock(Arc::new(move || later));
    assert!(matches!(late.redeem(&text, &signature, "segredo"), Err(Refusal::Ticket)), "bilhete vencido");
    let (text, signature) = ticket_until("pc1", "acme", "n3", "segredo", real_now() + MAX_TICKET_AHEAD + 60);
    assert_eq!(redeem(e.connect, &text, &signature, "segredo").await.status(), 401, "validade longa demais");
}

#[test]
fn a_second_rotation_keeps_the_token_whose_delivery_was_never_confirmed() {
    let dir = crate::physical_tempdir();
    let store = Store::new(dir.path().to_path_buf(), None);
    let (id, first) = store.enroll("máquina pc-b").unwrap();
    let delivered = store.rotate(&id).unwrap();
    // A outra ponta gravou `delivered`, mas a resposta se perdeu: o anúncio seguinte gira de novo.
    let next = store.rotate(&id).unwrap();
    assert!(store.verify(first.as_bytes()).is_some(), "o token antigo segue valendo até uma entrega se confirmar");
    assert!(store.verify(delivered.as_bytes()).is_some(), "quem recebeu a entrega sem confirmação não perde o acesso");
    assert!(store.verify(next.as_bytes()).is_some());
    assert!(store.verify(first.as_bytes()).is_none(), "usar o token mais novo encerra os outros");
    assert!(store.verify(delivered.as_bytes()).is_none());
}

#[test]
fn using_the_new_token_survives_a_restart() {
    let dir = crate::physical_tempdir();
    let store = Store::new(dir.path().to_path_buf(), None);
    let (id, first) = store.enroll("máquina pc-b").unwrap();
    let delivered = store.rotate(&id).unwrap();
    assert!(store.verify(delivered.as_bytes()).is_some());
    let reopened = Store::new(dir.path().to_path_buf(), None);
    assert!(reopened.verify(first.as_bytes()).is_none(), "o antigo não volta a valer depois de reiniciar");
    assert_eq!(reopened.proof_key(&id), Some(hangar_core::credentials::hash(delivered.as_bytes())), "a prova usa o token em uso");
}

#[tokio::test]
async fn redeem_without_a_link_answers_not_linked() {
    let dir = crate::physical_tempdir();
    let store = Arc::new(Store::new(dir.path().to_path_buf(), None));
    let linked = store.clone();
    let e = entrance(move |state, _| state.connect_devices = linked).await;
    let (text, signature) = ticket("pc1", "acme", "n1", "segredo");
    let refused = redeem(e.connect, &text, &signature, "segredo").await;
    assert_eq!(refused.status(), 409);
    assert!(refused.text().await.unwrap().contains("connect_not_linked"));
    // A ligação chega com o servidor de pé.
    store.set_identity(Some(identity()));
    assert_eq!(redeem(e.connect, &text, &signature, "segredo").await.status(), 202);
}

#[tokio::test]
async fn ticket_of_another_machine_or_account_is_refused() {
    let e = plain().await;
    for (machine, account) in [("pc2", "acme"), ("pc1", "outra")] {
        let (text, signature) = ticket(machine, account, "n1", "segredo");
        assert_eq!(redeem(e.connect, &text, &signature, "segredo").await.status(), 401, "{machine} {account}");
    }
    assert!(e.store.list().is_empty());
    // A mesma pasta nas duas lojas: a recusa da outra máquina não pode gastar o nonce.
    let dir = crate::physical_tempdir();
    let (text, signature) = ticket("pc1", "acme", "n2", "segredo");
    let elsewhere = Store::new(dir.path().to_path_buf(), Some(ServiceIdentity { machine_id: "pc2".into(), ..identity() }));
    assert!(matches!(elsewhere.redeem(&text, &signature, "segredo"), Err(Refusal::Ticket)));
    Store::new(dir.path().to_path_buf(), Some(identity())).redeem(&text, &signature, "segredo").unwrap();
}

#[test]
fn ticket_signature_matches_the_shared_vector() {
    let v = vector();
    let (text, signature, secret) = (v["ticket"].as_str().unwrap(), v["signature"].as_str().unwrap(), v["secret"].as_str().unwrap());
    assert_eq!(sign(text), signature, "mesmos bytes do repositório hangar-connect");
    let ticket: Value = serde_json::from_str(text).unwrap();
    assert_eq!(ticket["verifier"], hash(secret.as_bytes()));
    let dir = crate::physical_tempdir();
    // O vetor vale até 2100: o relógio fica um minuto antes, dentro do que a máquina aceita.
    let store = Store::new(dir.path().to_path_buf(), Some(identity())).with_clock(Arc::new(|| 4102444800u64 - 60));
    store.redeem(text, signature, secret).unwrap();
    assert_eq!(store.list()[0].name, "celular");
}

#[tokio::test]
async fn pending_device_has_no_access_until_approved() {
    let e = plain().await;
    let (text, signature) = ticket("pc1", "acme", "n1", "s1");
    let accepted = redeem(e.connect, &text, &signature, "s1").await;
    let id = body_of(accepted).await["device_id"].as_str().unwrap().to_owned();
    assert_eq!(status(e.connect, &id, "s1").await, json!({"status": "pending"}));
    let wrong = post(e.connect, "/api/connect/redeem/status", json!({"device_id": id, "secret": "s2"})).send().await.unwrap();
    assert_eq!(wrong.status(), 401);
    let rows = body_of(get(e.main, "/api/connect/devices").bearer_auth(OWNER).send().await.unwrap()).await;
    assert_eq!(rows, json!([{"device_id": id, "name": "celular", "status": "pending", "last_seen": null}]), "o nome é o assinado, não o do corpo");
    assert_eq!(get(e.connect, "/api/connect/devices").send().await.unwrap().status(), 401);
    // Recusado não recebe token, nem aprovado depois.
    let removed = client().delete(format!("http://{}/api/connect/devices/{id}", e.main)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(removed.status(), 200);
    assert_eq!(status(e.connect, &id, "s1").await, json!({"status": "denied"}));
    assert_eq!(approve(e.main, &id).await, 404);
    // Aprovado recebe uma vez só, e só o hash fica no disco.
    let (id, token) = paired(&e, "n2").await;
    assert_eq!(status(e.connect, &id, "segredo-n2").await, json!({"status": "approved"}));
    assert_eq!(api_status(e.connect, &token).await, 200);
    let disk = std::fs::read_to_string(e.dir.path().join("connect/devices.json")).unwrap();
    assert!(!disk.contains(&token) && disk.contains(&hash(token.as_bytes())));
}

#[test]
fn at_most_five_pending_requests_and_they_expire() {
    let dir = crate::physical_tempdir();
    let now = Arc::new(AtomicU64::new(1000));
    let clock = now.clone();
    let store = Store::new(dir.path().to_path_buf(), Some(identity())).with_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
    let ids: Vec<String> = (0..5).map(|i| {
        let (text, signature) = ticket_until("pc1", "acme", &format!("n{i}"), "segredo", 1060);
        store.redeem(&text, &signature, "segredo").unwrap()
    }).collect();
    let (text, signature) = ticket_until("pc1", "acme", "sexto", "segredo", 1060);
    assert!(matches!(store.redeem(&text, &signature, "segredo"), Err(Refusal::TooManyPending)));
    store.remove(&ids[0]).unwrap();
    store.redeem(&text, &signature, "segredo").expect("o pedido acima não gastou o nonce");
    now.store(1000 + PENDING_TTL, Ordering::SeqCst);
    assert_eq!(store.status(&ids[1], "segredo").unwrap(), Poll::Denied, "pedido vencido não vira aprovação");
    assert!(matches!(store.approve(&ids[1], None), Err(Refusal::Unknown)));
}

#[test]
fn approved_token_not_picked_up_in_time_expires() {
    let dir = crate::physical_tempdir();
    let now = Arc::new(AtomicU64::new(1000));
    let clock = now.clone();
    let store = Store::new(dir.path().to_path_buf(), Some(identity())).with_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
    let ids: Vec<String> = ["n1", "n2"].into_iter().map(|nonce| {
        let (text, signature) = ticket_until("pc1", "acme", nonce, "segredo", 1060);
        let id = store.redeem(&text, &signature, "segredo").unwrap();
        store.approve(&id, None).unwrap();
        id
    }).collect();
    now.store(1000 + PENDING_TTL - 1, Ordering::SeqCst);
    assert!(matches!(store.status(&ids[0], "segredo").unwrap(), Poll::Approved(Some(_))));
    now.store(1000 + PENDING_TTL, Ordering::SeqCst);
    assert_eq!(store.status(&ids[1], "segredo").unwrap(), Poll::Denied, "segredo vazado depois do prazo não recolhe token");
    assert_eq!(store.status(&ids[0], "segredo").unwrap(), Poll::Approved(None), "quem buscou a tempo não vence");
}

#[tokio::test(flavor = "multi_thread")]
async fn device_token_works_on_every_owner_route() {
    let tmux = tmux();
    let socket = tmux.socket.clone();
    let e = entrance(move |state, _| state.term = terms(socket)).await;
    let (_, token) = paired(&e, "rotas").await;
    assert_eq!(api_status(e.connect, &token).await, 200);
    let cookie = get(e.connect, "/api/client-api").header("cookie", format!("__Host-cp_token={token}"));
    assert_eq!(cookie.send().await.unwrap().status(), 200);
    let write = post(e.connect, "/api/qualquer", json!({})).header("cookie", format!("__Host-cp_token={token}"));
    assert_eq!(write.send().await.unwrap().status(), 401, "cookie só lê");
    let sse = get(e.connect, "/api/sessions/events").bearer_auth(&token).send().await.unwrap();
    assert_eq!(sse.status(), 200);
    assert!(sse.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    // Ao Python vai a senha, dentro do processo; a credencial do aparelho não sai da camada.
    assert_eq!(get(e.connect, "/api/qualquer").bearer_auth(&token).send().await.unwrap().status(), 200);
    assert_eq!(e.python.last_hit().1["authorization"], format!("Bearer {OWNER}"));
    assert_eq!(get(e.connect, &format!("/api/qualquer?token={token}&x=1")).send().await.unwrap().status(), 200);
    assert_eq!(e.python.last_hit().0, format!("/api/qualquer?token={OWNER}&x=1"));
    let terminal = open_terminal(e.connect, &token).await;
    drop(terminal);
    let (voice, _) = connect_async(format!("ws://{}/api/voice?token={token}", e.connect)).await.expect("voz pelo Connect");
    drop(voice);
    let call = voice_support::voice_server().await;
    let (_, call_token) = call.devices.enroll("celular").unwrap();
    open_call(call.addr, &call_token).await;
}

#[tokio::test]
async fn device_token_works_on_8765_too() {
    let e = plain().await;
    let (_, token) = paired(&e, "duas").await;
    assert_eq!(api_status(e.main, &token).await, 200);
    assert_eq!(api_status(e.connect, &token).await, 200);
    assert_eq!(get(e.main, &format!("/api/client-api?token={token}")).send().await.unwrap().status(), 200);
    let cookie = get(e.main, "/api/client-api").header("cookie", format!("cp_token={token}"));
    assert_eq!(cookie.send().await.unwrap().status(), 200);
    assert_eq!(get(e.main, "/api/qualquer").bearer_auth(&token).send().await.unwrap().status(), 200);
    let (_, headers) = e.python.last_hit();
    assert_eq!(headers["authorization"], format!("Bearer {OWNER}"));
    assert_eq!(headers["x-forwarded-for"], "127.0.0.1");
}

#[tokio::test]
async fn self_upgrade_needs_the_owner_password_and_is_refused_on_connect() {
    let e = plain().await;
    let enroll = |addr| post(addr, "/api/connect/devices/self", json!({"device_name": "notebook"}));
    assert_eq!(enroll(e.main).send().await.unwrap().status(), 401);
    let created = enroll(e.main).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(created.status(), 200);
    let token = body_of(created).await["token"].as_str().unwrap().to_owned();
    assert_eq!(api_status(e.connect, &token).await, 200);
    assert_eq!(enroll(e.main).bearer_auth(&token).send().await.unwrap().status(), 403, "aparelho não cria outro");
    assert_eq!(enroll(e.connect).bearer_auth(OWNER).send().await.unwrap().status(), 401);
    assert_eq!(enroll(e.connect).bearer_auth(&token).send().await.unwrap().status(), 401);
    assert_eq!(e.store.list().len(), 1);
}

#[tokio::test]
async fn owner_password_is_refused_through_connect() {
    let e = plain().await;
    let path = "/api/client-api";
    for request in [
        get(e.connect, path).bearer_auth(OWNER),
        get(e.connect, &format!("{path}?token={OWNER}")),
        get(e.connect, path).header("cookie", format!("__Host-cp_token={OWNER}")),
    ] {
        assert_eq!(request.send().await.unwrap().status(), 401);
    }
    assert_eq!(get(e.connect, "/api/qualquer").bearer_auth(OWNER).send().await.unwrap().status(), 401);
    assert_eq!(e.python.hits_to("/api/qualquer"), 0);
    let socket = connect_async(format!("ws://{}/api/sessions/t/term?token={OWNER}", e.connect)).await;
    assert!(matches!(socket, Err(tungstenite::Error::Http(ref r)) if r.status() == 401));
    // Recusa sem ler o corpo: o que foi anunciado nunca chega.
    let mut stream = TcpStream::connect(e.connect).await.unwrap();
    stream.write_all(b"POST /api/sessions HTTP/1.1\r\nHost: h\r\nContent-Length: 1000000\r\n\r\n").await.unwrap();
    let mut head = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut head)).await.unwrap().unwrap();
    assert_eq!(&head, b"HTTP/1.1 401");
}

#[tokio::test]
async fn everything_else_is_denied_by_default() {
    let e = plain().await;
    for path in ["/", "/assets/app.js", "/__hangar_server/health", "/api/sessions", "/api/peers", "/api/connect/devices",
        "/api/sync/setup", "/api/sync/status", "/api/sync/vault", "/api/sync/guests", "/api/peers/pin%67", "/api/peers/ping/", "/docs", "/internal/diag"] {
        assert_eq!(get(e.connect, path).send().await.unwrap().status(), 401, "{path}");
    }
    for path in ["/api/sync/register", "/api/sync/logout", "/api/sync/guests/ana/delete"] {
        assert_eq!(post(e.connect, path, json!({})).send().await.unwrap().status(), 401, "{path}");
    }
    assert_eq!(e.python.hits_to("/docs"), 0);
    assert_eq!(get(e.connect, "/api/peers/ping").send().await.unwrap().status(), 200);
    assert_ne!(get(e.connect, "/api/sync/prelogin?user=ana").send().await.unwrap().status(), 401, "o login precisa do sal");
    assert_ne!(post(e.connect, "/api/sync/login", json!({})).send().await.unwrap().status(), 401);
    let preflight = client().request(reqwest::Method::OPTIONS, format!("http://{}/api/sessions", e.connect))
        .header("origin", "https://app.example").header("access-control-request-method", "GET");
    assert_eq!(preflight.send().await.unwrap().status(), 200);
    assert_eq!(e.python.hits_to("/api/sessions"), 0);
}

#[tokio::test]
async fn login_guest_works_through_connect() {
    let (_home, _python, connect) = guest_server().await;
    let me = get(connect, "/api/me").bearer_auth(GUEST).send().await.unwrap();
    assert_eq!(me.status(), 200);
    assert_eq!(body_of(me).await["role"], "guest");
    assert_eq!(get(connect, "/api/me").bearer_auth("errado").send().await.unwrap().status(), 401);
    // A senha ao lado do convidado some antes das rotas: acertar não muda nada.
    let probing = get(connect, &format!("/api/me?token={OWNER}")).bearer_auth(GUEST).send().await.unwrap();
    assert_eq!(body_of(probing).await["role"], "guest");
}

/// O `syncLogin` do app, pela entrada: `prelogin` e `login` abertos, `vault` e `logout` só com a sessão.
#[tokio::test]
async fn sync_vault_and_logout_need_a_valid_session_through_connect() {
    let e = entrance(|state, dir| {
        state.login_guests = Arc::new(Service::new(Settings { guests: dir.join("guests.json"), vault: dir.join("vault.json"),
            runtime_config: dir.join("config.json"), sync_enabled: true, bootstrap: "bootstrap".into(),
            secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into() }));
    }).await;
    let registered = post(e.main, "/api/sync/register", json!({"user": "ana", "salt": "sal", "auth_hash": "hash", "bootstrap": "bootstrap"}));
    assert_eq!(registered.send().await.unwrap().status(), 200);
    let vault = |cookie: &str| get(e.connect, "/api/sync/vault").header("cookie", cookie.to_owned());
    let logout = |cookie: &str| post(e.connect, "/api/sync/logout", json!({})).header("cookie", cookie.to_owned());
    for cookie in ["outro=1", "cp_sync=", "cp_sync=ana.9999999999.assinatura-falsa"] {
        assert_eq!(vault(cookie).send().await.unwrap().status(), 401, "{cookie}");
        assert_eq!(logout(cookie).send().await.unwrap().status(), 401, "{cookie}");
    }
    assert_eq!(get(e.connect, "/api/sync/vault").bearer_auth(OWNER).send().await.unwrap().status(), 401, "a senha do dono não abre o cofre");
    let login = post(e.connect, "/api/sync/login", json!({"user": "ana", "auth_hash": "hash"})).send().await.unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();
    let read = vault(&cookie).send().await.unwrap();
    assert_eq!(read.status(), 200);
    assert!(body_of(read).await["enc_blob"].is_null());
    let write = client().put(format!("http://{}/api/sync/vault", e.connect)).header("cookie", cookie.clone())
        .header("content-type", "application/json").body(r#"{"enc_blob":{},"base_rev":0}"#);
    assert_eq!(write.send().await.unwrap().status(), 401, "pela entrada o cofre só se lê");
    assert_eq!(logout(&cookie).send().await.unwrap().status(), 200);
    assert_eq!(e.python.hits_to("/api/sync/vault"), 0);
}

#[tokio::test]
async fn robot_on_connect_never_blocks_devices() {
    let e = plain().await;
    let (_, token) = paired(&e, "robo").await;
    for _ in 0..20 {
        assert!(connect_async(format!("ws://{}/api/voice", e.connect)).await.is_err());
        assert_eq!(api_status(e.connect, "errado").await, 401);
        assert_eq!(get(e.connect, "/api/client-api?token=errado").send().await.unwrap().status(), 401);
        let _ = post(e.connect, "/api/sync/login", json!({"user": "x", "password": "y"})).send().await.unwrap();
    }
    assert!(!e.auth.is_blocked_at(CONNECT_PEER, Instant::now()), "nada do que falhou aqui chegou ao contador");
    assert_eq!(api_status(e.connect, &token).await, 200);
    // Convidado válido e Python ainda alcançam o contador: cheio, ele não pode barrar o aparelho.
    e.auth.mark_blocked(CONNECT_PEER);
    assert_eq!(api_status(e.connect, &token).await, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_device_closes_open_streams() {
    let tmux = tmux();
    let socket = tmux.socket.clone();
    let e = entrance(move |state, _| state.term = terms(socket)).await;
    let (id, token) = paired(&e, "fim").await;
    let sse = get(e.connect, "/api/sessions/events").bearer_auth(&token).send().await.unwrap();
    assert_eq!(sse.status(), 200);
    let mut sse = Box::pin(sse.bytes_stream());
    let mut terminal = open_terminal(e.connect, &token).await;
    // Repasse ao Python: o eco prova que o cano está de pé.
    let mut piped = TcpStream::connect(e.connect).await.unwrap();
    piped.write_all(format!("GET /api/sessions/cc/proxy-ws-test?token={token} HTTP/1.1\r\nHost: h\r\n\
        Connection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(5), piped.read_exact(&mut byte)).await.unwrap().unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    let call = voice_support::voice_server_with_long_grace().await;
    let (call_id, call_token) = call.devices.enroll("celular").unwrap();
    let mut voice = open_call(call.addr, &call_token).await;

    e.store.remove(&id).unwrap();
    call.devices.remove(&call_id).unwrap();

    tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(chunk) = sse.next().await {
            if chunk.is_err() { break; }
        }
    }).await.expect("o SSE cai com o vigia");
    assert_eq!(closed(&mut terminal).await, Some(4410));
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(12), piped.read_to_end(&mut rest)).await.expect("o vigia fecha em até 5 s").unwrap();
    assert_eq!(rest, [0x88, 0x02, 0x11, 0x3A]);
    closed(&mut voice).await;
    // Com prazo sem dono de dois minutos, só o `Stop` derruba o app-server aqui.
    assert!(call.app_server_sees("<eof>").await, "a chamada acaba junto com o aparelho");
    assert_eq!(api_status(e.connect, &token).await, 401);
    assert_eq!(api_status(e.main, &token).await, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_revokes_only_itself_and_what_it_approved() {
    let e = plain().await;
    let (first, first_token) = paired(&e, "a").await;
    let (second, second_token) = paired_by(&e, &first_token, "b").await;
    let (third, third_token) = paired_by(&e, &second_token, "c").await;
    let (other, other_token) = paired(&e, "d").await;
    let revoke = |id: &str, token: &str| client().delete(format!("http://{}/api/connect/devices/{id}", e.connect)).bearer_auth(token).send();

    let refused = revoke(&other, &second_token).await.unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(body_of(refused).await["detail"]["code"], "connect_device_not_yours");
    assert_eq!(revoke(&first, &second_token).await.unwrap().status(), 403, "nem quem o aprovou");
    assert_eq!(api_status(e.connect, &other_token).await, 200);

    // Quem pode aprovar o pedido pode recusá-lo: o cartão do celular continua servindo.
    let pending = requested(&e, "e").await;
    assert_eq!(revoke(&pending, &other_token).await.unwrap().status(), 200);

    assert_eq!(revoke(&third, &first_token).await.unwrap().status(), 200, "aprovado por quem ele aprovou");
    assert_eq!(api_status(e.connect, &third_token).await, 401);
    assert_eq!(revoke(&other, &other_token).await.unwrap().status(), 200, "a si mesmo");
    assert_eq!(api_status(e.connect, &other_token).await, 401);

    let by_owner = client().delete(format!("http://{}/api/connect/devices/{second}", e.main)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(by_owner.status(), 200, "a senha do dono segue sem limite");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_device_takes_everything_it_approved() {
    let e = plain().await;
    let (first, first_token) = paired(&e, "a").await;
    let (second, second_token) = paired_by(&e, &first_token, "b").await;
    let (_, third_token) = paired_by(&e, &second_token, "c").await;
    let (_, owners_token) = paired(&e, "d").await;
    // Reaprovar-se não troca quem aprovou: a cascata alcança do mesmo jeito.
    let again = client().post(format!("http://{}/api/connect/devices/{second}/approve", e.connect)).bearer_auth(&second_token).send().await.unwrap();
    assert_eq!(again.status(), 200);
    let sse = get(e.connect, "/api/sessions/events").bearer_auth(&third_token).send().await.unwrap();
    assert_eq!(sse.status(), 200);
    let mut sse = Box::pin(sse.bytes_stream());

    let removed = client().delete(format!("http://{}/api/connect/devices/{first}", e.main)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(removed.status(), 200);

    for token in [&first_token, &second_token, &third_token] {
        assert_eq!(api_status(e.connect, token).await, 401);
    }
    assert_eq!(api_status(e.connect, &owners_token).await, 200, "o aprovado pelo dono fica");
    assert_eq!(e.store.list().len(), 1);
    tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(chunk) = sse.next().await {
            if chunk.is_err() { break; }
        }
    }).await.expect("o SSE de quem foi junto também cai");
    assert!(matches!(e.store.approve("qualquer", Some(first.as_str())), Err(Refusal::Credential)), "revogado não aprova mais");
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
}

fn set_aside_files(dir: &Path) -> usize {
    std::fs::read_dir(dir).unwrap().filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".bad-")).count()
}

/// Falha de leitura não diz nada do conteúdo: o arquivo bom fica no lugar, e nada grava por cima dele.
#[test]
fn unreadable_devices_file_stays_and_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (id, token) = Store::new(dir.path().to_path_buf(), None).enroll("celular").unwrap();
    let file = dir.path().join("devices.json");
    let saved = std::fs::read(&file).unwrap();
    set_mode(&file, 0o000);
    if std::fs::read(&file).is_ok() { return; } // root lê tudo

    let store = Store::new(dir.path().to_path_buf(), None);
    assert!(store.fault().is_some());
    assert!(store.list().is_empty());
    assert!(matches!(store.enroll("outro"), Err(Refusal::Io(_))), "gravar agora apagaria quem ficou no arquivo");
    set_mode(&file, 0o600);
    assert_eq!(std::fs::read(&file).unwrap(), saved);
    assert_eq!(set_aside_files(dir.path()), 0);

    let back = Store::new(dir.path().to_path_buf(), None);
    assert!(back.fault().is_none());
    assert_eq!(back.verify(token.as_bytes()), Some(id));
}

#[test]
fn corrupt_devices_file_goes_aside_and_the_store_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("devices.json"), "{quebrado").unwrap();
    let store = Store::new(dir.path().to_path_buf(), None);
    assert!(store.fault().is_none());
    assert_eq!(set_aside_files(dir.path()), 1);
    store.enroll("celular").unwrap();
}

#[test]
fn corrupt_devices_file_that_cannot_move_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("devices.json");
    std::fs::write(&file, "{quebrado").unwrap();
    set_mode(dir.path(), 0o500);
    if std::fs::write(dir.path().join("root"), "").is_ok() { set_mode(dir.path(), 0o700); return; } // root grava em tudo

    let store = Store::new(dir.path().to_path_buf(), None);
    assert!(store.fault().is_some());
    assert!(matches!(store.enroll("celular"), Err(Refusal::Io(_))));
    set_mode(dir.path(), 0o700);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{quebrado");
}

#[tokio::test(flavor = "multi_thread")]
async fn nav_link_carries_no_device_token_and_no_password() {
    let dir = tempfile::tempdir().unwrap();
    let srv = crate::list_support::server(dir.path(), 1).await;
    let (_, token) = srv.devices.enroll("celular").unwrap();
    let mut list = crate::list_support::open(srv.addr).await;
    let sent = post(srv.addr, "/api/sessions/s0/nav", json!({"url": "/api/sessions/s0/pages/abc"})).bearer_auth(&token).send().await.unwrap();
    assert_eq!(sent.status(), 200);
    let nav = crate::fake::next_named(&mut list, "nav").await;
    assert!(nav.data.contains("page_ticket=") && !nav.data.contains(&token) && !nav.data.contains(OWNER), "{}", nav.data);
    let sent = post(srv.addr, "/api/sessions/s0/nav", json!({"url": "/api/sessions/s0/pages/abc"})).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(sent.status(), 200);
    let nav = crate::fake::next_named(&mut list, "nav").await;
    assert!(nav.data.contains("page_ticket=") && !nav.data.contains(OWNER), "pedido do dono também: {}", nav.data);
}

#[tokio::test]
async fn owner_secret_routes_are_refused_to_a_device() {
    const PEER_TOKEN: &str = "token-secreto-da-outra-maquina";
    let peers = json!({"outra": {"app": {"id": "outra", "label": "Outra", "address": "https://outra.test", "token": PEER_TOKEN}}}).to_string();
    let e = entrance(move |state, dir| with_home(state, dir, &peers)).await;
    let (_, token) = paired(&e, "senha").await;
    for addr in [e.main, e.connect] {
        for path in ["/api/alcance/pareamento?endereco=lan", "/api/peers/abc/token", "/api/peers/abc/%74oken", "/api/peers/abc/token/"] {
            let refused = get(addr, path).bearer_auth(&token).send().await.unwrap();
            assert_eq!(refused.status(), 403, "{addr} {path}");
            assert!(!refused.text().await.unwrap().contains(OWNER));
        }
        // Redirecionamento do Rust e do Python: a query trocada não sai no `Location`.
        for path in ["/api/peers/", "/api/me/", "/api/redirect-query"] {
            let redirected = get(addr, &format!("{path}?token={token}&x=1")).send().await.unwrap();
            assert_eq!(redirected.status(), 307, "{addr} {path}");
            let location = redirected.headers()["location"].to_str().unwrap();
            assert!(location.contains(&token) && !location.contains(OWNER), "{location}");
        }
        let listed = body_of(get(addr, "/api/peers/app-list").bearer_auth(&token).send().await.unwrap()).await;
        assert_eq!(listed["servers"][0]["id"], "outra");
        assert!(!listed.to_string().contains(PEER_TOKEN), "{listed}");
        let write = client().put(format!("http://{addr}/api/peers/app-list")).bearer_auth(&token)
            .header("content-type", "application/json").body(json!({"rev": listed["rev"], "servers": listed["servers"]}).to_string());
        assert_eq!(write.send().await.unwrap().status(), 403, "gravar a lista mascarada apagaria os tokens");
    }
    assert_ne!(get(e.main, "/api/peers/abc/token").bearer_auth(OWNER).send().await.unwrap().status(), 403, "o dono segue passando");
    let owners = body_of(get(e.main, "/api/peers/app-list").bearer_auth(OWNER).send().await.unwrap()).await;
    assert_eq!(owners["servers"][0]["token"], PEER_TOKEN);
}

#[tokio::test]
async fn owner_password_with_reserved_characters_stays_in_the_process() {
    const PASSWORD: &str = "dono+senha/com=sinais";
    let e = entrance(|state, _| {
        state.cfg.auth_token = PASSWORD.into();
        state.auth = Auth::new(PASSWORD);
    }).await;
    let (_, token) = e.store.enroll("celular").unwrap();
    for path in ["/api/peers/", "/api/redirect-query"] {
        let redirected = get(e.connect, &format!("{path}?token={token}")).send().await.unwrap();
        assert_eq!(redirected.status(), 307, "{path}");
        let location = redirected.headers()["location"].to_str().unwrap();
        assert!(location.contains(&token) && !location.contains("dono"), "{location}");
    }
}

#[tokio::test]
async fn connect_device_is_recorded_in_the_device_list() {
    let e = entrance(|state, dir| state.devices = Arc::new(Registry::new(dir.join("seen.json")))).await;
    let (_, token) = paired(&e, "lista").await;
    let opened = get(e.connect, "/api/sessions/events?device=ios%3Bc-1").bearer_auth(&token).send().await.unwrap();
    assert_eq!(opened.status(), 200);
    let seen: Value = serde_json::from_str(&std::fs::read_to_string(e.dir.path().join("seen.json")).unwrap()).unwrap();
    assert_eq!(seen[0]["id"], "c-1");
}

#[tokio::test]
async fn proof_answers_with_device_hash() {
    let e = entrance(|state, dir| with_home(state, dir, "{}")).await;
    let (id, token) = paired(&e, "prova").await;
    let challenge = "desafio-com-tamanho-certo";
    let expected = |key: &[u8], signed: String| -> String {
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
        let digest = ring::hmac::sign(&key, signed.as_bytes());
        digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    };
    let public = "pc1.acme.hangar.dev.br";
    let proof = body_of(get(e.connect, &format!("/api/peers/prova?desafio={challenge}&device={id}")).header("host", public).send().await.unwrap()).await;
    let identifier = proof["identificador"].as_str().unwrap();
    let device_key = hash(token.as_bytes());
    assert_eq!(proof["prova"], expected(device_key.as_bytes(), format!("{challenge}|{identifier}|{public}")));
    // Rede com o mesmo IP local repassa a pergunta ao Connect: o celular confere com o host que ele chamou.
    assert_ne!(proof["prova"], expected(device_key.as_bytes(), format!("{challenge}|{identifier}|192.168.1.5:8765")), "prova repassada não vale");
    let owner = body_of(get(e.main, &format!("/api/peers/prova?desafio={challenge}")).header("host", public).send().await.unwrap()).await;
    assert_eq!(owner["prova"], expected(OWNER.as_bytes(), format!("{challenge}|{identifier}")), "sem `device` a prova segue como era");
    let unknown = get(e.connect, &format!("/api/peers/prova?desafio={challenge}&device=outro")).send().await.unwrap();
    assert_eq!(unknown.status(), 404);
}

/// Pela internet, prova assinada com a senha deixaria testar palpites fora da máquina.
#[tokio::test]
async fn owner_proof_is_not_public_through_connect() {
    let e = entrance(|state, dir| with_home(state, dir, "{}")).await;
    let challenge = "desafio-com-tamanho-certo";
    for query in [format!("desafio={challenge}"), format!("desafio={challenge}&device="), format!("device=&desafio={challenge}")] {
        let refused = get(e.connect, &format!("/api/peers/prova?{query}")).send().await.unwrap();
        assert_eq!(refused.status(), 401, "{query}");
    }
    // `device` repetido vazio não cai na senha: o valor que vale é o último, e ele não é aparelho.
    let tricked = get(e.connect, &format!("/api/peers/prova?desafio={challenge}&device=x&device=")).send().await.unwrap();
    assert_eq!(tricked.status(), 404);
}
