//! O catálogo só passa ao Rust junto com a coordenação verdadeira do preparo.
#[test]
fn preparation_routes_belong_to_rust() {
    use axum::http::Method;
    use hangar_server::migration_status::rust_route;
    assert!(
        rust_route(&Method::POST, "/api/claude-configs"),
        "o cadastro Claude, preparo incluído, é do Rust; voltou a passar pelo Python"
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

#[tokio::test]
async fn ready_preparation_is_reused_after_restart_until_explicit_force() {
    use axum::{Router, routing::post};
    use hangar_server::accounts::{AccountService, Provider, bridge::AccountsBridge,
        environment::AccountEnvironment};
    use serde_json::json;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let environment = AccountEnvironment::from_map([
        ("HOME".into(), home.clone()), ("USERPROFILE".into(), home),
    ].into());
    let first_service = AccountService::new(environment.clone());
    let account = first_service.create(Provider::Codex, "extra", |_|Ok(())).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bridge = AccountsBridge::new(listener.local_addr().unwrap(),"synthetic".into(),"instance".into()).unwrap();
    let router = Router::new().route("/internal/accounts/prepare",post(move || {
        seen.fetch_add(1,Ordering::SeqCst);
        async { json!({"status":"ready","trust_pending":false,"issues":[]}).to_string() }
    }));
    let server = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
    let first = first_service.prepare(&account,bridge.clone(),false,None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15),async {
        while first_service.preparation_status(&account,&bridge).await["status"] == "running" {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let service = AccountService::new(environment);
    let cached = service.prepare(&account,bridge.clone(),false,None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15),async {
        while service.preparation_status(&account,&bridge).await["status"] == "running" {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let forced = service.prepare(&account,bridge.clone(),true,None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15),async {
        while service.preparation_status(&account,&bridge).await["status"] == "running" {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert_eq!(first["status"],"running");
    assert_eq!(cached["status"],"ready","abrir a conta pronta não pode criar outro worker");
    assert_eq!(forced["status"],"running");
    assert_eq!(calls.load(Ordering::SeqCst),2,"primeiro preparo e sincronização explícita");
}

#[tokio::test]
async fn first_open_waits_for_preparation_and_expired_birth_budget_refuses_open() {
    use axum::{Router, routing::post};
    use hangar_server::accounts::{AccountService, Provider, bridge::AccountsBridge,
        environment::AccountEnvironment};
    use serde_json::json;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    for id in ["default","extra"] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(),home.clone()),("USERPROFILE".into(),home),
        ].into()));
        let account = if id == "default" { service.resolve(Provider::Codex,id).unwrap() }
            else { service.create(Provider::Codex,id,|_|Ok(())).unwrap() };
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let prepared_home = account.home.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bridge = AccountsBridge::new(listener.local_addr().unwrap(),"synthetic".into(),"instance".into()).unwrap();
        let router = Router::new().route("/internal/accounts/prepare",post(move || {
            std::fs::create_dir_all(&prepared_home).unwrap();
            seen.fetch_add(1,Ordering::SeqCst);
            async { json!({"status":"ready","trust_pending":false,"issues":[]}).to_string() }
        }));
        let server = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
        let ready = service.prepare_for_session(&account,bridge.clone(),
            tokio::time::Instant::now()+std::time::Duration::from_secs(15)).await;
        tokio::time::timeout(std::time::Duration::from_secs(15),async {
            while service.preparation_status(&account,&bridge).await["status"] == "running" {
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let expired = service.prepare_for_session(&account,bridge.clone(),tokio::time::Instant::now()).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        assert_eq!(ready.unwrap()["status"],"ready","primeira abertura precisa esperar a confirmação");
        assert_eq!(calls.load(Ordering::SeqCst),1,"conta padrão também precisa do primeiro preparo");
        assert_eq!(expired.unwrap_err().code,"codex_account_preparing");
    }
}
