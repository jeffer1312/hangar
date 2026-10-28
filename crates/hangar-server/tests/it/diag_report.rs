//! `POST /api/diag/report` com um worker falso: só o dono envia, o corpo do C1 é conferido, o código
//! do worker volta ao cliente, o 429 vira `diag_report_rate_limited` e o corpo enviado cabe no teto.
//! O diário inteiro sai da pasta de logs do processo (`log_base`): sem diário, `diary` é `failed`.
use axum::{Router, body::Bytes, extract::State, http::{HeaderMap, StatusCode}, routing::{post, put}};
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const OWNER: &str = "diag-owner";
const BODY_MAX: usize = 250 * 1024;

/// O que o worker falso recebeu e com que status responde ao relatório.
struct Worker {
    status: Mutex<u16>,
    erro: Mutex<&'static str>,
    reports: Mutex<Vec<(HeaderMap, Bytes)>>,
    diaries: Mutex<Vec<(HeaderMap, Bytes)>>,
}

async fn report(State(w): State<Arc<Worker>>, headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    w.reports.lock().unwrap().push((headers, body));
    let status = *w.status.lock().unwrap();
    let reply = if status == 201 { json!({"id": "diag/abcd2345", "code": "abcd2345", "upload": "u".repeat(32)}) } else { json!({"erro": *w.erro.lock().unwrap()}) };
    (StatusCode::from_u16(status).unwrap(), reply.to_string())
}

async fn diary(State(w): State<Arc<Worker>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    w.diaries.lock().unwrap().push((headers, body));
    StatusCode::CREATED
}

struct Fixture { base: String, worker: Arc<Worker>, _home: tempfile::TempDir, tasks: Vec<tokio::task::JoinHandle<()>> }

impl Drop for Fixture { fn drop(&mut self) { for t in &self.tasks { t.abort(); } } }

async fn serve(router: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); }))
}

impl Fixture {
    async fn start(status: u16) -> Self { Self::with_base(status, None).await }

    /// `base` troca o endereço do worker (porta fechada = worker fora do ar).
    async fn with_base(status: u16, base: Option<String>) -> Self {
        let home = tempfile::tempdir().unwrap();
        let worker = Arc::new(Worker { status: Mutex::new(status), erro: Mutex::new("x"), reports: Mutex::default(), diaries: Mutex::default() });
        let (worker_addr, worker_task) = serve(Router::new().route("/api/relatorio", post(report))
            .route("/api/relatorio/diario", put(diary)).with_state(worker.clone())).await;
        let (upstream, python) = serve(Router::new().fallback(|| async { "python" })).await;
        let mut state = AppState::new(Config {
            listen: "127.0.0.1:0".parse().unwrap(), upstream, internal_secret: "diag-internal".into(),
            auth_token: OWNER.into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into()));
        state.diag_report_base = base.unwrap_or_else(|| format!("http://{worker_addr}"));
        let (addr, server) = serve(hangar_server::routes::router(Arc::new(state))).await;
        Fixture { base: format!("http://{addr}"), worker, _home: home, tasks: vec![worker_task, python, server] }
    }

    async fn post(&self, owner: bool, headers: &[(&str, &str)], body: Value) -> (u16, Value) {
        let mut req = reqwest::Client::new().post(format!("{}/api/diag/report", self.base)).json(&body);
        if owner { req = req.bearer_auth(OWNER); }
        for (k, v) in headers { req = req.header(*k, *v); }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        (status, serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap_or(Value::Null))
    }
}

fn input() -> Value {
    json!({"text": "a fila travou", "client": "native 0.1.0 linux-x86_64",
        "failures": [{"ts": 1793130757.2, "route": "POST /api/sessions/*/queue/*/steer", "status": 503, "code": "runtime_unhealthy"},
                     {"ts": 1793130700.0, "route": "GET /api/sessions", "status": 0, "code": null}]})
}

fn code_of(body: &Value) -> &Value { &body["detail"]["code"] }

#[tokio::test(flavor = "multi_thread")]
async fn guest_and_stranger_are_refused_before_anything_is_sent() {
    let f = Fixture::start(201).await;
    let (status, body) = f.post(true, &[("x-hangar-guest", "1")], input()).await;
    assert_eq!((status, code_of(&body)), (403, &json!("diag_report_forbidden")));
    let (status, body) = f.post(false, &[], input()).await;
    assert_eq!((status, code_of(&body)), (401, &json!("unauthorized")));
    assert!(f.worker.reports.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn body_outside_the_contract_is_422() {
    let f = Fixture::start(201).await;
    let failure = |code: Value, route: &str| json!({"ts": 1.0, "route": route, "status": 500, "code": code});
    let bad = [
        json!({"client": "x"}),
        json!({"text": "x".repeat(2001), "client": "x", "failures": []}),
        json!({"client": "x".repeat(201), "failures": []}),
        json!({"client": "x", "failures": vec![failure(Value::Null, "/a"); 51]}),
        json!({"client": "x", "failures": [failure(json!("Maiúscula"), "/a")]}),
        json!({"client": "x", "failures": [failure(json!(""), "/a")]}),
        json!({"client": "x", "failures": [failure(Value::Null, &"r".repeat(201))]}),
        json!({"client": "x", "failures": [], "text": "x".repeat(70 * 1024)}),
    ];
    for body in bad {
        let (status, reply) = f.post(true, &[], body.clone()).await;
        assert_eq!((status, code_of(&reply)), (422, &json!("diag_report_bad_request")), "{}", &body.to_string()[..80.min(body.to_string().len())]);
    }
    assert!(f.worker.reports.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_gets_the_worker_code_and_the_body_fits() {
    let f = Fixture::start(201).await;
    let (status, body) = f.post(true, &[], input()).await;
    assert_eq!((status, &body["code"]), (201, &json!("abcd2345")), "{body}");

    let reports = f.worker.reports.lock().unwrap().clone();
    assert_eq!(reports.len(), 1);
    let (headers, sent) = &reports[0];
    assert!(sent.len() <= BODY_MAX, "corpo de {} bytes", sent.len());
    let stamp = headers["x-hangar-stamp"].to_str().unwrap();
    assert!(stamp.starts_with("hangar-server/") && stamp.len() > "hangar-server/".len(), "{stamp}");
    let report: Value = serde_json::from_slice(sent).unwrap();
    assert_eq!((&report["kind"], &report["outcome"]), (&json!("diagnostico"), &json!("aberto")));
    assert!(report["report"].as_str().unwrap().contains("runtime_unhealthy"));

    // Diário presente na máquina = um PUT com a senha e o código; ausente = nenhum PUT e `failed`.
    let diaries = f.worker.diaries.lock().unwrap().clone();
    match diaries.as_slice() {
        [] => assert_eq!(body["diary"], json!("failed")),
        [(h, gz)] => {
            assert_eq!(body["diary"], json!("sent"));
            assert_eq!((h["x-hangar-code"].to_str().unwrap(), h["x-hangar-upload"].to_str().unwrap()), ("abcd2345", "u".repeat(32).as_str()));
            assert_eq!(h["content-type"], "application/gzip");
            assert_eq!(&gz[..2], &[0x1f, 0x8b]);
        }
        many => panic!("{} envios do diário", many.len()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_429_becomes_rate_limited_and_other_status_refused() {
    let f = Fixture::start(429).await;
    let (status, body) = f.post(true, &[], input()).await;
    assert_eq!((status, code_of(&body)), (429, &json!("diag_report_rate_limited")));

    *f.worker.status.lock().unwrap() = 503;
    let (status, body) = f.post(true, &[], input()).await;
    assert_eq!((status, code_of(&body), &body["detail"]["params"]["status"]), (502, &json!("diag_report_refused"), &json!(503)));
    assert!(f.worker.diaries.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn group_goes_to_the_worker_and_its_refusals_have_their_own_codes() {
    let f = Fixture::start(201).await;
    let mut grouped = input();
    grouped["group"] = json!("abcd2345");
    assert_eq!(f.post(true, &[], grouped.clone()).await.0, 201);
    assert_eq!(f.post(true, &[], input()).await.0, 201);
    {
        let reports = f.worker.reports.lock().unwrap();
        assert_eq!(reports[0].0["x-hangar-group"], "abcd2345");
        assert!(!reports[1].0.contains_key("x-hangar-group"), "sem grupo, sem cabeçalho");
    }
    for bad in ["ABCD2345", "abcd234", "abcd2345/x"] {
        let mut body = input();
        body["group"] = json!(bad);
        assert_eq!(code_of(&f.post(true, &[], body).await.1), &json!("diag_report_bad_request"), "{bad}");
    }
    assert_eq!(f.worker.reports.lock().unwrap().len(), 2);

    for (status, erro, code) in [(429, "grupo-cheio", "diag_report_group_full"), (400, "grupo", "diag_report_group_unknown")] {
        *f.worker.status.lock().unwrap() = status;
        *f.worker.erro.lock().unwrap() = erro;
        let (got, body) = f.post(true, &[], grouped.clone()).await;
        assert_eq!((got, code_of(&body)), (409, &json!(code)), "{erro}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_down_is_unreachable() {
    let f = Fixture::with_base(201, Some("http://127.0.0.1:9".into())).await;
    let (status, body) = f.post(true, &[], input()).await;
    assert_eq!((status, code_of(&body)), (502, &json!("diag_report_unreachable")));
}
