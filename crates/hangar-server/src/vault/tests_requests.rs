//! Pedidos do cofre na dona: uma resposta só, cartão trocado, prazos, chaveiro trancado no cadastro,
//! senha liberada, entrada temporária e liberação do primeiro uso; e o caminho pelas rotas.
use super::*;
use crate::{auth::TrustedHosts, config::Config, routes::{AppState, router}};
use futures_util::future::BoxFuture;
use remote::WaitStatus;
use requests::{Answer, Card, Decision, IDLE, Kind, RequestError, Requests, Start};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Mutex, time::Duration};
use wire::{AskReq, UseKind, UseReq};

const VALUE: &str = "senha-pedida-Hx42";
const OWNER: &str = "dono";

#[derive(Default)]
struct Keys { key: Mutex<Option<[u8; 32]>>, locked: Mutex<bool> }

impl KeyStore for Keys {
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> {
        let found = if *self.locked.lock().unwrap() { Err(KeyError::Locked) } else { Ok(self.key.lock().unwrap().map(Zeroizing::new)) };
        Box::pin(async move { found })
    }
    fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>> {
        *self.key.lock().unwrap() = Some(*key);
        Box::pin(async { Ok(()) })
    }
}

async fn vault_in(dir: &std::path::Path) -> (Arc<Keys>, Vault) {
    let keys = Arc::new(Keys::default());
    let vault = Vault::new(keys.clone());
    vault.create(dir).await.unwrap();
    (keys, vault)
}

fn entry(name: &str, policy: Policy) -> NewEntry {
    NewEntry { name: name.into(), username: None, password: Zeroizing::new(VALUE.into()), url: None, notes: None, policy }
}

fn ask(key: &str, label: &str) -> AskReq {
    AskReq { origin_server: "delphi".into(), session_key: key.into(), session: "s1".into(), label: label.into(), reason: "login".into(), placeholder: None }
}

fn use_req(key: &str, entry: &str) -> UseReq {
    UseReq { origin_server: "delphi".into(), session_key: key.into(), session: "s1".into(), entry: entry.into(), kind: UseKind::Exec,
        reason: "deploy".into(), target: "wrangler whoami".into() }
}

fn decide(card: &Card, decision: Decision) -> Answer {
    Answer { interaction_id: card.interaction_id.clone(), decision, password: None, keep: None, name: None, policy: None }
}

fn save(card: &Card, keep: bool) -> Answer {
    Answer { password: Some(Zeroizing::new(VALUE.into())), keep: Some(keep), ..decide(card, Decision::Save) }
}

fn opened(start: Start) -> Card {
    match start { Start::Opened(card) => card, Start::Value(_) => panic!("devia abrir pedido, saiu o valor") }
}

fn value(start: Start) -> String {
    match start { Start::Value(v) => v.as_str().to_owned(), Start::Opened(_) => panic!("devia sair o valor, abriu pedido") }
}

#[tokio::test]
async fn only_the_first_of_two_answers_counts() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    vault.add(dir.path(), entry("github", Policy::Ask)).await.unwrap();
    let reqs = Arc::new(Requests::default());
    let card = opened(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap());
    assert_eq!(card.kind, Kind::Approve);
    let (a, b) = tokio::join!(reqs.answer(&vault, dir.path(), &card.id, decide(&card, Decision::Allow)),
        reqs.answer(&vault, dir.path(), &card.id, decide(&card, Decision::Deny)));
    assert!(a.is_ok() != b.is_ok(), "uma vale, a outra não");
    assert_eq!(a.err().or(b.err()), Some(RequestError::Answered));
    let again = reqs.answer(&vault, dir.path(), &card.id, decide(&card, Decision::Deny)).await;
    assert_eq!(again.err(), Some(RequestError::Answered), "depois de respondido também");
    assert!(reqs.feed().borrow().requests.is_empty(), "o cartão saiu do evento");
}

#[tokio::test]
async fn a_stale_card_is_refused_and_the_request_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    let mut old = decide(&card, Decision::Decline);
    old.interaction_id = "cartao-velho".into();
    assert_eq!(reqs.answer(&vault, dir.path(), &card.id, old).await.err(), Some(RequestError::Stale));
    assert_eq!(reqs.feed().borrow().requests.len(), 1);
    reqs.answer(&vault, dir.path(), &card.id, decide(&card, Decision::Decline)).await.unwrap();
    assert!(matches!(reqs.wait(&card.id, Duration::ZERO).await, Ok(WaitStatus::Declined)));
}

#[tokio::test]
async fn ten_minutes_without_an_answer_close_as_timed_out_while_the_origin_waits() {
    let dir = tempfile::tempdir().unwrap();
    let _vault = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    let mut timed_out = false;
    for _ in 0..30 {
        match reqs.wait(&card.id, wire::WAIT_HOLD).await {
            Ok(WaitStatus::Pending) => continue,
            Ok(WaitStatus::TimedOut) => { timed_out = true; break; }
            Ok(_) => panic!("desfecho inesperado"),
            Err(error) => panic!("{error:?}"),
        }
    }
    assert!(timed_out);
    assert!(start.elapsed() >= requests::EXPIRES - Duration::from_secs(1), "não fechou antes dos 10 min");
    assert!(reqs.feed().borrow().requests.is_empty());
    // A espera nova (quem pediu tentou de novo depois de uma falha de rede) ainda lê o desfecho.
    assert!(matches!(reqs.wait(&card.id, Duration::ZERO).await, Ok(WaitStatus::TimedOut)));
    tokio::time::sleep(IDLE + Duration::from_secs(1)).await;
    assert_eq!(reqs.wait(&card.id, Duration::ZERO).await.err(), Some(RequestError::Lost), "60 s depois da última espera, some");
}

#[tokio::test]
async fn a_request_without_any_wait_for_60s_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let _vault = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    tokio::time::pause();
    tokio::time::sleep(IDLE - Duration::from_secs(5)).await;
    assert_eq!(reqs.feed().borrow().requests.len(), 1, "ainda dentro dos 60 s");
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(reqs.feed().borrow().requests.is_empty());
    assert_eq!(reqs.wait(&card.id, Duration::ZERO).await.err(), Some(RequestError::Lost));
}

#[tokio::test]
async fn saving_with_the_keyring_locked_keeps_the_request_open() {
    let dir = tempfile::tempdir().unwrap();
    let (keys, _) = vault_in(dir.path()).await;
    // Outro `Vault`: a chave não está na memória, como depois de reiniciar com o chaveiro trancado.
    let vault = Vault::new(keys.clone());
    *keys.locked.lock().unwrap() = true;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    let refused = reqs.answer(&vault, dir.path(), &card.id, save(&card, true)).await;
    assert_eq!(refused.err(), Some(RequestError::Vault(VaultError::Key(KeyError::Locked))));
    assert_eq!(reqs.feed().borrow().requests.len(), 1, "o cartão continua");
    assert_eq!(reqs.feed().borrow().state, Some(State::Locked));
    assert!(matches!(reqs.wait(&card.id, Duration::ZERO).await, Ok(WaitStatus::Pending)));
    *keys.locked.lock().unwrap() = false;
    reqs.answer(&vault, dir.path(), &card.id, save(&card, true)).await.unwrap();
    assert!(matches!(reqs.wait(&card.id, Duration::ZERO).await, Ok(WaitStatus::Saved { temporary: false, .. })));
}

#[tokio::test]
async fn a_free_entry_hands_the_value_without_a_request() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    vault.add(dir.path(), entry("api", Policy::Free)).await.unwrap();
    let reqs = Arc::new(Requests::default());
    assert_eq!(value(reqs.start_use(&vault, dir.path(), use_req("k1", "API")).await.unwrap()), VALUE);
    assert!(reqs.feed().borrow().requests.is_empty());
    assert_eq!(reqs.feed().borrow().state, Some(State::Open));
}

#[tokio::test]
async fn a_temporary_entry_serves_only_its_session_and_never_reaches_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    reqs.answer(&vault, dir.path(), &card.id, save(&card, false)).await.unwrap();
    match reqs.wait(&card.id, Duration::ZERO).await {
        Ok(WaitStatus::Saved { entry, temporary }) => assert_eq!((entry.as_str(), temporary), ("github", true)),
        _ => panic!("devia sair saved"),
    }
    assert_eq!(value(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap()), VALUE);
    assert_eq!(value(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap()), VALUE, "vale enquanto durar");
    let other = reqs.start_use(&vault, dir.path(), use_req("k2", "github")).await;
    assert_eq!(other.err(), Some(RequestError::Vault(VaultError::UnknownEntry)));
    assert!(vault.list(dir.path()).await.unwrap().is_empty());
}

#[tokio::test]
async fn saving_with_keep_grants_exactly_one_use_to_that_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    reqs.answer(&vault, dir.path(), &card.id, save(&card, true)).await.unwrap();
    assert!(matches!(reqs.wait(&card.id, Duration::ZERO).await, Ok(WaitStatus::Saved { temporary: false, .. })));
    assert_eq!(vault.list(dir.path()).await.unwrap()[0].policy, Policy::Ask, "padrão é pedir sempre");
    assert!(opened(reqs.start_use(&vault, dir.path(), use_req("k2", "github")).await.unwrap()).kind == Kind::Approve, "outra sessão pede");
    assert_eq!(value(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap()), VALUE);
    let second = opened(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap());
    assert_eq!(second.entry_name.as_deref(), Some("github"), "o segundo uso já pede");
}

#[tokio::test]
async fn the_first_use_belongs_to_the_saved_entry_not_to_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    reqs.answer(&vault, dir.path(), &card.id, save(&card, true)).await.unwrap();
    let id = vault.list(dir.path()).await.unwrap()[0].id.clone();
    vault.remove(dir.path(), &id).await.unwrap();
    vault.add(dir.path(), entry("github", Policy::Ask)).await.unwrap();
    let asked = opened(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap());
    assert_eq!(asked.kind, Kind::Approve, "outra entrada com o mesmo nome pede de novo");
}

#[tokio::test]
async fn keeping_replaces_the_temporary_value_of_that_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_, vault) = vault_in(dir.path()).await;
    let reqs = Arc::new(Requests::default());
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    let old = Answer { password: Some(Zeroizing::new("valor-velho".into())), ..save(&card, false) };
    reqs.answer(&vault, dir.path(), &card.id, old).await.unwrap();
    let card = reqs.start_ask(dir.path(), ask("k1", "github")).unwrap();
    reqs.answer(&vault, dir.path(), &card.id, save(&card, true)).await.unwrap();
    assert_eq!(value(reqs.start_use(&vault, dir.path(), use_req("k1", "github")).await.unwrap()), VALUE);
}

struct Server { addr: SocketAddr, st: Arc<AppState>, _dir: tempfile::TempDir }

async fn serve() -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config { listen: addr, upstream: "127.0.0.1:9".parse().unwrap(), auth_token: OWNER.into(),
        internal_secret: "privado".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.hangar_dir = dir.path().join("hangar");
    std::fs::create_dir_all(&state.hangar_dir).unwrap();
    state.vault = Arc::new(Vault::new(Arc::new(Keys::default())));
    let st = Arc::new(state);
    let app = router(st.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, st, _dir: dir }
}

async fn call(srv: &Server, method: reqwest::Method, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, String) {
    let mut req = reqwest::Client::new().request(method, format!("http://{}{path}", srv.addr)).bearer_auth(OWNER);
    for (k, v) in headers { req = req.header(*k, *v); }
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

fn code(text: &str) -> Value { serde_json::from_str::<Value>(text).unwrap_or(Value::Null)["detail"]["code"].clone() }

#[tokio::test(flavor = "multi_thread")]
async fn an_approval_over_the_routes_refuses_stale_guest_and_echo_then_hands_the_value_until_closed() {
    use reqwest::Method;
    let srv = serve().await;
    assert_eq!(call(&srv, Method::PUT, "/api/vault/config", &[], Some(json!({"owner": "self"}))).await.0, 200);
    let added = call(&srv, Method::POST, "/api/vault/entries", &[], Some(json!({"name": "github", "password": VALUE}))).await;
    assert_eq!(added.0, 200);
    let used = call(&srv, Method::POST, "/api/vault/use", &[], Some(json!({"origin_server": "delphi", "session_key": "k1", "session": "s1",
        "entry": "github", "kind": "exec", "reason": "deploy", "target": "gh auth status"}))).await;
    assert_eq!(used.0, 200);
    assert!(!used.1.contains(VALUE));
    let id = serde_json::from_str::<Value>(&used.1).unwrap()["request_id"].as_str().unwrap().to_owned();
    let card = srv.st.vault_requests.feed().borrow().requests[0].clone();
    assert_eq!((card.id.as_str(), card.origin.as_str(), card.target.as_deref()), (id.as_str(), "delphi::s1", Some("gh auth status")));
    let path = format!("/api/vault/requests/{id}");

    let stale = call(&srv, Method::POST, &path, &[], Some(json!({"interaction_id": "velho", "decision": "allow"}))).await;
    assert_eq!((stale.0, code(&stale.1)), (409, json!("erro_interacao_desatualizada")));
    let guest = call(&srv, Method::POST, &path, &[("x-hangar-guest", "1")], Some(json!({"interaction_id": card.interaction_id, "decision": "allow"}))).await;
    assert_eq!(guest.0, 403);
    let echo = call(&srv, Method::POST, &path, &[], Some(json!({"interaction_id": card.interaction_id, "decision": "allow", "password": VALUE}))).await;
    assert_eq!((echo.0, code(&echo.1)), (422, json!("erro_corpo_invalido")));
    assert!(!echo.1.contains(VALUE));

    assert_eq!(call(&srv, Method::POST, &path, &[], Some(json!({"interaction_id": card.interaction_id, "decision": "allow"}))).await.0, 204);
    let twice = call(&srv, Method::POST, &path, &[], Some(json!({"interaction_id": card.interaction_id, "decision": "deny"}))).await;
    assert_eq!((twice.0, code(&twice.1)), (409, json!("erro_cofre_pedido_respondido")));
    let waited = call(&srv, Method::GET, &format!("{path}/wait"), &[], None).await;
    assert_eq!(waited.0, 200);
    assert_eq!(serde_json::from_str::<Value>(&waited.1).unwrap(), json!({"status": "allowed", "value": VALUE}));
    // A espera mais nova (quem pediu tentou de novo) ainda lê o desfecho, até o pedido fechar.
    let again = call(&srv, Method::GET, &format!("{path}/wait"), &[], None).await;
    assert_eq!(serde_json::from_str::<Value>(&again.1).unwrap()["status"], json!("allowed"));
    assert_eq!(call(&srv, Method::DELETE, &path, &[], None).await.0, 204);
    let gone = call(&srv, Method::GET, &format!("{path}/wait"), &[], None).await;
    assert_eq!((gone.0, code(&gone.1)), (404, json!("erro_cofre_pedido_perdido")));
    assert_eq!(call(&srv, Method::DELETE, &path, &[], None).await.0, 404);
}
