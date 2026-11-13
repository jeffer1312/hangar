//! Mission Control pela API do dono: vínculo, etapas, portão, evidências e configuração das etapas.
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::edge::Edge;
use hangar_mission::Store;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "dono-da-prova";
const ITEM: &str = "/api/mission/items/help/PM%3A1%2F2";

struct Bench { dir: tempfile::TempDir, app: axum::Router }

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let edge = Edge {
            trusted: TrustedHosts::parse("127.0.0.1"),
            auth: Auth::configured(TOKEN),
            diag: DiagClient::new("127.0.0.1:9".parse().unwrap(), String::new()).with_diary(Some(dir.path().join("diary"))),
        };
        let app = hangar_mission::http::router::<()>(edge, Arc::new(Store::new(dir.path().join("mission"))));
        Self { dir, app }
    }

    async fn raw(&self, method: &str, uri: &str, token: Option<&str>, body: Body) -> (StatusCode, Vec<u8>, Option<String>) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token { request = request.header("authorization", format!("Bearer {token}")); }
        let mut request = request.header("content-type", "application/json").body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo("127.0.0.1:40000".parse::<std::net::SocketAddr>().unwrap()));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let mime = response.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_owned);
        (status, to_bytes(response.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec(), mime)
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let (status, bytes, _) = self.raw(method, uri, Some(TOKEN), body).await;
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn link(&self, session: &str) -> Value {
        let (status, body) = self.send("PUT", ITEM, Some(json!({"key": "PM-1", "title": "Imprimir", "url": "https://help.invalid/pm/PM-1", "session": session}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["item"].clone()
    }

    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_server_offers_the_default_stages_and_refuses_strangers() {
    let bench = Bench::new();
    let (status, body) = bench.send("GET", "/api/mission", None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body["stages"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["planejamento", "execucao", "revisao", "validacao", "entrega"]);
    assert_eq!(body["items"], json!([]));
    for token in [None, Some("errado")] {
        let (status, _, _) = bench.raw("GET", "/api/mission", token, Body::empty()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_moves_stages_but_waits_for_the_owner_at_the_gate() {
    let bench = Bench::new();
    let item = bench.link("pm-1").await;
    assert_eq!((item["key"].clone(), item["session"].clone()), (json!("PM-1"), json!("pm-1")));

    let report = |stage: &str| json!({"session": "pm-1", "stage": stage, "note": format!("indo para {stage}")});
    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("validacao"))).await;
    assert_eq!((status, body["item"]["stage"].clone()), (StatusCode::OK, json!("validacao")), "entrar no portão é livre");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("entrega"))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("mission_gate_pending")), "{body}");

    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "approve"}))).await;
    assert_eq!((status, body["item"]["stage"].clone()), (StatusCode::OK, json!("entrega")), "aprovar leva à etapa seguinte");
    assert_eq!(body["item"]["approved"], json!(["validacao"]));

    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "changes", "note": "o print não mostra o botão"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["item"]["stage"], json!("validacao"), "recusar volta uma etapa");
    assert_eq!(body["item"]["approved"], json!([]), "voltar para o portão fecha o portão");
    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("entrega"))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("mission_gate_pending")), "{body}");
    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "changes", "note": "ainda falta"}))).await;
    assert_eq!((status, body["item"]["stage"].clone(), body["item"]["approved"].clone()), (StatusCode::OK, json!("revisao"), json!([])));

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "outra", "stage": "execucao"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("mission_session_not_linked")));
    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1", "stage": "inventada"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
}

#[tokio::test(flavor = "multi_thread")]
async fn evidence_is_copied_served_and_removed_with_the_ticket() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let png = bench.file("tela.png", b"\x89PNG prova");
    let video = bench.file("fluxo.mp4", b"mp4 de prova");
    let script = bench.file("rodar.sh", b"echo");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1",
        "evidence": [{"path": png, "caption": "tela de impressão"}, {"path": video}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evidence = body["item"]["evidence"].as_array().unwrap().clone();
    assert_eq!(evidence.len(), 2);
    assert_eq!((evidence[0]["mime"].clone(), evidence[0]["caption"].clone(), evidence[0]["by"].clone()),
        (json!("image/png"), json!("tela de impressão"), json!("pm-1")));
    std::fs::remove_file(&png).unwrap();

    let id = evidence[0]["id"].as_str().unwrap();
    let (status, bytes, mime) = bench.raw("GET", &format!("/api/mission/evidence/{id}"), Some(TOKEN), Body::empty()).await;
    assert_eq!((status, bytes.as_slice(), mime.as_deref()), (StatusCode::OK, &b"\x89PNG prova"[..], Some("image/png")), "a cópia sobrevive ao original");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1", "evidence": [{"path": script}]}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")), "só imagem, vídeo, PDF ou texto");
    let (status, _) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1", "evidence": [{"path": "relativo.png"}]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, bytes, _) = bench.raw("POST", &format!("{ITEM}/evidence?name=gravado.webm&caption=do%20dono"), Some(TOKEN), Body::from("webm")).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["item"]["evidence"][2]["by"], json!("dono"));

    let (status, _) = bench.send("DELETE", ITEM, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = bench.raw("GET", &format!("/api/mission/evidence/{id}"), Some(TOKEN), Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let left = std::fs::read_dir(bench.dir.path().join("mission/evidence")).unwrap().count();
    assert_eq!(left, 0, "apagar o ticket leva os arquivos");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_session_one_ticket_and_custom_stages_drop_what_left() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let (status, _) = bench.send("PUT", "/api/mission/items/help/PM-2", Some(json!({"key": "PM-2", "title": "", "url": "https://help.invalid/pm/PM-2", "session": "pm-1"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = bench.send("GET", "/api/mission", None).await;
    let sessions: Vec<Value> = body["items"].as_array().unwrap().iter().map(|i| i["session"].clone()).collect();
    assert_eq!(sessions, [Value::Null, json!("pm-1")], "a sessão sai do ticket anterior");

    bench.send("PATCH", "/api/mission/items/help/PM-2", Some(json!({"action": "stage", "stage": "revisao"}))).await;
    let stages = json!({"stages": [{"id": "fazer", "name": "Fazer"}, {"id": "conferir", "name": "Conferir", "gate": true, "statuses": ["Em Testes"]}]});
    let (status, body) = bench.send("PUT", "/api/mission/stages", Some(stages)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][1]["stage"], Value::Null, "etapa que saiu volta a ser deduzida");
    assert_eq!(body["stages"][1]["statuses"], json!(["Em Testes"]));

    for bad in [json!({"stages": []}), json!({"stages": [{"id": "Maiúscula", "name": "x"}]}), json!({"stages": [{"id": "a", "name": "x"}, {"id": "a", "name": "y"}]})] {
        let (status, body) = bench.send("PUT", "/api/mission/stages", Some(bad)).await;
        assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
    }
}
