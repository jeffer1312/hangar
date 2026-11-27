//! Apagar outra máquina da conta também a tira do `peers.json` desta, e a lista dos aparelhos diz qual
//! credencial foi dada a qual máquina. Serviço falso em porta livre; nada toca o serviço real nem espera relógio.
use crate::fake::{Fake, OWNER, client, config, spawn_fake, spawn_state};

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::delete;
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::connect::devices::Store;
use hangar_server::connect::state::{self, Linked};
use hangar_server::connect::supervisor::Supervisor;
use hangar_server::routes::AppState;
use serde_json::{Value, json};

const MACHINE_KEY: &str = "chave-de-maquina-de-teste";

fn vector() -> Value {
    serde_json::from_str(include_str!("../fixtures/connect-ticket-vector.json")).unwrap()
}

/// As máquinas que a conta apagou, na ordem dos pedidos.
#[derive(Default)]
struct Account {
    removed: Mutex<Vec<String>>,
}

async fn remove(State(account): State<Arc<Account>>, UrlPath(id): UrlPath<String>, headers: HeaderMap) -> StatusCode {
    if !headers.get(header::AUTHORIZATION).is_some_and(|value| value.as_bytes() == format!("Bearer {MACHINE_KEY}").as_bytes()) {
        return StatusCode::UNAUTHORIZED;
    }
    account.removed.lock().unwrap().push(id);
    StatusCode::NO_CONTENT
}

async fn spawn_service() -> (Arc<Account>, SocketAddr) {
    let account = Arc::new(Account::default());
    let app = axum::Router::new().route("/v1/machines/{id}", delete(remove)).with_state(account.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (account, addr)
}

fn linked_at(service: SocketAddr) -> Linked {
    Linked { service_url: format!("http://{service}"), account: "acme".into(), machine_id: "pc-a".into(), machine_key: MACHINE_KEY.into(),
        host: "pc-a.acme.hangar.dev.br".into(), guest_host: "pc-a--guest.acme.hangar.dev.br".into(), preview_host: "pc-a--preview.acme.hangar.dev.br".into(),
        frps_addr: "frps.hangar.dev.br:443".into(), frps_token: "token-do-frps-de-teste".into(),
        service_public_key: vector()["public_key"].as_str().unwrap().to_owned(), guest_published: false, preview_published: false }
}

struct Machine {
    main: SocketAddr,
    store: Arc<Store>,
    account: Arc<Account>,
    _python: Arc<Fake>,
    dir: tempfile::TempDir,
}

impl Machine {
    fn peers_path(&self) -> PathBuf {
        self.dir.path().join(".hangar/config/peers.json")
    }

    fn peers(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.peers_path()).unwrap()).unwrap()
    }

    fn write_peers(&self, peers: &Value) {
        std::fs::write(self.peers_path(), peers.to_string()).unwrap();
    }
}

/// `pc-a`, ligada à conta do serviço falso, com `peers.json` vazio.
async fn machine() -> Machine {
    let (account, service) = spawn_service().await;
    let dir = crate::physical_tempdir();
    let connect_dir = dir.path().join("connect");
    let linked = linked_at(service);
    state::write(&connect_dir, &linked).unwrap();
    let settings = dir.path().join(".hangar/config");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(settings.join("peers.json"), "{}").unwrap();
    std::fs::write(settings.join(".env"), "").unwrap();
    let (python, _) = spawn_fake().await;
    let mut app = AppState::with_server_id(config("127.0.0.1"), "pc-a".to_owned());
    let root = dir.path().to_str().unwrap();
    app.accounts = AccountService::new(AccountEnvironment::from_pairs([("HOME", root), ("USERPROFILE", root)]));
    let store = Arc::new(Store::new(connect_dir.clone(), Some(linked.identity().unwrap())));
    app.connect_devices = store.clone();
    app.connect_supervisor = Arc::new(Supervisor::new(connect_dir, dir.path().join("logs"), store.clone(), 1));
    let main = spawn_state(app).await;
    Machine { main, store, account, _python: python, dir }
}

/// A entrada que a ligação pela conta grava para `pc-b`, com a chave do identificador dela (`notebook`), não do nome na conta.
fn linked_peer(device: &str) -> Value {
    json!({"base_url": "https://pc-b.acme.hangar.dev.br", "token": "token-de-b", "connect": true, "connect_return_device": device})
}

async fn remove_machine(m: &Machine, id: &str) -> (u16, Value) {
    let response = client().delete(format!("http://{}/api/connect/machines/{id}", m.main)).bearer_auth(OWNER).send().await.unwrap();
    (response.status().as_u16(), serde_json::from_str(&response.text().await.unwrap()).unwrap())
}

async fn devices(m: &Machine) -> (u16, Vec<Value>) {
    let response = client().get(format!("http://{}/api/connect/devices", m.main)).bearer_auth(OWNER).send().await.unwrap();
    (response.status().as_u16(), serde_json::from_str(&response.text().await.unwrap()).unwrap())
}

#[tokio::test]
async fn removing_a_machine_from_the_account_takes_it_off_this_peer_list() {
    let m = machine().await;
    let (issued, token) = m.store.enroll("máquina pc-b").unwrap();
    m.write_peers(&json!({"notebook": linked_peer(&issued), "lab": {"base_url": "http://127.0.0.1:9", "token": "senha-do-lab"}}));

    let (status, body) = remove_machine(&m, "pc-b").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({"ok": true, "peers_removed": ["notebook"]}), "achada pelo endereço, não pelo nome na conta");
    assert_eq!(*m.account.removed.lock().unwrap(), vec!["pc-b".to_owned()]);
    let peers = m.peers();
    assert!(peers.get("notebook").is_none(), "{peers}");
    assert!(peers.get("lab").is_some(), "as outras máquinas ficam: {peers}");
    assert_eq!(m.store.verify(token.as_bytes()), None, "a credencial dada a ela foi revogada, como no DELETE /api/peers");

    let (status, body) = remove_machine(&m, "pc-b").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["peers_removed"], json!([]), "sem entrada, nada a tirar");
}

/// A lista da conta mostra como "neste app" pela chave, pelo endereço do Connect ou pelo Connect como caminho extra:
/// remover tira todas, e as de outras máquinas ficam.
#[tokio::test]
async fn every_entry_the_account_list_shows_as_added_goes() {
    let m = machine().await;
    m.write_peers(&json!({
        "manual": {"base_url": "https://pc-b.acme.hangar.dev.br/", "token": "senha-de-b"},
        "pelo-tailscale": {"base_url": "http://100.64.0.2:8765", "token": "senha-de-b", "connect_url": "https://pc-b.acme.hangar.dev.br"},
        "pc-b": {"base_url": "http://192.168.0.9:8765", "token": "senha-de-b"},
        "lab": {"base_url": "http://127.0.0.1:9", "token": "senha-do-lab", "connect_url": "https://pc-c.acme.hangar.dev.br"}}));
    let (status, body) = remove_machine(&m, "pc-b").await;
    assert_eq!(status, 200, "{body}");
    let mut removed: Vec<String> = serde_json::from_value(body["peers_removed"].clone()).unwrap();
    removed.sort();
    assert_eq!(removed, ["manual", "pc-b", "pelo-tailscale"]);
    let peers = m.peers();
    assert!(peers.get("lab").is_some(), "{peers}");
}

#[tokio::test]
async fn a_peer_list_that_cannot_be_changed_says_the_machine_is_still_there() {
    let m = machine().await;
    std::fs::write(m.peers_path(), "{quebrado").unwrap();
    let (status, body) = remove_machine(&m, "pc-b").await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["detail"]["code"], "connect_machine_peer_left");
    assert_eq!(*m.account.removed.lock().unwrap(), vec!["pc-b".to_owned()], "a conta já tinha apagado a máquina");
}

#[tokio::test]
async fn the_device_list_tells_which_credential_went_to_which_machine() {
    let m = machine().await;
    let (issued, _) = m.store.enroll("máquina pc-b").unwrap();
    let (app, _) = m.store.enroll_install("Pixel", Some("install-1")).unwrap();
    m.write_peers(&json!({"notebook": linked_peer(&issued)}));

    let (status, rows) = devices(&m).await;
    assert_eq!(status, 200);
    let row = |id: &str| rows.iter().find(|row| row["device_id"] == id).cloned().unwrap();
    assert_eq!(row(&issued)["peer"], "notebook");
    assert!(row(&app).get("peer").is_none(), "app não ganha o campo: {}", row(&app));

    std::fs::write(m.peers_path(), "{quebrado").unwrap();
    let (status, rows) = devices(&m).await;
    assert_eq!(status, 200, "peers.json ilegível não derruba a lista");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.get("peer").is_none()), "{rows:?}");
}
