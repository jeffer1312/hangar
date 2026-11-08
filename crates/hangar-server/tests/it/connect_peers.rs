//! Máquinas da mesma conta: descoberta pelo serviço, ligação com credencial por direção e o que passa
//! por ela. Serviço falso e duas máquinas de verdade em portas livres; nada toca o serviço real.
use crate::connect_port::{free_port, wait_up};
use crate::fake::{Fake, OWNER, client, config, spawn_fake, spawn_state};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::connect::devices::{Status, Store};
use hangar_server::connect::state::{self, Linked};
use hangar_server::connect::supervisor::Supervisor;
use hangar_server::groups::outbox::{Outbox, OutboxItem, OutboxSend};
use hangar_server::groups::peers::{PeerBook, PeerClient, PeerError};
use hangar_server::groups::service::BoxFuture;
use hangar_server::login_guests::store::hash;
use hangar_server::machine_peers::{self, network};
use hangar_server::routes::AppState;
use serde_json::{Value, json};

const MACHINE_KEY: &str = "chave-de-maquina-de-teste";

fn vector() -> Value {
    serde_json::from_str(include_str!("../fixtures/connect-ticket-vector.json")).unwrap()
}

fn sign(text: &str) -> String {
    let seed = vector()["seed_hex"].as_str().unwrap().to_owned();
    let seed: Vec<u8> = (0..seed.len()).step_by(2).map(|i| u8::from_str_radix(&seed[i..i + 2], 16).unwrap()).collect();
    STANDARD.encode(ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap().sign(text.as_bytes()))
}

fn host_of(id: &str) -> String {
    format!("{id}.acme.hangar.dev.br")
}

/// As máquinas da conta e o endereço local que faz as vezes de `https://<host>`.
#[derive(Default)]
struct Account {
    urls: Mutex<HashMap<String, String>>,
}

fn holds_the_key(headers: &HeaderMap) -> bool {
    headers.get(header::AUTHORIZATION).is_some_and(|value| value.as_bytes() == format!("Bearer {MACHINE_KEY}").as_bytes())
}

async fn machines(State(account): State<Arc<Account>>, headers: HeaderMap) -> (StatusCode, String) {
    if !holds_the_key(&headers) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    let rows: Vec<Value> = account.urls.lock().unwrap().keys()
        .map(|id| json!({"machine_id": id, "name": id, "host": host_of(id), "online": true})).collect();
    (StatusCode::OK, json!(rows).to_string())
}

async fn devices(State(account): State<Arc<Account>>, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    if !holds_the_key(&headers) {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    let Some(url) = account.urls.lock().unwrap().get(&id).cloned() else { return (StatusCode::NOT_FOUND, String::new()) };
    let asked: Value = serde_json::from_slice(&body).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let ticket = json!({"machine_id": id, "account": "acme", "device_name": asked["device_name"], "verifier": asked["verifier"],
        "nonce": hash(asked["verifier"].to_string().as_bytes()), "expires_at": now + 60}).to_string();
    let signature = sign(&ticket);
    (StatusCode::OK, json!({"url": url, "ticket": ticket, "signature": signature}).to_string())
}

async fn spawn_service() -> (Arc<Account>, SocketAddr) {
    let account = Arc::new(Account::default());
    let app = axum::Router::new().route("/v1/machines", get(machines)).route("/v1/machines/{id}/devices", post(devices))
        .with_state(account.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (account, addr)
}

struct Machine {
    id: &'static str,
    main: SocketAddr,
    connect: SocketAddr,
    python: Arc<Fake>,
    store: Arc<Store>,
    dir: tempfile::TempDir,
}

fn linked_as(id: &str, service: SocketAddr) -> Linked {
    Linked { service_url: format!("http://{service}"), account: "acme".into(), machine_id: id.into(), machine_key: MACHINE_KEY.into(),
        host: host_of(id), guest_host: format!("{id}-guest.acme.hangar.dev.br"), preview_host: format!("{id}-preview.acme.hangar.dev.br"),
        frps_addr: "frps.hangar.dev.br:443".into(), frps_token: "token-do-frps-de-teste".into(),
        service_public_key: vector()["public_key"].as_str().unwrap().to_owned(), guest_published: false, preview_published: false }
}

async fn machine(id: &'static str, server_id: &str, service: SocketAddr) -> Machine {
    let dir = crate::physical_tempdir();
    let connect_dir = dir.path().join("connect");
    let linked = linked_as(id, service);
    state::write(&connect_dir, &linked).unwrap();
    let settings = dir.path().join(".hangar/config");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(settings.join("peers.json"), "{}").unwrap();
    std::fs::write(settings.join(".env"), "").unwrap();
    let (python, upstream) = spawn_fake().await;
    let mut app = AppState::with_server_id(config(upstream, "127.0.0.1"), server_id.to_owned());
    let root = dir.path().to_str().unwrap();
    app.accounts = AccountService::new(AccountEnvironment::from_pairs([("HOME", root), ("USERPROFILE", root)]));
    let store = Arc::new(Store::new(connect_dir.clone(), Some(linked.identity().unwrap())));
    app.connect_devices = store.clone();
    app.connect_supervisor = Arc::new(Supervisor::new(connect_dir, dir.path().join("logs"), store.clone(), 1));
    let connect = free_port();
    app.connect = Some(connect);
    let main = spawn_state(app).await;
    wait_up(connect).await;
    Machine { id, main, connect, python, store, dir }
}

struct World {
    service: SocketAddr,
    a: Machine,
    b: Machine,
}

async fn world() -> World {
    world_named("pc-a", "pc-b").await
}

/// As máquinas `pc-a` e `pc-b` da conta, com estes `CP_SERVER_ID`.
async fn world_named(server_a: &str, server_b: &str) -> World {
    let (account, service) = spawn_service().await;
    let (a, b) = (machine("pc-a", server_a, service).await, machine("pc-b", server_b, service).await);
    for m in [&a, &b] {
        account.urls.lock().unwrap().insert(m.id.to_owned(), format!("http://{}", m.connect));
    }
    World { service, a, b }
}

fn peers_path(m: &Machine) -> PathBuf {
    m.dir.path().join(".hangar/config/peers.json")
}

fn peers_of(m: &Machine) -> Value {
    serde_json::from_str(&std::fs::read_to_string(peers_path(m)).unwrap()).unwrap()
}

fn write_peers(m: &Machine, peers: &Value) {
    std::fs::write(peers_path(m), peers.to_string()).unwrap();
}

fn point_at(m: &Machine, peer: &str, url: &str) {
    let mut peers = peers_of(m);
    peers[peer]["base_url"] = json!(url);
    write_peers(m, &peers);
}

fn service_of(m: &Machine) -> machine_peers::Service {
    let mut service = machine_peers::Service::new(m.dir.path(), None, OWNER.into(), m.main.port(), "127.0.0.1".into(), m.id.into()).unwrap();
    service.connect_dir = Some(m.dir.path().join("connect"));
    service.connect_devices = Some(m.store.clone());
    service
}

/// O que a máquina de verdade teria com a ligação de pé.
fn linked_service(m: &Machine) -> machine_peers::Service {
    let mut service = service_of(m);
    service.connect = Some(format!("https://{}", host_of(m.id)));
    service
}

async fn start_link(a: &Machine, id: &str) -> reqwest::Response {
    client().post(format!("http://{}/api/connect/peers/{id}/link", a.main)).bearer_auth(OWNER).send().await.unwrap()
}

async fn link_row(a: &Machine, id: &str) -> Value {
    let response = client().get(format!("http://{}/api/connect/peers", a.main)).bearer_auth(OWNER).send().await.unwrap();
    let rows: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    rows.as_array().unwrap().iter().find(|row| row["machine_id"] == id).cloned().unwrap_or(Value::Null)
}

async fn wait_stage(a: &Machine, id: &str, stage: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let row = link_row(a, id).await;
        if row["stage"] == stage {
            return row;
        }
        assert!(Instant::now() < deadline, "esperava {stage}: {row}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn card(m: &Machine) -> String {
    m.store.list().into_iter().find(|row| row.status == Status::Pending).expect("cartão esperando").device_id
}

async fn approve_card(b: &Machine) {
    let approved = client().post(format!("http://{}/api/connect/devices/{}/approve", b.main, card(b))).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(approved.status(), 200);
}

async fn linked_pair() -> World {
    let w = world().await;
    assert_eq!(start_link(&w.a, w.b.id).await.status(), 202);
    wait_stage(&w.a, w.b.id, "waiting_approval").await;
    approve_card(&w.b).await;
    wait_stage(&w.a, w.b.id, "linked").await;
    w
}

#[tokio::test]
async fn discovery_lists_account_machines_without_tailscale() {
    let w = world().await;
    let mut service = service_of(&w.a);
    let rows = network::discover_with(&service, json!({})).await.unwrap();
    let host = host_of("pc-b");
    assert_eq!(rows, json!([{"nome": "pc-b", "base_url": format!("https://{host}"), "hosts": [host],
        "connect": {"machine_id": "pc-b", "name": "pc-b", "host": host, "online": true}}]), "a própria máquina fica de fora");
    service.connect_dir = None;
    let refused = network::discover_with(&service, json!({})).await.unwrap_err();
    assert_eq!(refused.code, "descoberta_sem_tailscale", "sem Connect e sem Tailscale, a recusa de antes");
}

#[tokio::test]
async fn peer_link_stores_a_device_token_not_the_owner_password() {
    let w = world().await;
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 202, "o pedido volta sem esperar o cartão");
    assert_eq!(wait_stage(&w.a, "pc-b", "waiting_approval").await["peer"], Value::Null);
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 409, "uma ligação por vez");
    assert!(peers_of(&w.a).get("pc-b").is_none(), "sem cartão, nada no peers.json");
    approve_card(&w.b).await;
    assert_eq!(wait_stage(&w.a, "pc-b", "linked").await["peer"], "pc-b");
    let entry = peers_of(&w.a)["pc-b"].clone();
    assert_eq!(entry["base_url"], format!("https://{}", host_of("pc-b")).as_str(), "o host que a conta informa");
    let token = entry["token"].as_str().unwrap();
    assert_ne!(token, OWNER);
    assert!(w.b.store.verify(token.as_bytes()).is_some(), "o token é um aparelho de B");
}

#[tokio::test]
async fn both_peers_hold_device_tokens_by_direction() {
    let w = linked_pair().await;
    let (to_b, to_a) = (peers_of(&w.a)["pc-b"].clone(), peers_of(&w.b)["pc-a"].clone());
    let (token_to_b, token_to_a) = (to_b["token"].as_str().unwrap(), to_a["token"].as_str().unwrap());
    assert!(w.b.store.verify(token_to_b.as_bytes()).is_some() && w.a.store.verify(token_to_b.as_bytes()).is_none());
    assert!(w.a.store.verify(token_to_a.as_bytes()).is_some() && w.b.store.verify(token_to_a.as_bytes()).is_none());
    assert!(token_to_a != OWNER && token_to_b != OWNER);
    assert_eq!(to_a["base_url"], format!("https://{}", host_of("pc-a")).as_str());
    assert_eq!(to_b["connect_return_device"], w.a.store.verify(token_to_a.as_bytes()).unwrap().as_str(), "cada lado guarda o id do que emitiu");
    assert_eq!(to_a["connect_return_device"], w.b.store.verify(token_to_b.as_bytes()).unwrap().as_str());
    assert!(!peers_of(&w.a).to_string().contains(token_to_a), "o que A emitiu não fica em claro em A");
    assert!(!peers_of(&w.b).to_string().contains(token_to_b));
}

async fn record(State(seen): State<Arc<Mutex<Vec<Value>>>>, body: Bytes) -> String {
    seen.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
    "[]".to_owned()
}

async fn identify(State(seen): State<Arc<Mutex<Vec<Value>>>>, headers: HeaderMap) -> String {
    let presented = headers.get(header::AUTHORIZATION).map_or("", |value| value.to_str().unwrap());
    seen.lock().unwrap().push(json!(presented));
    "{}".to_owned()
}

/// Faz as vezes da outra máquina: anota o corpo de `POST /api/peers` e a credencial de `check`.
async fn recorder() -> (Arc<Mutex<Vec<Value>>>, String) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let app = axum::Router::new().route("/api/peers", post(record)).route("/api/peers/identificador", get(identify)).with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (seen, format!("http://{addr}"))
}

#[tokio::test]
async fn announce_never_sends_the_owner_password_through_connect() {
    let w = world().await;
    let (seen, url) = recorder().await;
    let (device, _) = w.a.store.enroll("pc-b").unwrap();
    write_peers(&w.a, &json!({"pc-b": {"base_url": url, "token": "emitido-por-b", "connect": true, "connect_return_device": device}}));
    let service = linked_service(&w.a);
    let rows = network::announce(&service, "").await;
    assert_eq!(rows[0]["estado"], "ok", "{rows}");
    let sent = seen.lock().unwrap()[0].clone();
    assert_eq!(sent["base_url"], format!("https://{}", host_of("pc-a")).as_str());
    assert!(!sent.to_string().contains(OWNER));
    assert_eq!(w.a.store.verify(sent["token"].as_str().unwrap().as_bytes()), Some(device));
    // Sem o que entregar, não manda nada: nunca a senha no lugar.
    write_peers(&w.a, &json!({"pc-b": {"base_url": url, "token": "emitido-por-b", "connect": true}}));
    let rows = network::announce(&service, "").await;
    assert_eq!(rows[0]["motivo"], "sem_credencial_connect", "{rows}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// O app salva um par do Connect com a senha do dono: a entrada nasce sem marca.
#[tokio::test]
async fn owner_entry_on_a_connect_host_never_gets_the_password() {
    let w = world().await;
    let saved = client().post(format!("http://{}/api/peers", w.a.main)).bearer_auth(OWNER).header("content-type", "application/json")
        .body(json!({"id": "pc-b", "base_url": format!("https://{}", host_of("pc-b")), "token": "aparelho-do-app"}).to_string()).send().await.unwrap();
    assert_eq!(saved.status(), 200);
    assert_eq!(peers_of(&w.a)["pc-b"]["connect"], Value::Null);
    for service in [linked_service(&w.a), service_of(&w.a)] {
        let rows = network::announce(&service, "").await;
        assert_eq!(rows[0]["motivo"], "sem_credencial_connect", "ligada ou não, sem tentar a rede: {rows}");
    }
}

#[tokio::test]
async fn owner_save_keeps_the_mark_only_on_the_same_address() {
    let w = linked_pair().await;
    let entry = peers_of(&w.a)["pc-b"].clone();
    let save = |base: &Value| client().post(format!("http://{}/api/peers", w.a.main)).bearer_auth(OWNER).header("content-type", "application/json")
        .body(json!({"id": "pc-b", "base_url": base, "token": "outro"}).to_string());
    assert_eq!(save(&entry["base_url"]).send().await.unwrap().status(), 200);
    assert_eq!(peers_of(&w.a)["pc-b"]["connect_return_device"], entry["connect_return_device"]);
    assert_eq!(peers_of(&w.a)["pc-b"]["connect"], true);
    assert_eq!(save(&json!("http://100.64.0.2:8765")).send().await.unwrap().status(), 200);
    assert_eq!(peers_of(&w.a)["pc-b"].get("connect"), None, "o dono mudou de endereço");
    assert_eq!(peers_of(&w.a)["pc-b"].get("connect_return_device"), None);
}

/// Aparelho de A tenta levar a credencial que A emitiu para B.
#[tokio::test]
async fn a_device_cannot_redirect_the_return_credential() {
    let w = linked_pair().await;
    let (seen, url) = recorder().await;
    let held = peers_of(&w.b)["pc-a"]["token"].as_str().unwrap().to_owned();
    let return_device = peers_of(&w.a)["pc-b"]["connect_return_device"].clone();
    let (device, token) = w.a.store.enroll("aparelho").unwrap();
    let plant = |id: &str| client().post(format!("http://{}/api/peers", w.a.main)).bearer_auth(&token).header("content-type", "application/json")
        .body(json!({"id": id, "base_url": url, "token": "x", "connect": true, "connect_return_device": return_device}).to_string());
    assert_eq!(plant("pc-b").send().await.unwrap().status(), 409, "aparelho não muda o endereço de um par");
    assert_eq!(plant("armadilha").send().await.unwrap().status(), 200);
    assert_eq!(peers_of(&w.a)["armadilha"]["connect_return_device"], device.as_str(), "o id vem de quem gravou, nunca do corpo");
    point_at(&w.a, "pc-b", &format!("http://{}", free_port()));
    network::announce(&linked_service(&w.a), "").await;
    let sent = seen.lock().unwrap()[0]["token"].as_str().unwrap().to_owned();
    assert_eq!(w.a.store.verify(sent.as_bytes()), Some(device), "só o token do próprio aparelho");
    assert!(w.a.store.verify(held.as_bytes()).is_some());
}

#[tokio::test]
async fn redelivery_keeps_the_old_token_until_it_arrives() {
    let w = linked_pair().await;
    let service = linked_service(&w.a);
    let old = peers_of(&w.b)["pc-a"]["token"].as_str().unwrap().to_owned();
    point_at(&w.a, "pc-b", &format!("http://{}", free_port()));
    assert_eq!(network::announce(&service, "").await[0]["estado"], "falhou");
    assert!(w.a.store.verify(old.as_bytes()).is_some(), "B fora do ar não perde o acesso");
    point_at(&w.a, "pc-b", &format!("http://{}", w.b.connect));
    assert_eq!(network::announce(&service, "").await[0]["estado"], "ok");
    let new = peers_of(&w.b)["pc-a"]["token"].as_str().unwrap().to_owned();
    assert_ne!(new, old);
    assert!(w.a.store.verify(new.as_bytes()).is_some());
    assert!(w.a.store.verify(old.as_bytes()).is_none());
}

#[tokio::test]
async fn renaming_keeps_the_credential_and_removing_a_peer_revokes_it() {
    let w = linked_pair().await;
    point_at(&w.a, "pc-b", &format!("http://{}", w.b.connect));
    let to_b = peers_of(&w.a)["pc-b"]["token"].as_str().unwrap().to_owned();
    let mut service = linked_service(&w.a);
    service.initial_id = "novo".into();
    assert_eq!(network::announce(&service, "pc-a").await[0]["estado"], "ok");
    assert!(peers_of(&w.b).get("pc-a").is_none() && peers_of(&w.b).get("novo").is_some());
    assert!(w.b.store.verify(to_b.as_bytes()).is_some(), "apagar o nome velho não corta quem renomeou");
    let to_a = peers_of(&w.b)["novo"]["token"].as_str().unwrap().to_owned();
    assert!(w.a.store.verify(to_a.as_bytes()).is_some());
    let removed = client().delete(format!("http://{}/api/peers/pc-b", w.a.main)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(removed.status(), 200);
    assert!(w.a.store.verify(to_a.as_bytes()).is_none(), "par apagado perde o que recebeu");
}

/// Quem guarda o token grava uma entrada de sombra com o próprio aparelho: o dono apaga e ele cai igual.
#[tokio::test]
async fn a_shadow_entry_does_not_save_a_device_from_removal() {
    let w = linked_pair().await;
    let held = peers_of(&w.b)["pc-a"]["token"].as_str().unwrap().to_owned();
    let shadow = client().post(format!("http://{}/api/peers", w.a.connect)).bearer_auth(&held).header("content-type", "application/json")
        .body(json!({"id": "sombra", "base_url": "https://x.test", "token": "t", "connect": true}).to_string()).send().await.unwrap();
    assert_eq!(shadow.status(), 200);
    assert_eq!(peers_of(&w.a)["sombra"]["connect_return_device"], peers_of(&w.a)["pc-b"]["connect_return_device"]);
    let removed = client().delete(format!("http://{}/api/peers/pc-b", w.a.main)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(removed.status(), 200);
    assert!(w.a.store.verify(held.as_bytes()).is_none());
}

/// Dois anúncios juntos: a confirmação do primeiro não promove o token do segundo.
#[test]
fn settling_an_older_rotation_changes_nothing() {
    let dir = crate::physical_tempdir();
    let store = Store::new(dir.path().to_path_buf(), None);
    let (id, old) = store.enroll("pc-b").unwrap();
    let first = store.rotate(&id).unwrap();
    let second = store.rotate(&id).unwrap();
    store.settle(&id, &first).unwrap();
    assert!(store.verify(old.as_bytes()).is_some());
    // Sem promover: a outra ponta pode ter gravado o primeiro e perdido o segundo.
    assert!(store.verify(first.as_bytes()).is_some());
    store.settle(&id, &second).unwrap();
    assert!(store.verify(old.as_bytes()).is_none());
    assert!(store.verify(second.as_bytes()).is_some());
}

#[tokio::test]
async fn link_refuses_a_key_already_on_another_address_before_the_card() {
    let w = world().await;
    let tailscale = json!({"pc-b": {"base_url": "http://100.64.0.2:8765", "token": "senha-de-b"}});
    write_peers(&w.a, &tailscale);
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 409);
    assert_eq!(peers_of(&w.a), tailscale);
    assert!(w.b.store.list().is_empty(), "nenhum cartão gasto");
}

/// Quem atende em B escolhe o `identificador`: não pode ser o de um par que já existe.
#[tokio::test]
async fn the_other_side_cannot_pick_an_existing_key() {
    let w = world_named("pc-a", "outra").await;
    let known = json!({"outra": {"base_url": "http://100.64.0.3:8765", "token": "senha-da-outra"}});
    write_peers(&w.a, &known);
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 202);
    wait_stage(&w.a, "pc-b", "waiting_approval").await;
    approve_card(&w.b).await;
    assert_eq!(wait_stage(&w.a, "pc-b", "failed").await["code"], "peers_other_address");
    assert_eq!(peers_of(&w.a), known);
    assert!(w.a.store.list().is_empty(), "o que A emitiu sai junto");
}

#[tokio::test]
async fn the_handover_cannot_replace_a_peer_on_the_other_side() {
    let w = world().await;
    let known = json!({"pc-a": {"base_url": "http://100.64.0.1:8765", "token": "senha-de-a"}});
    write_peers(&w.b, &known);
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 202);
    wait_stage(&w.a, "pc-b", "waiting_approval").await;
    approve_card(&w.b).await;
    assert_eq!(wait_stage(&w.a, "pc-b", "failed").await["code"], "connect_peer_handover_failed");
    assert_eq!(peers_of(&w.b), known);
}

#[tokio::test]
async fn names_that_cannot_work_are_refused_before_the_ticket() {
    let w = world_named("pc-b", "pc-b").await;
    let long = "m".repeat(40);
    assert_eq!(start_link(&w.a, &long).await.status(), 400, "cabe no serviço, não no peers.json");
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 400, "o mesmo nome desta máquina");
    assert!(w.b.store.list().is_empty());
}

#[tokio::test]
async fn a_device_only_checks_a_peer_on_its_own_origin() {
    let w = world().await;
    let (seen, url) = recorder().await;
    let (_, device) = w.a.store.enroll("aparelho").unwrap();
    write_peers(&w.a, &json!({"longe": {"base_url": "https://longe.tail.ts.net", "token": "token-de-longe"}, "perto": {"base_url": url, "token": "token-de-perto"},
        "cifrado": {"base_url": url.replace("http://", "https://"), "token": "token-em-https"}, "porta": {"base_url": "http://127.0.0.1:1", "token": "token-de-outra-porta"}}));
    let check = |id: &str, token: &str| client().get(format!("http://{}/api/peers/check?url={url}&id={id}", w.a.main)).bearer_auth(token.to_owned());
    for id in ["longe", "cifrado", "porta", "perto"] {
        assert_eq!(check(id, &device).send().await.unwrap().status(), 200, "{id}");
    }
    assert_eq!(check("longe", OWNER).send().await.unwrap().status(), 200);
    assert_eq!(seen.lock().unwrap().clone(), vec![json!(""), json!(""), json!(""), json!("Bearer token-de-perto"), json!("Bearer token-de-longe")],
        "outro host, esquema ou porta: aparelho não leva o token");
}

/// Aparelho grava um "par" e pede o anúncio: a senha do dono iria ao endereço que ele escolheu.
#[tokio::test]
async fn peer_written_by_a_device_never_receives_the_owner_password() {
    let w = world().await;
    let (seen, url) = recorder().await;
    let (_, device) = w.a.store.enroll("aparelho").unwrap();
    let planted = client().post(format!("http://{}/api/peers", w.a.main)).bearer_auth(&device).header("content-type", "application/json")
        .body(json!({"id": "armadilha", "base_url": url, "token": "qualquer"}).to_string()).send().await.unwrap();
    assert_eq!(planted.status(), 200);
    assert_eq!(peers_of(&w.a)["armadilha"]["connect"], true);
    assert_eq!(peers_of(&w.a)["armadilha"].get("connect_return_device"), None, "só registrar não põe o token do aparelho na troca");
    let announced = client().post(format!("http://{}/api/peers/anunciar", w.a.main)).bearer_auth(&device).send().await.unwrap();
    assert_eq!(announced.status(), 200);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn hangar_send_reaches_a_peer_through_connect() {
    let w = linked_pair().await;
    // `https://pc-…` só existe com o túnel: aqui a entrada de cada máquina é uma porta local.
    point_at(&w.a, "pc-b", &format!("http://{}", w.b.connect));
    point_at(&w.b, "pc-a", &format!("http://{}", w.a.connect));
    for (from, to, peer) in [(&w.a, &w.b, "pc-b"), (&w.b, &w.a, "pc-a")] {
        to.python.set_input_reply(Some((StatusCode::OK, json!({"ok": true}))));
        let body = json!({"text": format!("[de: {}::s] oi", from.id), "steer": true});
        let peers = PeerClient::new(PeerBook::new(Some(peers_path(from))));
        peers.call(peer, reqwest::Method::POST, "/api/sessions/recado/input", Some(&body)).await.unwrap();
        assert_eq!(to.python.hits_to("/api/sessions/recado/input"), 1, "{peer}");
        assert_eq!(serde_json::from_slice::<Value>(&to.python.last_body()).unwrap(), body);
        assert_eq!(to.python.last_hit().1["authorization"], format!("Bearer {OWNER}").as_str());
    }
}

struct Through(PeerClient);

impl OutboxSend for Through {
    fn send<'a>(&'a self, item: &'a OutboxItem) -> BoxFuture<'a, Result<(), PeerError>> {
        Box::pin(async move {
            let body = json!({"text": "da fila"});
            self.0.call(item.destination(), reqwest::Method::POST, "/api/sessions/fila/input", Some(&body)).await.map(drop)
        })
    }

    fn failed(&self, _item: &OutboxItem, _code: &str, _error: &PeerError, _retry: bool) {}
}

#[tokio::test]
async fn group_outbox_retries_when_peer_is_offline() {
    let w = linked_pair().await;
    w.b.python.set_input_reply(Some((StatusCode::OK, json!({"ok": true}))));
    let reachable = format!("http://{}", w.b.connect);
    point_at(&w.a, "pc-b", &format!("http://{}", free_port()));
    let groups = w.a.dir.path().join("groups");
    std::fs::create_dir_all(&groups).unwrap();
    let peers = PeerClient::new(PeerBook::new(Some(peers_path(&w.a))));
    let outbox = Outbox::open_with_ladder(groups.clone(), Arc::new(Through(peers)), vec![Duration::from_millis(20)]).unwrap();
    let item = OutboxItem::Leave { owner: "pc-b".into(), gid: "g1".into(), member: "m".into(), origin: "pc-a".into() };
    outbox.enqueue(vec![item.clone()]).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(outbox.pending(), vec![item], "máquina fora: o item fica");
    assert!(std::fs::read_to_string(groups.join("outbox.json")).unwrap().contains("g1"), "e fica no disco");
    assert_eq!(w.b.python.hits_to("/api/sessions/fila/input"), 0);
    point_at(&w.a, "pc-b", &reachable);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !outbox.pending().is_empty() {
        assert!(Instant::now() < deadline, "a fila não andou com a máquina de volta");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(w.b.python.hits_to("/api/sessions/fila/input"), 1);
}

#[tokio::test]
async fn tailscale_discovery_is_unchanged() {
    let w = world().await;
    let status = json!({"Peer": {
        "n1": {"Online": true, "HostName": "pc-b", "DNSName": "", "TailscaleIPs": ["127.0.0.1"]},
        "n2": {"Online": false, "HostName": "desligada", "DNSName": "", "TailscaleIPs": ["127.0.0.1"]},
    }});
    let mut service = service_of(&w.a);
    service.port = w.b.main.port();
    service.connect_dir = None;
    let base = format!("http://127.0.0.1:{}", w.b.main.port());
    let before = network::discover_with(&service, status.clone()).await.unwrap();
    assert_eq!(before, json!([{"nome": "pc-b", "base_url": base, "hosts": ["127.0.0.1"]}]));
    // Vista pelas duas fontes: uma linha só, com o endereço do Tailscale.
    service.connect_dir = Some(w.a.dir.path().join("connect"));
    let both = network::discover_with(&service, status).await.unwrap();
    assert_eq!(both.as_array().unwrap().len(), 1, "{both}");
    assert_eq!(both[0]["base_url"], base.as_str());
    assert_eq!(both[0]["connect"]["machine_id"], "pc-b");
}

#[tokio::test]
async fn account_failure_is_never_a_quiet_empty_discovery() {
    let w = world().await;
    let mut service = service_of(&w.a);
    service.port = w.b.main.port();
    let connect_dir = w.a.dir.path().join("connect");
    state::write(&connect_dir, &linked_as("pc-a", free_port())).unwrap();

    let one = json!({"Peer": {"n1": {"Online": true, "HostName": "pc-b", "DNSName": "", "TailscaleIPs": ["127.0.0.1"]}}});
    let rows = network::discover_with(&service, one).await.unwrap();
    assert_eq!(rows[0]["nome"], "pc-b", "o Tailscale segue valendo");
    assert_eq!(rows[0]["connect_error"]["code"], "connect_peer_unreachable", "{rows}");

    let none = json!({"Peer": {"n1": {"Online": false}}});
    let refused = network::discover_with(&service, none.clone()).await.unwrap_err();
    assert_eq!(refused.code, "connect_peer_unreachable", "sem linha nenhuma, a lista vazia mentiria");

    std::fs::write(connect_dir.join("connect.json"), "{quebrado").unwrap();
    let refused = network::discover_with(&service, none).await.unwrap_err();
    assert_eq!(refused.code, "connect_state_unreadable");
}

#[tokio::test]
async fn denied_card_leaves_nothing_behind() {
    let w = world().await;
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 202);
    wait_stage(&w.a, "pc-b", "waiting_approval").await;
    let denied = client().delete(format!("http://{}/api/connect/devices/{}", w.b.main, card(&w.b))).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(denied.status(), 200);
    assert_eq!(wait_stage(&w.a, "pc-b", "failed").await["code"], "connect_peer_denied");
    assert_eq!(peers_of(&w.a), json!({}));
    assert!(w.a.store.list().is_empty(), "A não emite nada sem o cartão de B");
}

#[tokio::test]
async fn a_link_that_fails_after_the_card_revokes_the_token_on_the_other_side() {
    let w = world_named("pc", "pc").await;
    assert_eq!(start_link(&w.a, "pc-b").await.status(), 202);
    wait_stage(&w.a, "pc-b", "waiting_approval").await;
    approve_card(&w.b).await;
    assert_eq!(wait_stage(&w.a, "pc-b", "failed").await["code"], "connect_peer_same_identity");
    assert!(w.b.store.list().iter().all(|device| device.status != Status::Approved), "B não fica com credencial que ninguém guarda");
}

/// Disco roubado: a chave de máquina consegue o bilhete, mas o bilhete só abre um cartão.
#[tokio::test]
async fn a_machine_key_alone_never_gets_past_the_card() {
    let w = world().await;
    let secret = "segredo-de-quem-roubou";
    let issued = client().post(format!("http://{}/v1/machines/pc-b/devices", w.service)).bearer_auth(MACHINE_KEY)
        .body(json!({"device_name": "ladrao", "verifier": hash(secret.as_bytes())}).to_string()).send().await.unwrap();
    let issued: Value = serde_json::from_str(&issued.text().await.unwrap()).unwrap();
    let body = json!({"ticket": issued["ticket"], "signature": issued["signature"], "secret": secret}).to_string();
    let post_b = |path: &str| client().post(format!("http://{}{path}", w.b.connect)).header("content-type", "application/json");
    let redeemed = post_b("/api/connect/redeem").body(body).send().await.unwrap();
    assert_eq!(redeemed.status(), 202);
    let redeemed: Value = serde_json::from_str(&redeemed.text().await.unwrap()).unwrap();
    let polled = post_b("/api/connect/redeem/status").body(json!({"device_id": redeemed["device_id"], "secret": secret}).to_string()).send().await.unwrap();
    let polled: Value = serde_json::from_str(&polled.text().await.unwrap()).unwrap();
    assert_eq!(polled, json!({"status": "pending"}));
    let approve = format!("/api/connect/devices/{}/approve", card(&w.b));
    for path in ["/api/peers", approve.as_str()] {
        let refused = post_b(path).bearer_auth(MACHINE_KEY).body("{}").send().await.unwrap();
        assert_eq!(refused.status(), 401, "{path}");
    }
    assert_eq!(w.b.store.list()[0].status, Status::Pending);
}

/// A máquina emite uma credencial sem cartão dela: só quem tem a senha, e de fora do Connect.
#[tokio::test]
async fn only_the_owner_outside_connect_starts_a_link() {
    let w = world().await;
    let (_, device) = w.a.store.enroll("aparelho").unwrap();
    let link = |addr: SocketAddr| client().post(format!("http://{addr}/api/connect/peers/pc-b/link"));
    assert_eq!(link(w.a.main).send().await.unwrap().status(), 401);
    assert_eq!(link(w.a.main).bearer_auth(&device).send().await.unwrap().status(), 403);
    assert_eq!(link(w.a.connect).bearer_auth(&device).send().await.unwrap().status(), 401);
    assert_eq!(link(w.a.connect).bearer_auth(OWNER).send().await.unwrap().status(), 401);
    assert_eq!(link_row(&w.a, "pc-b").await, Value::Null);
    assert_eq!(start_link(&w.a, "pc-a").await.status(), 400, "consigo mesma não");
}
