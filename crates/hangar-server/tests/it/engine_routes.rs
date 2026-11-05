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
