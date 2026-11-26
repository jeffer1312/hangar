use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use hangar_core::terminal_process::{PRIVATE_ENV_KEYS, pid_alive};
use hangar_integrations::{
    AuthConfig, ClientConfig, ClientError, IntegrationConfig, IntegrationRegistry,
    IntegrationSecrets, McpCallOutput, McpClientManager, MemoryKeyStore, OAuthTokens,
    PermissionPolicy, SecretStore, TokenRefresher, ToolAccess, ToolsCache, Transport,
    classify_tool_access,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Barrier, Notify, RwLock};
use zeroize::Zeroizing;

struct TestEnv {
    _dir: TempDir,
    registry: Arc<IntegrationRegistry>,
    secrets: Arc<SecretStore>,
    tools_cache: Arc<ToolsCache>,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("criar diretório temporário");
        let registry = Arc::new(IntegrationRegistry::new(dir.path()));
        let key_store = Arc::new(MemoryKeyStore::new());
        let secrets = Arc::new(SecretStore::new(dir.path(), key_store));
        let tools_cache = Arc::new(ToolsCache::new());
        Self {
            _dir: dir,
            registry,
            secrets,
            tools_cache,
        }
    }

    fn manager(&self, config: ClientConfig) -> McpClientManager {
        McpClientManager::new(
            self.registry.clone(),
            self.secrets.clone(),
            self.tools_cache.clone(),
            config,
        )
    }
}

fn fixture_bin() -> String {
    env!("CARGO_BIN_EXE_hangar-fixture-mcp").to_owned()
}

fn stdio_config(
    id: &str,
    policy: PermissionPolicy,
    extra_env_keys: Vec<String>,
) -> IntegrationConfig {
    IntegrationConfig {
        id: id.to_owned(),
        name: format!("Fixture {id}"),
        preset: None,
        enabled: true,
        transport: Transport::LocalStdio {
            command: vec![fixture_bin()],
            cwd: None,
            env_keys: extra_env_keys,
        },
        auth: AuthConfig::None,
        policy,
        pinned: false,
        projects: vec![],
        allowed_tools: BTreeSet::new(),
        created_at: 1.0,
        updated_at: 1.0,
    }
}

fn http_config(
    id: &str,
    url: String,
    auth: AuthConfig,
    header_names: Vec<String>,
) -> IntegrationConfig {
    IntegrationConfig {
        id: id.to_owned(),
        name: format!("HTTP {id}"),
        preset: None,
        enabled: true,
        transport: Transport::RemoteHttp { url, header_names },
        auth,
        policy: PermissionPolicy::AllowAll,
        pinned: false,
        projects: vec![],
        allowed_tools: BTreeSet::new(),
        created_at: 1.0,
        updated_at: 1.0,
    }
}

async fn wait_until_idle_stopped(manager: &McpClientManager, id: &str, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if !manager.running_ids().iter().any(|running| running == id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!(
        "integração '{id}' não encerrou dentro de {:?}; ainda rodando: {:?}",
        deadline,
        manager.running_ids()
    );
}

async fn wait_for_file(path: &Path, deadline: Duration) -> String {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Ok(content) = tokio::fs::read_to_string(path).await
            && !content.trim().is_empty()
        {
            return content;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "arquivo sinal '{}' não apareceu dentro de {:?}",
        path.display(),
        deadline
    );
}

async fn wait_until_pid_dead(pid: u32, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if !pid_alive(pid).unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("processo {pid} ainda estava vivo após {:?}", deadline);
}

#[derive(Clone)]
struct HttpFixtureHandler;

impl ServerHandler for HttpFixtureHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = Arc::new(
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let read_echo = Tool::new("read_echo", "Echo de leitura", schema)
            .with_annotations(ToolAnnotations::new().read_only(true));
        Ok(ListToolsResult::with_all_items(vec![read_echo]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let headers = context
            .extensions
            .get::<axum::http::request::Parts>()
            .map(|p| p.headers.clone())
            .unwrap_or_default();
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let custom = headers
            .get("x-workspace-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        match request.name.as_ref() {
            "read_echo" => {
                let text = request
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                    ContentBlock::text(format!("{text}|auth={auth}|ws={custom}")),
                ])))
            }
            _ => Err(ErrorData::invalid_params("tool not found", None)),
        }
    }
}

#[derive(Clone, Default)]
struct AuthGateState {
    expected_token: Arc<RwLock<String>>,
    required_custom_header: Option<(&'static str, &'static str)>,
    include_www_authenticate: bool,
    sync_401_barrier: Option<Arc<Barrier>>,
    barrier_timed_out: Arc<AtomicBool>,
    notify_on_401_at: Option<(Arc<AtomicUsize>, usize, Arc<Notify>)>,
}

async fn auth_gate(State(state): State<AuthGateState>, req: Request, next: Next) -> Response {
    let expected = state.expected_token.read().await.clone();
    let expected_header = format!("Bearer {expected}");
    let auth_ok = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        == Some(expected_header.as_str());
    if !auth_ok {
        if let Some(barrier) = &state.sync_401_barrier
            && tokio::time::timeout(Duration::from_secs(2), barrier.wait())
                .await
                .is_err()
        {
            state.barrier_timed_out.store(true, Ordering::SeqCst);
        }
        if let Some((counter, target, notify)) = &state.notify_on_401_at
            && counter.fetch_add(1, Ordering::SeqCst) + 1 == *target
        {
            // `notify_one` preserva o sinal mesmo se a espera for criada após o segundo 401.
            notify.notify_one();
        }
        let mut resp = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        if state.include_www_authenticate {
            resp.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                HeaderValue::from_static(r#"Bearer error="invalid_token""#),
            );
        }
        return resp;
    }
    if let Some((name, value)) = state.required_custom_header {
        let custom_ok = req.headers().get(name).and_then(|v| v.to_str().ok()) == Some(value);
        if !custom_ok {
            return (StatusCode::FORBIDDEN, "missing custom header").into_response();
        }
    }
    next.run(req).await
}

async fn spawn_http_mcp_server(state: AuthGateState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind porta loopback");
    let addr = listener.local_addr().expect("ler porta local");
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(["127.0.0.1", "localhost"])
        .disable_allowed_hosts();
    let service = StreamableHttpService::new(
        || Ok(HttpFixtureHandler),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new()
        .route_service("/mcp", service.clone())
        .route_service("/mcp/", service)
        .layer(axum::middleware::from_fn_with_state(state, auth_gate));
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, handle)
}

struct MockRefresher {
    calls: AtomicUsize,
    next_token: String,
    expected_token_gate: Option<Arc<RwLock<String>>>,
}

impl TokenRefresher for MockRefresher {
    fn refresh<'a>(
        &'a self,
        _integration_id: &'a str,
    ) -> BoxFuture<'a, Result<String, ClientError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.expected_token_gate {
                *gate.write().await = self.next_token.clone();
            }
            Ok(self.next_token.clone())
        })
    }
}

#[tokio::test]
async fn lazy_start_lists_tools_populates_cache_and_classifies_access() {
    let env = TestEnv::new();
    let mut config = stdio_config("fixture-a", PermissionPolicy::ReadFreeAskWrite, vec![]);
    config.allowed_tools.insert("write_action".to_owned());
    env.registry.create(config.clone()).unwrap();

    let manager = env.manager(ClientConfig {
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    });
    assert!(
        manager.running_ids().is_empty(),
        "cadastro e criação do manager não podem subir o processo stdio"
    );

    let tools = manager.list_tools("fixture-a").await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expected in [
        "read_echo",
        "write_action",
        "unannotated_tool",
        "slow_tool",
        "dump_env",
    ] {
        assert!(
            names.contains(&expected),
            "faltou ferramenta {expected}: {names:?}"
        );
    }
    assert_eq!(manager.running_ids(), vec!["fixture-a".to_owned()]);

    let cached = env
        .tools_cache
        .get("fixture-a")
        .expect("tools_cache populado");
    assert_eq!(cached, tools);

    let read_echo = cached.iter().find(|t| t.name == "read_echo").unwrap();
    let write_action = cached.iter().find(|t| t.name == "write_action").unwrap();
    let unannotated = cached
        .iter()
        .find(|t| t.name == "unannotated_tool")
        .unwrap();
    assert_eq!(read_echo.read_only_hint, Some(true));
    assert_eq!(write_action.read_only_hint, Some(false));
    assert_eq!(unannotated.read_only_hint, None);

    // Fail-closed: ausência de readOnlyHint é tratada como escrita.
    let empty_allowed = BTreeSet::new();
    assert_eq!(
        classify_tool_access(
            PermissionPolicy::ReadFreeAskWrite,
            &empty_allowed,
            Some(true),
            "read_echo"
        ),
        ToolAccess::Read
    );
    assert_eq!(
        classify_tool_access(
            PermissionPolicy::ReadFreeAskWrite,
            &empty_allowed,
            Some(false),
            "write_action"
        ),
        ToolAccess::AskWrite
    );
    assert_eq!(
        classify_tool_access(
            PermissionPolicy::ReadFreeAskWrite,
            &empty_allowed,
            None,
            "unannotated_tool"
        ),
        ToolAccess::AskWrite
    );
    assert_eq!(
        classify_tool_access(
            PermissionPolicy::ReadOnly,
            &empty_allowed,
            None,
            "unannotated_tool"
        ),
        ToolAccess::Blocked
    );
    assert_eq!(
        classify_tool_access(
            PermissionPolicy::ReadOnly,
            &empty_allowed,
            Some(true),
            "read_echo"
        ),
        ToolAccess::Read
    );
    assert_eq!(
        classify_tool_access(
            PermissionPolicy::AllowAll,
            &empty_allowed,
            None,
            "unannotated_tool"
        ),
        ToolAccess::AllowedAlways
    );

    let summaries = env
        .tools_cache
        .summaries(&config)
        .expect("summaries no cache");
    let summary_map: BTreeMap<&str, ToolAccess> = summaries
        .iter()
        .map(|s| (s.name.as_str(), s.effective_access))
        .collect();
    assert_eq!(summary_map["read_echo"], ToolAccess::Read);
    assert_eq!(summary_map["write_action"], ToolAccess::AllowedAlways);
    assert_eq!(summary_map["unannotated_tool"], ToolAccess::AskWrite);

    let echo: McpCallOutput = manager
        .call_tool("fixture-a", "read_echo", json!({"text": "alo-mcp"}))
        .await
        .unwrap();
    assert!(!echo.is_error);
    assert_eq!(echo.text, "alo-mcp");

    let unannotated_out = manager
        .call_tool("fixture-a", "unannotated_tool", json!({}))
        .await
        .unwrap();
    assert_eq!(unannotated_out.text, "ok-unannotated");

    manager.shutdown().await;
    assert!(manager.running_ids().is_empty());
}

#[test]
fn stdio_sanitizes_private_env_and_injects_secrets_env() {
    let exe = std::env::current_exe().expect("resolver executável do teste");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "--exact",
        "stdio_sanitizes_private_env_child_probe",
        "--nocapture",
    ])
    .env("HANGAR_TEST_SUBPROCESS", "1");
    for key in PRIVATE_ENV_KEYS {
        cmd.env(key, "segredo-pai-proibido");
    }
    let output = cmd.output().expect("executar subprocesso de sanitização");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "subprocesso de sanitização falhou: status={:?}\nstdout={stdout}\nstderr={stderr}",
        output.status.code()
    );
}

#[tokio::test]
async fn stdio_sanitizes_private_env_child_probe() {
    if std::env::var_os("HANGAR_TEST_SUBPROCESS").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }

    let env = TestEnv::new();
    let config = stdio_config(
        "env-test",
        PermissionPolicy::AllowAll,
        vec!["FIXTURE_SECRET_VAR".to_owned()],
    );
    env.registry.create(config).unwrap();

    let mut secrets = IntegrationSecrets::default();
    secrets.env.insert(
        "FIXTURE_SECRET_VAR".to_owned(),
        Zeroizing::new("valor-secreto-ok".to_owned()),
    );
    // Mesmo que alguém tente gravar chave privada do servidor em `secrets.env` (inclusive em caixa diferente), ela não deve atravessar.
    secrets.env.insert(
        "CP_AUTH_TOKEN".to_owned(),
        Zeroizing::new("tentativa-override".to_owned()),
    );
    secrets.env.insert(
        "cp_auth_token".to_owned(),
        Zeroizing::new("tentativa-lowercase".to_owned()),
    );
    env.secrets.set("env-test", secrets).await.unwrap();

    let manager = env.manager(ClientConfig {
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    });
    let output = manager
        .call_tool("env-test", "dump_env", json!({}))
        .await
        .unwrap();
    let payload: Value = serde_json::from_str(&output.text).expect("dump_env retorna JSON");
    let pid = payload["pid"].as_u64().expect("pid presente") as u32;
    let private_present = payload["private_present"]
        .as_array()
        .expect("private_present array");
    assert!(
        private_present.is_empty(),
        "chaves privadas vazaram para o processo stdio: {private_present:?}"
    );
    for key in PRIVATE_ENV_KEYS {
        assert!(
            !private_present.iter().any(|v| v.as_str() == Some(key)),
            "chave privada {key} presente"
        );
    }
    assert_eq!(payload["fixture_secret_var"], "valor-secreto-ok");

    manager.shutdown().await;
    wait_until_pid_dead(pid, Duration::from_secs(2)).await;
    assert!(!pid_alive(pid).unwrap());
}

#[tokio::test]
async fn idle_ttl_terminates_stdio_process_and_respawns_on_next_call() {
    let env = TestEnv::new();
    env.registry
        .create(stdio_config(
            "idle-test",
            PermissionPolicy::AllowAll,
            vec![],
        ))
        .unwrap();

    let manager = env.manager(ClientConfig {
        idle_ttl: Duration::from_millis(120),
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    });

    let first = manager
        .call_tool("idle-test", "dump_env", json!({}))
        .await
        .unwrap();
    let pid1 = serde_json::from_str::<Value>(&first.text).unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    assert!(pid_alive(pid1).unwrap());
    assert_eq!(manager.running_ids(), vec!["idle-test".to_owned()]);

    wait_until_idle_stopped(&manager, "idle-test", Duration::from_secs(2)).await;
    wait_until_pid_dead(pid1, Duration::from_secs(2)).await;
    assert!(
        !pid_alive(pid1).unwrap(),
        "processo ocioso deveria encerrar"
    );

    let second = manager
        .call_tool("idle-test", "dump_env", json!({}))
        .await
        .unwrap();
    let pid2 = serde_json::from_str::<Value>(&second.text).unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    assert_ne!(pid1, pid2, "nova chamada deve subir nova vida limpa");
    assert!(pid_alive(pid2).unwrap());

    manager.shutdown().await;
    assert!(!pid_alive(pid2).unwrap());
}

#[tokio::test]
async fn call_timeout_kills_stuck_stdio_without_blocking_other_integrations() {
    let env = TestEnv::new();
    let started_signal = env._dir.path().join("slow_started.flag");
    env.registry
        .create(stdio_config(
            "slow-int",
            PermissionPolicy::AllowAll,
            vec!["FIXTURE_SLOW_STARTED_FILE".to_owned()],
        ))
        .unwrap();
    env.secrets
        .set(
            "slow-int",
            IntegrationSecrets {
                env: BTreeMap::from([(
                    "FIXTURE_SLOW_STARTED_FILE".to_owned(),
                    Zeroizing::new(started_signal.display().to_string()),
                )]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();
    env.registry
        .create(stdio_config("fast-int", PermissionPolicy::AllowAll, vec![]))
        .unwrap();

    let manager = Arc::new(env.manager(ClientConfig {
        call_timeout: Duration::from_millis(300),
        exit_grace: Duration::from_millis(100),
        idle_ttl: Duration::from_secs(60),
        ..ClientConfig::default()
    }));

    let dump = manager
        .call_tool("slow-int", "dump_env", json!({}))
        .await
        .unwrap();
    let slow_pid = serde_json::from_str::<Value>(&dump.text).unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    assert!(pid_alive(slow_pid).unwrap());

    // Subcaso A: independência entre integrações coordenada por arquivo-sinal.
    let release_signal = env._dir.path().join("slow_release.flag");
    env.registry
        .create(stdio_config(
            "gated-slow",
            PermissionPolicy::AllowAll,
            vec![
                "FIXTURE_SLOW_STARTED_FILE".to_owned(),
                "FIXTURE_SLOW_RELEASE_FILE".to_owned(),
            ],
        ))
        .unwrap();
    let gated_started = env._dir.path().join("gated_slow_started.flag");
    env.secrets
        .set(
            "gated-slow",
            IntegrationSecrets {
                env: BTreeMap::from([
                    (
                        "FIXTURE_SLOW_STARTED_FILE".to_owned(),
                        Zeroizing::new(gated_started.display().to_string()),
                    ),
                    (
                        "FIXTURE_SLOW_RELEASE_FILE".to_owned(),
                        Zeroizing::new(release_signal.display().to_string()),
                    ),
                ]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();
    let indep_manager = Arc::new(env.manager(ClientConfig {
        call_timeout: Duration::from_secs(5),
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    }));
    // Aquece `fast-int` no mesmo gerenciador do subcaso A antes de iniciar a chamada lenta.
    let warmup = indep_manager
        .call_tool("fast-int", "read_echo", json!({"text": "warmup"}))
        .await
        .unwrap();
    assert_eq!(warmup.text, "warmup");
    let m_gated = Arc::clone(&indep_manager);
    let gated_task = tokio::spawn(async move {
        m_gated
            .call_tool("gated-slow", "slow_tool", json!({"delay_ms": 5_000}))
            .await
    });
    wait_for_file(&gated_started, Duration::from_secs(3)).await;
    let fast_out = indep_manager
        .call_tool("fast-int", "read_echo", json!({"text": "paralelo"}))
        .await
        .unwrap();
    assert_eq!(fast_out.text, "paralelo");
    assert!(
        !gated_task.is_finished(),
        "chamada lenta ainda deveria aguardar o sinal quando fast-int respondeu"
    );
    tokio::fs::write(&release_signal, b"go").await.unwrap();
    let gated_res = gated_task.await.unwrap().unwrap();
    assert_eq!(gated_res.text, "slow-done");
    indep_manager.shutdown().await;

    // Subcaso B: encerramento por timeout mata a árvore do processo preso.
    let m_slow = manager.clone();
    let slow_task = tokio::spawn(async move {
        m_slow
            .call_tool("slow-int", "slow_tool", json!({"delay_ms": 10_000}))
            .await
    });
    wait_for_file(&started_signal, Duration::from_secs(2)).await;

    let slow_res = slow_task.await.unwrap();
    assert!(
        matches!(slow_res, Err(ClientError::Timeout(_))),
        "esperado Timeout, veio {slow_res:?}"
    );
    wait_until_pid_dead(slow_pid, Duration::from_secs(2)).await;
    assert!(
        !manager.running_ids().iter().any(|id| id == "slow-int"),
        "slow-int deve ter sido removida de running_ids"
    );

    manager.shutdown().await;
}

#[tokio::test]
async fn invalidate_and_shutdown_terminate_process_trees_immediately() {
    let env = TestEnv::new();
    env.registry
        .create(stdio_config("int-1", PermissionPolicy::AllowAll, vec![]))
        .unwrap();
    env.registry
        .create(stdio_config("int-2", PermissionPolicy::AllowAll, vec![]))
        .unwrap();

    let manager = env.manager(ClientConfig {
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    });

    let p1 = serde_json::from_str::<Value>(
        &manager
            .call_tool("int-1", "dump_env", json!({}))
            .await
            .unwrap()
            .text,
    )
    .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    let p2 = serde_json::from_str::<Value>(
        &manager
            .call_tool("int-2", "dump_env", json!({}))
            .await
            .unwrap()
            .text,
    )
    .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    assert!(pid_alive(p1).unwrap() && pid_alive(p2).unwrap());

    manager.invalidate("int-1").await;
    assert!(!pid_alive(p1).unwrap(), "invalidate deve matar int-1");
    assert!(pid_alive(p2).unwrap(), "int-2 deve permanecer viva");
    assert_eq!(manager.running_ids(), vec!["int-2".to_owned()]);

    manager.shutdown().await;
    assert!(!pid_alive(p2).unwrap(), "shutdown deve matar int-2");
    assert!(manager.running_ids().is_empty());
}

#[tokio::test]
async fn remote_http_sends_bearer_and_custom_headers() {
    let state = AuthGateState {
        expected_token: Arc::new(RwLock::new("tok-bearer-123".to_owned())),
        required_custom_header: Some(("x-workspace-id", "ws-99")),
        include_www_authenticate: true,
        ..Default::default()
    };
    let (addr, server_handle) = spawn_http_mcp_server(state).await;

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "http-bearer",
            format!("http://{addr}/mcp"),
            AuthConfig::Bearer,
            vec!["X-Workspace-Id".to_owned()],
        ))
        .unwrap();

    let secrets = IntegrationSecrets {
        bearer_token: Some(Zeroizing::new("tok-bearer-123".to_owned())),
        headers: BTreeMap::from([(
            "X-Workspace-Id".to_owned(),
            Zeroizing::new("ws-99".to_owned()),
        )]),
        ..IntegrationSecrets::default()
    };
    env.secrets.set("http-bearer", secrets).await.unwrap();

    let manager = env.manager(ClientConfig::default());
    let tools = manager.list_tools("http-bearer").await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "read_echo");

    let out = manager
        .call_tool("http-bearer", "read_echo", json!({"text": "ping-http"}))
        .await
        .unwrap();
    assert_eq!(out.text, "ping-http|auth=Bearer tok-bearer-123|ws=ws-99");

    manager.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn remote_http_oauth_refreshes_once_on_401_and_returns_auth_expired_if_still_401() {
    let expected_token_gate = Arc::new(RwLock::new("token-valido-v2".to_owned()));
    let state = AuthGateState {
        expected_token: expected_token_gate.clone(),
        required_custom_header: None,
        include_www_authenticate: true,
        ..Default::default()
    };
    let (addr, server_handle) = spawn_http_mcp_server(state).await;

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "http-oauth",
            format!("http://{addr}/mcp"),
            AuthConfig::OAuthDcr,
            vec![],
        ))
        .unwrap();

    let refresher = Arc::new(MockRefresher {
        calls: AtomicUsize::new(0),
        next_token: "token-valido-v2".to_owned(),
        expected_token_gate: None,
    });
    let manager = env
        .manager(ClientConfig::default())
        .with_refresher(refresher.clone());

    // Sem tokens OAuth gravados ainda -> AuthRequired sem acionar refresher.
    let missing = manager.list_tools("http-oauth").await;
    assert!(
        matches!(missing, Err(ClientError::AuthRequired(_))),
        "sem oauth deve falhar com AuthRequired, veio {missing:?}"
    );
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 0);

    // Grava token expirado; primeiro 401 dispara refresh 1 vez e conclui a chamada com o novo token.
    env.secrets
        .set(
            "http-oauth",
            IntegrationSecrets {
                oauth: Some(OAuthTokens {
                    access_token: Zeroizing::new("token-expirado-v1".to_owned()),
                    ..OAuthTokens::default()
                }),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let out = manager
        .call_tool("http-oauth", "read_echo", json!({"text": "oauth-ok"}))
        .await
        .unwrap();
    assert_eq!(out.text, "oauth-ok|auth=Bearer token-valido-v2|ws=");
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 1);

    // Servidor gira o token para outro e o refresher continua devolvendo token inválido -> AuthExpired (refresh chamado 1 vez).
    *expected_token_gate.write().await = "token-v3-remoto".to_owned();
    let stale_refresher = Arc::new(MockRefresher {
        calls: AtomicUsize::new(0),
        next_token: "token-ainda-rejeitado".to_owned(),
        expected_token_gate: None,
    });
    let manager_still_401 = env
        .manager(ClientConfig::default())
        .with_refresher(stale_refresher.clone());

    let expired = manager_still_401
        .call_tool("http-oauth", "read_echo", json!({"text": "fail"}))
        .await;
    assert!(
        matches!(expired, Err(ClientError::AuthExpired(_))),
        "401 persistente pós-refresh deve retornar AuthExpired, veio {expired:?}"
    );
    assert_eq!(
        stale_refresher.calls.load(Ordering::SeqCst),
        1,
        "refresh deve tentar exatamente 1 vez"
    );

    manager.shutdown().await;
    manager_still_401.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn remote_http_oauth_handles_401_without_www_authenticate_header() {
    let state = AuthGateState {
        expected_token: Arc::new(RwLock::new("token-bom".to_owned())),
        required_custom_header: None,
        include_www_authenticate: false,
        ..Default::default()
    };
    let (addr, server_handle) = spawn_http_mcp_server(state).await;

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "http-oauth-no-hdr",
            format!("http://{addr}/mcp"),
            AuthConfig::OAuthManualClient {
                client_id: "cid".to_owned(),
                scopes: vec!["read".to_owned()],
            },
            vec![],
        ))
        .unwrap();
    env.secrets
        .set(
            "http-oauth-no-hdr",
            IntegrationSecrets {
                oauth: Some(OAuthTokens {
                    access_token: Zeroizing::new("token-velho".to_owned()),
                    ..OAuthTokens::default()
                }),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let refresher = Arc::new(MockRefresher {
        calls: AtomicUsize::new(0),
        next_token: "token-bom".to_owned(),
        expected_token_gate: None,
    });
    let manager = env
        .manager(ClientConfig::default())
        .with_refresher(refresher.clone());

    let out = manager
        .call_tool(
            "http-oauth-no-hdr",
            "read_echo",
            json!({"text": "sem-www-auth"}),
        )
        .await
        .unwrap();
    assert_eq!(out.text, "sem-www-auth|auth=Bearer token-bom|ws=");
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 1);

    manager.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn list_tools_paginates_and_handles_disabled_tool_not_found_and_is_error() {
    let env = TestEnv::new();

    let mut paginated = stdio_config(
        "paginated",
        PermissionPolicy::AllowAll,
        vec!["FIXTURE_PAGINATE".to_owned()],
    );
    paginated.enabled = false;
    env.registry.create(paginated).unwrap();

    env.secrets
        .set(
            "paginated",
            IntegrationSecrets {
                env: BTreeMap::from([(
                    "FIXTURE_PAGINATE".to_owned(),
                    Zeroizing::new("1".to_owned()),
                )]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let manager = env.manager(ClientConfig {
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    });

    // Integração desabilitada recusa antes de subir processo.
    assert_eq!(
        manager.list_tools("paginated").await,
        Err(ClientError::Disabled)
    );
    assert!(manager.running_ids().is_empty());

    // Habilita e verifica paginação por next_cursor.
    env.registry
        .update(
            "paginated",
            hangar_integrations::IntegrationUpdate {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

    let tools = manager.list_tools("paginated").await.unwrap();
    assert_eq!(tools.len(), 5);

    // Erro de ferramenta (`is_error == true`) preserva texto e flag sem virar `Err`.
    let tool_err = manager
        .call_tool("paginated", "tool_error_sample", json!({}))
        .await
        .unwrap();
    assert!(tool_err.is_error);
    assert_eq!(tool_err.text, "falha controlada da ferramenta");

    // Ferramenta desconhecida retorna `ToolNotFound`.
    let missing = manager
        .call_tool("paginated", "inexistente", json!({}))
        .await;
    assert_eq!(
        missing,
        Err(ClientError::ToolNotFound("inexistente".to_owned()))
    );

    manager.shutdown().await;
}

#[tokio::test]
async fn remote_http_oauth_refreshes_on_mid_session_401_and_requires_refresher() {
    let expected_token_gate = Arc::new(RwLock::new("token-inicial-v1".to_owned()));
    let state = AuthGateState {
        expected_token: expected_token_gate.clone(),
        required_custom_header: None,
        include_www_authenticate: false,
        ..Default::default()
    };
    let (addr, server_handle) = spawn_http_mcp_server(state).await;

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "http-mid-401",
            format!("http://{addr}/mcp"),
            AuthConfig::OAuthDcr,
            vec![],
        ))
        .unwrap();
    env.secrets
        .set(
            "http-mid-401",
            IntegrationSecrets {
                oauth: Some(OAuthTokens {
                    access_token: Zeroizing::new("token-inicial-v1".to_owned()),
                    ..OAuthTokens::default()
                }),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let refresher = Arc::new(MockRefresher {
        calls: AtomicUsize::new(0),
        next_token: "token-girado-v2".to_owned(),
        expected_token_gate: Some(expected_token_gate.clone()),
    });
    let manager = env
        .manager(ClientConfig::default())
        .with_refresher(refresher.clone());

    // Primeira chamada abre sessão normalmente com v1.
    let ok1 = manager
        .call_tool("http-mid-401", "read_echo", json!({"text": "antes"}))
        .await
        .unwrap();
    assert_eq!(ok1.text, "antes|auth=Bearer token-inicial-v1|ws=");
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 0);

    // Servidor passa a rejeitar v1 no meio da sessão; chamada seguinte dispara 401 -> refresh 1 vez -> v2.
    *expected_token_gate.write().await = "outro-token-que-refresher-vai-atualizar".to_owned();
    let ok2 = manager
        .call_tool("http-mid-401", "read_echo", json!({"text": "depois"}))
        .await
        .unwrap();
    assert_eq!(ok2.text, "depois|auth=Bearer token-girado-v2|ws=");
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 1);

    // Chamada subsequente reutiliza o token renovado sem novo refresh.
    let ok3 = manager
        .call_tool("http-mid-401", "read_echo", json!({"text": "terceira"}))
        .await
        .unwrap();
    assert_eq!(ok3.text, "terceira|auth=Bearer token-girado-v2|ws=");
    assert_eq!(refresher.calls.load(Ordering::SeqCst), 1);

    // Manager sem refresher recebe 401 e devolve AuthExpired.
    *expected_token_gate.write().await = "token-v3".to_owned();
    let no_refresher = env.manager(ClientConfig::default());
    let err = no_refresher
        .call_tool(
            "http-mid-401",
            "read_echo",
            json!({"text": "sem-refresher"}),
        )
        .await;
    assert!(matches!(err, Err(ClientError::AuthExpired(_))), "{err:?}");

    manager.shutdown().await;
    no_refresher.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn concurrent_oauth_401_refreshes_once_without_cancelling_each_other() {
    let expected_token_gate = Arc::new(RwLock::new("token-inicial-v1".to_owned()));
    let barrier = Arc::new(Barrier::new(2));
    let barrier_timed_out = Arc::new(AtomicBool::new(false));
    let state = AuthGateState {
        expected_token: expected_token_gate.clone(),
        required_custom_header: None,
        include_www_authenticate: false,
        sync_401_barrier: Some(barrier),
        barrier_timed_out: barrier_timed_out.clone(),
        ..Default::default()
    };
    let (addr, server_handle) = spawn_http_mcp_server(state).await;

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "http-concurrent-401",
            format!("http://{addr}/mcp"),
            AuthConfig::OAuthDcr,
            vec![],
        ))
        .unwrap();
    env.secrets
        .set(
            "http-concurrent-401",
            IntegrationSecrets {
                oauth: Some(OAuthTokens {
                    access_token: Zeroizing::new("token-inicial-v1".to_owned()),
                    ..OAuthTokens::default()
                }),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let refresher = Arc::new(MockRefresher {
        calls: AtomicUsize::new(0),
        next_token: "token-renovado-v2".to_owned(),
        expected_token_gate: Some(expected_token_gate.clone()),
    });
    let manager = env
        .manager(ClientConfig::default())
        .with_refresher(refresher.clone());

    // Abre uma sessão ativa com v1 antes da rotação simultânea de 401.
    let warmup = manager
        .call_tool("http-concurrent-401", "read_echo", json!({"text": "warm"}))
        .await
        .unwrap();
    assert_eq!(warmup.text, "warm|auth=Bearer token-inicial-v1|ws=");

    // Servidor passa a rejeitar v1; barreira garante que as duas chamadas recebam 401 ao mesmo tempo.
    *expected_token_gate.write().await = "token-intermediario-rejeita-v1".to_owned();
    let (res_a, res_b) = tokio::join!(
        manager.call_tool(
            "http-concurrent-401",
            "read_echo",
            json!({"text": "call-a"})
        ),
        manager.call_tool(
            "http-concurrent-401",
            "read_echo",
            json!({"text": "call-b"})
        ),
    );

    let out_a = res_a.expect("chamada A não deveria ser cancelada pela B");
    let out_b = res_b.expect("chamada B não deveria ser cancelada pela A");
    assert_eq!(out_a.text, "call-a|auth=Bearer token-renovado-v2|ws=");
    assert_eq!(out_b.text, "call-b|auth=Bearer token-renovado-v2|ws=");
    assert!(
        !barrier_timed_out.load(Ordering::SeqCst),
        "ambas as chamadas concorrentes devem alcançar a barreira de 401 sem timeout"
    );
    assert_eq!(
        refresher.calls.load(Ordering::SeqCst),
        1,
        "refresh concorrente no mesmo slot deve ocorrer exatamente 1 vez"
    );

    manager.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn reaper_preserves_active_calls_and_invalidate_cancels_in_flight_call() {
    let env = TestEnv::new();
    let started_signal = env._dir.path().join("invalidate_race_started.flag");
    env.registry
        .create(stdio_config(
            "active-race",
            PermissionPolicy::AllowAll,
            vec!["FIXTURE_SLOW_STARTED_FILE".to_owned()],
        ))
        .unwrap();
    env.secrets
        .set(
            "active-race",
            IntegrationSecrets {
                env: BTreeMap::from([(
                    "FIXTURE_SLOW_STARTED_FILE".to_owned(),
                    Zeroizing::new(started_signal.display().to_string()),
                )]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let manager = Arc::new(env.manager(ClientConfig {
        idle_ttl: Duration::from_millis(70),
        call_timeout: Duration::from_secs(3),
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    }));

    let dump = manager
        .call_tool("active-race", "dump_env", json!({}))
        .await
        .unwrap();
    let pid1 = serde_json::from_str::<Value>(&dump.text).unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    assert!(pid_alive(pid1).unwrap());

    // Chamada mais longa que `idle_ttl`; o reaper não pode encerrar a sessão ativa.
    let active_out = manager
        .call_tool("active-race", "slow_tool", json!({"delay_ms": 220}))
        .await
        .unwrap();
    assert_eq!(active_out.text, "slow-done");
    assert!(
        pid_alive(pid1).unwrap(),
        "reaper não pode matar sessão enquanto chamada está em voo"
    );
    assert_eq!(manager.running_ids(), vec!["active-race".to_owned()]);

    // Depois de concluir e ficar ociosa, o reaper encerra normalmente.
    wait_until_idle_stopped(&manager, "active-race", Duration::from_secs(2)).await;
    wait_until_pid_dead(pid1, Duration::from_secs(2)).await;
    assert!(!pid_alive(pid1).unwrap());

    // Segunda fase: `invalidate` durante uma chamada em voo cancela a chamada e mata o processo filho.
    let _ = tokio::fs::remove_file(&started_signal).await;
    let dump2 = manager
        .call_tool("active-race", "dump_env", json!({}))
        .await
        .unwrap();
    let pid2 = serde_json::from_str::<Value>(&dump2.text).unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    assert!(pid_alive(pid2).unwrap());

    let m_call = manager.clone();
    let in_flight = tokio::spawn(async move {
        m_call
            .call_tool("active-race", "slow_tool", json!({"delay_ms": 10_000}))
            .await
    });

    wait_for_file(&started_signal, Duration::from_secs(2)).await;
    manager.invalidate("active-race").await;

    let call_res = in_flight.await.unwrap();
    assert!(
        matches!(call_res, Err(ClientError::Transport(_))),
        "invalidate deve cancelar chamada em voo com ClientError::Transport, veio {call_res:?}"
    );
    assert!(
        !pid_alive(pid2).unwrap(),
        "invalidate deve encerrar imediatamente o processo filho"
    );
    assert!(manager.running_ids().is_empty());

    manager.shutdown().await;
}

#[tokio::test]
async fn connect_timeout_and_external_cancel_kill_hanging_stdio_init() {
    let env = TestEnv::new();

    // Parte A: `connect_timeout` derruba processo que trava no handshake `initialize`.
    let pid_file_timeout = env._dir.path().join("hang_init_timeout.pid");
    env.registry
        .create(stdio_config(
            "hang-init-timeout",
            PermissionPolicy::AllowAll,
            vec![
                "FIXTURE_HANG_INIT".to_owned(),
                "FIXTURE_PID_FILE".to_owned(),
            ],
        ))
        .unwrap();
    env.secrets
        .set(
            "hang-init-timeout",
            IntegrationSecrets {
                env: BTreeMap::from([
                    (
                        "FIXTURE_HANG_INIT".to_owned(),
                        Zeroizing::new("1".to_owned()),
                    ),
                    (
                        "FIXTURE_PID_FILE".to_owned(),
                        Zeroizing::new(pid_file_timeout.display().to_string()),
                    ),
                ]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let timeout_manager = Arc::new(env.manager(ClientConfig {
        connect_timeout: Duration::from_millis(800),
        exit_grace: Duration::from_millis(50),
        ..ClientConfig::default()
    }));
    let m_timeout = Arc::clone(&timeout_manager);
    let timeout_task = tokio::spawn(async move { m_timeout.list_tools("hang-init-timeout").await });
    // Garante que o processo filho iniciou e gravou o PID antes de aguardar o encerramento por timeout.
    let pid_timeout: u32 = wait_for_file(&pid_file_timeout, Duration::from_secs(3))
        .await
        .trim()
        .parse()
        .unwrap();
    let timeout_res = timeout_task.await.unwrap();
    assert!(
        matches!(timeout_res, Err(ClientError::Timeout(_))),
        "handshake travado deve falhar com Timeout, veio {timeout_res:?}"
    );
    wait_until_pid_dead(pid_timeout, Duration::from_secs(2)).await;
    assert!(timeout_manager.running_ids().is_empty());
    timeout_manager.shutdown().await;

    // Parte B: cancelamento externo da future (drop durante `open_session`) não pode vazar o processo.
    let pid_file_cancel = env._dir.path().join("hang_init_cancel.pid");
    env.registry
        .create(stdio_config(
            "hang-init-cancel",
            PermissionPolicy::AllowAll,
            vec![
                "FIXTURE_HANG_INIT".to_owned(),
                "FIXTURE_PID_FILE".to_owned(),
            ],
        ))
        .unwrap();
    env.secrets
        .set(
            "hang-init-cancel",
            IntegrationSecrets {
                env: BTreeMap::from([
                    (
                        "FIXTURE_HANG_INIT".to_owned(),
                        Zeroizing::new("1".to_owned()),
                    ),
                    (
                        "FIXTURE_PID_FILE".to_owned(),
                        Zeroizing::new(pid_file_cancel.display().to_string()),
                    ),
                ]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let cancel_manager = env.manager(ClientConfig {
        connect_timeout: Duration::from_secs(10),
        exit_grace: Duration::from_millis(50),
        ..ClientConfig::default()
    });
    let pid_text = {
        let list_fut = cancel_manager.list_tools("hang-init-cancel");
        tokio::pin!(list_fut);
        tokio::select! {
            res = &mut list_fut => panic!("list_tools não deveria concluir quando FIXTURE_HANG_INIT=1: {res:?}"),
            pid_str = wait_for_file(&pid_file_cancel, Duration::from_secs(2)) => pid_str,
        }
    };
    let pid_cancel: u32 = pid_text.trim().parse().unwrap();
    // Ao sair do bloco acima, `list_fut` é dropada e `PendingStdioGuard` precisa limpar o processo filho.
    wait_until_pid_dead(pid_cancel, Duration::from_secs(2)).await;
    assert!(cancel_manager.running_ids().is_empty());
    cancel_manager.shutdown().await;
}

#[tokio::test]
async fn rejects_reserved_http_headers_and_repeated_pagination_cursor() {
    let env = TestEnv::new();

    // Parte A: cabeçalhos HTTP reservados em `secrets.headers` são recusados fail-closed.
    for (idx, reserved) in ["Authorization", "Mcp-Session-Id"].iter().enumerate() {
        let id = format!("http-reserved-{idx}");
        env.registry
            .create(http_config(
                &id,
                "http://127.0.0.1:1/mcp".to_owned(),
                AuthConfig::None,
                vec![(*reserved).to_owned()],
            ))
            .unwrap();
        env.secrets
            .set(
                &id,
                IntegrationSecrets {
                    headers: BTreeMap::from([(
                        (*reserved).to_owned(),
                        Zeroizing::new("valor-proibido".to_owned()),
                    )]),
                    ..IntegrationSecrets::default()
                },
            )
            .await
            .unwrap();

        let manager = env.manager(ClientConfig::default());
        let res = manager.list_tools(&id).await;
        assert!(
            matches!(res, Err(ClientError::Protocol(ref msg)) if msg.contains("reservado")),
            "header reservado '{reserved}' deveria falhar com Protocol, veio {res:?}"
        );
        manager.shutdown().await;
    }

    // Parte B: cursor de paginação repetido aborta `list_tools` com erro de protocolo sem loop infinito.
    env.registry
        .create(stdio_config(
            "loop-cursor",
            PermissionPolicy::AllowAll,
            vec!["FIXTURE_PAGINATE_LOOP".to_owned()],
        ))
        .unwrap();
    env.secrets
        .set(
            "loop-cursor",
            IntegrationSecrets {
                env: BTreeMap::from([(
                    "FIXTURE_PAGINATE_LOOP".to_owned(),
                    Zeroizing::new("1".to_owned()),
                )]),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let manager = env.manager(ClientConfig {
        call_timeout: Duration::from_millis(300),
        exit_grace: Duration::from_millis(100),
        ..ClientConfig::default()
    });
    let res = manager.list_tools("loop-cursor").await;
    assert!(
        matches!(res, Err(ClientError::Protocol(ref msg)) if msg.contains("cursor")),
        "cursor repetido deveria falhar com Protocol, veio {res:?}"
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn concurrent_cold_oauth_401_deduplicates_refresh_and_rejects_blank_token() {
    struct SyncColdRefresher {
        calls: AtomicUsize,
        second_401_ready: Arc<Notify>,
        next_token: String,
        expected_token_gate: Arc<RwLock<String>>,
    }

    impl TokenRefresher for SyncColdRefresher {
        fn refresh<'a>(
            &'a self,
            _integration_id: &'a str,
        ) -> BoxFuture<'a, Result<String, ClientError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                // Segura o primeiro refresh até a segunda chamada fria também falhar com 401 no handshake.
                tokio::time::timeout(Duration::from_secs(3), self.second_401_ready.notified())
                    .await
                    .expect("segunda chamada fria deveria atingir 401 antes de renovar o token");
                *self.expected_token_gate.write().await = self.next_token.clone();
                Ok(self.next_token.clone())
            })
        }
    }

    struct BlankRefresher {
        calls: AtomicUsize,
    }

    impl TokenRefresher for BlankRefresher {
        fn refresh<'a>(
            &'a self,
            _integration_id: &'a str,
        ) -> BoxFuture<'a, Result<String, ClientError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok("   ".to_owned())
            })
        }
    }

    let expected_token_gate = Arc::new(RwLock::new("token-cold-v2".to_owned()));
    let second_401_ready = Arc::new(Notify::new());
    let counter_401 = Arc::new(AtomicUsize::new(0));
    let state = AuthGateState {
        expected_token: expected_token_gate.clone(),
        include_www_authenticate: true,
        notify_on_401_at: Some((counter_401, 2, second_401_ready.clone())),
        ..Default::default()
    };
    let (addr, server_handle) = spawn_http_mcp_server(state).await;

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "http-cold-401",
            format!("http://{addr}/mcp"),
            AuthConfig::OAuthDcr,
            vec![],
        ))
        .unwrap();
    env.secrets
        .set(
            "http-cold-401",
            IntegrationSecrets {
                oauth: Some(OAuthTokens {
                    access_token: Zeroizing::new("token-expirado-v1".to_owned()),
                    ..OAuthTokens::default()
                }),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let refresher = Arc::new(SyncColdRefresher {
        calls: AtomicUsize::new(0),
        second_401_ready,
        next_token: "token-cold-v2".to_owned(),
        expected_token_gate: expected_token_gate.clone(),
    });
    let manager = env
        .manager(ClientConfig::default())
        .with_refresher(refresher.clone());

    let (res_a, res_b) = tokio::join!(
        manager.call_tool("http-cold-401", "read_echo", json!({"text": "cold-a"})),
        manager.call_tool("http-cold-401", "read_echo", json!({"text": "cold-b"})),
    );
    assert_eq!(res_a.unwrap().text, "cold-a|auth=Bearer token-cold-v2|ws=");
    assert_eq!(res_b.unwrap().text, "cold-b|auth=Bearer token-cold-v2|ws=");
    assert_eq!(
        refresher.calls.load(Ordering::SeqCst),
        1,
        "duas falhas 401 frias sobrepostas devem acionar refresh exatamente 1 vez"
    );

    // Quando o token em uso volta a dar 401 e o refresher devolve vazio, falha com AuthExpired.
    *expected_token_gate.write().await = "token-v3-remoto".to_owned();
    let blank_refresher = Arc::new(BlankRefresher {
        calls: AtomicUsize::new(0),
    });
    let blank_manager = env
        .manager(ClientConfig::default())
        .with_refresher(blank_refresher.clone());
    let blank_res = blank_manager
        .call_tool("http-cold-401", "read_echo", json!({"text": "blank"}))
        .await;
    assert!(
        matches!(blank_res, Err(ClientError::AuthExpired(ref msg)) if msg.contains("vazio")),
        "token renovado vazio deve falhar com AuthExpired, veio {blank_res:?}"
    );
    assert_eq!(blank_refresher.calls.load(Ordering::SeqCst), 1);

    manager.shutdown().await;
    blank_manager.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn list_tools_generation_fencing_discards_write_after_invalidation() {
    #[derive(Clone)]
    struct GatedListHandler {
        started: Arc<Notify>,
        allow: Arc<Notify>,
    }

    impl ServerHandler for GatedListHandler {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            self.started.notify_one();
            tokio::time::timeout(Duration::from_secs(3), self.allow.notified())
                .await
                .map_err(|_| ErrorData::internal_error("allow timeout", None))?;
            let schema = Arc::new(
                json!({"type": "object"})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            Ok(ListToolsResult::with_all_items(vec![Tool::new(
                "stale_tool",
                "Ferramenta obsoleta",
                schema,
            )]))
        }
    }

    let started = Arc::new(Notify::new());
    let allow = Arc::new(Notify::new());
    let handler = GatedListHandler {
        started: started.clone(),
        allow: allow.clone(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().unwrap();
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new().route_service("/mcp", service);
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "gated-tools",
            format!("http://{addr}/mcp"),
            AuthConfig::None,
            vec![],
        ))
        .unwrap();

    let manager = Arc::new(env.manager(ClientConfig::default()));
    let mgr_task = manager.clone();
    let list_task = tokio::spawn(async move { mgr_task.list_tools("gated-tools").await });

    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("list_tools deve alcançar servidor");
    // Invalida o cache enquanto `list_tools` ainda está em voo.
    env.tools_cache.invalidate("gated-tools");
    allow.notify_one();

    let returned = list_task.await.unwrap().expect("list_tools deve concluir");
    assert_eq!(returned.len(), 1);
    assert!(
        env.tools_cache.get("gated-tools").is_none(),
        "resultado iniciado antes de invalidate não pode voltar para o cache"
    );

    manager.shutdown().await;
    server_handle.abort();
}

#[tokio::test]
async fn oauth_refresh_discards_token_if_integration_target_changed_during_refresh() {
    struct TargetSwitchRefresher {
        registry: Arc<IntegrationRegistry>,
        new_url: String,
    }

    impl TokenRefresher for TargetSwitchRefresher {
        fn refresh<'a>(
            &'a self,
            integration_id: &'a str,
        ) -> BoxFuture<'a, Result<String, ClientError>> {
            let new_url = self.new_url.clone();
            Box::pin(async move {
                self.registry
                    .update(
                        integration_id,
                        hangar_integrations::IntegrationUpdate {
                            transport: Some(Transport::RemoteHttp {
                                url: new_url,
                                header_names: vec![],
                            }),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                Ok("token-do-servidor-a".to_owned())
            })
        }
    }

    let state_a = AuthGateState {
        expected_token: Arc::new(RwLock::new("token-valido-a".to_owned())),
        ..Default::default()
    };
    let (addr_a, handle_a) = spawn_http_mcp_server(state_a).await;

    let hits_b = Arc::new(AtomicUsize::new(0));
    let hits_b_clone = hits_b.clone();
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = listener_b.local_addr().unwrap();
    let app_b = Router::new().fallback(move || {
        let hits = hits_b_clone.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            StatusCode::UNAUTHORIZED
        }
    });
    let handle_b = tokio::spawn(async move {
        let _ = axum::serve(listener_b, app_b).await;
    });

    let env = TestEnv::new();
    env.registry
        .create(http_config(
            "oauth-target-switch",
            format!("http://{addr_a}/mcp"),
            AuthConfig::OAuthDcr,
            vec![],
        ))
        .unwrap();
    env.secrets
        .set(
            "oauth-target-switch",
            IntegrationSecrets {
                oauth: Some(OAuthTokens {
                    access_token: Zeroizing::new("token-expirado-a".to_owned()),
                    ..OAuthTokens::default()
                }),
                ..IntegrationSecrets::default()
            },
        )
        .await
        .unwrap();

    let refresher = Arc::new(TargetSwitchRefresher {
        registry: env.registry.clone(),
        new_url: format!("http://{addr_b}/mcp"),
    });
    let manager = env
        .manager(ClientConfig::default())
        .with_refresher(refresher);

    let res = manager
        .call_tool("oauth-target-switch", "read_echo", json!({"text": "oi"}))
        .await;
    assert!(
        matches!(res, Err(ClientError::AuthExpired(_))),
        "alteração de destino durante refresh deve abortar com AuthExpired, veio {res:?}"
    );
    assert_eq!(
        hits_b.load(Ordering::SeqCst),
        0,
        "token renovado para o servidor A nunca pode ser enviado ao servidor B"
    );

    manager.shutdown().await;
    handle_a.abort();
    handle_b.abort();
}
