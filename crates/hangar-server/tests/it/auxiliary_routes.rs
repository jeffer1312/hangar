use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};

use hangar_core::process::{CommandError, CommandOutput, Commands};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};

#[derive(Default)]
struct ScriptedCommands {
    replies: Mutex<VecDeque<Result<CommandOutput, CommandError>>>,
    calls: Mutex<Vec<(String, Vec<String>)>>,
}

impl Commands for ScriptedCommands {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        _deadline: Duration,
    ) -> futures_util::future::BoxFuture<'a, Result<CommandOutput, CommandError>> {
        self.calls.lock().unwrap().push((
            program.into(),
            args.iter().map(|arg| (*arg).into()).collect(),
        ));
        let response = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("comando externo previsto no roteiro");
        Box::pin(async move { response })
    }
}

struct Fixture {
    home: tempfile::TempDir,
    base: String,
    upstream_calls: Arc<AtomicUsize>,
    quota_path: std::path::PathBuf,
    native_commands: Arc<dyn Commands>,
    #[cfg(target_os = "linux")]
    router: Router,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Fixture {
    async fn start() -> Self {
        Self::with_commands(Arc::new(ScriptedCommands::default())).await
    }

    async fn with_commands(commands: Arc<dyn Commands>) -> Self {
        let home = tempfile::tempdir().unwrap();
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_calls = Arc::new(AtomicUsize::new(0));
        let calls = upstream_calls.clone();
        let upstream_task = tokio::spawn(async move {
            let router = Router::new().fallback(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::SERVICE_UNAVAILABLE }
            });
            axum::serve(upstream, router).await.unwrap();
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address,
            upstream: upstream_address,
            internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.origins_home = home.path().to_path_buf();
        state.accounts = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
                ("CP_DEPLOY_SECRET".into(), "fixture-secret".into()),
            ]
            .into(),
        ));
        let native_commands = state.local_commands.clone();
        state.local_commands = commands;
        let quota_path = state.accounts.quota_path();
        let router = hangar_server::routes::router(Arc::new(state));
        #[cfg(target_os = "linux")]
        let handler_router = router.clone();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            home,
            base: format!("http://{address}"),
            upstream_calls,
            quota_path,
            native_commands,
            #[cfg(target_os = "linux")]
            router: handler_router,
            tasks: vec![task, upstream_task],
        }
    }

    async fn stop(mut self) {
        let mut failures = Vec::new();
        for task in self.tasks.drain(..) {
            task.abort();
            match task.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => failures.push(error),
            }
        }
        assert!(
            failures.is_empty(),
            "servidor de prova entrou em pânico: {failures:?}"
        );
    }
}

async fn isolated(name: &str) -> bool {
    let exact = crate::exact_name(module_path!(), name);
    if std::env::var("HANGAR_AUX_TEST").as_deref() == Ok(exact.as_str()) {
        return true;
    }
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/usr/bin/install", bin.join("install")).unwrap();
    let mut environment = std::collections::BTreeMap::new();
    for name in ["SystemRoot", "WINDIR", "ComSpec"] {
        if let Ok(value) = std::env::var(name) {
            environment.insert(name.into(), value);
        }
    }
    for (key, path) in [
        ("HOME", root.path().to_path_buf()),
        ("USERPROFILE", root.path().to_path_buf()),
        ("LOCALAPPDATA", root.path().join("AppData/Local")),
        ("APPDATA", root.path().join("AppData/Roaming")),
        ("XDG_CONFIG_HOME", root.path().join(".config")),
        ("XDG_CACHE_HOME", root.path().join(".cache")),
        ("XDG_DATA_HOME", root.path().join(".local/share")),
        ("XDG_RUNTIME_DIR", root.path().join("run")),
        ("TMPDIR", root.path().join("tmp")),
        ("TEMP", root.path().join("tmp")),
        ("TMP", root.path().join("tmp")),
    ] {
        std::fs::create_dir_all(&path).unwrap();
        environment.insert(key.into(), path.to_string_lossy().into_owned());
    }
    environment.insert("PATH".into(), bin.to_string_lossy().into_owned());
    environment.insert("TZ".into(), "UTC".into());
    environment.insert("LANG".into(), "C.UTF-8".into());
    environment.insert("HANGAR_AUX_TEST".into(), exact.clone());
    let output = hangar_server::command_capture::capture_bytes(
        vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--exact".into(),
            exact,
            "--nocapture".into(),
        ],
        AccountEnvironment::from_map(environment),
        Duration::from_secs(180),
        Default::default(),
    )
    .await
    .expect("o processo isolado precisa terminar");
    assert!(
        output.success,
        "teste isolado falhou:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    crate::assert_ran_one(&output.stdout);
    false
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

#[cfg(unix)]
impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn native_deploy_failure_keeps_the_cause_and_scrubs_secrets() {
    if !isolated("native_deploy_failure_keeps_the_cause_and_scrubs_secrets").await {
        return;
    }
    let fixture = Fixture::start().await;
    let executable = std::path::PathBuf::from(std::env::var_os("PATH").unwrap()).join("systemctl");
    crate::write_executable(
        &executable,
        "#!/bin/sh\nprintf 'Unit hangar-deploy.service not found; token=secret-value\\n' >&2\nexit 1\n",
    );
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(buffer.clone())
        .finish();
    let result = tracing::instrument::WithSubscriber::with_subscriber(
        fixture.native_commands.run(
            "systemctl",
            &["--user", "start", "--no-block", "hangar-deploy.service"],
            Duration::from_secs(30),
        ),
        subscriber,
    )
    .await
    .unwrap();
    assert!(!result.success);
    let captured = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(
        captured.contains("hangar-deploy.service not found"),
        "a causa do systemctl precisa estar no diagnóstico"
    );
    assert!(captured.contains("service_start"));
    assert!(!captured.contains("secret-value"));
    fixture.stop().await;
}

#[tokio::test]
async fn encoded_paths_and_invalid_subpaths_never_reach_python() {
    if !isolated("encoded_paths_and_invalid_subpaths_never_reach_python").await {
        return;
    }
    let fixture = Fixture::start().await;
    let client = reqwest::Client::new();
    let response = client
        .put(format!("{}/api/credenciais/%63ookie", fixture.base))
        .bearer_auth("fixture-owner")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    for path in [
        "/api/preview/unknown",
        "/api/deploy/github-webhook/unknown",
        "/api/credenciais/cookie/unknown",
    ] {
        assert_eq!(
            client
                .get(format!("{}{path}", fixture.base))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn native_command_keeps_the_operating_system_error_cause() {
    if !isolated("native_command_keeps_the_operating_system_error_cause").await {
        return;
    }
    let fixture = Fixture::start().await;
    let program = fixture.home.path().join("incompatible.exe");
    std::fs::write(&program, "arquivo sem permissão ou formato de executável").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let result = fixture
        .native_commands
        .run(program.to_str().unwrap(), &[], Duration::from_secs(30))
        .await;
    let error = match result {
        Err(CommandError::Io(error)) => error,
        _ => panic!("a execução precisa falhar com a causa do sistema"),
    };
    assert!(
        error.raw_os_error().is_some(),
        "perder o errno esconde a causa da falha de instalação"
    );
    fixture.stop().await;
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn cookie_invalidation_finishes_when_the_request_is_cancelled() {
    if !isolated("cookie_invalidation_finishes_when_the_request_is_cancelled").await {
        return;
    }
    use notify::Watcher;
    use std::io::Write;
    use tower::ServiceExt;
    let fixture = Fixture::start().await;
    std::fs::create_dir_all(fixture.quota_path.parent().unwrap()).unwrap();
    std::fs::write(
        &fixture.quota_path,
        json!({"engine:oc":{"cota":{"estado":"lida"}}}).to_string(),
    )
    .unwrap();
    let watched = fixture.quota_path.canonicalize().unwrap();
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = changed.send(event);
    })
    .unwrap();
    watcher
        .watch(
            watched.parent().unwrap(),
            notify::RecursiveMode::NonRecursive,
        )
        .unwrap();
    let cookie = hangar_opencode_cookie::path(fixture.home.path());
    std::fs::create_dir_all(cookie.parent().unwrap()).unwrap();
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(&cookie)
            .status()
            .unwrap()
            .success()
    );
    let mut request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/credenciais/cookie")
        .header("authorization", "Bearer fixture-owner")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"id":"engine:oc","workspace_id":"new-workspace","auth_cookie":"new-cookie"})
                .to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let handler = tokio::spawn(fixture.router.clone().oneshot(request));
    // Abrir a outra ponta confirma que a gravação já entrou na leitura, que só termina no EOF.
    let fifo = cookie.clone();
    let writer =
        tokio::task::spawn_blocking(move || std::fs::OpenOptions::new().write(true).open(fifo));
    let mut writer = tokio::time::timeout(Duration::from_secs(60), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    handler.abort();
    assert!(handler.await.unwrap_err().is_cancelled());
    writer.write_all(b"{}").unwrap();
    drop(writer);
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let event = changes
                .recv()
                .await
                .expect("observador ativo")
                .expect("evento de arquivo válido");
            if event.paths.contains(&watched)
                && matches!(
                    event.kind,
                    notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                        | notify::EventKind::Create(_)
                )
            {
                break;
            }
        }
    })
    .await
    .expect("a invalidação precisa terminar mesmo após cancelar o handler");
    let cache: Value =
        serde_json::from_slice(&std::fs::read(&fixture.quota_path).unwrap()).unwrap();
    assert!(cache.get("engine:oc").is_none());
    drop(watcher);
    fixture.stop().await;
}

#[tokio::test]
async fn cookie_is_saved_without_python() {
    if !isolated("cookie_is_saved_without_python").await {
        return;
    }
    let fixture = Fixture::start().await;
    std::fs::create_dir_all(fixture.quota_path.parent().unwrap()).unwrap();
    std::fs::write(
        &fixture.quota_path,
        json!({"engine:oc":{"cota":{"estado":"lida"}}, "engine:other":{"cota":{"estado":"lida"}}})
            .to_string(),
    )
    .unwrap();
    let response = reqwest::Client::new()
        .put(format!("{}/api/credenciais/cookie", fixture.base))
        .bearer_auth("fixture-owner")
        .json(
            &json!({"id":"engine:oc", "workspace_id":" ws-1 ", "auth_cookie":" synthetic-cookie "}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "a escrita deve ser atendida pelo Rust"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "engine:oc");
    assert_eq!(body["cookie_definido"], true);
    assert!(body.get("auth_cookie").is_none());
    let saved: Value = serde_json::from_slice(
        &std::fs::read(fixture.home.path().join(".claude/.hangar-opencode.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["engine:oc"]["workspace_id"], "ws-1");
    assert_eq!(saved["engine:oc"]["auth_cookie"], "synthetic-cookie");
    let quotas: Value =
        serde_json::from_slice(&std::fs::read(&fixture.quota_path).unwrap()).unwrap();
    assert!(quotas.get("engine:oc").is_none());
    assert_eq!(quotas["engine:other"]["cota"]["estado"], "lida");
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn cookie_saved_stays_successful_when_quota_cache_cannot_be_written() {
    if !isolated("cookie_saved_stays_successful_when_quota_cache_cannot_be_written").await {
        return;
    }
    let fixture = Fixture::start().await;
    std::fs::create_dir_all(&fixture.quota_path).unwrap();
    let client = reqwest::Client::new();
    let url = format!("{}/api/credenciais/cookie", fixture.base);
    for body in [
        json!({"id":"a", "workspace_id":"w", "auth_cookie":"synthetic-cookie", "ignored":true}),
        json!({"id":"a", "workspace_id":"", "auth_cookie":"synthetic-cookie"}),
    ] {
        let expected = !body["workspace_id"].as_str().unwrap().is_empty();
        let response = client
            .put(&url)
            .bearer_auth("fixture-owner")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<Value>().await.unwrap()["cookie_definido"],
            expected
        );
    }
    let saved = hangar_opencode_cookie::read(fixture.home.path()).unwrap();
    assert!(!saved.contains_key("a"));
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn owner_forms_authenticate_validate_and_keep_method_contract_without_python() {
    if !isolated("owner_forms_authenticate_validate_and_keep_method_contract_without_python").await
    {
        return;
    }
    let fixture = Fixture::start().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for (path, method) in [
        ("/api/credenciais/cookie", reqwest::Method::PUT),
        ("/api/preview", reqwest::Method::POST),
    ] {
        let response = client
            .request(method.clone(), format!("{}{path}", fixture.base))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        let response = client
            .request(method, format!("{}{path}", fixture.base))
            .bearer_auth("fixture-owner")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 422);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["detail"][0]["type"], "missing");
    }
    for body in [
        json!({"id":7}),
        json!({"id":"a", "workspace_id":null}),
        json!({"id":"a", "auth_cookie":"x".repeat(4097)}),
    ] {
        let response = client
            .put(format!("{}/api/credenciais/cookie", fixture.base))
            .bearer_auth("fixture-owner")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 422);
    }
    for body in [
        json!({"port":0}),
        json!({"port":65536}),
        json!({"port":3000,"extra":true}),
        json!({"port":3000.5}),
        json!({"port":false}),
    ] {
        let response = client
            .post(format!("{}/api/preview", fixture.base))
            .bearer_auth("fixture-owner")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 422);
    }
    assert_eq!(
        client
            .head(format!("{}/api/preview", fixture.base))
            .send()
            .await
            .unwrap()
            .status(),
        405
    );
    let redirect = client
        .get(format!("{}/api/preview/", fixture.base))
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status(), 307);
    assert!(
        redirect.headers()["location"]
            .to_str()
            .unwrap()
            .ends_with("/api/preview")
    );
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn webhook_is_public_by_signature_and_preview_keeps_port_coercion_without_python() {
    if !isolated("webhook_is_public_by_signature_and_preview_keeps_port_coercion_without_python")
        .await
    {
        return;
    }
    let commands = Arc::new(ScriptedCommands::default());
    commands
        .replies
        .lock()
        .unwrap()
        .push_back(Ok(CommandOutput {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }));
    for _ in 0..3 {
        commands.replies.lock().unwrap().extend([
            Ok(CommandOutput {
                success: true,
                stdout: r#"{"Self":{"DNSName":"fixture.ts.net."}}"#.into(),
                stderr: String::new(),
            }),
            Ok(CommandOutput {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            }),
        ]);
    }
    let fixture = Fixture::with_commands(commands.clone()).await;
    let client = reqwest::Client::new();
    let bytes = br#"{"ref":"refs/heads/main"}"#;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"fixture-secret");
    let digest = ring::hmac::sign(&key, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let response = client
        .post(format!("{}/api/deploy/github-webhook", fixture.base))
        .header("x-github-event", "push")
        .header("x-hub-signature-256", format!("sha256={digest}"))
        .body(bytes.as_slice())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["status"],
        "accepted"
    );
    for (input, expected) in [
        (json!("3000"), 3000),
        (json!(3000.0), 3000),
        (json!(true), 1),
    ] {
        let response = client
            .post(format!("{}/api/preview", fixture.base))
            .bearer_auth("fixture-owner")
            .json(&json!({"port":input}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.json::<Value>().await.unwrap()["port"], expected);
    }
    assert_eq!(commands.calls.lock().unwrap()[0].0, "systemctl");
    assert_eq!(fixture.upstream_calls.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}
