use futures_util::future::BoxFuture;
use hangar_server::accounts::{AccountKey, claude_login::WindowClient, claude_window::WindowBackend};
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

/// Janela do processo de prova: a referência Python guarda as janelas falsas e a classificação do código.
struct ContractWindows { client: reqwest::Client, url: String }
impl WindowBackend for ContractWindows {
    fn call<'a>(&'a self, key: &'a AccountKey, operation: &'a str, action: &'a str, code: Option<&'a str>)
        -> BoxFuture<'a, Result<Value, &'static str>> {
        Box::pin(async move {
            let body = json!({"key": key, "operation": operation, "action": action, "code": code});
            let response = self.client.post(&self.url).json(&body).send().await.map_err(|_| "contract_window_unreachable")?;
            if !response.status().is_success() { return Err("contract_window_refused"); }
            response.json().await.map_err(|_| "contract_window_unreadable")
        })
    }
    fn sweep(&self) -> BoxFuture<'_, ()> { Box::pin(async {}) }
}

/// Janela falsa: registra a ação; `refuse_open` recusa a abertura.
struct Recorder { actions: tokio::sync::mpsc::UnboundedSender<String>, refuse_open: Arc<std::sync::atomic::AtomicBool> }
impl WindowBackend for Recorder {
    fn call<'a>(&'a self, _: &'a AccountKey, _: &'a str, action: &'a str, _: Option<&'a str>) -> BoxFuture<'a, Result<Value, &'static str>> {
        self.actions.send(action.to_owned()).unwrap();
        let refused = action == "open" && self.refuse_open.load(std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move { if refused { Err("window_test_refused") } else { Ok(json!({"ok":true,"url":null})) } })
    }
    fn sweep(&self) -> BoxFuture<'_, ()> { Box::pin(async {}) }
}

fn recorder(refuse_open: Arc<std::sync::atomic::AtomicBool>) -> (WindowClient, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let (actions, observed) = tokio::sync::mpsc::unbounded_channel();
    (WindowClient::with(Arc::new(Recorder { actions, refuse_open })), observed)
}

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
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: upstream.parse().unwrap(),
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts.claude_windows = WindowClient::with(Arc::new(ContractWindows {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        url: format!("http://{upstream}/__contract__/window"),
    }));
    println!("ACCOUNT_HTTP:{addr}");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    hangar_server::serve_until_with_state(listener, state, std::future::pending::<()>())
        .await
        .unwrap();
}

#[tokio::test]
async fn open_attempt_keeps_its_window_and_guard_after_300_seconds() {
    use hangar_server::accounts::{AccountService, GuardMode, Provider, environment::AccountEnvironment};
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    ));
    let account = service
        .create(Provider::Claude, "work", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
    let (client, mut observed) = recorder(Default::default());
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
        
        crate::write_executable(&fixture.join("claude"), format!("#!/usr/bin/env node\n{source}"));
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

#[cfg(windows)]
#[test]
fn onboarding_waits_for_a_reader_without_delete_sharing() {
    use hangar_server::accounts::{
        AccountService, catalog::Account, environment::AccountEnvironment,
    };
    use serde_json::Value;
    use std::{fs, os::windows::fs::OpenOptionsExt};
    let root = tempfile::tempdir().unwrap();
    let account_home = root.path().join(".claude-work");
    fs::create_dir(&account_home).unwrap();
    let config = account_home.join(".claude.json");
    fs::write(&config, r#"{"theme":"dark"}"#).unwrap();
    let home = root.path().to_string_lossy().into_owned();
    let service = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    ));
    // Um CLI lendo o arquivo: só FILE_SHARE_READ, como o open() do Python e do Node.
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(&config)
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(60));
        drop(reader);
    });
    service.complete_claude_onboarding(&Account {
        id: "work".into(),
        home: account_home,
        is_default: false,
    });
    release.join().unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(saved["hasCompletedOnboarding"], true);
    assert_eq!(saved["theme"], "dark");
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_window_creation_and_missing_cli_release_the_attempt_guard() {
    use hangar_server::accounts::{AccountService, GuardMode, Provider, environment::AccountEnvironment};
    use std::sync::atomic::{AtomicBool, Ordering};
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
    let (client, mut observed) = recorder(failing.clone());
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
}
