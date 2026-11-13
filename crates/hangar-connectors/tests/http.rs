//! Rotas do dono: autorização antes de qualquer leitura e tradução dos erros do contrato.
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use hangar_connectors::host::{Host, HostOptions};
use hangar_connectors::registry::Registry;
use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::edge::Edge;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hangar-fixture-connector");
const TOKEN: &str = "dono-da-prova";

struct Bench { dir: tempfile::TempDir, host: Arc<Host>, app: axum::Router }

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(Registry::new(dir.path().join("connectors.json")), HostOptions::default());
        let edge = Edge {
            trusted: TrustedHosts::parse("127.0.0.1"),
            auth: Auth::configured(TOKEN),
            diag: DiagClient::new("127.0.0.1:9".parse().unwrap(), String::new()).with_diary(Some(dir.path().join("diary"))),
        };
        let app = hangar_connectors::http::router::<()>(edge, host.clone());
        Self { dir, host, app }
    }

    fn manifest(&self, id: &str, args: &[&str]) -> String { self.manifest_with(id, args, json!({})) }

    fn manifest_with(&self, id: &str, args: &[&str], extra: Value) -> String {
        let mut command = vec![FIXTURE.to_owned(), "--id".into(), id.to_owned()];
        command.extend(args.iter().map(|arg| arg.to_string()));
        let path = self.dir.path().join(format!("{id}.json"));
        let mut manifest = json!({"manifest_version": 1, "id": id, "name": id, "version": "1", "protocol_version": 1,
            "capabilities": {"work_tracking": 1}, "command": command});
        manifest.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        std::fs::write(&path, manifest.to_string()).unwrap();
        path.to_string_lossy().into_owned()
    }

    async fn send(&self, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token { request = request.header("authorization", format!("Bearer {token}")); }
        if body.is_some() { request = request.header("content-type", "application/json"); }
        let mut request = request.body(body.map_or_else(Body::empty, |body| Body::from(body.to_string()))).unwrap();
        request.extensions_mut().insert(ConnectInfo("127.0.0.1:40000".parse::<std::net::SocketAddr>().unwrap()));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let host = self.host.clone();
        std::thread::spawn(move || tokio::runtime::Runtime::new().unwrap().block_on(host.shutdown())).join().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_registers_lists_reads_and_removes() {
    let bench = Bench::new();
    let manifest = bench.manifest("prova", &[]);
    let (status, body) = bench.send("POST", "/api/connectors", Some(TOKEN), Some(json!({"manifest": manifest}))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["connector"]["id"], "prova");

    let (status, body) = bench.send("GET", "/api/connectors", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["connectors"][0]["running"], false, "listar não inicia o processo");

    let (status, body) = bench.send("GET", "/api/connectors/prova/tickets?limit=2&query=impress%C3%A3o", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][1]["id"], "cartão:1/2");
    assert_eq!(body["next_cursor"], "pagina-2");
    assert!(chrono::DateTime::parse_from_rfc3339(body["fetched_at"].as_str().unwrap()).is_ok());

    // ID com barra viaja codificado como um componente só.
    let (status, body) = bench.send("GET", "/api/connectors/prova/tickets/cart%C3%A3o%3A2%2F1", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], "cartão:2/1");
    assert_eq!(body["description_markdown"], "**negrito** e `código`");

    let (status, body) = bench.send("PATCH", "/api/connectors/prova", Some(TOKEN), Some(json!({"enabled": false}))).await;
    assert_eq!((status, body["connector"]["enabled"].clone()), (StatusCode::OK, json!(false)));
    let (status, body) = bench.send("GET", "/api/connectors/prova/tickets", Some(TOKEN), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("connector_disabled")));

    let (status, _) = bench.send("DELETE", "/api/connectors/prova", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = bench.send("GET", "/api/connectors/prova/tickets", Some(TOKEN), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("connector_not_found")));
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_owner_token_nothing_is_read_or_started() {
    let bench = Bench::new();
    let pids = bench.dir.path().join("pids");
    let manifest = bench.manifest("prova", &["--pid-file", pids.to_str().unwrap()]);
    for token in [None, Some("errado")] {
        let (status, _) = bench.send("POST", "/api/connectors", token, Some(json!({"manifest": manifest}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert!(bench.host.list().unwrap().is_empty(), "cadastro recusado não grava");
    bench.send("POST", "/api/connectors", Some(TOKEN), Some(json!({"manifest": manifest}))).await;
    for (method, uri) in [("GET", "/api/connectors"), ("GET", "/api/connectors/prova/tickets"), ("GET", "/api/connectors/prova/tickets/x"), ("DELETE", "/api/connectors/prova")] {
        let (status, _) = bench.send(method, uri, Some("errado"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
    assert!(!pids.exists(), "recusa não lança processo");
    assert_eq!(bench.host.list().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn contract_errors_keep_their_codes_without_looking_like_hangar_auth() {
    let bench = Bench::new();
    let manifest = bench.manifest("sem-login", &["--fail", "work_tracking.list=auth_required"]);
    bench.send("POST", "/api/connectors", Some(TOKEN), Some(json!({"manifest": manifest}))).await;
    let (status, body) = bench.send("GET", "/api/connectors/sem-login/tickets", Some(TOKEN), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::FAILED_DEPENDENCY, json!("auth_required")));
    let (status, body) = bench.send("GET", "/api/connectors/sem-login/tickets?jql=x", Some(TOKEN), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
    let (status, body) = bench.send("GET", "/api/connectors/sem-login/tickets?limit=500", Some(TOKEN), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
    let (status, body) = bench.send("POST", "/api/connectors", Some(TOKEN), Some(json!({"manifest": "relativo.json"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("manifest_invalid")));
}

#[tokio::test(flavor = "multi_thread")]
async fn detail_carries_the_start_prompt_and_links_survive_in_the_catalog() {
    let bench = Bench::new();
    let manifest = bench.manifest_with("prova", &[], json!({"start_prompt": "/iniciar {key}"}));
    bench.send("POST", "/api/connectors", Some(TOKEN), Some(json!({"manifest": manifest}))).await;
    let (status, body) = bench.send("GET", "/api/connectors/prova/tickets/t-1", Some(TOKEN), None).await;
    assert_eq!((status, body["start_prompt"].clone()), (StatusCode::OK, json!("/iniciar #t-1")), "{body}");

    let uri = "/api/connectors/prova/links/cart%C3%A3o%3A2%2F1";
    let (status, body) = bench.send("PUT", uri, Some(TOKEN), Some(json!({"session": "pm-19816"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = bench.send("GET", "/api/connectors", Some(TOKEN), None).await;
    assert_eq!(body["connectors"][0]["links"], json!({"cartão:2/1": "pm-19816"}));

    let (status, body) = bench.send("PUT", uri, Some(TOKEN), Some(json!({"session": "com espaço"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
    let (status, _) = bench.send("PUT", uri, Some("errado"), Some(json!({"session": "x"}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = bench.send("DELETE", uri, Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = bench.send("GET", "/api/connectors", Some(TOKEN), None).await;
    assert!(body["connectors"][0].get("links").is_none(), "{body}");
    let (status, body) = bench.send("PUT", "/api/connectors/outro/links/t", Some(TOKEN), Some(json!({"session": "x"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("connector_not_found")));
}
