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
        if let Some(raw) = case["raw_body"].as_str() {
            request = request.body(raw.to_owned());
        }
        if let Some(headers) = case["headers"].as_object() {
            for (name, value) in headers {
                request = request.header(name, value.as_str().unwrap());
            }
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
    config: Arc<Mutex<Value>>,
    heard: Arc<Mutex<Vec<(String, String)>>>,
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
        let config = Arc::new(Mutex::new(
            json!({"providers":[],"organization":{"llm_api_key":"fixture","llm_base_url":"http://127.0.0.1:1","dictation_organization_mode":"none"}}),
        ));
        let heard: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let configuration = config.clone();
        let seen = heard.clone();
        let task = tokio::spawn(async move {
            axum::serve(upstream,Router::new().route("/internal/transcription/config",get(move || {
                let value=configuration.lock().unwrap().clone();async move { ([("content-type","application/json")],value.to_string()) }
            })).fallback(move |uri:axum::http::Uri,body:axum::body::Bytes| {
                seen.lock().unwrap().push((uri.path().to_owned(),String::from_utf8_lossy(&body).into_owned()));
                async move {
                    if uri.path().starts_with("/stall/"){std::future::pending::<()>().await;}
                    let status=if uri.path().starts_with("/first/"){axum::http::StatusCode::INTERNAL_SERVER_ERROR}else{axum::http::StatusCode::OK};
                    (status,[("content-type","application/json")],json!({"text":"Hoje vamos conferir o ditado completo.","choices":[{"message":{"content":"Hoje vamos conferir o ditado completo."}}]}).to_string())
                }
            })).await.unwrap();
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
            config,
            heard,
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

#[tokio::test]
async fn dictation_public_audio_edges_match_python() {
    let server = PublicServer::start().await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/dictation_public/edges.json"
    ))
    .unwrap();
    let endpoint = format!("http://{}", server.state.cfg.upstream);
    let before = directory_snapshot(server._directory.path());
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap().iter().filter(|c| {
        !c["id"].as_str().unwrap().ends_with("limit")
            && !c["id"].as_str().unwrap().ends_with("_timeout")
    }) {
        server.heard.lock().unwrap().clear();
        let mut providers = vec![
            json!({"id":"fixture","name":"fixture","kind":"openai","base_url":endpoint,"model":"fixture"}),
        ];
        if case["id"] == "warnings" {
            providers.insert(0,json!({"id":"first","name":"primeiro","kind":"openai","base_url":format!("{endpoint}/first"),"model":"fixture"}));
        }
        if case["id"] == "stt_error" {
            providers.clear();
        }
        *server.config.lock().unwrap() =
            json!({"providers":providers,"organization":{"dictation_organization_mode":"none"}});
        let mut request = reqwest::Client::new()
            .post(format!("{}{}", server.base, case["path"].as_str().unwrap()))
            .bearer_auth("owner")
            .body("audio");
        for (name, value) in case["headers"].as_object().unwrap() {
            request = request.header(name, value.as_str().unwrap());
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap();
        if json!(status) != case["status"] || body != case["response"] {
            differences.push(
                json!({"id":case["id"],"status":status,"actual":body,"expected":case["response"]}),
            );
        }
        if let Some(filename) = case["calls"][0]["query"]["filename"].as_str() {
            let heard = server.heard.lock().unwrap();
            assert!(
                heard.iter().any(|(_, body)| body.contains(&format!(
                    "filename=\"audio.{}\"",
                    filename.rsplit('.').next().unwrap()
                ))),
                "{}: {heard:?}",
                case["id"]
            );
        }
        assert!(body.get("path").is_none());
        assert_eq!(
            directory_snapshot(server._directory.path()),
            before,
            "o ditado global gravou um arquivo"
        );
    }
    assert!(differences.is_empty(), "{differences:#?}");
}

#[tokio::test]
async fn dictation_public_declared_audio_limit_matches_python() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = PublicServer::start().await;
    let mut stream = tokio::net::TcpStream::connect(server.base.trim_start_matches("http://"))
        .await
        .unwrap();
    stream.write_all(format!("POST /api/dictation/transcribe HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer owner\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",100*1024*1024+1).as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(
        response.contains("content-type: text/plain; charset=utf-8"),
        "{response}"
    );
    assert!(server.heard.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dictation_public_provider_test_selects_only_requested_provider() {
    let server = PublicServer::start().await;
    let endpoint = format!("http://{}", server.state.cfg.upstream);
    *server.config.lock().unwrap() = json!({"providers":[{"id":"first","name":"primeiro","kind":"openai","base_url":format!("{endpoint}/first")},{"id":"second","name":"segundo","kind":"openai","base_url":endpoint}],"legacy":{"id":"legacy","kind":"openai","base_url":endpoint}});
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "{}/api/transcription/providers/first/test",
            server.base
        ))
        .bearer_auth("owner")
        .body("audio")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"]["code"],
        "transcription_provider_failed"
    );
    let heard = server.heard.lock().unwrap().clone();
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].0, "/first/audio/transcriptions");
    assert!(heard[0].1.contains("filename=\"audio.wav\""));
    server.heard.lock().unwrap().clear();
    let response = client
        .post(format!(
            "{}/api/transcription/providers/second/test",
            server.base
        ))
        .header("x-filename", "chosen.webm")
        .bearer_auth("owner")
        .body("audio")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["provider"],
        "segundo"
    );
    let heard = server.heard.lock().unwrap();
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].0, "/audio/transcriptions");
    assert!(heard[0].1.contains("filename=\"audio.webm\""));
}

#[tokio::test]
async fn dictation_public_without_cleanup_keeps_dictation_deadline() {
    let server = PublicServer::start().await;
    *server.config.lock().unwrap() = json!({"dictation_max_minutes":2,"providers":[{"id":"stall","name":"stall","kind":"openai","base_url":format!("http://{}/stall",server.state.cfg.upstream)}]});
    let url = format!("{}/api/dictation/transcribe?limpar=0", server.base);
    let response = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .bearer_auth("owner")
            .body("audio")
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while server.heard.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), response)
        .await
        .expect("a rota global usou o prazo de arquivo (120 s), não o ditado (60 s)")
        .unwrap();
    assert_eq!(response.status(), 504);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"]["code"],
        "transcription_network_failed"
    );
    tokio::time::resume();
}

fn directory_snapshot(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(path: &std::path::Path, output: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            output.push(entry.path());
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), output);
            }
        }
    }
    let mut paths = Vec::new();
    visit(root, &mut paths);
    paths.sort();
    paths
}

#[tokio::test]
async fn dictation_public_global_does_not_inherit_a_session_from_query() {
    let server = PublicServer::start().await;
    let endpoint = format!("http://{}", server.state.cfg.upstream);
    *server.config.lock().unwrap() = json!({"providers":[{"id":"p","kind":"openai","name":"fixture","base_url":endpoint}],"organization":{"dictation_organization_mode":"external_api","llm_base_url":endpoint,"llm_api_key":"fixture","llm_model":"organization-model"}});
    let response=reqwest::Client::new().post(format!("{}/api/dictation/transcribe?organization_mode=external_api&include_recent_messages=true&session=inexistente&generation=outra&organization_account=outra",server.base)).bearer_auth("owner").body("audio").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["raw"], "Hoje vamos conferir o ditado completo.");
    assert!(
        body["organization_code"].is_null(),
        "a sessão indevida acionou a leitura de referências: {body}"
    );
    let heard = server.heard.lock().unwrap();
    assert_eq!(
        heard
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["/audio/transcriptions", "/chat/completions"]
    );
    let prompt: Value = serde_json::from_str(&heard[1].1).unwrap();
    assert_eq!(prompt["model"], "organization-model");
    assert!(!heard[1].1.contains("inexistente"));
}

#[tokio::test]
async fn dictation_public_chunked_limits_preserve_original_errors() {
    let server = PublicServer::start().await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/dictation_public/edges.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["transport"] == "chunked")
    {
        let chunk = bytes::Bytes::from(vec![b'x'; 1024 * 1024]);
        let stream = futures_util::stream::iter(
            (0..101).map(move |_| Ok::<_, std::io::Error>(chunk.clone())),
        );
        let response = reqwest::Client::new()
            .post(format!("{}{}", server.base, case["path"].as_str().unwrap()))
            .bearer_auth("owner")
            .header("content-type", "application/json")
            .body(reqwest::Body::wrap_stream(stream))
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        if json!(status) != case["status"] || body != case["response"] {
            differences.push(
                json!({"id":case["id"],"actual":body,"status":status,"expected":case["response"]}),
            );
        }
        assert!(server.heard.lock().unwrap().is_empty());
    }
    assert!(differences.is_empty(), "{differences:#?}");
}

#[tokio::test]
async fn dictation_public_180_second_deadline_cancels_both_audio_routes() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/dictation_public/edges.json"
    ))
    .unwrap();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["timeout_seconds"] == 180)
    {
        let server = PublicServer::start().await;
        let endpoint = format!("http://{}", server.state.cfg.upstream);
        *server.config.lock().unwrap() = json!({"dictation_max_minutes":10,"providers":[{"id":"fixture","kind":"openai","base_url":format!("{endpoint}/stall")}],"organization":{"dictation_organization_mode":"none"}});
        let url = format!("{}{}", server.base, case["path"].as_str().unwrap());
        let response = tokio::spawn(async move {
            reqwest::Client::new()
                .post(url)
                .bearer_auth("owner")
                .body("audio")
                .send()
                .await
                .unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while server.heard.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(181)).await;
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), response)
            .await
            .expect("a rota deixou passar o teto público de180s")
            .unwrap();
        assert_eq!(json!(response.status().as_u16()), case["status"]);
        assert_eq!(response.json::<Value>().await.unwrap(), case["response"]);
        assert_eq!(
            server.state.transcription_slots.available_permits(),
            4,
            "a tentativa vencida reteve sua vaga"
        );
        tokio::time::resume();
        server.config.lock().unwrap()["providers"][0]["base_url"] = json!(endpoint);
        let response = reqwest::Client::new()
            .post(format!("{}{}", server.base, case["path"].as_str().unwrap()))
            .bearer_auth("owner")
            .body("audio")
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "a tentativa seguinte ficou presa à anterior"
        );
    }
}
