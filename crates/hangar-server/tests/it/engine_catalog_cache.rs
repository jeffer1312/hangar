//! Invalidação explícita do catálogo e permanência da leitura publicada até o prazo.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    engines::{catalog, store},
    routes::AppState,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn engine_catalog_cache_matches_python_mutations_and_expiry() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/catalog-cache.json"
    ))
    .unwrap();
    let generation = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_addr = provider.local_addr().unwrap();
    let (gen_copy, calls_copy) = (generation.clone(), calls.clone());
    let provider_app = axum::Router::new().route(
        "/v1/models",
        axum::routing::get(move || {
            let (generation, calls) = (gen_copy.clone(), calls_copy.clone());
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!({"data":[{"id":format!("v{}", generation.load(Ordering::SeqCst))}]})
                        .to_string(),
                )
            }
        }),
    );
    let provider_task = tokio::spawn(async move {
        axum::serve(provider, provider_app).await.unwrap();
    });
    let home = tempfile::tempdir().unwrap();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.path().to_string_lossy().into_owned())].into(),
    ));
    let path = store::path(&service.env);
    let original = json!({"motor":{"base_url":format!("http://{provider_addr}"),"api_key":"fixture-key","model":"v0"}});
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, original.to_string()).unwrap();
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
    state.accounts = service.clone();
    let app = hangar_server::routes::router(Arc::new(state));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/engines/motor");
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        match name {
            "cached" => {
                generation.store(1, Ordering::SeqCst);
            }
            "same-put-invalidates" => {
                assert_eq!(
                    client
                        .put(&url)
                        .bearer_auth("fixture-owner")
                        .json(&json!({}))
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    200
                );
            }
            "delete-recreate-invalidates" => {
                generation.store(2, Ordering::SeqCst);
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
                std::fs::write(&path, original.to_string()).unwrap();
            }
            "external-delete-keeps-cache" => {
                std::fs::write(&path, "{}").unwrap();
            }
            "expired-missing" => {
                tokio::time::pause();
                tokio::time::advance(std::time::Duration::from_secs(300)).await;
            }
            "fresh" => {
                tokio::time::resume();
                std::fs::write(&path, original.to_string()).unwrap();
                generation.store(3, Ordering::SeqCst);
            }
            _ => {}
        }
        let actual = match catalog::models(&service, "motor", None, name == "fresh").await {
            Ok(models) => json!({"models":models}),
            Err(error) => json!({"status":error.status,"detail":error.detail}),
        };
        if actual != case["result"]
            || calls.load(Ordering::SeqCst) != case["calls"].as_u64().unwrap() as usize
        {
            failures.push(format!(
                "{name}: {actual}; consultas {}",
                calls.load(Ordering::SeqCst)
            ));
        }
    }
    server.abort();
    provider_task.abort();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
