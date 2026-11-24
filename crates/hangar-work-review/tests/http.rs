//! Rotas do dono: autorização antes de tudo, forma das respostas e mapa de erros.
mod common;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use common::{Bench, review};
use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::edge::Edge;
use hangar_core::guest::{Caller, GuestGrant, Kind};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "dono-da-prova";

struct Routes { bench: Bench, app: axum::Router }

fn router(bench: &Bench, engine: Arc<hangar_work_review::Engine>) -> axum::Router {
    let edge = Edge { trusted: TrustedHosts::parse("127.0.0.1"), auth: Auth::configured(TOKEN),
        diag: DiagClient::new().with_diary(Some(bench.path("diary"))) };
    hangar_work_review::http::router::<()>(edge, engine)
}

impl Routes {
    fn new() -> Self {
        let bench = Bench::new();
        let app = router(&bench, bench.engine.clone());
        Self { bench, app }
    }

    async fn raw(&self, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut request = request;
        request.extensions_mut().insert(ConnectInfo("127.0.0.1:40000".parse::<std::net::SocketAddr>().unwrap()));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        (status, headers, to_bytes(response.into_body(), 8 * 1024 * 1024).await.unwrap().to_vec())
    }

    async fn send(&self, method: &str, uri: &str, token: Option<&str>, body: Option<String>) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token { request = request.header("authorization", format!("Bearer {token}")); }
        let (status, _, bytes) = self.raw(request.body(body.map_or_else(Body::empty, Body::from)).unwrap()).await;
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn linked(&self) {
        self.bench.connector("help", true, &[]);
        self.bench.set_review("help", review("r-1", "waiting", &["47"], "2026-11-18T10:00:00Z"));
        let session = self.bench.session("s1", "life-1", None);
        self.bench.link("help", "1", &session);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_and_wrong_token_never_reach_the_connector() {
    let routes = Routes::new();
    routes.linked();
    for token in [None, Some("errado")] {
        let (status, _) = routes.send("GET", "/api/sessions/s1/work-review", token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let grant = GuestGrant { token: "convite".into(), token_hash: "h".into(), sessions: [("s1".to_owned(), Kind::Share)].into(), pair_roots: Vec::new(), invite: true };
    let mut request = Request::builder().method("POST").uri("/api/sessions/s1/work-review/decisions").header("authorization", "Bearer convite")
        .body(Body::from(json!({"review_id": "r-1", "decisions": [{"id": "47", "choice": "igualar"}]}).to_string())).unwrap();
    request.extensions_mut().insert(Caller::Guest(Arc::new(grant)));
    let (status, _, _) = routes.raw(request).await;
    assert!(status.is_client_error(), "convidado recusado: {status}");
    assert!(routes.bench.calls("help", "work_review.get").is_empty() && routes.bench.calls("help", "work_review.decide").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_or_unlinked_session_is_linked_false() {
    let routes = Routes::new();
    routes.bench.session("s1", "life-1", None);
    for name in ["s1", "desconhecida"] {
        let (status, body) = routes.send("GET", &format!("/api/sessions/{name}/work-review"), Some(TOKEN), None).await;
        assert_eq!((status, body), (StatusCode::OK, json!({"linked": false})));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unreadable_session_is_an_error_not_unlinked() {
    let routes = Routes::new();
    let (status, body) = routes.send("GET", "/api/sessions/ilegivel-1/work-review", Some(TOKEN), None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::SERVICE_UNAVAILABLE, json!("unavailable")), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_session_shows_the_review_and_serves_the_asset() {
    let routes = Routes::new();
    routes.linked();
    let (status, body) = routes.send("GET", "/api/sessions/s1/work-review", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((&body["linked"], &body["via"], &body["connector"], &body["ticket_id"], &body["key"], &body["supported"]),
        (&json!(true), &json!("ticket"), &json!("help"), &json!("1"), &json!("PM-1"), &json!(true)));
    assert_eq!(body["review"]["id"], "r-1");
    assert!(body["fetched_at"].is_string());
    let request = Request::builder().uri("/api/sessions/s1/work-review/assets/a%3A1?review_id=r-1")
        .header("authorization", format!("Bearer {TOKEN}")).body(Body::empty()).unwrap();
    let (status, headers, bytes) = routes.raw(request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=86400, immutable");
    assert!(bytes.starts_with(b"\x89PNG"));
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_answer_ok_refuse_stale_and_oversized_bodies() {
    let routes = Routes::new();
    routes.linked();
    let uri = "/api/sessions/s1/work-review/decisions";
    let stale = json!({"review_id": "r-0", "decisions": [{"id": "47", "choice": "igualar"}]}).to_string();
    let (status, body) = routes.send("POST", uri, Some(TOKEN), Some(stale)).await;
    assert_eq!((status, &body["detail"]["code"]), (StatusCode::CONFLICT, &json!("stale_review")), "{body}");
    let missing_reason = json!({"review_id": "r-1", "decisions": [{"id": "47", "choice": "manter"}]}).to_string();
    let (status, body) = routes.send("POST", uri, Some(TOKEN), Some(missing_reason)).await;
    assert_eq!((status, &body["detail"]["code"]), (StatusCode::BAD_REQUEST, &json!("invalid_params")), "{body}");
    let huge = json!({"review_id": "r-1", "decisions": [{"id": "47", "choice": "igualar", "reason": "x".repeat(600 * 1024)}]}).to_string();
    let (status, _) = routes.send("POST", uri, Some(TOKEN), Some(huge)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(routes.bench.calls("help", "work_review.decide").is_empty());
    let good = json!({"review_id": "r-1", "decisions": [{"id": "47", "choice": "igualar"}]}).to_string();
    let (status, body) = routes.send("POST", uri, Some(TOKEN), Some(good)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((&body["ok"], &body["review"]["id"]), (&json!(true), &json!("r-1")));
}

/// O que a pessoa marcou e não enviou volta na leitura seguinte, também com o servidor recriado.
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_draft_comes_back_until_the_round_changes() {
    let mut routes = Routes::new();
    routes.bench.connector("help", true, &[]);
    routes.bench.set_review("help", review("r-1", "waiting", &["47", "86"], "2026-11-18T10:00:00Z"));
    let session = routes.bench.session("s1", "life-1", None);
    routes.bench.link("help", "1", &session);
    let (uri, read) = ("/api/sessions/s1/work-review/draft", "/api/sessions/s1/work-review");
    let (status, body) = routes.send("GET", read, Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let draft = json!({"review_id": "r-1", "choices": {"47": {"choice": "manter", "reason": "regra do Delphi"},
        "86": {"choice": "igualar", "reason": ""}, "99": {"choice": "igualar", "reason": ""}}});
    let (status, body) = routes.send("PUT", uri, Some(TOKEN), Some(draft.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expected = json!({"47": {"choice": "manter", "reason": "regra do Delphi"}, "86": {"choice": "igualar", "reason": ""}});
    let (_, body) = routes.send("GET", read, Some(TOKEN), None).await;
    assert_eq!(body["draft"], expected, "a decisão que a rodada não tem fica de fora: {body}");
    // Outro aparelho que só conhece a 86 não apaga a 47: o servidor junta por decisão.
    let other = json!({"review_id": "r-1", "choices": {"86": {"choice": "manter", "reason": "do celular"}}});
    let (status, body) = routes.send("PUT", uri, Some(TOKEN), Some(other.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expected = json!({"47": {"choice": "manter", "reason": "regra do Delphi"}, "86": {"choice": "manter", "reason": "do celular"}});
    let (_, body) = routes.send("GET", read, Some(TOKEN), None).await;
    assert_eq!(body["draft"], expected, "juntado por decisão: {body}");
    routes.app = router(&routes.bench, hangar_work_review::Engine::new(routes.bench.host.clone(), routes.bench.store.clone(),
        routes.bench.sessions.clone(), routes.bench.diag.clone(), routes.bench.path("cache"), routes.bench.path("drafts")));
    let (_, body) = routes.send("GET", read, Some(TOKEN), None).await;
    assert_eq!(body["draft"], expected, "servidor recriado: {body}");
    let stale = json!({"review_id": "r-0", "choices": {}}).to_string();
    let (status, body) = routes.send("PUT", uri, Some(TOKEN), Some(stale)).await;
    assert_eq!((status, &body["detail"]["code"]), (StatusCode::CONFLICT, &json!("stale_review")), "{body}");
    routes.bench.set_review("help", review("r-2", "waiting", &["47"], "2026-11-18T11:00:00Z"));
    let (_, body) = routes.send("GET", read, Some(TOKEN), None).await;
    assert_eq!((&body["review"]["id"], body.get("draft")), (&json!("r-2"), None), "rodada nova começa sem escolha: {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn asset_without_review_id_is_a_bad_request() {
    let routes = Routes::new();
    routes.linked();
    let (status, body) = routes.send("GET", "/api/sessions/s1/work-review/assets/a1", Some(TOKEN), None).await;
    assert_eq!((status, &body["detail"]["code"]), (StatusCode::BAD_REQUEST, &json!("invalid_params")), "{body}");
    assert!(routes.bench.calls("help", "work_review.asset").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn asset_without_link_is_not_found() {
    let routes = Routes::new();
    routes.bench.session("s1", "life-1", None);
    let (status, body) = routes.send("GET", "/api/sessions/s1/work-review/assets/a1?review_id=r-1", Some(TOKEN), None).await;
    assert_eq!((status, &body["detail"]["code"]), (StatusCode::NOT_FOUND, &json!("work_review_not_linked")));
}
