use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};

#[test]
fn claude_login_routes_are_owned() {
    use axum::http::Method;
    for (method, path) in [
        (Method::GET, "/api/conta-estado"),
        (Method::POST, "/api/conta-estado/work/login"),
        (Method::GET, "/api/conta-estado/work/login/passo"),
        (Method::POST, "/api/conta-estado/work/login/codigo"),
        (Method::POST, "/api/conta-estado/work/login/cancelar"),
        (Method::POST, "/api/claude-configs/work/logout"),
    ] {
        assert!(
            hangar_server::migration_status::rust_route(&method, path),
            "login Claude ainda passa pelo Python: {path}"
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
    hangar_server::serve_until_with_state(listener, state, std::future::pending::<()>())
        .await
        .unwrap();
}

#[tokio::test]
async fn open_attempt_keeps_its_window_and_guard_after_300_seconds() {
    use axum::{Router, routing::post};
    use hangar_server::accounts::{
        AccountKey, AccountService, GuardMode, Provider, claude_login::WindowClient,
        environment::AccountEnvironment,
    };
    use serde_json::{Value, json};
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    ));
    let account = service
        .create(Provider::Claude, "work", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
    let (actions, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let router = Router::new().route(
        "/internal/accounts/claude-window",
        post(move |body: axum::body::Bytes| {
            let value: Value = serde_json::from_slice(&body).unwrap();
            actions
                .send(value["action"].as_str().unwrap().to_owned())
                .unwrap();
            async { json!({"ok":true,"url":null}).to_string() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = WindowClient::new(
        listener.local_addr().unwrap(),
        "synthetic".into(),
        "test".into(),
    )
    .unwrap();
    let server = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
    service
        .start_claude_login("work", client.clone())
        .await
        .unwrap();
    assert_eq!(observed.recv().await.as_deref(), Some("open"));
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(301)).await;
    tokio::time::resume();
    assert_eq!(
        service.claude_step("work", client.clone()).await.unwrap()["etapa"],
        "aguardando",
        "a idade da abertura não consome o prazo de confirmação"
    );
    assert_eq!(observed.recv().await.as_deref(), Some("read"));
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    service.cancel_claude("work", client.clone()).await.unwrap();
    assert_eq!(observed.recv().await.as_deref(), Some("close"));
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_ok()
    );
    service
        .start_claude_login("work", client.clone())
        .await
        .unwrap();
    assert_eq!(observed.recv().await.as_deref(), Some("open"));
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    service.cancel_claude("work", client).await.unwrap();
    assert_eq!(observed.recv().await.as_deref(), Some("close"));
    server.abort();
}

#[tokio::test]
async fn native_identity_cache_changes_with_credential_and_explicit_invalidation() {
    use hangar_server::accounts::{
        AccountKey, AccountService, Provider, environment::AccountEnvironment,
    };
    use std::fs;
    let root = tempfile::tempdir().unwrap();
    let fixture = root.path().join("native");
    let script = fixture.join("node_modules/@anthropic-ai/claude-code/cli.js");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    let source = "const fs=require('fs'),p=require('path'),d=process.env.CLAUDE_CONFIG_DIR;process.stdout.write(fs.readFileSync(p.join(d,'reply.json')));process.exit(1);";
    fs::write(&script, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(
            fixture.join("claude"),
            format!("#!/usr/bin/env node\n{source}"),
        )
        .unwrap();
        fs::set_permissions(fixture.join("claude"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let separator = if cfg!(windows) { ";" } else { ":" };
    let path = format!(
        "{}{separator}{}",
        fixture.display(),
        std::env::var("PATH").unwrap()
    );
    let home = root.path().to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.clone()),
            ("USERPROFILE".into(), home),
            ("PATH".into(), path),
        ]
        .into_iter()
        .chain(
            std::env::vars()
                .filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case("SystemRoot")),
        )
        .collect(),
    ));
    let account = service
        .create(Provider::Claude, "work", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
    fs::write(
        account.home.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"synthetic-old","refreshTokenExpiresAt":120000}}"#,
    )
    .unwrap();
    fs::write(
        account.home.join("reply.json"),
        r#"{"loggedIn":true,"email":"fixture@example.test","subscriptionType":"pro"}"#,
    )
    .unwrap();
    let first = service.read_claude_auth(&account).await;
    assert_eq!(first["loggedIn"], true);
    assert_eq!(first["refreshExpiresAt"], 120.0);
    fs::write(account.home.join("reply.json"), r#"{"loggedIn":false}"#).unwrap();
    assert_eq!(
        service.read_claude_auth(&account).await,
        first,
        "arquivo intacto conserva o cache"
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    tokio::time::resume();
    assert_eq!(
        service.read_claude_auth(&account).await["loggedIn"],
        false,
        "cache com arquivo expira após 120 s"
    );
    fs::write(account.home.join("reply.json"), r#"{"loggedIn":true}"#).unwrap();
    fs::write(
        account.home.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"synthetic-new"}}"#,
    )
    .unwrap();
    assert_eq!(
        service.read_claude_auth(&account).await["loggedIn"],
        true,
        "credencial alterada invalida antes do prazo"
    );
    fs::write(account.home.join("reply.json"), "invalid-json").unwrap();
    service.claude_auth.invalidate(&key);
    assert_eq!(
        service.read_claude_auth(&account).await["estado"],
        "indisponivel"
    );
    fs::write(
        account.home.join("reply.json"),
        b"{\"loggedIn\":true,\"email\":\"fixture\xff@example.test\"}",
    )
    .unwrap();
    service.claude_auth.invalidate(&key);
    let recovered = service.read_claude_auth(&account).await;
    assert_eq!(
        recovered["loggedIn"], true,
        "um byte ilegível no texto não descarta o booleano autenticado"
    );
    assert_eq!(recovered["email"], serde_json::Value::Null);
    fs::remove_file(account.home.join(".credentials.json")).unwrap();
    fs::write(account.home.join("reply.json"), r#"{"loggedIn":true}"#).unwrap();
    assert_eq!(service.read_claude_auth(&account).await["loggedIn"], true);
    fs::write(account.home.join("reply.json"), r#"{"loggedIn":false}"#).unwrap();
    assert_eq!(service.read_claude_auth(&account).await["loggedIn"], true);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    tokio::time::resume();
    assert_eq!(
        service.read_claude_auth(&account).await["loggedIn"],
        false,
        "cache sem arquivo expira após 30 s"
    );
}

#[test]
fn malformed_oauth_block_is_not_an_absent_credential() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".credentials.json"),
        r#"{"claudeAiOauth":[]}"#,
    )
    .unwrap();
    assert!(
        hangar_server::accounts::claude_auth::token_signature(root.path(), true).is_err(),
        "bloco OAuth ilegível deve recusar antes de abrir a janela"
    );
}

#[test]
fn native_identity_text_keeps_the_reference_contract() {
    use hangar_server::accounts::claude_auth::auth_public;
    use serde_json::json;
    let value = auth_public(Some(
        json!({"loggedIn":true,"email":"fixture�@example.test","subscriptionType":" pro "}),
    ));
    assert_eq!(value["estado"], "ok");
    assert_eq!(
        value["email"],
        serde_json::Value::Null,
        "campo com byte ilegível não vira identidade pública"
    );
    assert_eq!(
        value["plano"], " pro ",
        "string válida conserva o contrato Python"
    );
    let value = auth_public(Some(
        json!({"loggedIn":false,"email":"","subscriptionType":[]}),
    ));
    assert_eq!(value["email"], "");
    assert_eq!(value["plano"], serde_json::Value::Null);
}

#[test]
fn onboarding_updates_both_default_readers_without_losing_configuration() {
    use hangar_server::accounts::{
        AccountService, catalog::Account, environment::AccountEnvironment,
    };
    use serde_json::Value;
    use std::fs;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("nested")).unwrap();
    let account_home = root.path().join(".claude");
    fs::create_dir(&account_home).unwrap();
    fs::write(account_home.join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();
    fs::write(root.path().join(".claude.json"), r#"{"existing":"kept"}"#).unwrap();
    let home = root.path().join("nested/..").to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    ));
    let account = Account {
        id: "default".into(),
        home: account_home.clone(),
        is_default: true,
    };
    service.complete_claude_onboarding(&account);
    let pane: Value =
        serde_json::from_slice(&fs::read(account_home.join(".claude.json")).unwrap()).unwrap();
    let terminal: Value =
        serde_json::from_slice(&fs::read(root.path().join(".claude.json")).unwrap()).unwrap();
    assert_eq!(pane["hasCompletedOnboarding"], true);
    assert_eq!(pane["theme"], "dark");
    assert_eq!(
        terminal["hasCompletedOnboarding"], true,
        "a conta padrão com caminho equivalente também publica no leitor do terminal"
    );
    assert_eq!(terminal["existing"], "kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_window_creation_and_missing_cli_release_the_attempt_guard() {
    use axum::{Router, http::StatusCode, routing::post};
    use hangar_server::accounts::{
        AccountKey, AccountService, GuardMode, Provider, claude_login::WindowClient,
        environment::AccountEnvironment,
    };
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.clone()),
            ("USERPROFILE".into(), home),
            ("PATH".into(), String::new()),
        ]
        .into(),
    ));
    let account = service
        .create(Provider::Claude, "work", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
    let failing = Arc::new(AtomicBool::new(true));
    let fail_open = failing.clone();
    let (actions, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let router = Router::new().route(
        "/internal/accounts/claude-window",
        post(move |body: axum::body::Bytes| {
            let value: Value = serde_json::from_slice(&body).unwrap();
            let action = value["action"].as_str().unwrap();
            actions.send(action.to_owned()).unwrap();
            let reject = action == "open" && fail_open.load(Ordering::SeqCst);
            async move {
                (
                    if reject {
                        StatusCode::CONFLICT
                    } else {
                        StatusCode::OK
                    },
                    json!({"ok": !reject}).to_string(),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = WindowClient::new(
        listener.local_addr().unwrap(),
        "synthetic".into(),
        "test".into(),
    )
    .unwrap();
    let server = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
    assert!(
        service
            .start_claude_login("work", client.clone())
            .await
            .is_err()
    );
    assert_eq!(observed.recv().await.as_deref(), Some("open"));
    assert_eq!(observed.recv().await.as_deref(), Some("close"));
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_ok()
    );
    assert_eq!(
        service.claude_step("work", client.clone()).await.unwrap()["etapa"],
        "idle"
    );
    failing.store(false, Ordering::SeqCst);
    service
        .start_claude_login("work", client.clone())
        .await
        .unwrap();
    assert_eq!(observed.recv().await.as_deref(), Some("open"));
    assert!(
        service
            .confirm_claude("work", "synthetic-code".into(), client.clone())
            .await
            .is_err()
    );
    assert_eq!(observed.recv().await.as_deref(), Some("code"));
    assert_eq!(observed.recv().await.as_deref(), Some("close"));
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_ok()
    );
    assert_eq!(
        service.claude_step("work", client).await.unwrap()["etapa"],
        "idle"
    );
    server.abort();
}
