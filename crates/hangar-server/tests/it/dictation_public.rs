//! Paridade da borda pública com a caracterização Python congelada.
use axum::{
    Router,
    routing::{get, post},
};
use hangar_server::{
    auth::TrustedHosts,
    config::Config,
    routes::{AppState, router},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

#[tokio::test]
async fn dictation_public_characterized_contract() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    let configuration = Arc::new(Mutex::new(
        json!({"organization":{"dictation_organization_mode":"none"}}),
    ));
    let config = configuration.clone();
    let upstream_task = tokio::spawn(async move {
        axum::serve(
            upstream,
            Router::new()
                .route(
                    "/internal/transcription/config",
                    get(move || {
                        let data = config.lock().unwrap().clone();
                        async move { ([("content-type", "application/json")], data.to_string()) }
                    }),
                )
                .route(
                    "/v1/audio/transcriptions",
                    post(|| async {
                        (
                            [("content-type", "application/json")],
                            json!({"text":"Fala original."}).to_string(),
                        )
                    }),
                ),
        )
        .await
        .unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: address,
        internal_secret: "fixture-private".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.hangar_dir = temp.path().to_owned();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/dictation_public/responses.json"
    ))
    .unwrap();
    let client = reqwest::Client::new();
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        configuration.lock().unwrap()["providers"] = if case["path"]
            .as_str()
            .unwrap()
            .contains("limpar=0")
        {
            json!([{"id":"fixture","name":"fixture","kind":"openai","base_url":format!("http://{address}/v1"),"model":"fixture"}])
        } else {
            json!([])
        };
        let mut request = client
            .request(
                case["method"].as_str().unwrap().parse().unwrap(),
                format!("http://{addr}{}", case["path"].as_str().unwrap()),
            )
            .bearer_auth("fixture-owner");
        if !case["body"].is_null() {
            request = request.json(&case["body"]);
        } else if case["method"] == "POST" {
            request = request.body("audio");
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        if status != case["status"].as_u64().unwrap() as u16 || body != case["response"] {
            differences.push(json!({"path":case["path"],"input":case["body"],"status":status,"body":body,"expected":case["response"]}));
        }
    }
    server.abort();
    upstream_task.abort();
    let _ = server.await;
    let _ = upstream_task.await;
    assert!(
        differences.is_empty(),
        "diferenças de contrato: {differences:#?}"
    );
}

struct PublicServer {
    _directory: tempfile::TempDir,
    state: Arc<AppState>,
    base: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for PublicServer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl PublicServer {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(upstream,Router::new().route("/internal/transcription/config",get(||async {
            ([("content-type","application/json")],json!({"providers":[],"organization":{"llm_api_key":"fixture","llm_base_url":"http://127.0.0.1:1","dictation_organization_mode":"none"}}).to_string())
        }))).await.unwrap();
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: addr,
            upstream: upstream_addr,
            internal_secret: "private".into(),
            auth_token: "owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.hangar_dir = directory.path().to_owned();
        #[cfg(unix)]
        {
            state.share = Some(crate::share_pair_routes::share_state(
                &directory.path().join("shares.json"),
                directory.path(),
                Arc::new(Mutex::new(100.)),
            ));
        }
        let state = Arc::new(state);
        let app = router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            _directory: directory,
            state,
            base: format!("http://{addr}"),
            tasks: vec![task, server],
        }
    }
    async fn revise(&self, body: Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!("{}/api/ditado/relimpar", self.base))
            .bearer_auth("owner")
            .json(&body)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
}

#[tokio::test]
async fn dictation_public_limits_and_slots_preserve_raw() {
    let server = PublicServer::start().await;
    let raw = "á".repeat(1024 * 1024 / 2);
    let (status, body) = server
        .revise(json!({"texto":raw,"estilo":"prosa","organization_mode":"none"}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["text"], raw);
    assert_eq!(body["raw"], raw);
    assert_eq!(body["organization_code"], "dictation_text_too_large");
    let slot = server
        .state
        .dictation_slots
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    let raw = "uma frase com palavras suficientes para organizar";
    let (status, body) = server
        .revise(json!({"texto":raw,"estilo":"prosa","organization_mode":"external_api"}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["text"], raw);
    assert_eq!(body["raw"], raw);
    assert_eq!(body["organization_code"], "dictation_busy");
    let (_, body) = server
        .revise(json!({"texto":raw,"estilo":"prosa","organization_mode":"none"}))
        .await;
    assert_eq!(body["organization_code"], Value::Null);
    assert_eq!(body["text"], raw);
    drop(slot);
    let _slot = server
        .state
        .transcription_slots
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    for (method, path) in [
        ("POST", "/api/dictation/transcribe?limpar=0"),
        ("GET", "/api/transcription/providers/status"),
        ("POST", "/api/transcription/providers/p/test"),
    ] {
        let response = reqwest::Client::new()
            .request(method.parse().unwrap(), format!("{}{path}", server.base))
            .bearer_auth("owner")
            .body("audio")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(
            response.json::<Value>().await.unwrap()["detail"]["code"],
            "transcription_busy"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn dictation_public_invite_and_external_pair_authorization() {
    use hangar_server::share::{gate::GuestConn, store::Kind};
    let mut server = PublicServer::start().await;
    let share = server.state.share.as_ref().unwrap();
    let (_, invite) = share
        .store
        .create_redeemed("s0", Some("t:1".into()), Kind::Share)
        .unwrap();
    let (_, pair) = share
        .store
        .create_redeemed("s0", Some("t:1".into()), Kind::Pair)
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = hangar_server::routes::guest_router(server.state.clone());
    server.tasks.push(tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<GuestConn>(),
        )
        .await
        .unwrap();
    }));
    let client = reqwest::Client::new();
    let raw = "uma frase com palavras suficientes para organizar";
    for (token, session, status, code) in [
        (&invite, "s0", 200, "dictation_harness_unauthorized"),
        (&invite, "s1", 403, ""),
        (&pair, "s0", 403, ""),
    ] {
        let response=client.post(format!("http://{addr}/api/ditado/relimpar")).bearer_auth(token).json(&json!({"texto":raw,"estilo":"prosa","session":session,"organization_mode":"harness"})).send().await.unwrap();
        assert_eq!(response.status(), status);
        let body: Value = response.json().await.unwrap();
        if status == 200 {
            assert_eq!(body["text"], raw);
            assert_eq!(body["organization_code"], code);
        }
    }
    for path in [
        "/api/dictation/models?session=s0",
        "/api/transcription/providers/status",
    ] {
        assert_eq!(
            client
                .get(format!("http://{addr}{path}"))
                .bearer_auth(&invite)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
}

#[test]
fn dictation_public_generated_json_never_panics() {
    use proptest::{
        prelude::*,
        test_runner::{Config as PropertyConfig, RngSeed, TestRunner},
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(PublicServer::start());
    let input = prop_oneof![
        any::<bool>().prop_map(|v| json!(v)),
        any::<i64>().prop_map(|v| json!(v)),
        ".{0,128}".prop_map(|v| json!(v)),
        Just(Value::Null)
    ];
    TestRunner::new(PropertyConfig{cases:64,rng_seed:RngSeed::Fixed(0xD1C7A710),failure_persistence:None,..PropertyConfig::default()}).run(&(input.clone(),input.clone(),input),|(text,style,extra)|{
        let (status,body)=runtime.block_on(server.revise(json!({"texto":text,"estilo":style,"recent_messages":extra,"organization_mode":"none"})));
        prop_assert!([200,400,422].contains(&status));prop_assert!(body.is_object());Ok(())
    }).unwrap();
}

#[tokio::test]
async fn dictation_public_forwarded_authentication_and_guest_harness_limit() {
    let mut server = PublicServer::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = hangar_server::routes::terminal_router(server.state.clone());
    server.tasks.push(tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    }));
    let client = reqwest::Client::new();
    let raw = "uma frase com palavras suficientes para organizar";
    let request = || {
        client
            .post(format!("http://{addr}/__hangar_server/forwarded"))
            .header("x-hangar-path", "/api/ditado/relimpar")
            .json(&json!({"texto":raw,"estilo":"prosa","organization_mode":"harness"}))
    };
    assert_eq!(request().send().await.unwrap().status(), 404);
    let response = request()
        .header("x-hangar-internal", "private")
        .header("x-hangar-forwarded-guest", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["text"], raw);
    assert_eq!(body["organization_code"], "dictation_harness_unauthorized");
}
