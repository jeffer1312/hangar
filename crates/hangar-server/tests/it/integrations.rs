//! Testes HTTP, SSE, OAuth, Keyring e isolamento de segredos das integrações MCP externas.
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::future::BoxFuture;
use hangar_integrations::{
    ApprovalResolution, MemoryKeyStore, OpenApprovalRequest, SecretKey, SecretKeyError,
    SecretKeyStore,
};
use hangar_server::accounts::login_browser::{BrowserCommand, BrowserHost, LoginBrowser, Platform};
use hangar_server::config::Config;
use hangar_server::integrations::IntegrationsService;
use hangar_server::list::hub::ListHub;
use hangar_server::login_guests::{Service as LoginGuestService, Settings as LoginGuestSettings};
use hangar_server::routes::{AppState, serve_with_guest};
use hangar_server::vault::{Key as VaultKey, KeyError as VaultKeyError, KeyStore as VaultKeyStore, Vault};
use ring::digest::{SHA256, digest};
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
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use zeroize::Zeroizing;

use crate::fake::{OWNER, client, config};

fn fixture_mcp_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("executável de testes");
    let name = format!("hangar-fixture-mcp{}", std::env::consts::EXE_SUFFIX);
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join(&name))
        .find(|bin| bin.is_file())
        .unwrap_or_else(|| {
            panic!(
                "rode `scripts/cargo-pinned build -p hangar-integrations --bin hangar-fixture-mcp` antes: nenhum acima de {}",
                exe.display()
            )
        })
}

/// Chaveiro falso isolado para o Cofre de senhas, separado do chaveiro das integrações.
#[derive(Default)]
struct FakeVaultKeys {
    key: StdMutex<Option<[u8; 32]>>,
}

impl VaultKeyStore for FakeVaultKeys {
    fn read(&self) -> BoxFuture<'_, Result<Option<VaultKey>, VaultKeyError>> {
        let key = self.key.lock().unwrap().map(Zeroizing::new);
        Box::pin(async move { Ok(key) })
    }

    fn write(&self, key: VaultKey) -> BoxFuture<'_, Result<(), VaultKeyError>> {
        *self.key.lock().unwrap() = Some(*key);
        Box::pin(async { Ok(()) })
    }
}

/// Chaveiro de integrações que falha na escrita ou leitura para provar rollback e erro explícito.
struct FailingSecretKeys {
    read_result: StdMutex<Result<Option<[u8; 32]>, SecretKeyError>>,
    write_error: Option<SecretKeyError>,
}

impl FailingSecretKeys {
    fn failing_write(err: SecretKeyError) -> Arc<Self> {
        Arc::new(Self {
            read_result: StdMutex::new(Ok(None)),
            write_error: Some(err),
        })
    }

    fn failing_read(err: SecretKeyError) -> Arc<Self> {
        Arc::new(Self {
            read_result: StdMutex::new(Err(err)),
            write_error: Some(err),
        })
    }
}

impl SecretKeyStore for FailingSecretKeys {
    fn read(&self) -> BoxFuture<'_, Result<Option<SecretKey>, SecretKeyError>> {
        let res = self.read_result.lock().unwrap().map(|opt| opt.map(Zeroizing::new));
        Box::pin(async move { res })
    }

    fn write(&self, key: SecretKey) -> BoxFuture<'_, Result<(), SecretKeyError>> {
        if let Some(err) = self.write_error {
            return Box::pin(async move { Err(err) });
        }
        *self.read_result.lock().unwrap() = Ok(Some(*key));
        Box::pin(async { Ok(()) })
    }
}

struct TestServer {
    _dir: TempDir,
    addr: SocketAddr,
    state: Arc<AppState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn fake_login_browser(launched_urls: Arc<StdMutex<Vec<String>>>) -> Arc<LoginBrowser> {
    let opener_path = PathBuf::from("/usr/bin/true");
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        // LoginBrowser só abre em Linux/Windows; nos testes em outros alvos simula Linux.
        Platform::Linux
    };
    let launcher = Arc::new(move |cmd: BrowserCommand| {
        if let Some(url) = cmd.args.last() {
            launched_urls.lock().unwrap().push(url.clone());
        }
        Ok(())
    });
    let probe = Arc::new(move || BrowserHost {
        platform,
        chrome: None,
        local_state: None,
        default_opener: Some(opener_path.clone()),
    });
    Arc::new(LoginBrowser::with(launcher, probe))
}

fn build_isolated_state(
    dir: &TempDir,
    cfg: Config,
    secret_keys: Arc<dyn SecretKeyStore>,
) -> (AppState, Arc<StdMutex<Vec<String>>>) {
    let mut state = AppState::new(cfg);
    state.hangar_dir = dir.path().to_owned();
    state.origins_home = dir.path().join("empty-home");
    state.devices = Arc::new(hangar_server::devices::Registry::new(
        dir.path().join("devices.json"),
    ));
    state.connect_devices = Arc::new(hangar_server::connect::devices::Store::new(
        dir.path().join("connect"),
        None,
    ));
    state.integrations = Arc::new(IntegrationsService::with_key_store(dir.path(), secret_keys));
    state.hub = Arc::new(ListHub::with_feeds(
        state.vault_requests.feed(),
        state.integrations.approvals.feed(),
    ));
    state.vault = Arc::new(Vault::new(Arc::new(FakeVaultKeys::default())));
    let launched_urls = Arc::new(StdMutex::new(Vec::new()));
    state.login_browser = fake_login_browser(launched_urls.clone());
    (state, launched_urls)
}

async fn spawn_test_server(secret_keys: Arc<dyn SecretKeyStore>) -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = config("127.0.0.1");
    let (state, _launched_urls) = build_isolated_state(&dir, cfg, secret_keys);
    let state = Arc::new(state);
    let app = hangar_server::routes::router(state.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        _dir: dir,
        addr,
        state,
        task,
    }
}

async fn call_raw(
    addr: SocketAddr,
    method: Method,
    path: &str,
    token: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String, reqwest::header::HeaderMap) {
    let mut req = client().request(method, format!("http://{addr}{path}"));
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    if let Some(text) = body {
        req = req
            .header("content-type", "application/json")
            .body(text.to_owned());
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let text = resp.text().await.unwrap();
    (status, text, headers)
}

async fn call_owner(addr: SocketAddr, method: Method, path: &str, body: Option<Value>) -> (u16, Value, String) {
    let text = body.map(|v| v.to_string());
    let (status, raw, _) = call_raw(addr, method, path, Some(OWNER), &[], text.as_deref()).await;
    let json = serde_json::from_str(&raw).unwrap_or(Value::Null);
    (status, json, raw)
}

/// Repete apenas em falha de conexão até o prazo; resposta HTTP != 200 falha na hora.
async fn wait_http_ready(
    url: &str,
    method: Method,
    headers: &[(&str, &str)],
    token: Option<&str>,
) -> reqwest::Response {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut req = client().request(method.clone(), url);
        if let Some(tok) = token {
            req = req.bearer_auth(tok);
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status == 200 {
                    return resp;
                }
                let body = resp.text().await.unwrap_or_default();
                panic!("status inesperado {status} em {url}: {body}");
            }
            Err(err) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "servidor não respondeu em {url} até o prazo: {err}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Servidor OAuth 2.1 + MCP efêmero para os testes HTTP de OAuth e /test
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RegisteredMockClient {
    redirect_uris: Vec<String>,
}

#[derive(Clone)]
struct PendingMockCode {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
}

#[derive(Clone)]
struct MockOAuthMcpState {
    base_url: Arc<RwLock<String>>,
    clients: Arc<AsyncMutex<std::collections::HashMap<String, RegisteredMockClient>>>,
    codes: Arc<AsyncMutex<std::collections::HashMap<String, PendingMockCode>>>,
    active_access_token: Arc<RwLock<String>>,
    active_refresh_token: Arc<RwLock<Option<String>>>,
    force_invalid_grant: Arc<AtomicBool>,
    token_seq: Arc<AtomicUsize>,
    code_seq: Arc<AtomicUsize>,
    client_seq: Arc<AtomicUsize>,
}

impl MockOAuthMcpState {
    fn new() -> Self {
        Self {
            base_url: Arc::new(RwLock::new(String::new())),
            clients: Arc::new(AsyncMutex::new(Default::default())),
            codes: Arc::new(AsyncMutex::new(Default::default())),
            active_access_token: Arc::new(RwLock::new(String::new())),
            active_refresh_token: Arc::new(RwLock::new(None)),
            force_invalid_grant: Arc::new(AtomicBool::new(false)),
            token_seq: Arc::new(AtomicUsize::new(0)),
            code_seq: Arc::new(AtomicUsize::new(0)),
            client_seq: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[derive(Clone)]
struct MockOAuthMcpHandler;

impl ServerHandler for MockOAuthMcpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = Arc::new(
            json!({"type": "object", "properties": {"msg": {"type": "string"}}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let read_tool = Tool::new("echo_oauth", "Eco autenticado", schema.clone())
            .with_annotations(ToolAnnotations::new().read_only(true));
        let write_tool = Tool::new("write_oauth", "Escrita autenticada", schema)
            .with_annotations(ToolAnnotations::new().read_only(false));
        Ok(ListToolsResult::with_all_items(vec![read_tool, write_tool]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let msg = request
            .arguments
            .as_ref()
            .and_then(|a| a.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("");
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![
            ContentBlock::text(format!("ok:{msg}")),
        ])))
    }
}

fn mock_json(status: StatusCode, value: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_vec(&value).unwrap(),
    )
        .into_response()
}

async fn mock_auth_middleware(
    State(state): State<MockOAuthMcpState>,
    req: Request,
    next: Next,
) -> Response {
    let active = state.active_access_token.read().await.clone();
    let expected = format!("Bearer {active}");
    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if active.is_empty() || supplied != Some(expected.as_str()) {
        let base = state.base_url.read().await.clone();
        let mut resp = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        let header_val = format!(r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#);
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_str(&header_val).unwrap(),
        );
        return resp;
    }
    next.run(req).await
}

async fn mock_prm(State(state): State<MockOAuthMcpState>) -> Response {
    let base = state.base_url.read().await.clone();
    mock_json(
        StatusCode::OK,
        json!({
            "resource": format!("{base}/mcp"),
            "authorization_servers": [base],
            "scopes_supported": ["read", "write"]
        }),
    )
}

async fn mock_as_meta(State(state): State<MockOAuthMcpState>) -> Response {
    let base = state.base_url.read().await.clone();
    mock_json(
        StatusCode::OK,
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "registration_endpoint": format!("{base}/register"),
            "code_challenge_methods_supported": ["S256"]
        }),
    )
}

async fn mock_register(State(state): State<MockOAuthMcpState>, body: Bytes) -> Response {
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return mock_json(StatusCode::BAD_REQUEST, json!({"error": "invalid_request"}));
    };
    let redirect_uris: Vec<String> = payload
        .get("redirect_uris")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let id = state.client_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let client_id = format!("mock-dcr-client-{id}");
    state.clients.lock().await.insert(
        client_id.clone(),
        RegisteredMockClient {
            redirect_uris: redirect_uris.clone(),
        },
    );
    mock_json(
        StatusCode::CREATED,
        json!({
            "client_id": client_id,
            "redirect_uris": redirect_uris,
            "token_endpoint_auth_method": "none"
        }),
    )
}

async fn mock_authorize(
    State(state): State<MockOAuthMcpState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let client_id = params.get("client_id").cloned().unwrap_or_default();
    let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
    let state_param = params.get("state").cloned().unwrap_or_default();
    let code_challenge = params.get("code_challenge").cloned().unwrap_or_default();
    let method = params.get("code_challenge_method").map(String::as_str);

    if params.get("response_type").map(String::as_str) != Some("code")
        || client_id.is_empty()
        || redirect_uri.is_empty()
        || state_param.is_empty()
        || code_challenge.is_empty()
        || method != Some("S256")
    {
        return (StatusCode::BAD_REQUEST, "authorize invalido").into_response();
    }
    let clients = state.clients.lock().await;
    let Some(reg) = clients.get(&client_id) else {
        return (StatusCode::BAD_REQUEST, "client_id desconhecido").into_response();
    };
    if !reg.redirect_uris.contains(&redirect_uri) {
        return (StatusCode::BAD_REQUEST, "redirect_uri divergente").into_response();
    }
    drop(clients);

    let seq = state.code_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let code = format!("mock-code-{seq}");
    state.codes.lock().await.insert(
        code.clone(),
        PendingMockCode {
            client_id,
            redirect_uri: redirect_uri.clone(),
            code_challenge,
        },
    );
    let mut ser = form_urlencoded::Serializer::new(String::new());
    ser.append_pair("code", &code);
    ser.append_pair("state", &state_param);
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, format!("{redirect_uri}{sep}{}", ser.finish()))
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn mock_token(State(state): State<MockOAuthMcpState>, body: Bytes) -> Response {
    let form: std::collections::HashMap<String, String> = form_urlencoded::parse(&body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let grant_type = form.get("grant_type").map(String::as_str).unwrap_or("");
    let client_id = form.get("client_id").cloned().unwrap_or_default();
    match grant_type {
        "authorization_code" => {
            let code = form.get("code").cloned().unwrap_or_default();
            let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            let Some(pending) = state.codes.lock().await.remove(&code) else {
                return mock_json(StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"}));
            };
            let hashed = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()).as_ref());
            if pending.client_id != client_id
                || pending.redirect_uri != redirect_uri
                || hashed != pending.code_challenge
            {
                return mock_json(StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"}));
            }
            let seq = state.token_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let access = format!("oauth-access-secret-{seq}");
            let refresh = format!("oauth-refresh-secret-{seq}");
            *state.active_access_token.write().await = access.clone();
            *state.active_refresh_token.write().await = Some(refresh.clone());
            mock_json(
                StatusCode::OK,
                json!({
                    "access_token": access,
                    "refresh_token": refresh,
                    "token_type": "Bearer",
                    "expires_in": 3600
                }),
            )
        }
        "refresh_token" => {
            if state.force_invalid_grant.load(Ordering::SeqCst) {
                return mock_json(
                    StatusCode::BAD_REQUEST,
                    json!({"error": "invalid_grant", "error_description": "refresh revogado"}),
                );
            }
            let seq = state.token_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let access = format!("oauth-access-secret-{seq}");
            let refresh = format!("oauth-refresh-secret-{seq}");
            *state.active_access_token.write().await = access.clone();
            *state.active_refresh_token.write().await = Some(refresh.clone());
            mock_json(
                StatusCode::OK,
                json!({
                    "access_token": access,
                    "refresh_token": refresh,
                    "token_type": "Bearer",
                    "expires_in": 3600
                }),
            )
        }
        _ => mock_json(StatusCode::BAD_REQUEST, json!({"error": "unsupported_grant_type"})),
    }
}

async fn spawn_mock_oauth_mcp() -> (SocketAddr, MockOAuthMcpState, tokio::task::JoinHandle<()>) {
    let state = MockOAuthMcpState::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    *state.base_url.write().await = format!("http://{addr}");

    let mcp_service = StreamableHttpService::new(
        || Ok(MockOAuthMcpHandler),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let mcp_router = Router::new()
        .nest_service("/mcp", mcp_service)
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            mock_auth_middleware,
        ));
    let app = Router::new()
        .merge(mcp_router)
        .route("/.well-known/oauth-protected-resource", get(mock_prm))
        .route("/.well-known/oauth-authorization-server", get(mock_as_meta))
        .route("/register", post(mock_register))
        .route("/authorize", get(mock_authorize))
        .route("/token", post(mock_token))
        .with_state(state.clone());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state, handle)
}

async fn simulate_oauth_authorize(authorize_url: &str) -> (String, String) {
    let resp = client().get(authorize_url).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND);
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let parsed = reqwest::Url::parse(loc).unwrap();
    let code = parsed
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let state = parsed
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    (code, state)
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn catalog_returns_presets_offline_and_handles_cors_preflight() {
    let srv = spawn_test_server(Arc::new(MemoryKeyStore::new())).await;

    let (status, _, headers) = call_raw(
        srv.addr,
        Method::OPTIONS,
        "/api/integrations/catalog",
        None,
        &[
            ("origin", "http://localhost:5173"),
            ("access-control-request-method", "GET"),
            ("access-control-request-headers", "authorization"),
        ],
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(headers.get("access-control-allow-origin").and_then(|v| v.to_str().ok()), Some("*"));

    let (status, raw, headers) = call_raw(
        srv.addr,
        Method::GET,
        "/api/integrations/catalog",
        Some(OWNER),
        &[("origin", "http://localhost:5173")],
        None,
    )
    .await;
    assert_eq!(status, 200, "{raw}");
    assert_eq!(headers.get("access-control-allow-origin").and_then(|v| v.to_str().ok()), Some("*"));
    let body: Value = serde_json::from_str(&raw).unwrap();
    let presets = body["presets"].as_array().expect("presets deve ser array");
    assert!(presets.len() >= 10, "esperava presets embutidos: {body}");
    assert!(presets.iter().any(|p| p["id"] == "github"));
    assert!(presets.iter().any(|p| p["id"] == "google-gmail"));
    assert!(presets.iter().any(|p| p["id"] == "notion"));
}

#[tokio::test]
async fn admin_routes_and_callback_reject_strangers_guests_and_guest_port() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Configura um convidado com login e um convidado de convite para provar todas as fronteiras.
    let mut cfg = config("127.0.0.1");
    cfg.listen = addr;
    let (mut state, _) = build_isolated_state(&dir, cfg, Arc::new(MemoryKeyStore::new()));
    let login_guests = LoginGuestService::new(LoginGuestSettings {
        guests: dir.path().join("guests.json"),
        vault: dir.path().join("login-vault.json"),
        runtime_config: dir.path().join("runtime-config.json"),
        sync_enabled: false,
        bootstrap: String::new(),
        secret: b"fixture".to_vec(),
        secure: false,
        rate_max: 10.into(),
        rate_window: 300.into(),
    });
    login_guests
        .store
        .create("Ana", dir.path().to_str().unwrap(), false, true, "ana".into(), "login-guest-tok".into())
        .unwrap();
    state.login_guests = Arc::new(login_guests);
    let state = Arc::new(state);
    let app = hangar_server::routes::router(state.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let owner_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Porta separada do convidado com um token de convite válido.
    let share_dir = tempfile::tempdir().unwrap();
    let share_store = hangar_server::share::store::ShareStore::open(
        share_dir.path().join("shares.json"),
        Arc::new(|| 1_000.0),
    );
    let (_, share_guest_token) = share_store
        .create_redeemed("s0", Some("k:L-s0".into()), hangar_server::share::store::Kind::Share)
        .unwrap();
    struct AliveLife;
    impl hangar_server::share::life::LifeProbe for AliveLife {
        fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { Some(1) }) }
        fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
        fn sidecar(&self, name: &str) -> Option<(String, bool)> { Some((format!("L-{name}"), false)) }
        fn sidecar_exists(&self, _name: &str) -> bool { false }
    }
    struct DummyTs;
    impl hangar_server::share::tunnel::Tailscale for DummyTs {
        fn run<'a>(&'a self, _args: &'a [&'a str]) -> BoxFuture<'a, Result<hangar_server::share::tunnel::Output, String>> {
            Box::pin(async { Err("offline".into()) })
        }
        fn needs_operator(&self) -> bool { false }
    }
    let facts = Arc::new(hangar_server::share::life::LifeFacts::default());
    facts.apply_transfer_snapshot(Default::default(), Default::default());
    let share_state = Arc::new(hangar_server::share::ShareState {
        store: share_store,
        pairs: hangar_server::share::pairs::PairStore::open(share_dir.path().join("pairs.json")),
        lives: hangar_server::share::life::Lives::new(Arc::new(AliveLife), facts),
        tailscale: Arc::new(DummyTs),
        tunnel: Default::default(),
        connect: None,
    });
    let mut guest_cfg = config("127.0.0.1");
    guest_cfg.listen = addr;
    let (mut g_state, _) = build_isolated_state(&share_dir, guest_cfg, Arc::new(MemoryKeyStore::new()));
    g_state.share = Some(share_state);
    let g_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest_addr = g_listener.local_addr().unwrap();
    let g_app = hangar_server::routes::guest_router(Arc::new(g_state))
        .into_make_service_with_connect_info::<hangar_server::share::gate::GuestConn>();
    let guest_task = tokio::spawn(async move {
        axum::serve(g_listener, g_app).await.unwrap();
    });

    let admin_routes = [
        (Method::GET, "/api/integrations/catalog", None),
        (Method::GET, "/api/integrations", None),
        (Method::POST, "/api/integrations", Some("{}")),
        (Method::PATCH, "/api/integrations/github", Some("{}")),
        (Method::DELETE, "/api/integrations/github", None),
        (Method::POST, "/api/integrations/github/oauth/start", Some("{}")),
        (Method::POST, "/api/integrations/github/oauth/open-on-desktop", Some("{}")),
        (Method::POST, "/api/integrations/github/test", None),
        (Method::GET, "/api/integrations/github/tools", None),
        (Method::POST, "/api/integrations/approvals/req-1", Some("{}")),
        (Method::DELETE, "/api/integrations/approvals/req-1", None),
    ];

    for (idx, (method, path, body)) in admin_routes.iter().enumerate() {
        let origin = format!("198.51.100.{}", idx + 1);
        let (st_no_tok, _, _) = call_raw(addr, method.clone(), path, None, &[("x-forwarded-for", &origin)], *body).await;
        assert_eq!(st_no_tok, 401, "sem token deve recusar {method} {path}");

        let wrong_origin = format!("198.51.100.{}", idx + 40);
        let (st_wrong, _, _) = call_raw(addr, method.clone(), path, Some("errado"), &[("x-forwarded-for", &wrong_origin)], *body).await;
        assert_eq!(st_wrong, 401, "token errado deve recusar {method} {path}");

        // Mesmo com token do dono, cabeçalho de convidado deve receber 403.
        let (st_hdr, _, _) = call_raw(addr, method.clone(), path, Some(OWNER), &[("x-hangar-guest", "1")], *body).await;
        assert_eq!(st_hdr, 403, "x-hangar-guest deve receber 403 em {method} {path}");

        // Convidado com login também leva 403.
        let (st_login_guest, _, _) = call_raw(addr, method.clone(), path, Some("login-guest-tok"), &[], *body).await;
        assert_eq!(st_login_guest, 403, "login guest deve receber 403 em {method} {path}");

        // Porta de convidado com token de convite também recusa.
        let (st_guest_port, _, _) = call_raw(guest_addr, method.clone(), path, Some(&share_guest_token), &[], *body).await;
        assert!(matches!(st_guest_port, 401 | 403), "porta convidado {method} {path} -> {st_guest_port}");
    }

    // Muitas falhas seguidas de token na mesma origem contabilizam 429.
    for _ in 0..8 {
        let _ = call_raw(addr, Method::GET, "/api/integrations", Some("errado"), &[("x-forwarded-for", "203.0.113.9")], None).await;
    }
    let (st_429, _, _) = call_raw(addr, Method::GET, "/api/integrations", Some("errado"), &[("x-forwarded-for", "203.0.113.9")], None).await;
    assert_eq!(st_429, 429, "estouro de tentativas deve gerar 429");

    // Callback OAuth deve recusar x-hangar-guest e porta de convidado com 403/401.
    let (st_cb_hdr, _, _) = call_raw(
        addr,
        Method::GET,
        "/api/integrations/oauth/callback?state=s&code=c",
        None,
        &[("x-hangar-guest", "1")],
        None,
    )
    .await;
    assert_eq!(st_cb_hdr, 403);

    let (st_cb_guest, _, _) = call_raw(
        guest_addr,
        Method::GET,
        "/api/integrations/oauth/callback?state=s&code=c",
        Some(&share_guest_token),
        &[],
        None,
    )
    .await;
    assert_eq!(st_cb_guest, 403);

    owner_task.abort();
    guest_task.abort();
}

#[tokio::test]
async fn crud_persists_outside_vault_never_leaks_secrets_and_rolls_back_on_secret_failure() {
    let srv = spawn_test_server(Arc::new(MemoryKeyStore::new())).await;

    // Inicializa o Cofre real no mesmo hangar_dir e confirma isolamento.
    let (st_vault, _, _) = call_owner(srv.addr, Method::PUT, "/api/vault/config", Some(json!({"owner": "self"}))).await;
    assert_eq!(st_vault, 200);

    let secret_token = "tok-bearer-super-secreto-9911";
    let secret_header = "hdr-secreto-8822";
    let secret_client = "client-sec-7733";
    let create_body = json!({
        "id": "minha-api",
        "name": "Minha API",
        "preset": null,
        "enabled": true,
        "transport": {
            "kind": "remote_http",
            "url": "https://mcp.example.com/mcp",
            "header_names": ["x-api-key"]
        },
        "auth": { "kind": "bearer" },
        "policy": "read_free_ask_write",
        "pinned": true,
        "projects": ["/tmp/proj-a"],
        "allowed_tools": [],
        "secrets": {
            "bearer_token": secret_token,
            "headers": { "x-api-key": secret_header },
            "oauth_client_secret": secret_client
        }
    });
    let (st, created, raw_created) = call_owner(srv.addr, Method::POST, "/api/integrations", Some(create_body.clone())).await;
    assert_eq!(st, 201, "{raw_created}");
    assert_eq!(created["integration"]["id"], "minha-api");
    assert_eq!(created["integration"]["status"], "ready");
    assert_eq!(created["integration"]["pinned"], true);
    for forbidden in [secret_token, secret_header, secret_client, "bearer_token", "client_secret", "access_token", "refresh_token"] {
        assert!(!raw_created.contains(forbidden), "POST vazou campo/valor proibido '{forbidden}': {raw_created}");
    }

    // Duplicata devolve 409.
    let (st_dup, _, _) = call_owner(srv.addr, Method::POST, "/api/integrations", Some(create_body)).await;
    assert_eq!(st_dup, 409);

    // JSON malformado carregando segredo retorna 422 fixo sem ecoar o segredo.
    let broken = format!(r#"{{"id":"x","secrets":{{"bearer_token":"{secret_token}"}} "#);
    let (st_bad, raw_bad, _) = call_raw(srv.addr, Method::POST, "/api/integrations", Some(OWNER), &[], Some(&broken)).await;
    assert_eq!(st_bad, 422, "{raw_bad}");
    assert!(!raw_bad.contains(secret_token), "422 nunca pode ecoar segredo: {raw_bad}");
    assert!(raw_bad.contains("erro_corpo_invalido"), "{raw_bad}");

    // GET /api/integrations nunca devolve segredos nem campos de token.
    let (st_list, listed, raw_list) = call_owner(srv.addr, Method::GET, "/api/integrations", None).await;
    assert_eq!(st_list, 200, "{raw_list}");
    assert_eq!(listed["integrations"].as_array().unwrap().len(), 1);
    for forbidden in [secret_token, secret_header, secret_client, "bearer_token", "client_secret", "access_token", "refresh_token"] {
        assert!(!raw_list.contains(forbidden), "GET vazou campo/valor proibido '{forbidden}': {raw_list}");
    }

    // Confere que secrets.bin e config.json existem e o Cofre (/api/vault/entries e vault.bin) não contém nada da integração.
    assert!(srv._dir.path().join("integrations/config.json").is_file());
    let secrets_bin = std::fs::read(srv._dir.path().join("integrations/secrets.bin")).unwrap();
    assert!(!secrets_bin.windows(secret_token.len()).any(|w| w == secret_token.as_bytes()), "secrets.bin deve ser cifrado");
    let (st_ve, vault_entries, _) = call_owner(srv.addr, Method::GET, "/api/vault/entries", None).await;
    assert_eq!(st_ve, 200);
    assert_eq!(vault_entries["entries"].as_array().unwrap().len(), 0);

    // PATCH parcial atualiza campos, preserva bearer_token ao atualizar header e alterna enabled.
    let (st_patch, patched, raw_patch) = call_owner(
        srv.addr,
        Method::PATCH,
        "/api/integrations/minha-api",
        Some(json!({
            "name": "API Atualizada",
            "enabled": false,
            "pinned": false,
            "policy": "allow_all",
            "secrets": { "headers": { "x-api-key": "hdr-novo-3344" } }
        })),
    )
    .await;
    assert_eq!(st_patch, 200, "{raw_patch}");
    assert_eq!(patched["integration"]["name"], "API Atualizada");
    assert_eq!(patched["integration"]["status"], "disabled");
    assert!(!raw_patch.contains("hdr-novo-3344"));
    let stored_secret = srv.state.integrations.secrets.get("minha-api").await.unwrap().unwrap();
    assert_eq!(stored_secret.bearer_token.as_deref().map(String::as_str), Some(secret_token));
    assert_eq!(stored_secret.headers.get("x-api-key").map(|v| v.as_str()), Some("hdr-novo-3344"));

    let (st_miss, _, _) = call_owner(srv.addr, Method::PATCH, "/api/integrations/inexistente", Some(json!({"pinned": true}))).await;
    assert_eq!(st_miss, 404);

    // DELETE remove config e segredo.
    let (st_del, del_body, _) = call_owner(srv.addr, Method::DELETE, "/api/integrations/minha-api", None).await;
    assert_eq!(st_del, 200);
    assert_eq!(del_body, json!({"ok": true, "id": "minha-api"}));
    assert!(srv.state.integrations.registry.get("minha-api").unwrap().is_none());
    assert!(srv.state.integrations.secrets.get("minha-api").await.unwrap().is_none());
    let (st_del2, _, _) = call_owner(srv.addr, Method::DELETE, "/api/integrations/minha-api", None).await;
    assert_eq!(st_del2, 404);

    // Falha no chaveiro durante POST faz rollback da config criada (sem sucesso parcial).
    let srv_fail = spawn_test_server(FailingSecretKeys::failing_write(SecretKeyError::Bus)).await;
    let (st_fail, _, raw_fail) = call_owner(
        srv_fail.addr,
        Method::POST,
        "/api/integrations",
        Some(json!({
            "id": "rollback-api",
            "name": "Rollback",
            "transport": { "kind": "remote_http", "url": "https://mcp.example.com/mcp", "header_names": [] },
            "auth": { "kind": "bearer" },
            "secrets": { "bearer_token": "tok-falha" }
        })),
    )
    .await;
    assert_eq!(st_fail, 503, "falha ao gravar segredo deve retornar 503: {raw_fail}");
    assert!(
        srv_fail.state.integrations.registry.get("rollback-api").unwrap().is_none(),
        "rollback deve apagar config após falha em secrets.set"
    );
}

#[tokio::test]
async fn secrets_store_errors_are_surfaced_without_masking() {
    let srv = spawn_test_server(Arc::new(MemoryKeyStore::new())).await;
    let (st, _, _) = call_owner(
        srv.addr,
        Method::POST,
        "/api/integrations",
        Some(json!({
            "id": "corrompida",
            "name": "Corrompida",
            "transport": { "kind": "remote_http", "url": "https://mcp.example.com/mcp", "header_names": [] },
            "auth": { "kind": "bearer" },
            "secrets": { "bearer_token": "tok-1" }
        })),
    )
    .await;
    assert_eq!(st, 201);

    // Corrompe secrets.bin e reinicia serviço lendo do mesmo hangar_dir: GET deve retornar 500/503 explícito.
    let secrets_path = srv._dir.path().join("integrations/secrets.bin");
    let mut bytes = std::fs::read(&secrets_path).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    std::fs::write(&secrets_path, bytes).unwrap();

    let (st_err, err_body, raw_err) = call_owner(srv.addr, Method::GET, "/api/integrations", None).await;
    assert!(matches!(st_err, 500 | 503), "erro em secrets.bin deve aparecer como 500/503, veio {st_err}: {raw_err}");
    assert!(err_body["detail"]["code"].as_str().is_some(), "{raw_err}");

    // Chaveiro indisponível (Bus) também retorna 503 sem mascarar.
    let srv_bus = spawn_test_server(FailingSecretKeys::failing_read(SecretKeyError::Bus)).await;
    srv_bus
        .state
        .integrations
        .registry
        .create(hangar_integrations::IntegrationConfig {
            id: "integ-bus".into(),
            name: "Bus".into(),
            preset: None,
            enabled: true,
            transport: hangar_integrations::Transport::RemoteHttp {
                url: "https://mcp.example.com/mcp".into(),
                header_names: vec![],
            },
            auth: hangar_integrations::AuthConfig::Bearer,
            policy: Default::default(),
            pinned: false,
            projects: vec![],
            allowed_tools: BTreeSet::new(),
            created_at: 1.0,
            updated_at: 1.0,
        })
        .unwrap();
    let (st_bus, _, raw_bus) = call_owner(srv_bus.addr, Method::GET, "/api/integrations", None).await;
    assert_eq!(st_bus, 503, "{raw_bus}");
}

#[tokio::test]
async fn oauth_flow_uses_bound_local_port_handoff_browser_callback_test_and_expiry() {
    tokio::time::timeout(Duration::from_secs(15), async {
    let (mock_addr, mock_state, mock_task) = spawn_mock_oauth_mcp().await;

    // Sobe servidor com serve_with_guest e Config listen :0 para provar resolução da porta real vinculada.
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner_addr = listener.local_addr().unwrap();
    let probe_guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest_addr = probe_guest.local_addr().unwrap();
    drop(probe_guest);

    let mut cfg = config("127.0.0.1");
    cfg.listen = "127.0.0.1:0".parse().unwrap();
    let (mut state, launched_urls) = build_isolated_state(&dir, cfg, Arc::new(MemoryKeyStore::new()));
    let share_store = hangar_server::share::store::ShareStore::open(
        dir.path().join("shares.json"),
        Arc::new(|| 1_000.0),
    );
    let (_, guest_tok) = share_store
        .create_redeemed("s0", Some("k:L-s0".into()), hangar_server::share::store::Kind::Share)
        .unwrap();
    struct AliveLife;
    impl hangar_server::share::life::LifeProbe for AliveLife {
        fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { Some(1) }) }
        fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
        fn sidecar(&self, name: &str) -> Option<(String, bool)> { Some((format!("L-{name}"), false)) }
        fn sidecar_exists(&self, _name: &str) -> bool { false }
    }
    struct DummyTs;
    impl hangar_server::share::tunnel::Tailscale for DummyTs {
        fn run<'a>(&'a self, _args: &'a [&'a str]) -> BoxFuture<'a, Result<hangar_server::share::tunnel::Output, String>> {
            Box::pin(async { Err("offline".into()) })
        }
        fn needs_operator(&self) -> bool { false }
    }
    let oauth_facts = Arc::new(hangar_server::share::life::LifeFacts::default());
    oauth_facts.apply_transfer_snapshot(Default::default(), Default::default());
    state.share = Some(Arc::new(hangar_server::share::ShareState {
        store: share_store,
        pairs: hangar_server::share::pairs::PairStore::open(dir.path().join("pairs.json")),
        lives: hangar_server::share::life::Lives::new(
            Arc::new(AliveLife),
            oauth_facts,
        ),
        tailscale: Arc::new(DummyTs),
        tunnel: Default::default(),
        connect: None,
    }));

    let serve_task = tokio::spawn(async move {
        serve_with_guest(listener, state, Some(guest_addr)).await.unwrap();
    });

    // Aguarda portas do dono e do convidado subirem (preflight/health valem em RED e GREEN).
    wait_http_ready(
        &format!("http://{owner_addr}/__hangar_server/health"),
        Method::GET,
        &[],
        None,
    )
    .await;
    wait_http_ready(
        &format!("http://{guest_addr}/api/integrations/oauth/callback"),
        Method::OPTIONS,
        &[("origin", "http://localhost"), ("access-control-request-method", "GET")],
        None,
    )
    .await;

    // Cria integração OAuth DCR apontando para o mock local.
    let (st_create, _, raw_create) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations",
        Some(json!({
            "id": "mock-oauth",
            "name": "Mock OAuth",
            "transport": { "kind": "remote_http", "url": format!("http://{mock_addr}/mcp"), "header_names": [] },
            "auth": { "kind": "oauth_dcr" },
            "policy": "read_free_ask_write"
        })),
    )
    .await;
    assert_eq!(st_create, 201, "{raw_create}");

    // Antes do OAuth: /test deve retornar 424 auth_required (NUNCA 401!).
    let (st_test0, body_test0, raw_test0) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations/mock-oauth/test",
        None,
    )
    .await;
    assert_eq!(st_test0, 424, "{raw_test0}");
    assert_eq!(body_test0["detail"]["code"], "auth_required", "{raw_test0}");

    // Sem fluxo pendente ainda: open-on-desktop deve retornar 409.
    let (st_nopend, _, raw_nopend) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations/mock-oauth/oauth/open-on-desktop",
        Some(json!({})),
    )
    .await;
    assert_eq!(st_nopend, 409, "{raw_nopend}");

    // Celular sem HTTPS -> mode = handoff_desktop com porta real vinculada (não :0).
    let expected_redirect = format!("http://127.0.0.1:{}/api/integrations/oauth/callback", owner_addr.port());
    let (st_mob, mob_body, raw_mob) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations/mock-oauth/oauth/start",
        Some(json!({ "origin_client": "mobile" })),
    )
    .await;
    assert_eq!(st_mob, 200, "{raw_mob}");
    assert_eq!(mob_body["mode"], "handoff_desktop");
    assert_eq!(mob_body["redirect_uri"], expected_redirect);
    assert!(mob_body["reason"].as_str().is_some_and(|r| !r.is_empty()));

    // Desktop -> mode = open_url com redirect_uri usando a porta real.
    let (st_desk, desk_body, raw_desk) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations/mock-oauth/oauth/start",
        Some(json!({ "origin_client": "desktop" })),
    )
    .await;
    assert_eq!(st_desk, 200, "{raw_desk}");
    assert_eq!(desk_body["mode"], "open_url");
    assert_eq!(desk_body["redirect_uri"], expected_redirect);
    let authorize_url = desk_body["authorize_url"].as_str().unwrap().to_owned();

    // open-on-desktop recusa URL arbitrária não pendente (422) e não chama o launcher.
    let (st_evil, _, raw_evil) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations/mock-oauth/oauth/open-on-desktop",
        Some(json!({ "authorize_url": "https://evil.example/phish" })),
    )
    .await;
    assert_eq!(st_evil, 422, "{raw_evil}");
    assert!(launched_urls.lock().unwrap().is_empty());

    // open-on-desktop aceita URL pendente do OAuthManager.
    let (st_open, open_body, raw_open) = call_owner(
        owner_addr,
        Method::POST,
        "/api/integrations/mock-oauth/oauth/open-on-desktop",
        Some(json!({ "authorize_url": authorize_url })),
    )
    .await;
    assert_eq!(st_open, 200, "{raw_open}");
    assert_eq!(open_body["ok"], true);
    assert_eq!(launched_urls.lock().unwrap().as_slice(), &[authorize_url.clone()]);

    // Primeiro verifica escape de XSS quando o provedor devolve erro no callback usando o state pendente do fluxo mobile.
    let mob_auth_url = mob_body["authorize_url"].as_str().unwrap();
    let (_, mob_state) = simulate_oauth_authorize(mob_auth_url).await;
    let (st_xss, raw_xss, _) = call_raw(
        owner_addr,
        Method::GET,
        &format!("/api/integrations/oauth/callback?state={mob_state}&error=access_denied&error_description=%3Cscript%3Ealert(1)%3C/script%3E"),
        None,
        &[],
        None,
    )
    .await;
    assert!(matches!(st_xss, 400 | 422), "{st_xss}: {raw_xss}");
    assert!(!raw_xss.contains("<script>"), "callback não pode refletir HTML cru: {raw_xss}");
    assert!(raw_xss.contains("&lt;script&gt;"), "{raw_xss}");

    // Callback OAuth na porta do convidado falha com 401/403 mesmo quando há fluxo pendente.
    let (st_cb_guest, _, _) = call_raw(
        guest_addr,
        Method::GET,
        &format!("/api/integrations/oauth/callback?state=qualquer&code=qualquer&token={guest_tok}"),
        Some(&guest_tok),
        &[],
        None,
    )
    .await;
    assert!(matches!(st_cb_guest, 401 | 403), "porta convidado não pode atender callback: {st_cb_guest}");

    // Navegador conclui autorização e envia callback SEM token do dono na porta principal.
    let (code, cb_state) = simulate_oauth_authorize(&authorize_url).await;
    let cb_path = format!("/api/integrations/oauth/callback?code={code}&state={cb_state}");
    let (st_cb, html_cb, headers_cb) = call_raw(owner_addr, Method::GET, &cb_path, None, &[], None).await;
    assert_eq!(st_cb, 200, "{html_cb}");
    assert!(headers_cb.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").contains("text/html"));
    assert!(!html_cb.contains("oauth-access-secret"), "HTML de callback nunca pode vazar token");

    // Reutilizar state devolve erro (400/422).
    let (st_replay, _, _) = call_raw(owner_addr, Method::GET, &cb_path, None, &[], None).await;
    assert!(matches!(st_replay, 400 | 422));

    // Lista mostra status ready e sem last_error.
    let (st_l1, l1_body, raw_l1) = call_owner(owner_addr, Method::GET, "/api/integrations", None).await;
    assert_eq!(st_l1, 200);
    assert_eq!(l1_body["integrations"][0]["status"], "ready");
    assert!(l1_body["integrations"][0]["last_error"].is_null(), "{raw_l1}");
    assert!(!raw_l1.contains("oauth-access-secret"), "lista nunca vaza access_token");
    assert!(!raw_l1.contains("oauth-refresh-secret"), "lista nunca vaza refresh_token");

    // POST /test agora conecta no MCP protegido, descobre tools e limpa last_error.
    let (st_t1, t1_body, raw_t1) = call_owner(owner_addr, Method::POST, "/api/integrations/mock-oauth/test", None).await;
    assert_eq!(st_t1, 200, "{raw_t1}");
    assert_eq!(t1_body["status"], "ready");
    let tools = t1_body["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "echo_oauth" && t["effective_access"] == "read"));
    assert!(tools.iter().any(|t| t["name"] == "write_oauth" && t["effective_access"] == "ask_write"));

    let (st_gt, gt_body, _) = call_owner(owner_addr, Method::GET, "/api/integrations/mock-oauth/tools", None).await;
    assert_eq!(st_gt, 200);
    assert_eq!(gt_body["tools"], t1_body["tools"]);

    // Token invalida e refresh recebe invalid_grant -> /test retorna 424 auth_expired (NUNCA 401!) e lista mostra auth_expired.
    *mock_state.active_access_token.write().await = "outro-token-no-servidor".into();
    mock_state.force_invalid_grant.store(true, Ordering::SeqCst);
    let (st_exp, exp_body, raw_exp) = call_owner(owner_addr, Method::POST, "/api/integrations/mock-oauth/test", None).await;
    assert_eq!(st_exp, 424, "{raw_exp}");
    assert_eq!(exp_body["detail"]["code"], "auth_expired", "{raw_exp}");

    let (st_l2, l2_body, _) = call_owner(owner_addr, Method::GET, "/api/integrations", None).await;
    assert_eq!(st_l2, 200);
    assert_eq!(l2_body["integrations"][0]["id"], "mock-oauth");
    assert_eq!(l2_body["integrations"][0]["status"], "auth_expired");

    serve_task.abort();
    mock_task.abort();
    })
    .await
    .expect("prazo do fluxo OAuth excedido");
}

#[tokio::test]
async fn stdio_fixture_mcp_test_tools_patch_invalidation_and_delete_cleanup() {
    let srv = spawn_test_server(Arc::new(MemoryKeyStore::new())).await;
    let bin = fixture_mcp_bin();

    let (st_c, _, raw_c) = call_owner(
        srv.addr,
        Method::POST,
        "/api/integrations",
        Some(json!({
            "id": "fixture-stdio",
            "name": "Fixture Stdio",
            "transport": {
                "kind": "local_stdio",
                "command": [bin.to_string_lossy()],
                "env_keys": ["FIXTURE_SECRET"]
            },
            "auth": { "kind": "none" },
            "policy": "read_free_ask_write",
            "secrets": {
                "env": { "FIXTURE_SECRET": "segredo-stdio-v1" }
            }
        })),
    )
    .await;
    assert_eq!(st_c, 201, "{raw_c}");

    let (st_test, test_body, raw_test) = call_owner(srv.addr, Method::POST, "/api/integrations/fixture-stdio/test", None).await;
    assert_eq!(st_test, 200, "{raw_test}");
    assert_eq!(test_body["status"], "ready");
    let tools = test_body["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "read_echo" && t["effective_access"] == "read"));
    assert!(tools.iter().any(|t| t["name"] == "write_action" && t["effective_access"] == "ask_write"));
    assert!(tools.iter().any(|t| t["name"] == "unannotated_tool" && t["effective_access"] == "ask_write"));

    // Confirma entrega isolada de segredo por env ao filho e invalidação após PATCH.
    let env_v1 = srv
        .state
        .integrations
        .clients
        .call_tool("fixture-stdio", "dump_env", json!({"keys": ["FIXTURE_SECRET"]}))
        .await
        .unwrap();
    assert!(env_v1.text.contains("segredo-stdio-v1"), "{}", env_v1.text);

    let (st_patch, _, _) = call_owner(
        srv.addr,
        Method::PATCH,
        "/api/integrations/fixture-stdio",
        Some(json!({ "secrets": { "env": { "FIXTURE_SECRET": "segredo-stdio-v2" } } })),
    )
    .await;
    assert_eq!(st_patch, 200);

    let env_v2 = srv
        .state
        .integrations
        .clients
        .call_tool("fixture-stdio", "dump_env", json!({"keys": ["FIXTURE_SECRET"]}))
        .await
        .unwrap();
    assert!(env_v2.text.contains("segredo-stdio-v2"), "cliente deve ter sido invalidado após PATCH: {}", env_v2.text);

    // Desabilitada retorna 409 integration_disabled no /test.
    let (st_dis, _, _) = call_owner(
        srv.addr,
        Method::PATCH,
        "/api/integrations/fixture-stdio",
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(st_dis, 200);
    let (st_dis_test, dis_body, _) = call_owner(srv.addr, Method::POST, "/api/integrations/fixture-stdio/test", None).await;
    assert_eq!(st_dis_test, 409);
    assert_eq!(dis_body["detail"]["code"], "integration_disabled");

    let (st_del, _, _) = call_owner(srv.addr, Method::DELETE, "/api/integrations/fixture-stdio", None).await;
    assert_eq!(st_del, 200);
    srv.state.integrations.shutdown().await;
}

#[tokio::test]
async fn approvals_decisions_and_cancel_work_and_persist_allow_always() {
    let srv = spawn_test_server(Arc::new(MemoryKeyStore::new())).await;
    let bin = fixture_mcp_bin();
    let (st_c, _, _) = call_owner(
        srv.addr,
        Method::POST,
        "/api/integrations",
        Some(json!({
            "id": "fixture-app",
            "name": "Fixture App",
            "transport": { "kind": "local_stdio", "command": [bin.to_string_lossy()], "env_keys": [] },
            "auth": { "kind": "none" },
            "policy": "read_free_ask_write"
        })),
    )
    .await;
    assert_eq!(st_c, 201);
    let (st_t, _, _) = call_owner(srv.addr, Method::POST, "/api/integrations/fixture-app/test", None).await;
    assert_eq!(st_t, 200);

    // 1. allow_once + interação obsoleta (409).
    let w_once = srv
        .state
        .integrations
        .approvals
        .open_request(OpenApprovalRequest {
            origin: "sessao-1".into(),
            session_key: "sk-1".into(),
            integration_id: "fixture-app".into(),
            integration_name: "Fixture App".into(),
            tool: "write_action".into(),
            arguments_preview: r#"{"payload":"a"}"#.into(),
            reason: "Escrever teste".into(),
        })
        .unwrap();
    let card_once = w_once.card().clone();
    let (st_stale, _, _) = call_owner(
        srv.addr,
        Method::POST,
        &format!("/api/integrations/approvals/{}", card_once.id),
        Some(json!({ "interaction_id": "velho", "decision": "allow_once" })),
    )
    .await;
    assert_eq!(st_stale, 409);

    let (st_once, b_once, _) = call_owner(
        srv.addr,
        Method::POST,
        &format!("/api/integrations/approvals/{}", card_once.id),
        Some(json!({ "interaction_id": card_once.interaction_id, "decision": "allow_once" })),
    )
    .await;
    assert_eq!(st_once, 200);
    assert_eq!(b_once["decision"], "allow_once");
    assert_eq!(w_once.wait().await, ApprovalResolution::AllowOnce);

    // 2. allow_always persiste em allowed_tools e muda effective_access no GET /tools.
    let w_always = srv
        .state
        .integrations
        .approvals
        .open_request(OpenApprovalRequest {
            origin: "sessao-1".into(),
            session_key: "sk-1".into(),
            integration_id: "fixture-app".into(),
            integration_name: "Fixture App".into(),
            tool: "write_action".into(),
            arguments_preview: "{}".into(),
            reason: "Liberar sempre".into(),
        })
        .unwrap();
    let card_always = w_always.card().clone();
    let (st_always, _, _) = call_owner(
        srv.addr,
        Method::POST,
        &format!("/api/integrations/approvals/{}", card_always.id),
        Some(json!({ "interaction_id": card_always.interaction_id, "decision": "allow_always" })),
    )
    .await;
    assert_eq!(st_always, 200);
    assert_eq!(w_always.wait().await, ApprovalResolution::AllowAlways);

    let (_, integ_list, _) = call_owner(srv.addr, Method::GET, "/api/integrations", None).await;
    assert_eq!(integ_list["integrations"][0]["allowed_tools"], json!(["write_action"]));
    let (_, tools_body, _) = call_owner(srv.addr, Method::GET, "/api/integrations/fixture-app/tools", None).await;
    assert!(
        tools_body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "write_action" && t["effective_access"] == "allowed_always")
    );

    // 3. deny gera cooldown na sessão.
    let w_deny = srv
        .state
        .integrations
        .approvals
        .open_request(OpenApprovalRequest {
            origin: "sessao-1".into(),
            session_key: "sk-1".into(),
            integration_id: "fixture-app".into(),
            integration_name: "Fixture App".into(),
            tool: "write_action".into(),
            arguments_preview: "{}".into(),
            reason: "Negar".into(),
        })
        .unwrap();
    let card_deny = w_deny.card().clone();
    let (st_deny, _, _) = call_owner(
        srv.addr,
        Method::POST,
        &format!("/api/integrations/approvals/{}", card_deny.id),
        Some(json!({ "interaction_id": card_deny.interaction_id, "decision": "deny" })),
    )
    .await;
    assert_eq!(st_deny, 200);
    assert_eq!(w_deny.wait().await, ApprovalResolution::Denied);

    let cooldown_res = srv.state.integrations.approvals.open_request(OpenApprovalRequest {
        origin: "sessao-1".into(),
        session_key: "sk-1".into(),
        integration_id: "fixture-app".into(),
        integration_name: "Fixture App".into(),
        tool: "write_action".into(),
        arguments_preview: "{}".into(),
        reason: "Repetir".into(),
    });
    assert!(matches!(cooldown_res, Err(hangar_integrations::ApprovalError::Cooldown)));

    // 4. DELETE cancela pedido e segundo DELETE dá 404.
    let w_cancel = srv
        .state
        .integrations
        .approvals
        .open_request(OpenApprovalRequest {
            origin: "sessao-2".into(),
            session_key: "sk-2".into(),
            integration_id: "fixture-app".into(),
            integration_name: "Fixture App".into(),
            tool: "write_action".into(),
            arguments_preview: "{}".into(),
            reason: "Cancelar".into(),
        })
        .unwrap();
    let cancel_id = w_cancel.card().id.clone();
    let (st_del, _, _) = call_owner(
        srv.addr,
        Method::DELETE,
        &format!("/api/integrations/approvals/{cancel_id}"),
        None,
    )
    .await;
    assert_eq!(st_del, 200);
    assert_eq!(w_cancel.wait().await, ApprovalResolution::Cancelled);

    let (st_del2, _, _) = call_owner(
        srv.addr,
        Method::DELETE,
        &format!("/api/integrations/approvals/{cancel_id}"),
        None,
    )
    .await;
    assert_eq!(st_del2, 404);
    srv.state.integrations.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn approvals_sse_publishes_to_owner_only_and_never_to_share_guest() {
    use futures_util::StreamExt;
    use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
    use hangar_server::list::facts::FactsClient;
    use hangar_server::list::mux::Mux;
    use hangar_server::list::procs::{ChildrenMap, ProcessView};
    use hangar_server::share::ShareState;
    use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
    use hangar_server::share::pairs::PairStore;
    use hangar_server::share::store::{Kind, ShareStore};
    use hangar_server::share::tunnel::{Output, Tailscale};

    struct DummyProcs;
    impl ProcessView for DummyProcs {
        fn children(&self, _age: Duration) -> std::io::Result<Arc<ChildrenMap>> { Ok(Arc::default()) }
        fn argv(&self, _pid: i64) -> Vec<String> { Vec::new() }
        fn cwd(&self, _pid: i64) -> Option<PathBuf> { None }
        fn env_var(&self, _pid: i64, _name: &str) -> std::io::Result<Option<std::ffi::OsString>> { Ok(None) }
        fn start_time(&self, _pid: i64) -> Option<f64> { None }
        fn fds(&self, _pid: i64) -> Vec<PathBuf> { Vec::new() }
    }
    struct DummyLife;
    impl LifeProbe for DummyLife {
        fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { Some(1) }) }
        fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
        fn sidecar(&self, name: &str) -> Option<(String, bool)> { Some((format!("L-{name}"), false)) }
        fn sidecar_exists(&self, _name: &str) -> bool { false }
    }
    struct DummyTs;
    impl Tailscale for DummyTs {
        fn run<'a>(&'a self, _args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
            Box::pin(async { Err("offline".into()) })
        }
        fn needs_operator(&self) -> bool { false }
    }

    tokio::time::timeout(Duration::from_secs(15), async {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let script = crate::list_support::sessions(root, 1);
    let home = root.join("home");
    let dirs = parse_dirs(
        &json!({
            "home": home,
            "claude": home.join(".claude"),
            "codex_home": home.join(".codex"),
            "pi_sessions": home.join(".pi/agent/sessions"),
            "omp_config": home.join(".omp"),
            "omp_agent": home.join(".omp/agent"),
            "kimi_home": home.join(".kimi-code")
        })
        .to_string(),
    );

    let owner_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner_addr = owner_listener.local_addr().unwrap();
    let probe_guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest_addr = probe_guest.local_addr().unwrap();
    drop(probe_guest);

    let mut cfg = config("127.0.0.1");
    cfg.listen = owner_addr;
    let (mut state, _) = build_isolated_state(&dir, cfg, Arc::new(MemoryKeyStore::new()));
    let list = Arc::new(ListBridge::new(
        ListEnv {
            mux: Mux::with_program(&script, Duration::from_secs(5)),
            capture_program: script.into_os_string(),
            procs: Arc::new(DummyProcs),
            dirs,
        },
        FactsClient::new(),
    ));
    list.set_problem_hold(Duration::ZERO);
    let share_store = ShareStore::open(root.join("shares.json"), Arc::new(|| 1_000.0));
    let (_, guest_token) = share_store
        .create_redeemed("s0", Some("k:L-s0".into()), Kind::Share)
        .unwrap();
    let facts = Arc::new(LifeFacts::default());
    facts.apply_transfer_snapshot(Default::default(), Default::default());
    let share = Arc::new(ShareState {
        store: share_store,
        pairs: PairStore::open(root.join("pairs.json")),
        lives: Lives::new(Arc::new(DummyLife), facts),
        tailscale: Arc::new(DummyTs),
        tunnel: Default::default(),
        connect: None,
    });
    list.set_shares(share.clone());
    state.list = list.clone();
    state.side.locate = Arc::new(hangar_server::locate::ListLocate { list });
    state.state = Arc::new(hangar_server::state::live::StateEnv::new(
        state.terminal.clone(),
        state.list.clone(),
        state.diag.clone(),
    ));
    state.share = Some(share);

    let integrations = state.integrations.clone();
    let task = tokio::spawn(async move {
        serve_with_guest(owner_listener, state, Some(guest_addr)).await.unwrap();
    });

    // Aguarda SSEs do dono e do convidado responderem com prazo e pausa controlada.
    let owner_resp = wait_http_ready(
        &format!("http://{owner_addr}/api/sessions/events?token={OWNER}"),
        Method::GET,
        &[],
        None,
    )
    .await;
    let guest_resp = wait_http_ready(
        &format!("http://{guest_addr}/api/sessions/events?token={guest_token}"),
        Method::GET,
        &[],
        None,
    )
    .await;

    let mut owner_sse = crate::fake::sse(owner_resp);
    let mut guest_sse = crate::fake::sse(guest_resp);

    // Abre aprovação e confirma entrega ao dono no evento `integrations_approvals`.
    let waiter = integrations
        .approvals
        .open_request(OpenApprovalRequest {
            origin: "sessao-sse".into(),
            session_key: "sk-sse".into(),
            integration_id: "github".into(),
            integration_name: "GitHub".into(),
            tool: "create_issue".into(),
            arguments_preview: r#"{"title":"Teste"}"#.into(),
            reason: "Criar issue".into(),
        })
        .unwrap();
    let req_id = waiter.card().id.clone();

    let ev = crate::fake::next_named(&mut owner_sse, "integrations_approvals").await;
    let payload: Value = serde_json::from_str(&ev.data).unwrap();
    assert_eq!(payload["requests"][0]["id"], req_id);
    assert_eq!(payload["requests"][0]["tool"], "create_issue");

    // Convidado recebe `sessions`, mas nunca `integrations_approvals`.
    let _ = crate::fake::next_named(&mut guest_sse, "sessions").await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(350);
    while let Ok(Some(Ok(ev))) = tokio::time::timeout_at(deadline, guest_sse.next()).await {
        assert_ne!(
            ev.event, "integrations_approvals",
            "convidado nunca pode receber eventos integrations_approvals"
        );
    }

    // Limpa pedido aberto.
    let (st_del, _, _) = call_owner(
        owner_addr,
        Method::DELETE,
        &format!("/api/integrations/approvals/{req_id}"),
        None,
    )
    .await;
    assert_eq!(st_del, 200);
    task.abort();
    })
    .await
    .expect("prazo do SSE excedido");
}
