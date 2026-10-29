//! Contratos de convidados medidos no Python, com upstream indisponível.
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::Value;
use std::sync::Arc;

#[tokio::test]
async fn login_guests_owner_identity_is_served_without_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/http.json"
    ))
    .unwrap();
    let row = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "dono")
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "guest-fixture".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            hangar_server::routes::router(Arc::new(state))
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/me"))
        .bearer_auth("owner")
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let raw = response.text().await.unwrap();
    task.abort();
    assert_eq!(
        status,
        row["status"].as_u64().unwrap() as u16,
        "GUE-010: a identidade não depende do Python"
    );
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body, row["response"]);
}

#[tokio::test]
async fn login_guests_python_http_contract_and_disk_effects() {
    use hangar_server::login_guests::{Service, Settings};
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/http.json"
    ))
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("projects")).unwrap();
    let settings = Settings {
        guests: home.path().join("guests.json"),
        vault: home.path().join("vault.json"),
        runtime_config: home.path().join("runtime-config.json"),
        sync_enabled: true,
        bootstrap: "bootstrap".into(),
        secret: b"fixture-secret".to_vec(),
        secure: false,
        rate_max: 10,
        rate_window: 300,
    };
    let mut service = Service::new(settings);
    service.now = || 2000000000;
    service.entropy = |n| vec![b'x'; n];
    service.id = || "ab".repeat(8);
    service.token = || "guest-token".into();
    let service = Arc::new(service);
    let app = axum::Router::new().fallback({
        let service = service.clone();
        move |request: axum::extract::Request| {
            let service = service.clone();
            async move {
                let owner = request
                    .headers()
                    .get("authorization")
                    .is_some_and(|h| h == "Bearer owner");
                service
                    .respond(owner, "127.0.0.1".into(), false, request)
                    .await
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    for row in fixture["rows"].as_array().unwrap() {
        let expected: Value = serde_json::from_str(
            &row.to_string()
                .replace("⟦ROOT⟧", &home.path().to_string_lossy()),
        )
        .unwrap();
        let method =
            reqwest::Method::from_bytes(expected["method"].as_str().unwrap().as_bytes()).unwrap();
        let mut request = client.request(
            method,
            format!("http://{address}{}", expected["path"].as_str().unwrap()),
        );
        if let Some(token) = expected["token"].as_str() {
            request = request.bearer_auth(token);
        }
        if let Some(cookie) = expected["cookie"].as_str() {
            request = request.header("cookie", format!("cp_sync={cookie}"));
        }
        if !expected["body"].is_null() {
            request = request.json(&expected["body"]);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let cookie = response
            .headers()
            .get("set-cookie")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            status,
            expected["status"].as_u64().unwrap() as u16,
            "{}: {body}",
            row["name"]
        );
        assert_eq!(body, expected["response"], "{}", row["name"]);
        assert_eq!(
            cookie,
            expected["set_cookie"].as_str().map(str::to_owned),
            "cookie {}",
            row["name"]
        );
        for (file, key) in [("guests.json", "guests"), ("vault.json", "vault")] {
            let actual = std::fs::read(home.path().join(file))
                .ok()
                .map(|b| serde_json::from_slice::<Value>(&b).unwrap())
                .unwrap_or(Value::Null);
            assert_eq!(actual, expected[key], "disco {file}: {}", row["name"]);
        }
    }
    task.abort();
}

#[test]
fn login_guests_gate_matches_python_matrix() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/http.json")).unwrap();
    for row in fixture["gate_rows"].as_array().unwrap() {
        let actual = hangar_server::login_guests::gate::allowed(row["method"].as_str().unwrap(), row["path"].as_str().unwrap(), |name| name == "ana");
        assert_eq!(actual, row["allowed"].as_bool().unwrap(), "GUE-031/032: {} {}", row["method"], row["path"]);
    }
}

#[tokio::test]
async fn login_guests_public_gate_denies_owner_routes_before_upstream() {
    use hangar_server::login_guests::{Service, Settings};
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen:address, upstream:crate::refused_address(),
        internal_secret:"fixture".into(), auth_token:"owner".into(), log_path:None,
        trusted:TrustedHosts::parse("127.0.0.1") });
    let service = Service::new(Settings { guests:home.path().join("guests.json"), vault:home.path().join("vault.json"),
        runtime_config:home.path().join("runtime-config.json"), sync_enabled:false, bootstrap:String::new(),
        secret:b"fixture".to_vec(), secure:false, rate_max:10, rate_window:300 });
    service.store.create("Ana", home.path().to_str().unwrap(), false, true, "ana".into(), "guest".into()).unwrap();
    state.login_guests = Arc::new(service);
    let task=tokio::spawn(async move { axum::serve(listener,hangar_server::routes::router(Arc::new(state))
        .into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let client=reqwest::Client::new();
    for path in ["/api/config", "/api/voice", "/api/peers", "/api/sessions/owner/share"] {
        let response=client.get(format!("http://{address}{path}")).bearer_auth("guest").send().await.unwrap();
        assert_eq!(response.status().as_u16(),403,"GUE-030/032: {path}");
        let body:Value=response.json().await.unwrap();
        assert_eq!(body["detail"]["code"],"erro_fora_do_convidado");
    }
    task.abort();
}

#[tokio::test]
async fn login_guests_body_limit_precedes_authentication() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let service=Arc::new(Service::new(Settings {guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10,rate_window:300}));
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/limits.json")).unwrap();
    let expected=&fixture["rows"][0];
    let request=axum::http::Request::post("/api/guests").header("content-length",104857601u64).body(axum::body::Body::empty()).unwrap();
    let response=service.respond(false,"127.0.0.1".into(),false,request).await;
    assert_eq!(response.status().as_u16(),expected["status"].as_u64().unwrap() as u16,"GUE-080: limite anterior à autenticação");
    assert_eq!(response.headers()["content-type"].to_str().unwrap(),expected["content_type"].as_str().unwrap());
    let body=axum::body::to_bytes(response.into_body(),1024).await.unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(),expected["response"].as_str().unwrap());
}
