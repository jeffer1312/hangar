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

#[tokio::test(flavor = "multi_thread")]
async fn http_probe_process() {
    let Ok(upstream) = std::env::var("ACCOUNT_HTTP_UPSTREAM") else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(Config {
        listen: addr,
        upstream: upstream.parse().unwrap(),
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
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
