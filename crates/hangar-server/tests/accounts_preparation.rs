//! O catálogo só passa ao Rust junto com a coordenação verdadeira do preparo.
#[test]
fn preparation_routes_belong_to_rust() {
    use axum::http::Method;
    use hangar_server::migration_status::rust_route;
    assert!(
        rust_route(&Method::POST, "/api/claude-configs"),
        "cadastro Claude ainda passa pelo Python"
    );
    assert!(
        rust_route(&Method::GET, "/api/codex-contas"),
        "catálogo Codex ainda passa pelo Python"
    );
}

#[tokio::test]
async fn lost_response_keeps_guard_and_force_waits_for_the_effective_worker() {
    use axum::{Router, http::StatusCode, response::IntoResponse, routing::post};
    use hangar_server::accounts::{
        AccountKey, AccountService, GuardMode, Provider, bridge::AccountsBridge,
        environment::AccountEnvironment,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    ));
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let (started, mut requests) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let finished = Arc::new(tokio::sync::Semaphore::new(0));
    let released = finished.clone();
    let router = Router::new()
        .route(
            "/internal/accounts/prepare",
            post(move |body: axum::body::Bytes| {
                started
                    .send(serde_json::from_slice(&body).unwrap())
                    .unwrap();
                async { StatusCode::SERVICE_UNAVAILABLE.into_response() }
            }),
        )
        .route(
            "/internal/accounts/prepare/wait",
            post(move || {
                let released = released.clone();
                async move {
                    released.acquire().await.unwrap().forget();
                    json!({"status":"ready","trust_pending":false,"issues":[]}).to_string()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bridge = AccountsBridge::new(
        listener.local_addr().unwrap(),
        "synthetic".into(),
        "instance".into(),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    assert_eq!(
        service.preparation_status(&account, &bridge).await["status"],
        "idle"
    );
    assert!(
        requests.try_recv().is_err(),
        "o cadastro não inicia preparo"
    );
    assert_eq!(
        service
            .prepare(&account, bridge.clone(), false, None)
            .await
            .unwrap()["status"],
        "running"
    );
    let first = requests.recv().await.unwrap();
    assert_eq!(first["force"], false);
    assert_eq!(
        service
            .prepare(&account, bridge.clone(), true, None)
            .await
            .unwrap()["status"],
        "running"
    );
    assert!(
        requests.try_recv().is_err(),
        "a resposta perdida iniciou outro gancho"
    );
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    finished.add_permits(1);
    let second = requests.recv().await.unwrap();
    assert_eq!(second["force"], true);
    assert_ne!(first["operation"], second["operation"]);
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    finished.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if service.preparation_status(&account, &bridge).await["status"] == "ready"
                && service
                    .locks
                    .try_acquire(&key, GuardMode::Exclusive)
                    .is_ok()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(requests.try_recv().is_err());
    server.abort();
}

#[tokio::test]
async fn newer_python_preparation_survives_return_to_rust() {
    use hangar_server::accounts::{
        AccountKey, AccountService, Provider, bridge::AccountsBridge,
        environment::AccountEnvironment,
    };
    use serde_json::json;
    use std::{
        fs,
        time::{Duration, UNIX_EPOCH},
    };
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let environment = AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    );
    let service = AccountService::new(environment.clone());
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let rust_result = service
        .locks
        .path(&key)
        .unwrap()
        .with_extension("prepare-result.json");
    fs::write(
        &rust_result,
        json!({"status":"error","issues":[{"code":"account_prepare_bridge_unavailable"}]})
            .to_string(),
    )
    .unwrap();
    let digest: String = ring::digest::digest(
        &ring::digest::SHA256,
        account.home.to_string_lossy().as_bytes(),
    )
    .as_ref()
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect();
    let python_result = root
        .path()
        .join(".hangar/codex-contas")
        .join(digest)
        .join("estado.json");
    fs::create_dir_all(python_result.parent().unwrap()).unwrap();
    let partial = json!({"status":"partial","trust_pending":false,"issues":[{"code":"configuration_collision"}]});
    fs::write(&python_result, json!({"public":partial}).to_string()).unwrap();
    fs::File::options()
        .write(true)
        .open(&rust_result)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    fs::File::options()
        .write(true)
        .open(&python_result)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(2))
        .unwrap();
    let restarted = AccountService::new(environment);
    let bridge = AccountsBridge::new(
        "127.0.0.1:1".parse().unwrap(),
        "synthetic".into(),
        "current".into(),
    )
    .unwrap();
    assert_eq!(
        restarted.preparation_status(&account, &bridge).await,
        partial,
        "o resultado antigo do Rust escondeu o preparo mais recente no fallback"
    );
    fs::File::options()
        .write(true)
        .open(&rust_result)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(3))
        .unwrap();
    assert_eq!(
        restarted.preparation_status(&account, &bridge).await["status"],
        "error",
        "o erro posterior do coordenador foi escondido pelo estado Python"
    );
}
