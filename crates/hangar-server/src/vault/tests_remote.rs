//! A dona vista de outra máquina: `vault.json`, o cliente `remote.rs` contra uma dona falsa e as
//! rotas de estado, configuração e lista na máquina que não é dona.
use super::*;
use crate::{auth::TrustedHosts, config::Config, groups::peers::{PeerBook, PeerClient}, routes::{AppState, router}};
use axum::{Router, body::Body, extract::{Path as UrlPath, RawQuery}, http::HeaderMap, response::Response, routing::{get, post}};
use futures_util::future::BoxFuture;
use remote::{Remote, RemoteError, UseStart, WaitStatus};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::{Mutex, atomic::{AtomicU32, Ordering}}, time::Instant};
use wire::{AskReq, UseKind, UseReq, WAIT_HOLD};

const VALUE: &str = "valor-da-dona-K7q";
const TOKEN: &str = "tok-lab";
const OWNER: &str = "dono";

/// Chaveiro que nunca abre: estas rotas não podem chegar à chave.
struct NoKeys;

impl KeyStore for NoKeys {
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> { Box::pin(async { Err(KeyError::Locked) }) }
    fn write(&self, _: Key) -> BoxFuture<'_, Result<(), KeyError>> { Box::pin(async { Err(KeyError::Locked) }) }
}

fn js(status: u16, body: Value) -> Response {
    Response::builder().status(status).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()
}

fn authorized(headers: &HeaderMap) -> bool { headers.get("authorization").is_some_and(|v| v == &format!("Bearer {TOKEN}")) }

async fn serve_app(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Porta que ninguém escuta: a dona fora do ar.
fn closed_port() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn peers_file(dir: &std::path::Path, addr: SocketAddr) -> std::path::PathBuf {
    let file = dir.join("peers.json");
    std::fs::write(&file, json!({"lab": {"base_url": format!("http://{addr}/"), "token": TOKEN}}).to_string()).unwrap();
    file
}

fn peers_for(dir: &std::path::Path, addr: SocketAddr) -> PeerClient { PeerClient::new(PeerBook::new(Some(peers_file(dir, addr)))) }

fn use_req(entry: &str) -> UseReq {
    UseReq { origin_server: "delphi".into(), session_key: "k1".into(), session: "s1".into(), entry: entry.into(), kind: UseKind::Exec,
        reason: "deploy".into(), target: "wrangler whoami".into() }
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_rides_a_real_25s_hold_then_gets_the_value() {
    let calls = Arc::new(AtomicU32::new(0));
    let seen = calls.clone();
    let app = Router::new().route("/api/vault/requests/{id}/wait", get(move |UrlPath(id): UrlPath<String>, headers: HeaderMap| {
        let calls = seen.clone();
        async move {
            if id != "abc123" || !authorized(&headers) { return js(400, json!({"detail": {"code": "teste_pedido_errado"}})); }
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                tokio::time::sleep(WAIT_HOLD).await;
                return js(200, json!({"status": "pending"}));
            }
            js(200, json!({"status": "allowed", "value": VALUE}))
        }
    }));
    let addr = serve_app(app).await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let remote = Remote { peers: &peers, owner: "lab" };
    let start = Instant::now();
    let first = remote.wait("abc123").await;
    assert!(matches!(first, Ok(WaitStatus::Pending)), "{:?}", first.err());
    assert!(start.elapsed() >= WAIT_HOLD, "a dona segurou de verdade e o cliente não desistiu antes");
    match remote.wait("abc123").await {
        Ok(WaitStatus::Allowed(value)) => assert_eq!(value.as_str(), VALUE),
        other => panic!("esperava allowed: {:?}", other.err()),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_answers_map_to_their_status_and_404_is_a_lost_request() {
    let app = Router::new().route("/api/vault/requests/{id}/wait", get(|UrlPath(id): UrlPath<String>| async move {
        match id.as_str() {
            "salvo" => js(200, json!({"status": "saved", "entry": "Banco", "temporary": true})),
            "negado" => js(200, json!({"status": "denied"})),
            "vencido" => js(200, json!({"status": "timed_out"})),
            "torto" => js(200, json!({"status": "allowed"})),
            "perdido" => js(404, json!({"detail": {"code": "erro_cofre_pedido_perdido", "params": {}, "msg": "pedido perdido"}})),
            // A 8768 (Python) não tem a rota: 404 sem código.
            _ => js(404, json!({"detail": "Not Found"})),
        }
    }));
    let addr = serve_app(app).await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let remote = Remote { peers: &peers, owner: "lab" };
    assert!(matches!(remote.wait("salvo").await, Ok(WaitStatus::Saved { entry, temporary: true }) if entry == "Banco"));
    assert!(matches!(remote.wait("negado").await, Ok(WaitStatus::Denied)));
    assert!(matches!(remote.wait("vencido").await, Ok(WaitStatus::TimedOut)));
    assert_eq!(remote.wait("perdido").await.err(), Some(RemoteError::Lost));
    assert_eq!(remote.wait("perdido").await.err().unwrap().code(), "erro_cofre_pedido_perdido");
    let torto = remote.wait("torto").await.err().unwrap();
    assert_eq!(torto.code(), "erro_cofre_dona_fora", "allowed sem valor não é sucesso");
    let connect = remote.wait("connect").await.err().unwrap();
    assert_eq!(connect.code(), "erro_cofre_dona_fora");
    assert!(connect.reason().contains("Connect"), "{}", connect.reason());
    // Id que a dona não gera nem sai daqui.
    assert_eq!(remote.wait("../x").await.err(), Some(RemoteError::Lost));
}

#[tokio::test(flavor = "multi_thread")]
async fn use_ask_and_cancel_follow_the_contract() {
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (use_seen, ask_seen) = (bodies.clone(), bodies.clone());
    let app = Router::new()
        .route("/api/vault/use", post(move |body: String| {
            let seen = use_seen.clone();
            async move {
                let body: Value = serde_json::from_str(&body).unwrap();
                let free = body["entry"] == "Livre";
                seen.lock().unwrap().push(body);
                if free { js(200, json!({"value": VALUE})) } else { js(200, json!({"request_id": "r1"})) }
            }
        }))
        .route("/api/vault/ask-entry", post(move |body: String| {
            let seen = ask_seen.clone();
            async move {
                seen.lock().unwrap().push(serde_json::from_str(&body).unwrap());
                js(200, json!({"request_id": "r2"}))
            }
        }))
        .route("/api/vault/requests/{id}", axum::routing::delete(|UrlPath(id): UrlPath<String>| async move {
            if id == "r1" { Response::builder().status(204).body(Body::empty()).unwrap() } else { js(404, json!({"detail": {"code": "erro_cofre_pedido_perdido"}})) }
        }));
    let addr = serve_app(app).await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let remote = Remote { peers: &peers, owner: "lab" };
    assert!(matches!(remote.use_(&use_req("Banco")).await, Ok(UseStart::Request(id)) if id == "r1"));
    match remote.use_(&use_req("Livre")).await {
        Ok(UseStart::Value(value)) => assert_eq!(value.as_str(), VALUE),
        other => panic!("esperava o valor: {:?}", other.err()),
    }
    let ask = AskReq { origin_server: "delphi".into(), session_key: "k1".into(), session: "s1".into(), label: "Token CF".into(),
        reason: "deploy".into(), placeholder: None };
    assert_eq!(remote.ask_entry(&ask).await, Ok("r2".to_owned()));
    assert_eq!(remote.cancel("r1").await, Ok(()));
    assert_eq!(remote.cancel("r9").await, Err(RemoteError::Lost));
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies[0], json!({"origin_server": "delphi", "session_key": "k1", "session": "s1", "entry": "Banco", "kind": "exec",
        "reason": "deploy", "target": "wrangler whoami"}));
    assert_eq!(bodies[2], json!({"origin_server": "delphi", "session_key": "k1", "session": "s1", "label": "Token CF", "reason": "deploy"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_that_is_down_or_unknown_is_owner_down_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), closed_port());
    let remote = Remote { peers: &peers, owner: "lab" };
    for error in [remote.wait("abc").await.err(), remote.use_(&use_req("Banco")).await.err(),remote.entries().await.err()] {
        let error = error.unwrap();
        assert_eq!(error.code(), "erro_cofre_dona_fora");
        assert!(error.reason().contains("lab"), "{}", error.reason());
    }
    let unknown = Remote { peers: &peers, owner: "ninguem" };
    let error = unknown.status().await.err().unwrap();
    assert!(error.code() == "erro_cofre_dona_fora" && error.reason().contains("peers.json"), "{}", error.reason());
}

struct Machine { addr: SocketAddr, dir: tempfile::TempDir }

/// Uma máquina que não é dona, com `lab` no `peers.json` apontando para `owner_addr`.
async fn machine(owner_addr: SocketAddr) -> Machine {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config { listen: addr, upstream: "127.0.0.1:9".parse().unwrap(), auth_token: OWNER.into(),
        internal_secret: "privado".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.hangar_dir = dir.path().join("hangar");
    std::fs::create_dir_all(&state.hangar_dir).unwrap();
    state.server_id = "delphi".into();
    state.vault = Arc::new(Vault::new(Arc::new(NoKeys)));
    state.peers = Arc::new(PeerClient::new(PeerBook::new(Some(peers_file(dir.path(), owner_addr)))));
    let app = router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Machine { addr, dir }
}

impl Machine {
    fn hangar(&self) -> std::path::PathBuf { self.dir.path().join("hangar") }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = reqwest::Client::new().request(method, format!("http://{}{path}", self.addr)).bearer_auth(OWNER);
        if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        (status, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
}

fn fake_owner_app(queries: Arc<Mutex<Vec<String>>>) -> Router {
    let (a, b) = (queries.clone(), queries);
    Router::new()
        .route("/api/vault/status", get(move |RawQuery(q): RawQuery| {
            let seen = a.clone();
            async move { seen.lock().unwrap().push(q.unwrap_or_default()); js(200, json!({"role": "owner", "state": "open"})) }
        }))
        .route("/api/vault/entries", get(move |RawQuery(q): RawQuery| {
            let seen = b.clone();
            async move {
                seen.lock().unwrap().push(q.unwrap_or_default());
                // Campo a mais que a dona mandasse por engano não chega a quem pediu.
                js(200, json!([{"id": "e1", "name": "Banco", "policy": "ask", "updated_at": 1.0, "password": VALUE}]))
            }
        }))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_machine_reports_the_owner_state_and_offline_with_the_reason() {
    use reqwest::Method;
    let queries = Arc::new(Mutex::new(Vec::new()));
    let srv = machine(serve_app(fake_owner_app(queries.clone())).await).await;
    assert_eq!(srv.call(Method::GET, "/api/vault/status", None).await, (200, json!({"role": "none", "state": "absent", "server_id": "delphi"})));
    let (status, body) = srv.call(Method::PUT, "/api/vault/config", Some(json!({"owner": "lab"}))).await;
    assert_eq!((status, body), (200, json!({"role": "remote", "owner": "lab", "state": "open"})));
    assert_eq!(config::load(&srv.hangar()).unwrap().owner.as_deref(), Some("lab"));
    assert_eq!(srv.call(Method::GET, "/api/vault/status", None).await, (200, json!({"role": "remote", "owner": "lab", "state": "open"})));
    // Pedido de outra máquina não é encaminhado de novo.
    assert_eq!(srv.call(Method::GET, "/api/vault/status?local=1", None).await,
        (200, json!({"role": "none", "state": "absent", "server_id": "delphi"})));

    let (status, list) = srv.call(Method::GET, "/api/vault/entries", None).await;
    assert_eq!((status, list), (200, json!([{"id": "e1", "name": "Banco", "policy": "ask", "updated_at": 1.0}])));
    assert!(queries.lock().unwrap().iter().all(|q| q == "local=1"), "{:?}", queries.lock().unwrap());

    // A dona sai do ar: o estado diz offline e o motivo, sem erro na rota.
    std::fs::write(srv.dir.path().join("peers.json"), json!({"lab": {"base_url": format!("http://{}", closed_port()), "token": TOKEN}}).to_string()).unwrap();
    let (status, body) = srv.call(Method::GET, "/api/vault/status", None).await;
    assert_eq!((status, body["role"].clone(), body["owner"].clone(), body["state"].clone()), (200, json!("remote"), json!("lab"), json!("offline")));
    assert!(body["detail"].as_str().is_some_and(|d| d.contains("lab")), "{body}");
    let (status, body) = srv.call(Method::GET, "/api/vault/entries", None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (502, json!("erro_cofre_dona_fora")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_holds_a_vault_cannot_point_to_another_owner() {
    use reqwest::Method;
    let srv = machine(serve_app(fake_owner_app(Arc::default())).await).await;
    std::fs::create_dir_all(srv.hangar().join("vault")).unwrap();
    std::fs::write(store::path(&srv.hangar()), b"cofre").unwrap();
    let (status, body) = srv.call(Method::PUT, "/api/vault/config", Some(json!({"owner": "lab"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_cofre_ja_e_dona")));
    assert!(!config::path(&srv.hangar()).exists(), "a recusa gravou o vault.json");
    assert_eq!(std::fs::read(store::path(&srv.hangar())).unwrap(), b"cofre");
}

#[tokio::test(flavor = "multi_thread")]
async fn config_checks_the_peer_and_keeps_the_backup_peer() {
    use reqwest::Method;
    let srv = machine(serve_app(fake_owner_app(Arc::default())).await).await;
    let (status, body) = srv.call(Method::PUT, "/api/vault/config", Some(json!({"owner": "ninguem"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_cofre_peer_desconhecido")));
    for bad in [json!({}), json!({"owner": "../x"}), json!({"dona": "lab"}), json!({"owner": 5})] {
        assert_eq!(srv.call(Method::PUT, "/api/vault/config", Some(bad.clone())).await.0, 422, "{bad}");
    }
    assert_eq!(srv.call(Method::PUT, "/api/vault/config", Some(json!({"backup_peer": "lab"}))).await.0, 200);
    assert_eq!(config::load(&srv.hangar()).unwrap(), config::Config { owner: None, backup_peer: Some("lab".into()) });
    assert_eq!(srv.call(Method::PUT, "/api/vault/config", Some(json!({"owner": "lab"}))).await.0, 200);
    assert_eq!(srv.call(Method::PUT, "/api/vault/config", Some(json!({"owner": null}))).await,
        (200, json!({"role": "none", "state": "absent", "server_id": "delphi"})));
    assert_eq!(config::load(&srv.hangar()).unwrap(), config::Config { owner: None, backup_peer: Some("lab".into()) });
    // vault.json torto é erro, nunca "sem dona".
    std::fs::write(config::path(&srv.hangar()), b"{ torto").unwrap();
    let (status, body) = srv.call(Method::GET, "/api/vault/status", None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (500, json!("erro_cofre_config_ilegivel")));
    let (status, body) = srv.call(Method::PUT, "/api/vault/config", Some(json!({"backup_peer": "lab"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (500, json!("erro_cofre_config_ilegivel")));
    // Tirar a dona reescreve o arquivo torto.
    assert_eq!(srv.call(Method::PUT, "/api/vault/config", Some(json!({"owner": null}))).await.0, 200);
    assert_eq!(config::load(&srv.hangar()).unwrap(), config::Config::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn another_machine_at_the_owner_address_guest_port_and_redirect_are_owner_down() {
    let app = Router::new()
        .route("/api/vault/status", get(|| async { js(200, json!({"role": "owner", "state": "open", "server_id": "outra"})) }))
        .route("/api/vault/requests/{id}/wait", get(|UrlPath(id): UrlPath<String>| async move {
            match id.as_str() {
                "outra" => js(200, json!({"status": "allowed", "value": VALUE, "server_id": "outra"})),
                "certa" => js(200, json!({"status": "pending", "server_id": "lab"})),
                "convidado" => js(403, json!({"detail": {"code": "erro_cofre_so_dono", "msg": "só o dono usa o cofre"}})),
                _ => Response::builder().status(302).header("location", "http://127.0.0.1:9/").body(Body::empty()).unwrap(),
            }
        }));
    let addr = serve_app(app).await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let remote = Remote { peers: &peers, owner: "lab" };
    let error = remote.status().await.err().unwrap();
    assert!(error.code() == "erro_cofre_dona_fora" && error.reason().contains("outra"), "{}", error.reason());
    let error = remote.wait("outra").await.err().unwrap();
    assert!(error.code() == "erro_cofre_dona_fora" && !error.reason().contains(VALUE), "{}", error.reason());
    assert!(matches!(remote.wait("certa").await, Ok(WaitStatus::Pending)), "server_id igual à dona é aceito");
    let error = remote.wait("convidado").await.err().unwrap();
    assert!(error.code() == "erro_cofre_dona_fora" && error.reason().contains("convidado"), "{}", error.reason());
    let error = remote.wait("redirect").await.err().unwrap();
    assert!(error.code() == "erro_cofre_dona_fora" && error.reason().contains("redirect"), "o redirect não é seguido: {}", error.reason());
}

#[test]
fn placement_follows_the_vault_file_first() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(config::placement(dir.path()), Ok(config::Placement::None));
    config::save(dir.path(), &config::Config { owner: Some("lab".into()), backup_peer: None }).unwrap();
    assert_eq!(config::placement(dir.path()), Ok(config::Placement::Remote("lab".into())));
    std::fs::create_dir_all(dir.path().join("vault")).unwrap();
    std::fs::write(store::path(dir.path()), b"x").unwrap();
    assert_eq!(config::placement(dir.path()), Ok(config::Placement::Owner));
}
