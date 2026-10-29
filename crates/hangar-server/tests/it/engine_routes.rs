//! Escritas públicas do catálogo com upstream indisponível.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use serde_json::{Value, json};
use std::sync::Arc;
#[tokio::test]
async fn engine_crud_preserves_secrets_and_refuses_corruption() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                root.path().to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    ));
    let path = hangar_server::engines::store::path(&state.accounts.env);
    let app = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/engines/motor");
    let response=client.put(&url).bearer_auth("fixture-owner").json(&json!({"base_url":"https://provider.invalid/v1/","api_key":"fixture-secret","model":"m","vision":true})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_ne!(body["motores"]["motor"]["api_key"], "fixture-secret");
    let response = client
        .put(&url)
        .bearer_auth("fixture-owner")
        .json(&json!({"label":"Alterado","api_key":body["motores"]["motor"]["api_key"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["motor"]["api_key"], "fixture-secret");
    assert_eq!(saved["motor"]["vision"], true);
    let unauthorized = client
        .delete(&url)
        .bearer_auth("fixture-wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    std::fs::write(&path, "corrompido").unwrap();
    let response = client
        .delete(&url)
        .bearer_auth("fixture-owner")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrompido");
    std::fs::write(&path, saved.to_string()).unwrap();
    assert_eq!(
        client
            .delete(&url)
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .delete(&url)
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    task.abort();
}

#[tokio::test]
async fn engine_routes_keep_auth_rate_limit() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    ));
    state.auth.mark_blocked("192.0.2.55");
    let app = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    for path in [
        "/api/engines",
        "/api/model-options?provider=claude&engine=motor",
        "/api/sessions/fixture/model/options",
    ] {
        let response = client
            .get(format!("http://{address}{path}"))
            .header("x-forwarded-for", "192.0.2.55")
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 429, "{path}");
        assert_eq!(response.headers()["retry-after"], "30");
    }
    task.abort();
}

#[tokio::test]
async fn engine_model_http_matches_python_without_bridge() {
    use crate::session_write_control::{open_headless, policy, send};
    use hangar_server::runtime::gateway::RuntimeRegistry;
    let golden: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/engine-http.json"
    ))
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                root.path().to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = upstream_listener.local_addr().unwrap();
    let pause = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let models = axum::Router::new().route(
        "/v1/models",
        axum::routing::get({
            let (pause, started, release) = (pause.clone(), started.clone(), release.clone());
            move || {
                let (pause, started, release) = (pause.clone(), started.clone(), release.clone());
                async move {
                    if pause.load(std::sync::atomic::Ordering::SeqCst) {
                        started.notify_one();
                        release.notified().await;
                    }
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        json!({"data":[{"id":"k3"}]}).to_string(),
                    )
                }
            }
        }),
    );
    let provider_task = tokio::spawn(async move {
        axum::serve(upstream_listener, models).await.unwrap();
    });
    std::fs::create_dir_all(root.path().join(".claude")).unwrap();
    std::fs::write(root.path().join(".claude/engines.json"),json!({"motor":{"base_url":format!("http://{provider}"),"api_key":"fixture-key","model":"k3"}}).to_string()).unwrap();
    let registry = Arc::new(
        RuntimeRegistry::new(
            policy(Arc::new(std::sync::atomic::AtomicUsize::new(0))).await,
            "secret-test".into(),
            "instance-test".into(),
        )
        .with_codex_environment(env.clone())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))),
    );
    let sink = open_headless(&registry, root.path(), "fixture").await;
    let sidecar = root.path().join(".hangar/claude-headless/fixture.json");
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    let base = json!({"name":"fixture","key":"k-fixture","cwd":root.path(),"config_dir":root.path().join(".claude"),"session_id":"sid-1","engine":"motor","permission_mode":"bypassPermissions"});
    std::fs::write(&sidecar, base.to_string()).unwrap();
    let (python, upstream) = crate::fake::spawn_fake().await;
    let mut state = AppState::new(crate::fake::config(upstream, "127.0.0.1"));
    state.accounts = AccountService::new(env);
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let address = crate::fake::spawn_state(state).await;
    for (scenario, model) in [
        ("headless", "k3"),
        ("outside_catalog", "absent"),
        ("not_engine", "k3"),
        ("fast_refused", "k3"),
    ] {
        let mut meta = base.clone();
        if scenario == "not_engine" {
            meta.as_object_mut().unwrap().remove("engine");
        }
        if scenario == "fast_refused" {
            meta["service_tier"] = json!("priority");
        }
        std::fs::write(&sidecar, meta.to_string()).unwrap();
        let (status, body) = send(
            address,
            "POST",
            "fixture/engine/model",
            &json!({"model":model}).to_string(),
        )
        .await;
        let expected = &golden["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == scenario)
            .unwrap()["result"];
        assert_eq!(
            status,
            expected["status"].as_u64().unwrap() as u16,
            "{scenario}: {body}"
        );
        if scenario == "headless" {
            assert_eq!(body, expected["body"]);
        } else {
            assert_eq!(body["detail"]["code"], expected["detail"]["code"]);
        }
    }
    std::fs::write(&sidecar, base.to_string()).unwrap();
    for body in [
        "[]",
        "{}",
        r#"{"model":7}"#,
        r#"{"model":"k3","extra":true}"#,
        r#"{"model":"k3","effort":7}"#,
    ] {
        assert_eq!(
            send(address, "POST", "fixture/engine/model", body).await.0,
            422
        );
    }
    let bodies: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/model-body.json"
    ))
    .unwrap();
    for case in bodies["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["status"] != 200 && case["name"] != "body-18")
    {
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let response = reqwest::Client::new()
            .post(format!(
                "http://{address}/api/sessions/fixture/engine/model"
            ))
            .bearer_auth(crate::fake::OWNER)
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        assert_eq!(
            status,
            case["status"].as_u64().unwrap() as u16,
            "{}: {text}",
            case["name"]
        );
        if status == 500 {
            assert_eq!(text, case["body"].as_str().unwrap());
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap(),
                case["body"],
                "{}",
                case["name"]
            );
        }
    }
    for case in bodies["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["name"].as_str().unwrap().starts_with("utf-"))
    {
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let response = reqwest::Client::new()
            .post(format!(
                "http://{address}/api/sessions/fixture/engine/model"
            ))
            .bearer_auth(crate::fake::OWNER)
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{}: {body}", case["name"]);
        assert_eq!(body["model"], "k3");
    }
    let schemas: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/body-schemas.json"
    ))
    .unwrap();
    for case in schemas["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["schema"] == "EngineModelBody" && case["name"] == "content-type")
    {
        let mut request = reqwest::Client::new()
            .post(format!(
                "http://{address}/api/sessions/fixture/engine/model"
            ))
            .bearer_auth(crate::fake::OWNER)
            .body("{}");
        if let Some(content_type) = case["content_type"].as_str() {
            request = request.header("content-type", content_type);
        }
        let response = request.send().await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            case["status"].as_u64().unwrap() as u16
        );
        let body: Value = response.json().await.unwrap();
        assert_eq!(body, case["body"], "Content-Type {}", case["content_type"]);
    }
    let serialization: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/model-serialization.json"
    ))
    .unwrap();
    for case in serialization["cases"].as_array().unwrap() {
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let response = reqwest::Client::new()
            .post(format!(
                "http://{address}/api/sessions/fixture/engine/model"
            ))
            .bearer_auth(crate::fake::OWNER)
            .header("content-type", "application/json")
            .header("origin", "https://fixture.invalid")
            .body(bytes)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let headers: serde_json::Map<String,Value> = response.headers().iter()
            .filter(|(name,_)| name.as_str() != "date")
            .map(|(name,value)|(name.to_string(),json!(value.to_str().unwrap())))
            .collect();
        assert_eq!(Value::Object(headers),case["headers"],"cabeçalhos de serialização");
        let text = response.text().await.unwrap();
        assert_eq!(
            status,
            case["status"].as_u64().unwrap() as u16,
            "serialização {text}"
        );
        let body = if status == 500 {
            json!(text)
        } else {
            serde_json::from_str::<Value>(&text).unwrap()
        };
        assert_eq!(body, case["body"]);
    }
    pause.store(true, std::sync::atomic::Ordering::SeqCst);
    let request = tokio::spawn(async move {
        send(address, "POST", "fixture/engine/model", r#"{"model":"k3"}"#).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert!(
        registry
            .ingress()
            .close("fixture", std::time::Duration::from_millis(20))
            .await
            .is_err(),
        "a troca de conta não pode atravessar a consulta do catálogo"
    );
    registry.ingress().open("fixture");
    release.notify_one();
    assert_eq!(request.await.unwrap().0, 200);
    assert_eq!(python.hits_to("/api/sessions/fixture/engine/model"), 0);
    let frames: Vec<Value> = sink
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| serde_json::from_str(e["frame"].as_str()?).ok())
        .collect();
    assert_eq!(
        frames
            .iter()
            .filter(|f| f["request"]["subtype"] == "set_model")
            .count(),
        9
    );
    provider_task.abort();
}

#[tokio::test]
async fn engine_http_schema_methods_and_cors_match_python() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), root.path().to_string_lossy().into_owned())].into(),
    ));
    let app = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let mut failures = Vec::new();
    let schemas: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/body-schemas.json"
    ))
    .unwrap();
    for case in schemas["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["status"] != 200 && case["schema"] != "EngineModelBody")
    {
        let (method, path) = match case["schema"].as_str().unwrap() {
            "EngineProbeBody" => ("POST", "/api/engines/modelos"),
            "CliproxyManagementBody" => ("PUT", "/api/engines/cliproxy/management-key"),
            "SyncBody" => ("POST", "/api/credenciais/sincronizar"),
            "ApelidoBody" => ("PUT", "/api/credenciais/apelido"),
            _ => unreachable!(),
        };
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let mut request = client
            .request(method.parse().unwrap(), format!("http://{address}{path}"))
            .bearer_auth("fixture-owner")
            .header("origin","https://fixture.invalid")
            .body(bytes);
        if let Some(content_type) = case["content_type"].as_str() {
            request = request.header("content-type", content_type);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers: serde_json::Map<String,Value> = response.headers().iter()
            .filter(|(name,_)| name.as_str() != "date")
            .map(|(name,value)|(name.to_string(),json!(value.to_str().unwrap())))
            .collect();
        if Value::Object(headers.clone()) != case["headers"] { failures.push(format!("{} {} cabeçalhos {headers:?} != {}",case["schema"],case["name"],case["headers"])); }
        let text = response.text().await.unwrap();
        let body = if case["status"] == 500 {
            json!(text)
        } else {
            serde_json::from_str::<Value>(&text).unwrap_or(json!(text))
        };
        if status != case["status"].as_u64().unwrap() as u16 || body != case["body"] {
            failures.push(format!(
                "{} {} tipo {}: status {status}, corpo {body}, esperado {} {}",
                case["schema"], case["name"], case["content_type"], case["status"], case["body"]
            ));
        }
    }
    let methods: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/methods.json"
    ))
    .unwrap();
    for case in methods["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["status"] != 200)
    {
        let response = client
            .request(
                case["method"].as_str().unwrap().parse().unwrap(),
                format!("http://{address}{}", case["path"].as_str().unwrap()),
            )
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let headers: serde_json::Map<String, Value> = response
            .headers()
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "allow" | "location"))
            .map(|(key, value)| {
                (
                    key.to_string(),
                    json!(
                        value
                            .to_str()
                            .unwrap()
                            .replace(&address.to_string(), "testserver")
                    ),
                )
            })
            .collect();
        let body = response.text().await.unwrap();
        if status != case["status"].as_u64().unwrap() as u16
            || Value::Object(headers.clone()) != case["headers"]
            || body != case["body"].as_str().unwrap()
        {
            failures.push(format!(
                "{} {}: status {status}, cabeçalhos {headers:?}, corpo {body:?}; esperado {case}",
                case["method"], case["path"]
            ));
        }
    }
    let response = client
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{address}/api/engines"),
        )
        .header("origin", "https://client.invalid")
        .header("access-control-request-method", "PUT")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(response.text().await.unwrap(), "OK");
    task.abort();
    assert!(
        failures.is_empty(),
        "{} divergências:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

proptest::proptest! {
    #[test]
    fn engine_body_external_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(),0..2048), json_type in proptest::prelude::any::<bool>()) {
        let mut headers=axum::http::HeaderMap::new();
        if json_type {headers.insert("content-type",axum::http::HeaderValue::from_static("application/json"));}
        use hangar_server::engines::body::{self,Schema};
        for schema in [Schema::Model,Schema::Probe,Schema::Management,Schema::Sync,Schema::Alias] {
            let _=body::typed(schema,&bytes,&headers);
        }
        let _=body::raw(&bytes);
    }
}

#[tokio::test]
async fn engine_providers_catalog_is_served_without_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/provider-catalog.json"
    )).unwrap();
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    let missing = root.path().join("missing").to_string_lossy().into_owned();
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(),root.path().to_string_lossy().into_owned()),
        ("PATH".into(),missing.clone()),
        ("SHELL".into(),missing),
    ].into()));
    let app = hangar_server::routes::router(Arc::new(state));
    let server = tokio::spawn(async move {
        axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    let response = reqwest::Client::new().get(format!("http://{address}/api/providers"))
        .bearer_auth("fixture-owner").send().await.unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    server.abort();
    assert_eq!(status,200,"catálogo sem ponte: {body}");
    let expected = fixture["cases"].as_array().unwrap().iter().find(|case|case["name"]=="none").unwrap();
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(),expected["result"]);
}

#[test]
fn engine_provider_selection_matches_python_logins_and_preference() {
    use hangar_server::{engines::providers::Snapshot,provider_discovery::Availability};
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/provider-catalog.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let probes = ["claude","codex","pi","omp","kimi"].into_iter().map(|name| {
            let available = case["available"].as_array().unwrap().iter().any(|value|value==name);
            (name.into(),Availability { available, reason:if available {None} else {Some("nao_encontrado")} })
        }).collect();
        let logins:Vec<_> = case["logins"].as_array().into_iter().flatten().map(Value::as_bool).collect();
        let codex:Vec<_> = case["codex"].as_array().unwrap().iter().map(|value|value.as_str().unwrap().to_owned()).collect();
        let snapshot = Snapshot::from_states(probes,&logins,case["engines"].as_bool().unwrap(),&codex,
            case["remembered"].as_str().map(str::to_owned));
        assert_eq!(snapshot.response(),case["result"],"{}",case["name"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn engine_model_options_preserve_share_permissions() {
    use crate::remote_browser::support::{self,Scene,Server,OWNER};
    use hangar_server::share::store::Kind;
    if support::isolated("engine_routes::engine_model_options_preserve_share_permissions", |_| {}).await { return; }
    let provider = Server::owner(axum::Router::new().route("/v1/models",axum::routing::get(||async {
        ([("content-type","application/json")],r#"{"data":[{"id":"fixture-model"}]}"#)
    }))).await;
    let scene = Scene::new().await;
    std::fs::write(scene.home.join(".claude/engines.json"),json!({"fixture-engine":{
        "base_url":format!("http://{}",provider.address),"api_key":"fixture-secret","model":"fixture-model"
    }}).to_string()).unwrap();
    let sidecar = scene.home.join(".hangar/claude-headless/headless.json");
    let mut meta:Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    meta["engine"]=json!("fixture-engine");
    std::fs::write(sidecar,meta.to_string()).unwrap();
    let invite = scene.guest_token("headless",Kind::Share).await;
    let terminal = scene.guest_token("s0",Kind::Share).await;
    let pair = scene.guest_token("headless",Kind::Pair).await;
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap();
    let mut failures = Vec::new();
    for (path,token,expected) in [
        ("/api/sessions/headless/model/options",&invite,200),
        ("/api/sessions/s0/model/options",&terminal,503),
        ("/api/engines",&invite,200),
        ("/api/model-options?provider=claude&engine=fixture-engine",&invite,200),
    ] {
        let owner = client.get(format!("http://{}{path}",scene.owner.address)).bearer_auth(OWNER).send().await.unwrap();
        let owner_status=owner.status().as_u16();
        let owner_body=owner.text().await.unwrap();
        assert_eq!(owner_status,expected,"precondição do dono em {path}: {owner_body}");
        let guest = client.get(format!("http://{}{path}",scene.guest.address)).bearer_auth(token).send().await.unwrap();
        let status=guest.status().as_u16();
        let body=guest.text().await.unwrap();
        if status!=expected || body!=owner_body {
            failures.push(format!("{path}: convidado {status} {body}, dono {owner_status} {owner_body}"));
        }
    }
    for (method,path,token) in [
        ("GET","/api/sessions/s0/model/options",&invite),
        ("GET","/api/sessions/headless/model/options",&pair),
        ("PUT","/api/engines/fixture-engine",&invite),
        ("GET","/api/providers",&invite),
    ] {
        let response=client.request(method.parse().unwrap(),format!("http://{}{path}",scene.guest.address))
            .bearer_auth(token).body("{}").send().await.unwrap();
        if response.status()!=403 { failures.push(format!("recusa {method} {path}: {}",response.status())); }
    }
    assert!(scene.upstream.paths().iter().any(|path|path=="/api/sessions/s0/model/options"),"sem motor conserva o transporte existente");
    scene.close().await;
    provider.close().await;
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
