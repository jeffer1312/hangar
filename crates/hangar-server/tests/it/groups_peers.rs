//! `peers.json` e a chamada a outra máquina (`PeerBook`, `PeerClient`) como o `peers.call` do Python.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use hangar_server::groups::peers::{PeerBook, PeerCfg, PeerClient, PeerError};
use serde_json::json;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn js(status: StatusCode, body: serde_json::Value) -> Response {
    Response::builder().status(status).header("content-type", "application/json").body(axum::body::Body::from(body.to_string())).unwrap()
}

/// Cliente com `lab` apontando para `addr`.
fn client_for(dir: &std::path::Path, addr: SocketAddr) -> PeerClient {
    let file = dir.join("peers.json");
    std::fs::write(&file, json!({"lab": {"base_url": format!("http://{addr}/"), "token": "dono"}}).to_string()).unwrap();
    PeerClient::new(PeerBook::new(Some(file)))
}

#[test]
fn book_reads_only_entries_with_base_url_and_token() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("peers.json");
    std::fs::write(&file, json!({
        "lab": {"base_url": "http://100.64.0.2:8765/", "token": "t1"},
        "off": {"base_url": "http://100.64.0.3:8765", "token": "t2", "enabled": false},
        "tela": {"app": {"id": "tela", "address": "http://100.64.0.4:8765"}},
        "sem-token": {"base_url": "http://100.64.0.5:8765"},
        "torto": "x",
    }).to_string()).unwrap();
    let book = PeerBook::new(Some(file.clone()));
    assert_eq!(book.get("lab"), Some(PeerCfg { base_url: "http://100.64.0.2:8765".into(), token: "t1".into() }));
    assert_eq!(book.get("off").map(|c| c.token), Some("t2".into()), "desligada da varredura continua endereçável");
    for absent in ["tela", "sem-token", "torto", "ninguem"] { assert_eq!(book.get(absent), None, "{absent}"); }
    // Arquivo novo é relido; ausente ou torto = nenhuma máquina.
    std::fs::write(&file, json!({"lab": {"base_url": "http://100.64.0.9:8765", "token": "novo-token"}}).to_string()).unwrap();
    assert_eq!(book.get("lab").map(|c| c.token), Some("novo-token".into()));
    std::fs::write(&file, "{ torto").unwrap();
    assert_eq!(book.get("lab"), None);
    assert_eq!(PeerBook::new(None).get("lab"), None);
    assert_eq!(PeerBook::new(Some(dir.path().join("nao-existe.json"))).get("lab"), None);
}

#[tokio::test]
async fn unknown_server_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let client = PeerClient::new(PeerBook::new(Some(dir.path().join("peers.json"))));
    let err = client.call("lab", reqwest::Method::POST, "/x", None).await.unwrap_err();
    assert!(matches!(err, PeerError::Unknown) && !err.is_transport());
    assert_eq!(err.text("lab"), "servidor 'lab' não está em peers.json (ou sem base_url/token)");
}

#[tokio::test]
async fn answers_and_refusals_like_python() {
    let app = Router::new()
        .route("/ok", any(|headers: HeaderMap, body: String| async move {
            js(StatusCode::OK, json!({"auth": headers.get("authorization").and_then(|v| v.to_str().ok()), "ct": headers.get("content-type").and_then(|v| v.to_str().ok()), "body": body}))
        }))
        .route("/vazio", any(|| async { StatusCode::OK }))
        .route("/html", any(|| async { "<html>" }))
        .route("/recusa", any(|| async {
            js(StatusCode::CONFLICT, json!({"detail": {"code": "erro_x", "params": {"n": 1}, "msg": "não"}}))
        }))
        .route("/cru", any(|| async { (StatusCode::BAD_GATEWAY, "fora do ar") }))
        .route("/grande", get(|| async { vec![b'a'; 2 << 20] }));
    let addr = serve(app).await;
    let dir = tempfile::tempdir().unwrap();
    let client = client_for(dir.path(), addr);
    let got = client.call("lab", reqwest::Method::POST, "/ok", Some(&json!({"peer": "casa::s0"}))).await.unwrap().unwrap();
    assert_eq!(got, json!({"auth": "Bearer dono", "ct": "application/json", "body": "{\"peer\":\"casa::s0\"}"}));
    assert_eq!(client.call("lab", reqwest::Method::POST, "/vazio", None).await.unwrap(), None);
    let err = client.call("lab", reqwest::Method::POST, "/html", None).await.unwrap_err();
    assert!(err.is_transport() && err.text("lab").starts_with("lab respondeu corpo ilegível: "), "{err:?}");

    let err = client.call("lab", reqwest::Method::POST, "/recusa", None).await.unwrap_err();
    let PeerError::Refused { status, detail } = &err else { panic!("{err:?}") };
    assert_eq!((*status, detail.clone()), (409, json!({"code": "erro_x", "params": {"n": 1}, "msg": "não"})));
    assert_eq!(err.text("lab"), "lab respondeu HTTP 409: {'code': 'erro_x', 'params': {'n': 1}, 'msg': 'não'}");
    let err = client.call("lab", reqwest::Method::POST, "/cru", None).await.unwrap_err();
    assert_eq!(err.text("lab"), "lab respondeu HTTP 502: fora do ar");

    let err = client.call("lab", reqwest::Method::GET, "/grande", None).await.unwrap_err();
    assert!(matches!(&err, PeerError::Transport(t) if t == "resposta maior que 1 MiB"), "{err:?}");
}

#[tokio::test]
async fn network_failure_is_transport() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let dir = tempfile::tempdir().unwrap();
    let err = client_for(dir.path(), addr).call("lab", reqwest::Method::POST, "/x", None).await.unwrap_err();
    assert!(err.is_transport() && err.text("lab").starts_with("lab inacessível: "), "{err:?}");
}

/// O redirect segue; para outro host (aqui, outra porta) ele vai sem o token de dono.
#[tokio::test]
async fn redirect_follows_without_authorization_to_another_host() {
    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let log = seen.clone();
    let other = serve(Router::new().route("/destino", any(move |headers: HeaderMap| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned));
            js(StatusCode::OK, json!({"ok": true}))
        }
    }))).await;
    let log = seen.clone();
    let home = serve(Router::new()
        .route("/fora", any(move || async move { Response::builder().status(302).header("location", format!("http://{other}/destino")).body(axum::body::Body::empty()).unwrap() }))
        .route("/dentro", any(|| async { (StatusCode::FOUND, [("location", "/aqui")]).into_response() }))
        .route("/aqui", any(move |headers: HeaderMap| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned));
                js(StatusCode::OK, json!({"ok": "aqui"}))
            }
        }))).await;
    let dir = tempfile::tempdir().unwrap();
    let client = client_for(dir.path(), home);
    assert_eq!(client.call("lab", reqwest::Method::GET, "/fora", None).await.unwrap(), Some(json!({"ok": true})));
    assert_eq!(client.call("lab", reqwest::Method::GET, "/dentro", None).await.unwrap(), Some(json!({"ok": "aqui"})));
    assert_eq!(*seen.lock().unwrap(), vec![None, Some("Bearer dono".to_owned())]);
}
