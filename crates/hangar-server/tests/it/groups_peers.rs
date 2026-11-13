//! `peers.json` e a chamada a outra máquina (`PeerBook`, `PeerClient`) como o `peers.call` do Python.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use hangar_server::groups::peers::{ConnectRoute, PeerBook, PeerCfg, PeerClient, PeerError, Route};
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
    assert_eq!(book.get("lab"), Some(PeerCfg { base_url: "http://100.64.0.2:8765".into(), token: "t1".into(), connect: None }));
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
    let addr = crate::refused_address();
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

#[test]
fn enabled_ids_skip_disabled_and_app_only_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.json");
    std::fs::write(&path, r#"{"vps":{"base_url":"http://a","token":"t"},"off":{"base_url":"http://b","token":"t","enabled":false},"app":{"app":true}}"#).unwrap();
    assert_eq!(PeerClient::new(PeerBook::new(Some(path))).enabled_ids(), vec!["vps".to_owned()]);
}

/// Respostas de `lab` nas rotas entre máquinas: `server_id` de quem atendeu decide quem é.
fn groups_app() -> Router {
    Router::new()
        .route("/api/groups/certo", any(|| async { js(StatusCode::OK, json!({"server_id": "lab", "protocol": 1, "gid": "certo"})) }))
        .route("/api/groups/outra", any(|| async { js(StatusCode::OK, json!({"server_id": "vm", "protocol": 1})) }))
        .route("/api/groups/sem-id", any(|| async { js(StatusCode::OK, json!({"protocol": 1})) }))
        .route("/api/groups/sem-protocolo", any(|| async { js(StatusCode::OK, json!({"server_id": "lab"})) }))
        .route("/api/groups/codigo", any(|| async {
            js(StatusCode::NOT_FOUND, json!({"detail": {"code": "erro_grupo_inexistente", "params": {}, "msg": "não"}}))
        }))
        .route("/api/groups/fastapi", any(|| async { js(StatusCode::NOT_FOUND, json!({"detail": "Not Found"})) }))
}

#[tokio::test]
async fn key_mismatch_is_refused_before_any_write() {
    let addr = serve(groups_app()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = client_for(dir.path(), addr);
    let got = client.call_group("lab", reqwest::Method::GET, "/api/groups/certo", None).await.unwrap();
    assert_eq!(got["gid"], "certo");
    // `lab` no peers.json atende como `vm`: recusa citando os dois, sem devolver o corpo a quem grava.
    let err = client.call_group("lab", reqwest::Method::PUT, "/api/groups/outra", Some(&json!({}))).await.unwrap_err();
    assert!(matches!(&err, PeerError::IdentityMismatch { key, reported } if key == "lab" && reported == "vm"), "{err:?}");
    let text = err.text("lab");
    assert!(text.contains("'lab'") && text.contains("'vm'"), "{text}");
    // Na sondagem (GET do grupo) o `server_id` é obrigatório.
    let err = client.call_group("lab", reqwest::Method::GET, "/api/groups/sem-id", None).await.unwrap_err();
    assert!(matches!(&err, PeerError::IdentityMismatch { reported, .. } if reported.is_empty()), "{err:?}");
}

#[tokio::test]
async fn missing_server_id_on_ok_op_is_accepted() {
    let app = groups_app().route("/api/groups/vazio/ops", any(|| async { StatusCode::NO_CONTENT }));
    let addr = serve(app).await;
    let dir = tempfile::tempdir().unwrap();
    let client = client_for(dir.path(), addr);
    let got = client.call_group("lab", reqwest::Method::PUT, "/api/groups/sem-id", Some(&json!({}))).await.unwrap();
    assert_eq!(got["protocol"], 1);
    assert_eq!(client.call_group("lab", reqwest::Method::POST, "/api/groups/vazio/ops", Some(&json!({}))).await.unwrap(), serde_json::Value::Null);
    // Presente e diferente continua recusado fora da sondagem.
    let err = client.call_group("lab", reqwest::Method::POST, "/api/groups/outra", Some(&json!({}))).await.unwrap_err();
    assert!(matches!(err, PeerError::IdentityMismatch { .. }), "{err:?}");
}

#[tokio::test]
async fn coded_404_is_not_old_version() {
    let addr = serve(groups_app()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = client_for(dir.path(), addr);
    let err = client.call_group("lab", reqwest::Method::GET, "/api/groups/codigo", None).await.unwrap_err();
    assert!(matches!(&err, PeerError::Refused { status: 404, .. }), "{err:?}");
    // 404 sem código: a rota não existe lá, máquina antiga (o 404 do FastAPI e o do axum).
    for path in ["/api/groups/fastapi", "/api/groups/nada/ops"] {
        let err = client.call_group("lab", reqwest::Method::POST, path, Some(&json!({}))).await.unwrap_err();
        assert!(matches!(err, PeerError::OldVersion), "{path}: {err:?}");
    }
    // O registro sem `protocol` é de antes do protocolo 1.
    let err = client.call_group("lab", reqwest::Method::GET, "/api/groups/sem-protocolo", None).await.unwrap_err();
    assert!(matches!(err, PeerError::OldVersion), "{err:?}");
    // Fora de `/api/groups/*` o 404 sem código continua recusa comum.
    let err = client.call("lab", reqwest::Method::GET, "/api/outra", None).await.unwrap_err();
    assert!(matches!(err, PeerError::Refused { status: 404, .. }), "{err:?}");
}

/// Máquina que conta os pedidos e devolve `status` com `{"via": via}`.
async fn counting(via: &'static str, status: StatusCode) -> (Arc<Mutex<Vec<Option<String>>>>, SocketAddr) {
    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let log = seen.clone();
    let addr = serve(Router::new().route("/x", any(move |headers: HeaderMap| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned));
            js(status, json!({"via": via}))
        }
    }))).await;
    (seen, addr)
}

fn write_routed(file: &std::path::Path, saved: &str, connect: &str) {
    std::fs::write(file, json!({"lab": {"base_url": saved, "token": "dono", "connect_url": connect, "connect_token": "aparelho",
        "connect_device": "abc"}}).to_string()).unwrap();
}

/// `lab` com o endereço salvo e o Connect; o cliente aceita o Connect.
fn routed(dir: &std::path::Path, saved: &str, connect: &str) -> PeerClient {
    let file = dir.join("peers.json");
    write_routed(&file, saved, connect);
    PeerClient::new(PeerBook::new(Some(file))).with_connect_route()
}

#[test]
fn the_connect_route_is_read_beside_the_saved_address_and_its_token_is_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("peers.json");
    write_routed(&file, "https://lab.tail.ts.net", "https://lab.acme.hangar.dev.br/");
    let book = PeerBook::new(Some(file.clone()));
    assert_eq!(book.get("lab").unwrap().connect, Some(ConnectRoute { url: "https://lab.acme.hangar.dev.br".into(), token: "aparelho".into() }));
    assert!(book.tokens().contains(&"aparelho".to_owned()), "a credencial de aparelho também sai do que volta ao modelo");
    // Com a marca `connect` o próprio endereço já é o Connect: não há caminho a somar.
    std::fs::write(&file, json!({"lab": {"base_url": "https://lab.acme.hangar.dev.br", "token": "t", "connect": true,
        "connect_url": "https://x", "connect_token": "y"}}).to_string()).unwrap();
    assert_eq!(book.get("lab").unwrap().connect, None);
}

#[tokio::test]
async fn a_closed_saved_address_goes_through_connect_and_the_next_call_starts_there() {
    let (connect_seen, connect) = counting("connect", StatusCode::OK).await;
    let dir = tempfile::tempdir().unwrap();
    let client = routed(dir.path(), &format!("http://{}", crate::refused_address()), &format!("http://{connect}"));
    assert_eq!(client.route("lab"), None);
    let got = client.call("lab", reqwest::Method::POST, "/x", Some(&json!({}))).await.unwrap();
    assert_eq!(got, Some(json!({"via": "connect"})));
    assert_eq!(*connect_seen.lock().unwrap(), vec![Some("Bearer aparelho".to_owned())], "pelo Connect só a credencial de aparelho");
    assert_eq!(client.route("lab"), Some(Route::Connect));
    // O endereço salvo voltou, mas a próxima chamada começa pelo caminho que atendeu.
    let (saved_seen, saved) = counting("saved", StatusCode::OK).await;
    write_routed(&dir.path().join("peers.json"), &format!("http://{saved}"), &format!("http://{connect}"));
    assert_eq!(client.call("lab", reqwest::Method::GET, "/x", None).await.unwrap(), Some(json!({"via": "connect"})));
    assert!(saved_seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_answer_from_the_saved_address_never_goes_through_connect() {
    let (connect_seen, connect) = counting("connect", StatusCode::OK).await;
    let (saved_seen, saved) = counting("saved", StatusCode::INTERNAL_SERVER_ERROR).await;
    let dir = tempfile::tempdir().unwrap();
    let client = routed(dir.path(), &format!("http://{saved}"), &format!("http://{connect}"));
    let err = client.call("lab", reqwest::Method::POST, "/x", Some(&json!({}))).await.unwrap_err();
    assert!(matches!(err, PeerError::Refused { status: 500, .. }), "{err:?}");
    assert_eq!(saved_seen.lock().unwrap().len(), 1);
    assert!(connect_seen.lock().unwrap().is_empty(), "o recado pode ter chegado: nada de segunda entrega");
    assert_eq!(client.route("lab"), None, "recusa não é caminho bom");
}

/// A conexão abriu e caiu no meio do corpo: o pedido chegou lá, então o Connect não é tentado.
#[tokio::test]
async fn a_broken_answer_after_connecting_never_goes_through_connect() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (connect_seen, connect) = counting("connect", StatusCode::OK).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let saved = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await;
        let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"via\"").await;
    });
    let dir = tempfile::tempdir().unwrap();
    let client = routed(dir.path(), &format!("http://{saved}"), &format!("http://{connect}"));
    let err = client.call("lab", reqwest::Method::POST, "/x", Some(&json!({}))).await.unwrap_err();
    assert!(err.is_transport(), "{err:?}");
    assert!(connect_seen.lock().unwrap().is_empty());
}

/// Quem não aceita o Connect (o anúncio, a emissão do aparelho) fica só no endereço salvo.
#[tokio::test]
async fn without_opting_in_the_connect_route_is_never_used() {
    let (connect_seen, connect) = counting("connect", StatusCode::OK).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("peers.json");
    write_routed(&file, &format!("http://{}", crate::refused_address()), &format!("http://{connect}"));
    let err = PeerClient::new(PeerBook::new(Some(file))).call("lab", reqwest::Method::POST, "/x", Some(&json!({}))).await.unwrap_err();
    assert!(err.is_transport(), "{err:?}");
    assert!(connect_seen.lock().unwrap().is_empty());
}

/// O dono de lá revogou o aparelho: o caminho sai do `peers.json` com a marca, e nada o religa sozinho.
#[tokio::test]
async fn a_refused_connect_credential_marks_the_entry() {
    let (_, connect) = counting("connect", StatusCode::UNAUTHORIZED).await;
    let dir = tempfile::tempdir().unwrap();
    let client = routed(dir.path(), &format!("http://{}", crate::refused_address()), &format!("http://{connect}"));
    let err = client.call("lab", reqwest::Method::GET, "/x", None).await.unwrap_err();
    assert!(matches!(err, PeerError::Refused { status: 401, .. }), "{err:?}");
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("peers.json")).unwrap()).unwrap();
    assert_eq!(saved["lab"]["connect_refused"], true);
    for field in ["connect_url", "connect_token", "connect_device"] { assert!(saved["lab"].get(field).is_none(), "{field}"); }
    assert_eq!(saved["lab"]["base_url"], format!("http://{}", crate::refused_address()).as_str());
    assert_eq!(client.route("lab"), None);
}

/// O 5xx do proxy prova que o Connect conecta: ele segue à frente. Só a falha de conexão nele o tira.
#[tokio::test]
async fn a_proxy_error_keeps_connect_in_front_and_only_a_connect_failure_drops_it() {
    use std::sync::atomic::{AtomicU16, Ordering};
    let answer = Arc::new(AtomicU16::new(200));
    let current = answer.clone();
    let connect = serve(Router::new().route("/x", any(move || {
        let current = current.clone();
        async move { js(StatusCode::from_u16(current.load(Ordering::Relaxed)).unwrap(), json!({"via": "connect"})) }
    }))).await;
    let closed = format!("http://{}", crate::refused_address());
    let dir = tempfile::tempdir().unwrap();
    let client = routed(dir.path(), &closed, &format!("http://{connect}"));
    client.call("lab", reqwest::Method::GET, "/x", None).await.unwrap();
    assert_eq!(client.route("lab"), Some(Route::Connect));
    answer.store(502, Ordering::Relaxed);
    let err = client.call("lab", reqwest::Method::GET, "/x", None).await.unwrap_err();
    assert!(matches!(err, PeerError::Refused { status: 502, .. }), "{err:?}");
    assert_eq!(client.route("lab"), Some(Route::Connect), "a próxima chamada não paga o prazo de conexão do endereço salvo");
    // O Connect nem conecta e o endereço salvo voltou: ele atende e passa à frente.
    let (saved_seen, saved) = counting("saved", StatusCode::OK).await;
    write_routed(&dir.path().join("peers.json"), &format!("http://{saved}"), &closed);
    assert_eq!(client.call("lab", reqwest::Method::GET, "/x", None).await.unwrap(), Some(json!({"via": "saved"})));
    assert_eq!(saved_seen.lock().unwrap().len(), 1);
    assert_eq!(client.route("lab"), Some(Route::Saved));
}
