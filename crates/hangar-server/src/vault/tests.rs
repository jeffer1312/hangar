use super::*;
use crate::{auth::TrustedHosts, config::Config, routes::{AppState, router}};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Mutex};

const PASSWORD: &str = "senha-do-cofre-Zq81";
const OWNER: &str = "dono";

/// Chaveiro falso: um item opcional, trancado ou não.
#[derive(Default)]
struct FakeKeys { key: Mutex<Option<[u8; 32]>>, locked: Mutex<bool>, writes: Mutex<u32>, wrong_size: Mutex<bool> }

impl KeyStore for FakeKeys {
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> {
        let found = if *self.locked.lock().unwrap() { Err(KeyError::Locked) }
            else if *self.wrong_size.lock().unwrap() { Err(KeyError::Lost) }
            else { Ok(self.key.lock().unwrap().map(Zeroizing::new)) };
        Box::pin(async move { found })
    }
    fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>> {
        *self.wrong_size.lock().unwrap() = false;
        *self.key.lock().unwrap() = Some(*key);
        *self.writes.lock().unwrap() += 1;
        Box::pin(async { Ok(()) })
    }
}

fn entry(name: &str) -> Entry {
    Entry { id: "e1".into(), name: name.into(), username: Some("ana".into()), password: Zeroizing::new(PASSWORD.into()),
        url: None, notes: None, policy: Policy::Ask, created_at: 1.0, updated_at: 1.0 }
}

fn contains(haystack: &[u8], needle: &str) -> bool { haystack.windows(needle.len()).any(|w| w == needle.as_bytes()) }

#[test]
fn the_sealed_file_round_trips_and_does_not_carry_the_password_in_clear() {
    let key = [7u8; 32];
    let sealed = store::seal(&key, &store::Doc { entries: vec![entry("banco")] }).unwrap();
    assert_eq!(sealed[0], store::VERSION);
    assert!(!contains(&sealed, PASSWORD));
    let doc = store::unseal(&key, &sealed).unwrap();
    assert_eq!((doc.entries[0].name.as_str(), doc.entries[0].password.as_str()), ("banco", PASSWORD));
    // Nonce novo a cada gravação: o mesmo cofre não sai igual duas vezes.
    assert_ne!(sealed, store::seal(&key, &doc).unwrap());
}

#[test]
fn a_tampered_or_foreign_file_is_an_error_never_an_empty_vault() {
    let key = [7u8; 32];
    let sealed = store::seal(&key, &store::Doc::default()).unwrap();
    let mut flipped = sealed.clone();
    *flipped.last_mut().unwrap() ^= 1;
    assert_eq!(store::unseal(&key, &flipped).err(), Some(store::StoreError::Unreadable));
    assert_eq!(store::unseal(&[8u8; 32], &sealed).err(), Some(store::StoreError::Unreadable), "outra chave");
    let mut version = sealed.clone();
    version[0] = 2;
    assert_eq!(store::unseal(&key, &version).err(), Some(store::StoreError::Unreadable));
    assert_eq!(store::unseal(&key, &sealed[..10]).err(), Some(store::StoreError::Unreadable));
}

#[tokio::test]
async fn a_tampered_vault_refuses_reads_and_writes_and_stays_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::new(Arc::new(FakeKeys::default()));
    vault.create(dir.path()).await.unwrap();
    let path = store::path(dir.path());
    let mut bytes = std::fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(vault.list(dir.path()).await.err(), Some(VaultError::Unreadable));
    let new = NewEntry { name: "x".into(), username: None, password: Zeroizing::new(PASSWORD.into()), url: None, notes: None, policy: Policy::Ask };
    assert_eq!(vault.add(dir.path(), new).await.err(), Some(VaultError::Unreadable));
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "a gravação recusada não tocou no arquivo");
}

#[tokio::test]
async fn a_vault_file_without_the_key_item_is_key_lost_and_no_key_is_created() {
    let dir = tempfile::tempdir().unwrap();
    Vault::new(Arc::new(FakeKeys::default())).create(dir.path()).await.unwrap();
    let empty = Arc::new(FakeKeys::default());
    let vault = Vault::new(empty.clone());
    assert_eq!(vault.status(dir.path()).await, Status { role: Role::Owner, state: State::KeyLost });
    assert_eq!(vault.list(dir.path()).await.err(), Some(VaultError::Key(KeyError::Lost)));
    vault.create(dir.path()).await.unwrap();
    assert_eq!(*empty.writes.lock().unwrap(), 0, "chave nova por cima de um cofre existente");
    assert_eq!(vault.status(dir.path()).await.state, State::KeyLost);
}

/// Item que sobrou sem 32 bytes e sem `vault.bin`: o cofre nasce com chave nova no lugar dele.
#[tokio::test]
async fn a_leftover_wrong_size_item_without_a_vault_is_replaced_on_create() {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(FakeKeys::default());
    *keys.wrong_size.lock().unwrap() = true;
    let vault = Vault::new(keys.clone());
    vault.create(dir.path()).await.unwrap();
    assert_eq!(*keys.writes.lock().unwrap(), 1);
    assert_eq!(vault.status(dir.path()).await, Status { role: Role::Owner, state: State::Open });
    assert_eq!(Vault::new(keys).status(dir.path()).await.state, State::Open, "outra vida do servidor lê a chave nova");
}

#[tokio::test]
async fn locked_keyring_then_opened_and_the_key_stays_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(FakeKeys::default());
    Vault::new(keys.clone()).create(dir.path()).await.unwrap();
    *keys.locked.lock().unwrap() = true;
    let vault = Vault::new(keys.clone());
    assert_eq!(vault.status(dir.path()).await, Status { role: Role::Owner, state: State::Locked });
    assert_eq!(vault.list(dir.path()).await.err(), Some(VaultError::Key(KeyError::Locked)));
    *keys.locked.lock().unwrap() = false;
    assert_eq!(vault.status(dir.path()).await.state, State::Open);
    // Trancar depois de aberto não fecha o cofre.
    *keys.locked.lock().unwrap() = true;
    assert!(vault.list(dir.path()).await.unwrap().is_empty());
}

#[tokio::test]
async fn without_a_vault_file_the_status_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::new(Arc::new(FakeKeys::default()));
    assert_eq!(vault.status(dir.path()).await, Status { role: Role::None, state: State::Absent });
    assert_eq!(vault.list(dir.path()).await.err(), Some(VaultError::Absent));
}

struct Server { addr: SocketAddr, dir: tempfile::TempDir }

async fn serve(keys: Arc<FakeKeys>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config { listen: addr, upstream: "127.0.0.1:9".parse().unwrap(), auth_token: OWNER.into(),
        internal_secret: "privado".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.hangar_dir = dir.path().to_owned();
    state.vault = Arc::new(Vault::new(keys));
    let app = router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, dir }
}

/// `(status, corpo cru)`; o corpo vai cru para provar o que volta de um JSON quebrado.
async fn call(addr: SocketAddr, method: reqwest::Method, path: &str, token: Option<&str>, headers: &[(&str, &str)], body: Option<&str>)
    -> (u16, String, reqwest::header::HeaderMap) {
    let mut req = reqwest::Client::new().request(method, format!("http://{addr}{path}"));
    if let Some(token) = token { req = req.bearer_auth(token); }
    for (k, v) in headers { req = req.header(*k, *v); }
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_owned()); }
    let resp = req.send().await.unwrap();
    let (status, headers) = (resp.status().as_u16(), resp.headers().clone());
    (status, resp.text().await.unwrap(), headers)
}

async fn owner(addr: SocketAddr, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let text = body.map(|b| b.to_string());
    let (status, raw, _) = call(addr, method, path, Some(OWNER), &[], text.as_deref()).await;
    (status, serde_json::from_str(&raw).unwrap_or(Value::Null))
}

fn code(body: &Value) -> &str { body["detail"]["code"].as_str().unwrap_or_default() }

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_creates_lists_without_the_password_reveals_edits_and_deletes() {
    use reqwest::Method;
    let srv = serve(Arc::new(FakeKeys::default())).await;
    assert_eq!(owner(srv.addr, Method::GET, "/api/vault/status", None).await, (200, json!({"role": "none", "state": "absent"})));
    let (status, body) = owner(srv.addr, Method::GET, "/api/vault/entries", None).await;
    assert_eq!((status, code(&body)), (409, "erro_cofre_ausente"));
    assert_eq!(owner(srv.addr, Method::PUT, "/api/vault/config", Some(json!({"owner": "self"}))).await,
        (200, json!({"role": "owner", "state": "open"})));
    // De novo: o cofre de pé fica como está.
    assert_eq!(owner(srv.addr, Method::PUT, "/api/vault/config", Some(json!({"owner": "self"}))).await.0, 200);

    let (status, body) = owner(srv.addr, Method::POST, "/api/vault/entries",
        Some(json!({"name": "Banco", "username": "ana", "password": PASSWORD, "url": "https://banco.example"}))).await;
    assert_eq!(status, 200);
    let id = body["id"].as_str().unwrap().to_owned();

    let (status, raw, _) = call(srv.addr, Method::GET, "/api/vault/entries", Some(OWNER), &[], None).await;
    assert_eq!(status, 200);
    assert!(!raw.contains(PASSWORD) && !raw.contains("password"), "a lista levou a senha: {raw}");
    let list: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!((list[0]["name"].clone(), list[0]["username"].clone(), list[0]["policy"].clone()), (json!("Banco"), json!("ana"), json!("ask")));

    let (status, raw, headers) = call(srv.addr, Method::GET, &format!("/api/vault/entries/{id}/password"), Some(OWNER), &[], None).await;
    assert_eq!((status, serde_json::from_str::<Value>(&raw).unwrap()), (200, json!({"password": PASSWORD})));
    assert_eq!(headers["cache-control"], "no-store");
    assert!(!contains(&std::fs::read(store::path(srv.dir.path())).unwrap(), PASSWORD), "senha em claro no vault.bin");

    let (status, _) = owner(srv.addr, Method::PATCH, &format!("/api/vault/entries/{id}"),
        Some(json!({"password": "outra-senha", "username": null, "policy": "free"}))).await;
    assert_eq!(status, 200);
    let list = owner(srv.addr, Method::GET, "/api/vault/entries", None).await.1;
    assert_eq!((list[0].get("username"), list[0]["policy"].clone()), (None, json!("free")));
    assert_eq!(owner(srv.addr, Method::GET, &format!("/api/vault/entries/{id}/password"), None).await.1, json!({"password": "outra-senha"}));

    let (status, raw, _) = call(srv.addr, Method::DELETE, &format!("/api/vault/entries/{id}"), Some(OWNER), &[], None).await;
    assert_eq!((status, raw.as_str()), (204, ""));
    let (status, body) = owner(srv.addr, Method::DELETE, &format!("/api/vault/entries/{id}"), None).await;
    assert_eq!((status, code(&body)), (404, "erro_cofre_entrada_desconhecida"));
    let (status, body) = owner(srv.addr, Method::GET, &format!("/api/vault/entries/{id}/password"), None).await;
    assert_eq!((status, code(&body)), (404, "erro_cofre_entrada_desconhecida"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_name_is_409_also_on_rename() {
    use reqwest::Method;
    let srv = serve(Arc::new(FakeKeys::default())).await;
    owner(srv.addr, Method::PUT, "/api/vault/config", Some(json!({"owner": "self"}))).await;
    assert_eq!(owner(srv.addr, Method::POST, "/api/vault/entries", Some(json!({"name": "Banco", "password": PASSWORD}))).await.0, 200);
    let (status, body) = owner(srv.addr, Method::POST, "/api/vault/entries", Some(json!({"name": " banco ", "password": "x"}))).await;
    assert_eq!((status, code(&body)), (409, "erro_cofre_nome_repetido"));
    let id = owner(srv.addr, Method::POST, "/api/vault/entries", Some(json!({"name": "Email", "password": "x"}))).await.1["id"].as_str().unwrap().to_owned();
    let (status, body) = owner(srv.addr, Method::PATCH, &format!("/api/vault/entries/{id}"), Some(json!({"name": "BANCO"}))).await;
    assert_eq!((status, code(&body)), (409, "erro_cofre_nome_repetido"));
    assert_eq!(owner(srv.addr, Method::GET, "/api/vault/entries", None).await.1.as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_body_carrying_the_password_is_refused_without_echo() {
    use reqwest::Method;
    let srv = serve(Arc::new(FakeKeys::default())).await;
    owner(srv.addr, Method::PUT, "/api/vault/config", Some(json!({"owner": "self"}))).await;
    let id = owner(srv.addr, Method::POST, "/api/vault/entries", Some(json!({"name": "Banco", "password": "x"}))).await.1["id"].as_str().unwrap().to_owned();
    let bodies = [
        format!(r#"{{"name": "a", "password": "{PASSWORD}""#),
        format!(r#"{{"name": "a", "password": "{PASSWORD}", "senha": "{PASSWORD}"}}"#),
        format!(r#"{{"name": 5, "password": "{PASSWORD}"}}"#),
        format!(r#"{{"name": "a", "password": "{PASSWORD}", "policy": "{PASSWORD}"}}"#),
        format!(r#"{{"name": "", "password": "{PASSWORD}"}}"#),
    ];
    for body in &bodies {
        let (status, raw, _) = call(srv.addr, Method::POST, "/api/vault/entries", Some(OWNER), &[], Some(body)).await;
        assert!(!raw.contains(PASSWORD), "a recusa ecoou a senha: {raw}");
        assert_eq!((status, code(&serde_json::from_str(&raw).unwrap())), (422, "erro_corpo_invalido"), "{body}");
        let (status, raw, _) = call(srv.addr, Method::PATCH, &format!("/api/vault/entries/{id}"), Some(OWNER), &[], Some(body)).await;
        assert!(!raw.contains(PASSWORD), "a recusa ecoou a senha: {raw}");
        assert_eq!(status, 422, "{body}");
    }
    assert_eq!(owner(srv.addr, Method::GET, &format!("/api/vault/entries/{id}/password"), None).await.1, json!({"password": "x"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_keyring_answers_its_code() {
    use reqwest::Method;
    let keys = Arc::new(FakeKeys::default());
    let srv = serve(keys.clone()).await;
    // Cofre criado por outra vida do servidor; esta encontra o chaveiro trancado.
    Vault::new(keys.clone()).create(srv.dir.path()).await.unwrap();
    *keys.locked.lock().unwrap() = true;
    assert_eq!(owner(srv.addr, Method::GET, "/api/vault/status", None).await, (200, json!({"role": "owner", "state": "locked"})));
    let (status, body) = owner(srv.addr, Method::GET, "/api/vault/entries", None).await;
    assert_eq!((status, code(&body)), (409, "erro_cofre_trancado"));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_and_never_a_guest() {
    use reqwest::Method;
    let srv = serve(Arc::new(FakeKeys::default())).await;
    owner(srv.addr, Method::PUT, "/api/vault/config", Some(json!({"owner": "self"}))).await;
    let id = owner(srv.addr, Method::POST, "/api/vault/entries", Some(json!({"name": "Banco", "password": PASSWORD}))).await.1["id"].as_str().unwrap().to_owned();
    let reveal = format!("/api/vault/entries/{id}/password");
    let (status, raw, _) = call(srv.addr, Method::GET, &reveal, Some(OWNER), &[("x-hangar-guest", "assinado")], None).await;
    assert_eq!((status, raw.contains(PASSWORD)), (403, false));
    let (status, _, _) = call(srv.addr, Method::GET, "/api/vault/entries", Some(OWNER), &[("x-hangar-guest", "assinado")], None).await;
    assert_eq!(status, 403);
    let (status, raw, _) = call(srv.addr, Method::GET, &reveal, None, &[], None).await;
    assert_eq!((status, raw.contains(PASSWORD)), (401, false));
    assert_eq!(call(srv.addr, Method::GET, &reveal, Some("outro"), &[], None).await.0, 401);
}

#[test]
fn the_migration_table_knows_every_vault_route() {
    use axum::http::Method;
    assert!(http::matches(&Method::GET, "/api/vault/status") && http::matches(&Method::PUT, "/api/vault/config"));
    assert!(http::matches(&Method::POST, "/api/vault/entries") && http::matches(&Method::DELETE, "/api/vault/entries/a"));
    assert!(http::matches(&Method::GET, "/api/vault/entries/a/password") && !http::matches(&Method::GET, "/api/vault/entries//password"));
    assert!(!http::matches(&Method::POST, "/api/vault/status") && !http::matches(&Method::GET, "/api/vault/outra"));
}
