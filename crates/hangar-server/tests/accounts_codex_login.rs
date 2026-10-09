use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};

#[test]
fn native_storage_parses_quoted_keys_tables_and_duplicates() {
    use hangar_server::accounts::{catalog::Account, codex_login::validate_storage};
    let root = tempfile::tempdir().unwrap();
    let account = Account {
        id: "alpha".into(),
        home: root.path().into(),
        is_default: false,
    };
    for config in [
        "cli_auth_credentials_store='file'\n",
        "\"cli_auth_credentials_store\" = \"file\"\n[tools]\nvalue = 'kept'\n",
    ] {
        std::fs::write(root.path().join("config.toml"), config).unwrap();
        assert!(validate_storage(&account).is_ok());
    }
    std::fs::write(
        root.path().join("config.toml"),
        "[tools]\ncli_auth_credentials_store='file'\n",
    )
    .unwrap();
    assert_eq!(
        validate_storage(&account).unwrap_err().code,
        "codex_account_auth_storage_invalid"
    );
    std::fs::write(
        root.path().join("config.toml"),
        "cli_auth_credentials_store='file'\ncli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    assert_eq!(
        validate_storage(&account).unwrap_err().code,
        "codex_account_prepare_required"
    );
    std::fs::write(
        root.path().join("config.toml"),
        "cli_auth_credentials_store = [\n",
    )
    .unwrap();
    assert_eq!(
        validate_storage(&account).unwrap_err().code,
        "codex_account_prepare_required"
    );
}

#[test]
fn codex_login_routes_are_owned() {
    use axum::http::Method;
    for method in [Method::POST, Method::GET, Method::DELETE] {
        assert!(
            hangar_server::migration_status::rust_route(&method, "/api/codex-contas/alpha/login"),
            "login Codex ainda passa pelo Python: {method}"
        );
    }
}

#[test]
fn legacy_device_routes_are_owned() {
    use axum::http::Method;
    for method in [Method::POST, Method::GET, Method::DELETE] {
        assert!(hangar_server::migration_status::rust_route(&method, "/api/credenciais/codex/login"),
            "device flow legado ainda passa pelo Python: {method}");
    }
    assert!(hangar_server::migration_status::rust_route(&Method::GET, "/api/credenciais/codex"));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_probe_process() {
    let Ok(upstream) = std::env::var("ACCOUNT_HTTP_UPSTREAM") else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: upstream.parse().unwrap(),
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    if let Ok(base) = std::env::var("ACCOUNT_DEVICE_UPSTREAM") {
        use hangar_server::accounts::codex_device_login::{DeviceOAuth,DeviceLogins};
        state.accounts.device_logins=DeviceLogins::new(DeviceOAuth::new(&base).unwrap());
    }
    println!("ACCOUNT_HTTP:{addr}");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    hangar_server::serve_until_with_state(
        listener,
        state,
        hangar_server::parent_gone(tokio::io::stdin()),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn login_deadline_keeps_guard_until_helper_cleanup() {
    use axum::{Router, routing::post};
    use hangar_server::accounts::{
        AccountKey, AccountService, GuardMode, Provider, bridge::AccountsBridge,
        codex_login::CodexInvalidator, environment::AccountEnvironment,
    };
    use serde_json::{Value, json};
    use std::{sync::Arc, time::Duration};
    let root = tempfile::tempdir().unwrap();
    let fixture = root.path().join("cli");
    let script = fixture.join("node_modules/@openai/codex/bin/codex.js");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    let source = r#"require('fs').appendFileSync(require('path').join(process.env.CODEX_HOME,'native-starts.jsonl'),JSON.stringify(process.pid)+'\n');
const rl=require('readline').createInterface({input:process.stdin});
rl.on('line', text=>{const m=JSON.parse(text);if(!m.id)return;
const result=m.method==='account/login/start'?{type:'chatgptDeviceCode',loginId:'fixture',verificationUrl:'https://example.test/device',userCode:'fixture'}:{};
process.stdout.write(JSON.stringify({id:m.id,result})+'\n');});"#;
    std::fs::write(&script, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = fixture.join("codex");
        std::fs::write(&executable, format!("#!/usr/bin/env node\n{source}")).unwrap();
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut environment: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let path = environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .unwrap()
        .1
        .clone();
    environment.retain(|key, _| !key.eq_ignore_ascii_case("PATH"));
    let dirs = std::iter::once(fixture.clone()).chain(std::env::split_paths(&path));
    environment.insert(
        "PATH".into(),
        std::env::join_paths(dirs).unwrap().to_string_lossy().into(),
    );
    for name in ["HOME", "USERPROFILE"] {
        environment.insert(name.into(), root.path().to_string_lossy().into());
    }
    environment.remove("CODEX_HOME");
    environment.remove("CLAUDE_CONFIG_DIR");
    let service = AccountService::new(AccountEnvironment::from_map(environment));
    let account = service.create(Provider::Codex, "work", |_| Ok(())).unwrap();
    std::fs::write(
        account.home.join("config.toml"),
        "cli_auth_credentials_store='file'\n",
    )
    .unwrap();
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let router = Router::new()
        .route(
            "/internal/accounts/facts",
            post(|body: axum::body::Bytes| async move {
                let body: Value = serde_json::from_slice(&body).unwrap();
                json!([{"key":body["keys"][0],"facts":{"complete":true,"sessions":[],"pids":[]}}])
                    .to_string()
            }),
        )
        .route(
            "/internal/accounts/codex-invalidate",
            post(|| async { json!({"ok":true}).to_string() }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let bridge = AccountsBridge::new(address, "synthetic".into(), "test".into()).unwrap();
    let invalidator = CodexInvalidator::new(address, "synthetic".into(), "test".into()).unwrap();
    let runtime = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(
        address,
        "synthetic".into(),
        "test".into(),
    ));
    let attempt = service
        .start_codex_login(
            &account,
            bridge.clone(),
            Some(runtime.clone()),
            invalidator.clone(),
        )
        .await
        .unwrap();
    assert_eq!(attempt["status"], "waiting");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(899)).await;
    tokio::time::resume();
    assert_eq!(service.codex_login_status(&account)["status"], "waiting");
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.codex_login_status(&account)["status"] == "waiting" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let final_state = service.codex_login_status(&account);
    assert_eq!(final_state["status"], "failed");
    assert_eq!(
        final_state["error"],
        json!({"code":"codex_account_login_timeout","params":{}})
    );
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_ok()
    );
    service.codex_logins.close().await;
    let starts = std::fs::read(account.home.join("native-starts.jsonl")).unwrap();
    service.codex_readers.close().await;
    assert_eq!(
        service.read_codex_auth(&account).await["status"],
        "unavailable"
    );
    assert_eq!(
        std::fs::read(account.home.join("native-starts.jsonl")).unwrap(),
        starts,
        "leitura depois do fechamento abriu auxiliar"
    );
    assert!(
        service
            .start_codex_login(&account, bridge, Some(runtime), invalidator)
            .await
            .is_err(),
        "shutdown aceitou auxiliar novo"
    );
    server.abort();
}

/// Barreira só do teste: segura a leitura real do Pi e confirma o término do writer nativo.
async fn native_secondary_writer_retains_ownership(start: bool) {
    use axum::{Router, routing::post};
    use hangar_server::accounts::{AccountService, AccountKey, Provider, GuardMode,
        bridge::AccountsBridge, environment::AccountEnvironment,
        codex_device_login::{DeviceBridge, DeviceLogins, DeviceOAuth}};
    use serde_json::{Value, json};
    use std::{path::Path, sync::{Arc, Mutex, mpsc}, time::Duration};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".codex")).unwrap();
    std::fs::create_dir_all(root.path().join(".hangar/auth")).unwrap();
    std::fs::create_dir_all(root.path().join(".pi/agent")).unwrap();
    std::fs::create_dir_all(root.path().join(".omp/agent")).unwrap();
    let pi = root.path().join(".pi/agent/auth.json");
    std::fs::write(&pi, "{}").unwrap();
    let db = root.path().join(".omp/agent/agent.db");
    rusqlite::Connection::open(&db).unwrap().execute(
        "create table auth_credentials (id integer primary key, provider text, credential_type text, data text, identity_key text)", []).unwrap();
    if !start {
        std::fs::write(root.path().join(".hangar/auth/openai-codex.json"),
            json!({"access":"synthetic","refresh":"synthetic","id_token":"","expires_ms":1,"account_id":"fixture","plano":""}).to_string()).unwrap();
    }
    let router = Router::new()
        .route("/api/accounts/deviceauth/usercode", post(|| async { json!({"device_auth_id":"fixture","user_code":"fixture","interval":1}).to_string() }))
        .route("/api/accounts/deviceauth/token", post(|| async { json!({"authorization_code":"fixture","code_verifier":"fixture"}).to_string() }))
        .route("/oauth/token", post(|| async { json!({"access_token":"synthetic","refresh_token":"synthetic","id_token":"","expires_in":1}).to_string() }))
        .route("/internal/accounts/facts", post(|body: axum::body::Bytes| async move {
            let body: Value = serde_json::from_slice(&body).unwrap();
            json!([{"key":body["keys"][0],"facts":{"complete":true,"sessions":[],"pids":[]}}]).to_string()
        }))
        .route("/internal/accounts/codex-invalidate", post(|| async { json!({"ok":true}).to_string() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(),root.path().to_string_lossy().into()),("USERPROFILE".into(),root.path().to_string_lossy().into())].into()));
    service.device_logins = DeviceLogins::new(DeviceOAuth::new(&format!("http://{address}")).unwrap());
    let (entered_tx, entered) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let gate = Mutex::new((entered_tx, released));
    let target = pi.clone();
    service.env.secondary_barrier = Some(Arc::new(move |path: &Path| {
        if path == target {
            let gate = gate.lock().unwrap();
            gate.0.send(()).unwrap();
            gate.1.recv_timeout(Duration::from_secs(40)).expect("a barreira Pi não foi liberada");
        }
    }));
    let key = AccountKey::new(Provider::Codex, &root.path().join(".codex")).unwrap();
    let runtime = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(address,"synthetic".into(),"fixture".into()));
    let facts = AccountsBridge::new(address,"synthetic".into(),"fixture".into()).unwrap();
    let bridge = DeviceBridge::new(address,"synthetic".into(),"fixture".into()).unwrap();
    let operation = |service: AccountService| { let (facts, runtime, bridge) = (facts.clone(), runtime.clone(), bridge.clone());
        async move {
            if start { service.start_device_login(facts, Some(runtime), bridge).await }
            else { service.propagate_device(facts, Some(runtime), bridge, false).await }
        }};
    let request = tokio::spawn(operation(service.clone()));
    let arrived = tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(20))).await.unwrap();
    if start { assert!(request.await.unwrap().is_ok()); } else { request.abort(); let _ = request.await; }
    let held = arrived.is_ok() && service.locks.try_acquire(&key, GuardMode::Exclusive).is_err();
    // Sem a guarda presa, um segundo writer entraria na mesma barreira: a asserção de posse decide.
    let second = if held { Some(operation(service.clone()).await) } else { None };
    let cancel = start.then(|| tokio::spawn({ let service = service.clone(); async move { service.device_logins.cancel(None).await } }));
    let mut closing = tokio::spawn({ let service = service.clone(); async move { service.device_logins.close().await } });
    // Pi retido não recebe prazo: cancelamento e shutdown esperam o fim real da escrita.
    let shutdown_held = tokio::time::timeout(Duration::from_secs(2), &mut closing).await.is_err();
    let cancel_held = cancel.as_ref().is_none_or(|cancel| !cancel.is_finished());
    let still_held = service.locks.try_acquire(&key, GuardMode::Exclusive).is_err();
    let _ = release.send(());
    tokio::time::timeout(Duration::from_secs(15), closing).await.expect("shutdown não terminou após liberar o writer").unwrap();
    if let Some(cancel) = cancel {
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), cancel).await.unwrap().unwrap().unwrap(), json!({"etapa":"idle"}));
    }
    server.abort();
    arrived.expect("o writer nativo não chegou à leitura real do Pi");
    assert!(held, "a perda do pedido liberou a conta antes do writer terminar");
    let code = second.unwrap().unwrap_err().code;
    assert_eq!(code, if start { "device_login_in_progress" } else { "codex_account_in_use" }, "outro login entrou com o writer vivo");
    assert!(shutdown_held, "shutdown não aguardou o writer efetivo");
    assert!(cancel_held, "o cancelamento não aguardou o writer efetivo");
    assert!(still_held, "a guarda saiu antes do fim do writer");
    assert!(service.locks.try_acquire(&key, GuardMode::Exclusive).is_ok());
    let written: Value = serde_json::from_slice(&std::fs::read(&pi).unwrap()).unwrap();
    assert_eq!(written["openai-codex"]["refresh"], "synthetic");
    let count: i64 = rusqlite::Connection::open(&db).unwrap()
        .query_row("select count(*) from auth_credentials", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 1, "o writer não terminou o omp ou duplicou a linha");
}

#[tokio::test(flavor = "multi_thread")]
async fn device_start_retains_guard_until_native_secondary_writer_finishes() { native_secondary_writer_retains_ownership(true).await; }
#[tokio::test(flavor = "multi_thread")]
async fn device_propagation_retains_guard_until_native_secondary_writer_finishes() { native_secondary_writer_retains_ownership(false).await; }

#[tokio::test]
async fn device_state_reads_destinations_locally_and_keeps_strict_profile_error() {
    use hangar_server::accounts::{AccountService, environment::AccountEnvironment, codex_device_login::DeviceBridge};
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".pi/agent")).unwrap();
    std::fs::write(root.path().join(".pi/agent/auth.json"), r#"{"openai-codex":{"type":"oauth","refresh":"synthetic"}}"#).unwrap();
    // Nenhum serviço Python escuta aqui: a leitura de estado não pode depender da ponte.
    let bridge = DeviceBridge::new("127.0.0.1:9".parse().unwrap(), "synthetic".into(), "fixture".into()).unwrap();
    let home = root.path().to_string_lossy().to_string();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home.clone())].into()));
    let state = service.device_state(&bridge).await.unwrap();
    assert_eq!(state, json!({"cofre":false,"plano":"","expira_em":null,"codex":false,"pi":true,"omp":false}));
    let strict = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home), ("OMP_PROFILE".into(), "../fora".into())].into()));
    let error = strict.device_state(&bridge).await.unwrap_err();
    assert_eq!((error.status, error.code), (503, "device_bridge_unavailable"));
}
