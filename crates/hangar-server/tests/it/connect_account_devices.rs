//! Aparelhos da conta: a leitura que aplica a revogação da conta nesta máquina e as rotas que repassam
//! ao serviço com a chave de máquina. Serviço falso em porta livre; nada toca o serviço real.
#![cfg(unix)]
use crate::connect_port::{free_port, wait_up};
use crate::fake::{Fake, OWNER, client, config, spawn_fake, spawn_state};

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{delete, get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use hangar_server::connect::devices::{Poll, Store};
use hangar_server::connect::state::{self, Linked};
use hangar_server::connect::supervisor::Supervisor;
use hangar_server::login_guests::store::hash;
use hangar_server::routes::AppState;
use serde_json::{Value, json};

const MACHINE_KEY: &str = "chave-de-maquina-de-teste";
const CODE: &str = "connect_account_devices_unreadable";

fn vector() -> Value {
    serde_json::from_str(include_str!("../fixtures/connect-ticket-vector.json")).unwrap()
}

fn sign(text: &str) -> String {
    let seed = vector()["seed_hex"].as_str().unwrap().to_owned();
    let seed: Vec<u8> = (0..seed.len()).step_by(2).map(|i| u8::from_str_radix(&seed[i..i + 2], 16).unwrap()).collect();
    STANDARD.encode(ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap().sign(text.as_bytes()))
}

/// Os aparelhos da conta no serviço falso, com o segredo de cada um.
#[derive(Default)]
struct Account {
    devices: Mutex<Vec<(Value, String)>>,
}

impl Account {
    fn add(&self, id: &str, status: &str, secret: &str) {
        self.devices.lock().unwrap().push((json!({"device_id": id, "name": format!("aparelho {id}"), "status": status,
            "created_at": 1.5, "approved_at": null, "revoked_at": null}), secret.to_owned()));
    }

    fn set(&self, id: &str, status: &str) {
        let mut devices = self.devices.lock().unwrap();
        let row = devices.iter_mut().find(|(row, _)| row["device_id"] == id).unwrap();
        row.0["status"] = json!(status);
    }

    fn status(&self, id: &str) -> Value {
        self.devices.lock().unwrap().iter().find(|(row, _)| row["device_id"] == id).unwrap().0["status"].clone()
    }
}

fn holds_the_key(headers: &HeaderMap) -> bool {
    headers.get(header::AUTHORIZATION).is_some_and(|value| value.as_bytes() == format!("Bearer {MACHINE_KEY}").as_bytes())
}

async fn list(State(account): State<Arc<Account>>, headers: HeaderMap) -> (StatusCode, String) {
    if !holds_the_key(&headers) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    let rows: Vec<Value> = account.devices.lock().unwrap().iter().map(|(row, _)| row.clone()).collect();
    (StatusCode::OK, json!(rows).to_string())
}

/// Pendente vira aprovado; revogado nunca volta, como no serviço.
fn approve_in(account: &Account, id: &str, secret: Option<&str>) -> StatusCode {
    let mut devices = account.devices.lock().unwrap();
    let Some((row, known)) = devices.iter_mut().find(|(row, _)| row["device_id"] == id) else { return StatusCode::NOT_FOUND };
    if secret.is_some_and(|secret| secret != known) {
        return StatusCode::FORBIDDEN;
    }
    if row["status"] == "revoked" {
        return StatusCode::CONFLICT;
    }
    row["status"] = json!("approved");
    StatusCode::OK
}

async fn approve(State(account): State<Arc<Account>>, UrlPath(id): UrlPath<String>, headers: HeaderMap) -> (StatusCode, String) {
    if !holds_the_key(&headers) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    (approve_in(&account, &id, None), json!({"status": "approved"}).to_string())
}

async fn vouch(State(account): State<Arc<Account>>, UrlPath(id): UrlPath<String>, headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    if !holds_the_key(&headers) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    let body: Value = serde_json::from_slice(&body).unwrap();
    let status = approve_in(&account, &id, body["device_secret"].as_str());
    (status, json!({"error": "sem permissão", "reason": null}).to_string())
}

async fn revoke(State(account): State<Arc<Account>>, UrlPath(id): UrlPath<String>, headers: HeaderMap) -> StatusCode {
    if !holds_the_key(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    let mut devices = account.devices.lock().unwrap();
    let Some((row, _)) = devices.iter_mut().find(|(row, _)| row["device_id"] == id) else { return StatusCode::NOT_FOUND };
    row["status"] = json!("revoked");
    StatusCode::NO_CONTENT
}

async fn spawn_service() -> (Arc<Account>, SocketAddr) {
    let account = Arc::new(Account::default());
    let app = axum::Router::new()
        .route("/v1/devices", get(list))
        .route("/v1/devices/{id}", delete(revoke))
        .route("/v1/devices/{id}/approve", post(approve))
        .route("/v1/devices/{id}/vouch", post(vouch))
        .with_state(account.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (account, addr)
}

fn linked_at(service: SocketAddr) -> Linked {
    Linked { service_url: format!("http://{service}"), account: "acme".into(), machine_id: "pc1".into(), machine_key: MACHINE_KEY.into(),
        host: "pc1.acme.hangar.dev.br".into(), guest_host: "pc1--guest.acme.hangar.dev.br".into(), preview_host: "pc1--preview.acme.hangar.dev.br".into(),
        frps_addr: "frps.hangar.dev.br:443".into(), frps_token: "token-do-frps-de-teste".into(),
        service_public_key: vector()["public_key"].as_str().unwrap().to_owned(), guest_published: false, preview_published: false }
}

struct Machine {
    main: SocketAddr,
    connect: SocketAddr,
    linked: Linked,
    store: Arc<Store>,
    supervisor: Arc<Supervisor>,
    _python: Arc<Fake>,
    dir: tempfile::TempDir,
}

impl Machine {
    fn connect_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("connect")
    }
}

/// `link`: grava a ligação com este serviço; sem ela, a máquina não está ligada.
async fn machine(service: SocketAddr, link: bool) -> Machine {
    let dir = crate::physical_tempdir();
    let connect_dir = dir.path().join("connect");
    let linked = linked_at(service);
    if link {
        state::write(&connect_dir, &linked).unwrap();
    }
    let (python, upstream) = spawn_fake().await;
    let mut app = AppState::new(config(upstream, "127.0.0.1"));
    let store = store_at(connect_dir.clone(), &linked);
    let supervisor = Arc::new(Supervisor::new(connect_dir, dir.path().join("logs"), store.clone(), 1));
    app.connect_devices = store.clone();
    app.connect_supervisor = supervisor.clone();
    let connect = free_port();
    app.connect = Some(connect);
    let main = spawn_state(app).await;
    wait_up(connect).await;
    Machine { main, connect, linked, store, supervisor, _python: python, dir }
}

/// Relógio dos aparelhos desta máquina e validade dos bilhetes.
const NOW: u64 = 1_000_000;

fn store_at(dir: std::path::PathBuf, linked: &Linked) -> Arc<Store> {
    Arc::new(Store::new(dir, Some(linked.identity().unwrap())).with_clock(Arc::new(|| NOW)))
}

/// Bilhete do serviço; `account_device` presente é o pré-aprovado da conta.
fn ticket(nonce: &str, secret: &str, account_device: Option<&str>) -> (String, String) {
    let mut body = json!({"machine_id": "pc1", "account": "acme", "device_name": "celular", "verifier": hash(secret.as_bytes()),
        "nonce": nonce, "expires_at": NOW + 60});
    if let Some(account_device) = account_device {
        body["preapproved"] = json!(true);
        body["account_device"] = json!(account_device);
    }
    let text = body.to_string();
    let signature = sign(&text);
    (text, signature)
}

/// Token de um aparelho liberado na conta como `account_device`, resgatado nesta máquina.
fn released(store: &Store, nonce: &str, account_device: &str) -> String {
    let secret = format!("segredo-{nonce}");
    let (text, signature) = ticket(nonce, &secret, Some(account_device));
    let id = store.redeem_from(&text, &signature, &secret, None).unwrap();
    let Poll::Approved(Some(token)) = store.status(&id, &secret).unwrap() else { panic!("{nonce} não saiu aprovado") };
    token
}

/// Aparelho aprovado à moda antiga, pelo cartão desta máquina: não tem aparelho da conta.
fn approved_here(store: &Store, nonce: &str) -> String {
    let secret = format!("segredo-{nonce}");
    let (text, signature) = ticket(nonce, &secret, None);
    let id = store.redeem_from(&text, &signature, &secret, None).unwrap();
    store.approve(&id, None).unwrap();
    let Poll::Approved(Some(token)) = store.status(&id, &secret).unwrap() else { panic!("{nonce} não saiu aprovado") };
    token
}

/// O porteiro da entrada do Connect com esse token.
async fn gate(m: &Machine, token: &str) -> u16 {
    client().get(format!("http://{}/api/connect/devices", m.connect)).bearer_auth(token).send().await.unwrap().status().as_u16()
}

fn on_disk(m: &Machine, token: &str) -> bool {
    store_at(m.connect_dir(), &m.linked).verify(token.as_bytes()).is_some()
}

#[tokio::test]
async fn a_device_revoked_in_the_account_loses_this_machine_on_the_next_read() {
    let (account, service) = spawn_service().await;
    account.add("D", "approved", "s-d");
    account.add("E", "approved", "s-e");
    let m = machine(service, true).await;
    let (d, e) = (released(&m.store, "n1", "D"), released(&m.store, "n2", "E"));
    assert_eq!(gate(&m, &d).await, 200);

    account.set("D", "revoked");
    m.supervisor.sync_account_devices(&m.linked).await;
    assert_eq!(gate(&m, &d).await, 401, "revogado na conta, recusado no porteiro");
    assert_eq!(gate(&m, &e).await, 200, "outro aparelho da conta fica");
    assert!(!on_disk(&m, &d) && on_disk(&m, &e), "a revogação foi ao disco");

    // Bilhete pedido antes da revogação e resgatado depois da leitura: nasce aprovado, e a leitura seguinte o tira.
    let late = released(&m.store, "n3", "D");
    assert_eq!(gate(&m, &late).await, 200);
    m.supervisor.sync_account_devices(&m.linked).await;
    assert_eq!(gate(&m, &late).await, 401, "cada leitura aplica a lista inteira de revogados");
}

#[tokio::test]
async fn the_first_read_of_the_account_comes_before_the_tunnel() {
    let (account, service) = spawn_service().await;
    account.add("D", "revoked", "s-d");
    let dir = crate::physical_tempdir();
    let linked = linked_at(service);
    let store = store_at(dir.path().to_path_buf(), &linked);
    let d = released(&store, "n1", "D");
    let supervisor = crate::connect_supervisor::supervisor(dir.path(), store.clone(), 8765);
    supervisor.start(&linked).await.unwrap();
    let gone = store.verify(d.as_bytes()).is_none();
    supervisor.stop().await;
    assert!(gone, "a subida já aplicou a revogação");
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[tokio::test]
async fn an_unreachable_service_keeps_the_devices_and_says_so_once() {
    let log = Log::default();
    let writer = log.clone();
    // O teste roda numa thread só: a leitura chamada daqui vê este assinante.
    let _log = tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(move || writer.clone()).finish());
    let closed = free_port();
    let m = machine(closed, true).await;
    let d = released(&m.store, "n1", "D");
    m.supervisor.sync_account_devices(&m.linked).await;
    m.supervisor.sync_account_devices(&m.linked).await;
    assert_eq!(gate(&m, &d).await, 200, "falha do serviço não tira ninguém");
    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    assert_eq!(text.matches(CODE).count(), 1, "{text}");
}

async fn relay(m: &Machine, method: reqwest::Method, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut request = client().request(method, format!("http://{}{path}", m.main));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.header("content-type", "application/json").body(body.to_string());
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn account_device_routes_relay_with_the_machine_key() {
    let (account, service) = spawn_service().await;
    account.add("D", "approved", "s-d");
    account.add("P", "pending", "s-p");
    let m = machine(service, true).await;
    let d = released(&m.store, "n1", "D");

    assert_eq!(relay(&m, reqwest::Method::GET, "/api/connect/account-devices", None, None).await.0, 401);
    let (status, rows) = relay(&m, reqwest::Method::GET, "/api/connect/account-devices", Some(OWNER), None).await;
    assert_eq!(status, 200, "{rows}");
    let seen: Vec<(Value, Value)> = rows.as_array().unwrap().iter().map(|row| (row["device_id"].clone(), row["status"].clone())).collect();
    assert_eq!(seen, vec![(json!("D"), json!("approved")), (json!("P"), json!("pending"))]);
    assert!(rows[0].get("secret").is_none() && rows[0]["name"] == "aparelho D");

    let (status, body) = relay(&m, reqwest::Method::POST, "/api/connect/account-devices/P/approve", Some(OWNER), None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(account.status("P"), "approved");
    let (status, body) = relay(&m, reqwest::Method::POST, "/api/connect/account-devices/X/approve", Some(OWNER), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (404, json!("connect_account_device_unknown")));

    let (status, body) = relay(&m, reqwest::Method::DELETE, "/api/connect/account-devices/D", Some(OWNER), None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(account.status("D"), "revoked");
    assert!(!on_disk(&m, &d), "o aparelho local de D saiu do disco antes da resposta");
    assert_eq!(gate(&m, &d).await, 401);
}

#[tokio::test]
async fn a_device_approved_here_vouches_only_with_its_own_secret() {
    let (account, service) = spawn_service().await;
    account.add("N", "pending", "s-n");
    let m = machine(service, true).await;
    let local = approved_here(&m.store, "n1");
    let path = "/api/connect/account-devices/self";

    let (status, body) = relay(&m, reqwest::Method::POST, path, Some(OWNER), Some(json!({"device_id": "N", "device_secret": "s-n"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (403, json!("connect_account_device_self_only")), "{body}");
    let wrong = client().post(format!("http://{}{path}", m.connect)).bearer_auth(&local).header("content-type", "application/json")
        .body(json!({"device_id": "N", "device_secret": "outro"}).to_string()).send().await.unwrap();
    assert_eq!(wrong.status().as_u16(), 403);
    let body: Value = serde_json::from_str(&wrong.text().await.unwrap()).unwrap();
    assert_eq!(body["detail"]["code"], "connect_account_device_refused", "a recusa do serviço chega a quem pediu");
    assert_eq!(account.status("N"), "pending");

    let right = client().post(format!("http://{}{path}", m.connect)).bearer_auth(&local).header("content-type", "application/json")
        .body(json!({"device_id": "N", "device_secret": "s-n"}).to_string()).send().await.unwrap();
    assert_eq!(right.status().as_u16(), 200);
    let body: Value = serde_json::from_str(&right.text().await.unwrap()).unwrap();
    assert_eq!(body, json!({"account_device": "N", "status": "approved"}));
    assert_eq!(account.status("N"), "approved");

    // Ligado a N: revogar N na conta alcança também a credencial antiga desta máquina.
    account.set("N", "revoked");
    m.supervisor.sync_account_devices(&m.linked).await;
    assert_eq!(gate(&m, &local).await, 401);
}

#[tokio::test]
async fn account_devices_need_a_linked_machine() {
    let (_account, service) = spawn_service().await;
    let m = machine(service, false).await;
    let (status, body) = relay(&m, reqwest::Method::GET, "/api/connect/account-devices", Some(OWNER), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("connect_not_linked")));
}
