use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hangar_integrations::{
    AuthConfig, ClientConfig, IntegrationConfig, IntegrationRegistry, IntegrationSecrets,
    IntegrationStatus, IntegrationUpdate, McpClientManager, MemoryKeyStore, OAUTH_CALLBACK_PATH,
    OAuthError, OAuthManager, OAuthOrigin, OAuthStartInput, OAuthStartOutcome, PermissionPolicy,
    PresetAuthKind, SecretStore, TokenRefresher, ToolsCache, Transport, catalog, compute_status,
    find_preset,
};
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
use tokio::sync::{Mutex, Notify, RwLock};
use zeroize::Zeroizing;

struct TestEnv {
    _dir: TempDir,
    registry: Arc<IntegrationRegistry>,
    secrets: Arc<SecretStore>,
    tools_cache: Arc<ToolsCache>,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("criar tempdir");
        let registry = Arc::new(IntegrationRegistry::new(dir.path()));
        let keys = Arc::new(MemoryKeyStore::new());
        let secrets = Arc::new(SecretStore::new(dir.path(), keys));
        let tools_cache = Arc::new(ToolsCache::new());
        Self {
            _dir: dir,
            registry,
            secrets,
            tools_cache,
        }
    }

    fn oauth_manager(&self) -> Arc<OAuthManager> {
        Arc::new(OAuthManager::new(
            self.registry.clone(),
            self.secrets.clone(),
        ))
    }

    fn client_manager(&self, oauth: Arc<OAuthManager>) -> McpClientManager {
        McpClientManager::new(
            self.registry.clone(),
            self.secrets.clone(),
            self.tools_cache.clone(),
            ClientConfig::default(),
        )
        .with_refresher(oauth)
    }
}

fn remote_config(
    id: &str,
    url: String,
    preset: Option<&str>,
    auth: AuthConfig,
) -> IntegrationConfig {
    IntegrationConfig {
        id: id.to_owned(),
        name: format!("Integração {id}"),
        preset: preset.map(str::to_owned),
        enabled: true,
        transport: Transport::RemoteHttp {
            url,
            header_names: vec![],
        },
        auth,
        policy: PermissionPolicy::AllowAll,
        pinned: false,
        projects: vec![],
        allowed_tools: BTreeSet::new(),
        created_at: 1_700_000_000.0,
        updated_at: 1_700_000_000.0,
    }
}

#[derive(Clone)]
struct RegisteredClient {
    client_secret: Option<String>,
    redirect_uris: Vec<String>,
}

#[derive(Clone)]
struct PendingMockAuth {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
}

#[derive(Clone)]
struct MockOAuthConfig {
    use_www_authenticate_discovery: bool,
    auth_server_path_prefix: String,
    oidc_fallback_only: bool,
    dcr_supported: bool,
    dcr_issues_secret: bool,
    cimd_supported: bool,
    rotate_refresh_tokens: bool,
}

impl Default for MockOAuthConfig {
    fn default() -> Self {
        Self {
            use_www_authenticate_discovery: false,
            auth_server_path_prefix: String::new(),
            oidc_fallback_only: false,
            dcr_supported: true,
            dcr_issues_secret: false,
            cimd_supported: false,
            rotate_refresh_tokens: true,
        }
    }
}

type RefreshGate = Arc<Mutex<Option<(Arc<Notify>, Arc<Notify>)>>>;

#[derive(Clone)]
struct MockOAuthServerState {
    base_url: Arc<RwLock<String>>,
    cfg: MockOAuthConfig,
    clients: Arc<Mutex<HashMap<String, RegisteredClient>>>,
    codes: Arc<Mutex<HashMap<String, PendingMockAuth>>>,
    active_access_token: Arc<RwLock<String>>,
    active_refresh_token: Arc<RwLock<Option<String>>>,
    force_invalid_grant: Arc<AtomicBool>,
    omit_refresh_token_on_issue: Arc<AtomicBool>,
    register_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    in_flight_refresh: Arc<AtomicUsize>,
    max_concurrent_refresh: Arc<AtomicUsize>,
    refresh_tokens_seen: Arc<Mutex<Vec<String>>>,
    authorize_resources: Arc<Mutex<Vec<String>>>,
    token_resources: Arc<Mutex<Vec<(String, String)>>>,
    token_seq: Arc<AtomicUsize>,
    code_seq: Arc<AtomicUsize>,
    refresh_gate: RefreshGate,
}

impl MockOAuthServerState {
    fn new(cfg: MockOAuthConfig) -> Self {
        Self {
            base_url: Arc::new(RwLock::new(String::new())),
            cfg,
            clients: Arc::new(Mutex::new(HashMap::new())),
            codes: Arc::new(Mutex::new(HashMap::new())),
            active_access_token: Arc::new(RwLock::new(String::new())),
            active_refresh_token: Arc::new(RwLock::new(None)),
            force_invalid_grant: Arc::new(AtomicBool::new(false)),
            omit_refresh_token_on_issue: Arc::new(AtomicBool::new(false)),
            register_calls: Arc::new(AtomicUsize::new(0)),
            refresh_calls: Arc::new(AtomicUsize::new(0)),
            in_flight_refresh: Arc::new(AtomicUsize::new(0)),
            max_concurrent_refresh: Arc::new(AtomicUsize::new(0)),
            refresh_tokens_seen: Arc::new(Mutex::new(Vec::new())),
            authorize_resources: Arc::new(Mutex::new(Vec::new())),
            token_resources: Arc::new(Mutex::new(Vec::new())),
            token_seq: Arc::new(AtomicUsize::new(0)),
            code_seq: Arc::new(AtomicUsize::new(0)),
            refresh_gate: Arc::new(Mutex::new(None)),
        }
    }

    async fn add_static_client(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        redirect_uris: Vec<String>,
    ) {
        self.clients.lock().await.insert(
            client_id.to_owned(),
            RegisteredClient {
                client_secret: client_secret.map(str::to_owned),
                redirect_uris,
            },
        );
    }
}

#[derive(Clone)]
struct MockMcpHandler;

impl ServerHandler for MockMcpHandler {
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
        let tool = Tool::new("echo_oauth", "Eco autenticado", schema)
            .with_annotations(ToolAnnotations::new().read_only(true));
        Ok(ListToolsResult::with_all_items(vec![tool]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let msg = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("");
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![
            ContentBlock::text(format!("ok:{msg}")),
        ])))
    }
}

fn json_response(status: StatusCode, value: Value) -> Response {
    let body = serde_json::to_vec(&value).expect("serializar JSON");
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn verify_pkce_s256(verifier: &str, expected_challenge: &str) -> bool {
    let hashed = digest(&SHA256, verifier.as_bytes());
    let computed = URL_SAFE_NO_PAD.encode(hashed.as_ref());
    computed == expected_challenge
}

async fn mcp_auth_middleware(
    State(state): State<MockOAuthServerState>,
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
        let query_suffix = req
            .uri()
            .query()
            .filter(|q| !q.is_empty())
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let meta_url = if state.cfg.use_www_authenticate_discovery {
            format!("{base}/custom-prm.json{query_suffix}")
        } else {
            format!("{base}/.well-known/oauth-protected-resource{query_suffix}")
        };
        let mut resp = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        let header_val = format!(r#"Bearer resource_metadata="{meta_url}""#);
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_str(&header_val).unwrap(),
        );
        return resp;
    }
    next.run(req).await
}

async fn protected_resource_metadata(
    State(state): State<MockOAuthServerState>,
    uri: axum::http::Uri,
) -> Response {
    if state.cfg.use_www_authenticate_discovery {
        return StatusCode::NOT_FOUND.into_response();
    }
    let base = state.base_url.read().await.clone();
    let issuer = format!("{base}{}", state.cfg.auth_server_path_prefix);
    let resource = match uri.query() {
        Some(q) if !q.is_empty() => format!("{base}/mcp?{q}"),
        _ => format!("{base}/mcp"),
    };
    json_response(
        StatusCode::OK,
        json!({
            "resource": resource,
            "authorization_servers": [issuer],
            "scopes_supported": ["read", "write"]
        }),
    )
}

async fn custom_protected_resource_metadata(State(state): State<MockOAuthServerState>) -> Response {
    let base = state.base_url.read().await.clone();
    let issuer = format!("{base}{}", state.cfg.auth_server_path_prefix);
    json_response(
        StatusCode::OK,
        json!({
            "resource": format!("{base}/mcp"),
            "authorization_servers": [issuer]
        }),
    )
}

async fn auth_server_metadata(State(state): State<MockOAuthServerState>) -> Response {
    if state.cfg.oidc_fallback_only {
        return StatusCode::NOT_FOUND.into_response();
    }
    build_as_metadata(&state).await
}

async fn openid_config_metadata(State(state): State<MockOAuthServerState>) -> Response {
    build_as_metadata(&state).await
}

async fn build_as_metadata(state: &MockOAuthServerState) -> Response {
    let base = state.base_url.read().await.clone();
    let mut obj = json!({
        "issuer": format!("{base}{}", state.cfg.auth_server_path_prefix),
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "code_challenge_methods_supported": ["S256"],
        "client_id_metadata_document_supported": state.cfg.cimd_supported
    });
    if state.cfg.dcr_supported {
        obj["registration_endpoint"] = json!(format!("{base}/register"));
    }
    json_response(StatusCode::OK, obj)
}

async fn register_client(State(state): State<MockOAuthServerState>, body: Bytes) -> Response {
    state.register_calls.fetch_add(1, Ordering::SeqCst);
    if !state.cfg.dcr_supported {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"error": "invalid_request", "error_description": "DCR desabilitado"}),
        );
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return json_response(StatusCode::BAD_REQUEST, json!({"error": "invalid_request"}));
    };
    let redirect_uris: Vec<String> = payload
        .get("redirect_uris")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if redirect_uris.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"error": "invalid_redirect_uri"}),
        );
    }
    let id_num = state.register_calls.load(Ordering::SeqCst);
    let client_id = format!("dcr-client-{id_num}");
    let client_secret = state
        .cfg
        .dcr_issues_secret
        .then(|| format!("dcr-secret-{id_num}"));

    state.clients.lock().await.insert(
        client_id.clone(),
        RegisteredClient {
            client_secret: client_secret.clone(),
            redirect_uris: redirect_uris.clone(),
        },
    );

    let mut resp = json!({
        "client_id": client_id,
        "redirect_uris": redirect_uris,
        "token_endpoint_auth_method": if client_secret.is_some() { "client_secret_post" } else { "none" }
    });
    if let Some(secret) = client_secret {
        resp["client_secret"] = json!(secret);
    }
    json_response(StatusCode::CREATED, resp)
}

async fn authorize_handler(
    State(state): State<MockOAuthServerState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let response_type = params.get("response_type").map(String::as_str);
    let client_id = params.get("client_id").cloned().unwrap_or_default();
    let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
    let state_param = params.get("state").cloned().unwrap_or_default();
    let code_challenge = params.get("code_challenge").cloned().unwrap_or_default();
    let method = params.get("code_challenge_method").map(String::as_str);

    if response_type != Some("code")
        || client_id.is_empty()
        || redirect_uri.is_empty()
        || state_param.is_empty()
        || code_challenge.is_empty()
        || method != Some("S256")
    {
        return (StatusCode::BAD_REQUEST, "parametros authorize invalidos").into_response();
    }

    // Para CIMD o client_id é uma URL HTTPS; para DCR/Manual precisa estar registrado.
    let is_cimd = state.cfg.cimd_supported && client_id.starts_with("https://");
    if !is_cimd {
        let clients = state.clients.lock().await;
        let Some(registered) = clients.get(&client_id) else {
            return (StatusCode::BAD_REQUEST, "client_id desconhecido").into_response();
        };
        if !registered.redirect_uris.contains(&redirect_uri) {
            return (StatusCode::BAD_REQUEST, "redirect_uri nao autorizada").into_response();
        }
    }

    if let Some(res) = params.get("resource") {
        state.authorize_resources.lock().await.push(res.clone());
    }

    let seq = state.code_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let code = format!("auth-code-{seq}");
    state.codes.lock().await.insert(
        code.clone(),
        PendingMockAuth {
            client_id,
            redirect_uri: redirect_uri.clone(),
            code_challenge,
        },
    );

    let mut ser = form_urlencoded::Serializer::new(String::new());
    ser.append_pair("code", &code);
    ser.append_pair("state", &state_param);
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let location = format!("{redirect_uri}{sep}{}", ser.finish());

    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn token_handler(State(state): State<MockOAuthServerState>, body: Bytes) -> Response {
    let form: HashMap<String, String> = form_urlencoded::parse(&body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let grant_type = form.get("grant_type").map(String::as_str).unwrap_or("");
    let client_id = form.get("client_id").cloned().unwrap_or_default();
    let client_secret = form.get("client_secret").cloned();
    if let Some(res) = form.get("resource") {
        state
            .token_resources
            .lock()
            .await
            .push((grant_type.to_owned(), res.clone()));
    }

    let is_cimd = state.cfg.cimd_supported && client_id.starts_with("https://");
    if !is_cimd {
        let clients = state.clients.lock().await;
        let Some(registered) = clients.get(&client_id) else {
            return json_response(
                StatusCode::UNAUTHORIZED,
                json!({"error": "invalid_client", "error_description": "client_id nao encontrado"}),
            );
        };
        if let Some(expected_secret) = &registered.client_secret
            && client_secret.as_deref() != Some(expected_secret.as_str())
        {
            return json_response(
                StatusCode::UNAUTHORIZED,
                json!({"error": "invalid_client", "error_description": "client_secret incorreto"}),
            );
        }
    }

    match grant_type {
        "authorization_code" => {
            let code = form.get("code").cloned().unwrap_or_default();
            let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
            let code_verifier = form.get("code_verifier").cloned().unwrap_or_default();

            let pending = state.codes.lock().await.remove(&code);
            let Some(pending) = pending else {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error": "invalid_grant", "error_description": "code invalido ou reutilizado"}),
                );
            };
            if pending.client_id != client_id || pending.redirect_uri != redirect_uri {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error": "invalid_grant", "error_description": "client_id ou redirect_uri divergente"}),
                );
            }
            if !verify_pkce_s256(&code_verifier, &pending.code_challenge) {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error": "invalid_grant", "error_description": "code_verifier invalido"}),
                );
            }

            let seq = state.token_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let access = format!("access-token-v{seq}");
            let refresh = if state.omit_refresh_token_on_issue.load(Ordering::SeqCst) {
                None
            } else {
                Some(format!("refresh-token-v{seq}"))
            };
            *state.active_access_token.write().await = access.clone();
            *state.active_refresh_token.write().await = refresh.clone();

            let mut resp = json!({
                "access_token": access,
                "token_type": "Bearer",
                "expires_in": 3600
            });
            if let Some(rt) = refresh {
                resp["refresh_token"] = json!(rt);
            }
            json_response(StatusCode::OK, resp)
        }
        "refresh_token" => {
            let in_flight = state.in_flight_refresh.fetch_add(1, Ordering::SeqCst) + 1;
            state
                .max_concurrent_refresh
                .fetch_max(in_flight, Ordering::SeqCst);
            state.refresh_calls.fetch_add(1, Ordering::SeqCst);

            let provided_rt = form.get("refresh_token").cloned().unwrap_or_default();
            state
                .refresh_tokens_seen
                .lock()
                .await
                .push(provided_rt.clone());

            let result = if state.force_invalid_grant.load(Ordering::SeqCst) {
                json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error": "invalid_grant", "error_description": "refresh token expirado ou revogado"}),
                )
            } else {
                let mut active_rt = state.active_refresh_token.write().await;
                if active_rt.as_deref() != Some(provided_rt.as_str()) {
                    json_response(
                        StatusCode::BAD_REQUEST,
                        json!({"error": "invalid_grant", "error_description": "refresh token nao corresponde ao ativo"}),
                    )
                } else {
                    let seq = state.token_seq.fetch_add(1, Ordering::SeqCst) + 1;
                    let new_access = format!("access-token-v{seq}");
                    *state.active_access_token.write().await = new_access.clone();
                    let mut resp = json!({
                        "access_token": new_access,
                        "token_type": "Bearer",
                        "expires_in": 3600
                    });
                    if state.cfg.rotate_refresh_tokens {
                        let new_rt = format!("refresh-token-v{seq}");
                        *active_rt = Some(new_rt.clone());
                        resp["refresh_token"] = json!(new_rt);
                    }
                    json_response(StatusCode::OK, resp)
                }
            };

            // Segura a resposta HTTP após calcular o resultado para simular latência de rede após emissão.
            let maybe_gate = state.refresh_gate.lock().await.take();
            if let Some((started, allow)) = maybe_gate {
                started.notify_one();
                if tokio::time::timeout(Duration::from_secs(3), allow.notified())
                    .await
                    .is_err()
                {
                    state.in_flight_refresh.fetch_sub(1, Ordering::SeqCst);
                    panic!("refresh_gate expirou antes de o teste liberar allow");
                }
            }
            state.in_flight_refresh.fetch_sub(1, Ordering::SeqCst);
            result
        }
        _ => json_response(
            StatusCode::BAD_REQUEST,
            json!({"error": "unsupported_grant_type"}),
        ),
    }
}

async fn spawn_mock_oauth_server(
    cfg: MockOAuthConfig,
) -> (
    SocketAddr,
    MockOAuthServerState,
    tokio::task::JoinHandle<()>,
) {
    let state = MockOAuthServerState::new(cfg.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind porta efêmera");
    let addr = listener.local_addr().unwrap();
    *state.base_url.write().await = format!("http://{addr}");

    let mcp_service = StreamableHttpService::new(
        || Ok(MockMcpHandler),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let mcp_router = Router::new().nest_service("/mcp", mcp_service).route_layer(
        axum::middleware::from_fn_with_state(state.clone(), mcp_auth_middleware),
    );

    let prefix = cfg.auth_server_path_prefix.trim_end_matches('/');
    let as_path_aware = if prefix.is_empty() {
        "/.well-known/oauth-authorization-server".to_owned()
    } else {
        format!("/.well-known/oauth-authorization-server{prefix}")
    };

    let mut app = Router::new()
        .merge(mcp_router)
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .route("/custom-prm.json", get(custom_protected_resource_metadata))
        .route(&as_path_aware, get(auth_server_metadata))
        .route(
            "/.well-known/openid-configuration",
            get(openid_config_metadata),
        )
        .route("/register", post(register_client))
        .route("/authorize", get(authorize_handler))
        .route("/token", post(token_handler));

    if !prefix.is_empty() {
        let oidc_path = format!("/.well-known/openid-configuration{prefix}");
        app = app.route(&oidc_path, get(openid_config_metadata));
    }

    let app = app.with_state(state.clone());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state, handle)
}

async fn simulate_browser_authorize(authorize_url: &str) -> (String, String) {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client
        .get(authorize_url)
        .send()
        .await
        .expect("requisição ao /authorize");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::FOUND,
        "esperava 302 do /authorize"
    );
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location no 302")
        .to_str()
        .unwrap();
    let query = location.split_once('?').expect("query no redirect").1;
    let params: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    (
        params.get("code").cloned().expect("code no redirect"),
        params.get("state").cloned().expect("state no redirect"),
    )
}

fn query_params(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

#[test]
fn catalog_exposes_all_12_presets_with_expected_urls_auth_and_guides() {
    let items = catalog();
    assert_eq!(items.len(), 12, "catálogo pronto deve conter 12 presets");

    let expected = [
        (
            "google-gmail",
            "https://gmailmcp.googleapis.com/mcp/v1self",
            PresetAuthKind::OAuthManualClient,
            false,
            true,
            true,
        ),
        (
            "google-drive",
            "https://drivemcp.googleapis.com/mcp/v1self",
            PresetAuthKind::OAuthManualClient,
            false,
            true,
            true,
        ),
        (
            "google-calendar",
            "https://calendarmcp.googleapis.com/mcp/v1self",
            PresetAuthKind::OAuthManualClient,
            false,
            true,
            true,
        ),
        (
            "notion",
            "https://mcp.notion.com/mcp",
            PresetAuthKind::OAuthDcr,
            false,
            false,
            false,
        ),
        (
            "linear",
            "https://mcp.linear.app/mcp",
            PresetAuthKind::OAuthDcr,
            false,
            false,
            false,
        ),
        (
            "slack",
            "https://mcp.slack.com/mcp",
            PresetAuthKind::OAuthManualClient,
            false,
            true,
            true,
        ),
        (
            "figma",
            "https://mcp.figma.com/mcp",
            PresetAuthKind::OAuthDcr,
            false,
            false,
            false,
        ),
        (
            "atlassian",
            "https://mcp.atlassian.com/v2/mcp",
            PresetAuthKind::OAuthDcr,
            false,
            false,
            false,
        ),
        (
            "github",
            "https://api.githubcopilot.com/mcp/",
            PresetAuthKind::OAuthDcrOrBearer,
            false,
            false,
            false,
        ),
        (
            "cloudflare",
            "https://docs.mcp.cloudflare.com/mcp",
            PresetAuthKind::OAuthDcr,
            true,
            false,
            false,
        ),
        (
            "sentry",
            "https://mcp.sentry.dev/mcp",
            PresetAuthKind::OAuthDcr,
            true,
            false,
            false,
        ),
        (
            "supabase",
            "https://mcp.supabase.com/mcp",
            PresetAuthKind::OAuthDcr,
            true,
            false,
            false,
        ),
    ];

    for (id, url, auth_kind, url_editable, loopback, requires_guide) in expected {
        let preset = find_preset(id).unwrap_or_else(|| panic!("preset '{id}' ausente"));
        assert_eq!(preset.url, url, "url divergente para {id}");
        assert_eq!(
            preset.auth_kind, auth_kind,
            "auth_kind divergente para {id}"
        );
        assert_eq!(
            preset.url_editable, url_editable,
            "url_editable divergente para {id}"
        );
        assert_eq!(
            preset.requires_loopback_redirect, loopback,
            "requires_loopback_redirect divergente para {id}"
        );
        assert!(
            matches!(
                preset.category.as_str(),
                "productivity" | "dev" | "cloud" | "design"
            ),
            "categoria inválida '{}' para {id}",
            preset.category
        );
        if requires_guide {
            let guide = preset
                .setup_guide
                .as_ref()
                .unwrap_or_else(|| panic!("setup_guide obrigatório para {id}"));
            assert!(!guide.title.trim().is_empty());
            assert!(!guide.steps.is_empty());
            let joined = guide.steps.join(" ");
            if id == "slack" {
                assert!(
                    joined.contains("http://localhost:") && joined.contains(OAUTH_CALLBACK_PATH),
                    "guia do Slack deve orientar URI http://localhost (não IP literal 127.0.0.1)"
                );
            } else {
                assert!(
                    joined.contains("127.0.0.1") && joined.contains(OAUTH_CALLBACK_PATH),
                    "guia de {id} deve orientar URI loopback"
                );
            }
            if id == "google-calendar" {
                assert!(
                    !preset.description.to_lowercase().contains("gerenciamento"),
                    "descrição do Google Agenda com calendar.readonly deve ser somente leitura"
                );
            }
            if id.starts_with("google-") {
                assert!(joined.contains("is_agentic") && joined.contains("@gmail.com"));
                assert!(joined.contains("7 dias"));
                assert!(
                    joined.contains("Público-alvo")
                        && joined.contains("Usuários de teste")
                        && joined.contains("Adicionar usuários"),
                    "guia Google de {id} deve orientar cadastro em Usuários de teste"
                );
            }
            if id == "slack" {
                assert!(joined.contains("is_mcp_enabled") && joined.contains("PKCE"));
                for scope in [
                    "search:read",
                    "channels:history",
                    "groups:history",
                    "im:history",
                    "mpim:history",
                    "channels:read",
                    "users:read",
                ] {
                    assert!(
                        joined.contains(scope),
                        "guia Slack deve listar o User Token Scope {scope}"
                    );
                }
            }
        }
        if id == "cloudflare" {
            assert!(
                preset.description.contains("docs.mcp.cloudflare.com/mcp")
                    && preset.description.contains("Workers"),
                "descrição da Cloudflare deve explicar o endpoint padrão de docs e edição para Workers/DNS"
            );
        }
    }
    assert!(find_preset("nao-existe").is_none());
}

#[tokio::test]
async fn dcr_pkce_s256_full_flow_persists_tokens_and_reuses_client_registration() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        use_www_authenticate_discovery: true,
        auth_server_path_prefix: "/tenant-a".to_owned(),
        dcr_supported: true,
        dcr_issues_secret: true,
        ..Default::default()
    })
    .await;

    let env = TestEnv::new();
    let cfg = remote_config(
        "notion-work",
        format!("http://{addr}/mcp"),
        Some("notion"),
        AuthConfig::OAuthDcr,
    );
    env.registry.create(cfg.clone()).unwrap();

    // Antes do login -> AuthRequired.
    assert_eq!(
        compute_status(&cfg, env.secrets.get("notion-work").await.unwrap().as_ref()),
        IntegrationStatus::AuthRequired
    );

    let oauth = env.oauth_manager();
    let started = oauth
        .start(
            "notion-work",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .expect("start DCR deve suceder");

    let (authorize_url, redirect_uri) = match started {
        OAuthStartOutcome::OpenUrl {
            authorize_url,
            redirect_uri,
        } => (authorize_url, redirect_uri),
        other => panic!("esperava OpenUrl no desktop, veio {other:?}"),
    };
    assert_eq!(
        redirect_uri,
        "http://127.0.0.1:8765/api/integrations/oauth/callback"
    );
    assert_eq!(server_state.register_calls.load(Ordering::SeqCst), 1);

    let params = query_params(&authorize_url);
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert!(params.contains_key("code_challenge"));
    assert!(
        !authorize_url.contains("dcr-secret"),
        "client_secret nunca pode vazar na authorize_url"
    );

    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    let callback = oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .expect("callback deve trocar code por tokens validando PKCE");
    assert_eq!(callback.integration_id, "notion-work");
    assert_eq!(callback.status, IntegrationStatus::Ready);

    let stored = env.secrets.get("notion-work").await.unwrap().unwrap();
    let tokens = stored.oauth.as_ref().expect("tokens OAuth salvos");
    assert_eq!(tokens.access_token.as_str(), "access-token-v1");
    assert_eq!(
        tokens.refresh_token.as_deref().map(String::as_str),
        Some("refresh-token-v1")
    );
    assert_eq!(tokens.registered_client_id.as_deref(), Some("dcr-client-1"));
    assert_eq!(
        tokens
            .registered_client_secret
            .as_deref()
            .map(String::as_str),
        Some("dcr-secret-1")
    );
    assert!(!tokens.auth_expired);
    assert_eq!(
        compute_status(&cfg, Some(&stored)),
        IntegrationStatus::Ready
    );

    // Segundo start com mesma porta e token_endpoint reutiliza client DCR sem novo /register.
    let started_again = oauth
        .start(
            "notion-work",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .expect("reconectar deve reutilizar client registrado");
    assert!(matches!(started_again, OAuthStartOutcome::OpenUrl { .. }));
    assert_eq!(
        server_state.register_calls.load(Ordering::SeqCst),
        1,
        "client DCR já salvo para mesmo token_endpoint e redirect_uri deve ser reutilizado"
    );

    server_handle.abort();
}

#[tokio::test]
async fn cimd_uses_client_id_metadata_url_without_calling_dcr_register() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        oidc_fallback_only: true,
        dcr_supported: true,
        cimd_supported: true,
        ..Default::default()
    })
    .await;

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "linear-cimd",
            format!("http://{addr}/mcp"),
            Some("linear"),
            AuthConfig::OAuthDcr,
        ))
        .unwrap();

    let cimd_url = "https://hangar.dev.br/oauth/client-metadata.json";
    let oauth = env.oauth_manager();
    let started = oauth
        .start(
            "linear-cimd",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: Some(cimd_url.to_owned()),
            },
        )
        .await
        .expect("CIMD deve funcionar sem chamar /register");

    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = started else {
        panic!("esperava OpenUrl");
    };
    assert_eq!(
        server_state.register_calls.load(Ordering::SeqCst),
        0,
        "CIMD não deve chamar /register"
    );
    let params = query_params(&authorize_url);
    assert_eq!(params.get("client_id").map(String::as_str), Some(cimd_url));

    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    let res = oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .expect("callback CIMD deve concluir");
    assert_eq!(res.status, IntegrationStatus::Ready);

    server_handle.abort();
}

#[tokio::test]
async fn manual_client_google_sends_client_secret_on_token_exchange_never_in_authorize_url() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        dcr_supported: false,
        ..Default::default()
    })
    .await;
    let redirect = "http://127.0.0.1:8765/api/integrations/oauth/callback".to_owned();
    let secret_value = "GOCSPX-super-segredo-google-123";
    server_state
        .add_static_client("google-cid-99", Some(secret_value), vec![redirect.clone()])
        .await;

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "gmail-pessoal",
            format!("http://{addr}/mcp"),
            Some("google-gmail"),
            AuthConfig::OAuthManualClient {
                client_id: "google-cid-99".to_owned(),
                scopes: vec!["https://www.googleapis.com/auth/gmail.readonly".to_owned()],
            },
        ))
        .unwrap();
    env.secrets
        .set(
            "gmail-pessoal",
            IntegrationSecrets {
                client_secret: Some(Zeroizing::new(secret_value.to_owned())),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let oauth = env.oauth_manager();
    let started = oauth
        .start(
            "gmail-pessoal",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .expect("manual client deve montar authorize_url");

    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = started else {
        panic!("esperava OpenUrl no desktop");
    };
    assert!(
        !authorize_url.contains(secret_value),
        "client_secret NUNCA pode aparecer na authorize_url"
    );
    let params = query_params(&authorize_url);
    assert_eq!(
        params.get("client_id").map(String::as_str),
        Some("google-cid-99")
    );
    assert_eq!(
        params.get("scope").map(String::as_str),
        Some("https://www.googleapis.com/auth/gmail.readonly")
    );

    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    let res = oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .expect("callback deve validar client_secret no /token");
    assert_eq!(res.status, IntegrationStatus::Ready);

    // client_secret original da integração deve continuar preservado após salvar tokens OAuth.
    let saved = env.secrets.get("gmail-pessoal").await.unwrap().unwrap();
    assert_eq!(
        saved.client_secret.as_deref().map(String::as_str),
        Some(secret_value)
    );
    assert_eq!(
        saved.oauth.as_ref().unwrap().access_token.as_str(),
        "access-token-v1"
    );

    server_handle.abort();
}

#[tokio::test]
async fn desktop_vs_mobile_redirect_and_handoff_rules() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        dcr_supported: true,
        ..Default::default()
    })
    .await;
    server_state
        .add_static_client(
            "google-cid",
            Some("sec"),
            vec!["http://127.0.0.1:9001/api/integrations/oauth/callback".to_owned()],
        )
        .await;

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "gmail-mob",
            format!("http://{addr}/mcp"),
            Some("google-gmail"),
            AuthConfig::OAuthManualClient {
                client_id: "google-cid".to_owned(),
                scopes: vec![],
            },
        ))
        .unwrap();
    env.registry
        .create(remote_config(
            "linear-mob",
            format!("http://{addr}/mcp"),
            Some("linear"),
            AuthConfig::OAuthDcr,
        ))
        .unwrap();

    let oauth = env.oauth_manager();

    // 1. Mobile + Google (requires_loopback_redirect: true) -> HandoffDesktop mesmo com HTTPS.
    let google_mob = oauth
        .start(
            "gmail-mob",
            OAuthStartInput {
                origin_client: OAuthOrigin::Mobile,
                local_port: 9001,
                public_base_url: Some("https://meu.tail.ts.net".to_owned()),
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap();
    match google_mob {
        OAuthStartOutcome::HandoffDesktop {
            authorize_url,
            redirect_uri,
            reason,
        } => {
            assert_eq!(
                redirect_uri,
                "http://127.0.0.1:9001/api/integrations/oauth/callback"
            );
            assert!(!reason.trim().is_empty());
            assert_eq!(
                query_params(&authorize_url)
                    .get("redirect_uri")
                    .map(String::as_str),
                Some("http://127.0.0.1:9001/api/integrations/oauth/callback")
            );
        }
        other => panic!("esperava HandoffDesktop para Google no mobile, veio {other:?}"),
    }

    // 2. Mobile + Linear sem HTTPS -> HandoffDesktop com callback loopback.
    let linear_no_https = oauth
        .start(
            "linear-mob",
            OAuthStartInput {
                origin_client: OAuthOrigin::Mobile,
                local_port: 9001,
                public_base_url: Some("http://192.168.1.10:8765".to_owned()),
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(
            linear_no_https,
            OAuthStartOutcome::HandoffDesktop { ref redirect_uri, .. }
            if redirect_uri == "http://127.0.0.1:9001/api/integrations/oauth/callback"
        ),
        "sem https deve fazer handoff desktop: {linear_no_https:?}"
    );

    // 3. Mobile + Linear com HTTPS público -> OpenUrl com callback HTTPS (removendo barra final).
    let linear_https = oauth
        .start(
            "linear-mob",
            OAuthStartInput {
                origin_client: OAuthOrigin::Mobile,
                local_port: 9001,
                public_base_url: Some("https://meu.tail.ts.net/".to_owned()),
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap();
    match linear_https {
        OAuthStartOutcome::OpenUrl {
            authorize_url,
            redirect_uri,
        } => {
            assert_eq!(
                redirect_uri,
                "https://meu.tail.ts.net/api/integrations/oauth/callback"
            );
            assert_eq!(
                query_params(&authorize_url)
                    .get("redirect_uri")
                    .map(String::as_str),
                Some("https://meu.tail.ts.net/api/integrations/oauth/callback")
            );
        }
        other => panic!("esperava OpenUrl com HTTPS para Linear no mobile, veio {other:?}"),
    }

    // 4. Desktop conectado remotamente segue a mesma política remota (HTTPS direto ou handoff para o servidor).
    let remote_desktop_handoff = oauth
        .start(
            "gmail-mob",
            OAuthStartInput {
                origin_client: OAuthOrigin::RemoteDesktop,
                local_port: 9001,
                public_base_url: Some("https://meu.tail.ts.net".to_owned()),
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(remote_desktop_handoff, OAuthStartOutcome::HandoffDesktop { .. }));

    let remote_desktop_https = oauth
        .start(
            "linear-mob",
            OAuthStartInput {
                origin_client: OAuthOrigin::RemoteDesktop,
                local_port: 9001,
                public_base_url: Some("https://meu.tail.ts.net/".to_owned()),
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        remote_desktop_https,
        OAuthStartOutcome::OpenUrl { ref redirect_uri, .. }
        if redirect_uri == "https://meu.tail.ts.net/api/integrations/oauth/callback"
    ));

    server_handle.abort();
}

#[tokio::test]
async fn state_security_single_use_entropy_expiry_and_provider_error() {
    let (addr, _, server_handle) = spawn_mock_oauth_server(MockOAuthConfig::default()).await;
    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "sec-test",
            format!("http://{addr}/mcp"),
            None,
            AuthConfig::OAuthDcr,
        ))
        .unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "sec-test",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };

    let state_param = query_params(&authorize_url).remove("state").unwrap();
    // 32 bytes em base64url sem padding têm sempre 43 caracteres.
    assert_eq!(state_param.len(), 43, "state deve ter 32 bytes base64url");
    assert_eq!(
        URL_SAFE_NO_PAD.decode(&state_param).unwrap().len(),
        32,
        "entropia mínima de 32 bytes no state"
    );

    // State desconhecido falha.
    assert_eq!(
        oauth
            .handle_callback("desconhecido", Some("c"), None, None)
            .await,
        Err(OAuthError::InvalidState)
    );

    // Erro do provedor com state válido retorna ProviderError e consome o state (uso único).
    let prov_err = oauth
        .handle_callback(
            &state_param,
            None,
            Some("access_denied"),
            Some("usuario recusou"),
        )
        .await;
    assert_eq!(
        prov_err,
        Err(OAuthError::ProviderError {
            error: "access_denied".to_owned(),
            description: Some("acesso negado pelo usuário ou provedor".to_owned()),
        })
    );
    // Replay do mesmo state falha com InvalidState.
    let (code, _) = simulate_browser_authorize(&authorize_url).await;
    assert_eq!(
        oauth
            .handle_callback(&state_param, Some(&code), None, None)
            .await,
        Err(OAuthError::InvalidState)
    );

    // Replay após sucesso também falha com InvalidState.
    let OAuthStartOutcome::OpenUrl {
        authorize_url: auth2,
        ..
    } = oauth
        .start(
            "sec-test",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code2, state2) = simulate_browser_authorize(&auth2).await;
    oauth
        .handle_callback(&state2, Some(&code2), None, None)
        .await
        .unwrap();
    assert_eq!(
        oauth
            .handle_callback(&state2, Some(&code2), None, None)
            .await,
        Err(OAuthError::InvalidState)
    );

    // State expirado (TTL = ZERO no gerenciador de teste, sem sleep frágil).
    let expired_mgr =
        OAuthManager::new(env.registry.clone(), env.secrets.clone()).with_state_ttl(Duration::ZERO);
    let OAuthStartOutcome::OpenUrl {
        authorize_url: auth3,
        ..
    } = expired_mgr
        .start(
            "sec-test",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let state3 = query_params(&auth3).remove("state").unwrap();
    assert_eq!(
        expired_mgr
            .handle_callback(&state3, Some("qualquer"), None, None)
            .await,
        Err(OAuthError::ExpiredState)
    );

    server_handle.abort();
}

#[tokio::test]
async fn concurrent_refresh_mcp_401_retry_and_invalid_grant_reconnect_flow() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        dcr_supported: true,
        rotate_refresh_tokens: true,
        ..Default::default()
    })
    .await;

    let env = TestEnv::new();
    let cfg = remote_config(
        "figma-rot",
        format!("http://{addr}/mcp"),
        Some("figma"),
        AuthConfig::OAuthDcr,
    );
    env.registry.create(cfg.clone()).unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "figma-rot",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .unwrap();
    // Temos access-token-v1 e refresh-token-v1.

    // Parte 1: duas chamadas simultâneas a refresh_access_token com refresh_token rotativo.
    let first_started = Arc::new(Notify::new());
    let allow_first = Arc::new(Notify::new());
    *server_state.refresh_gate.lock().await = Some((first_started.clone(), allow_first.clone()));

    let oauth_a = oauth.clone();
    let task_a = tokio::spawn(async move { oauth_a.refresh_access_token("figma-rot").await });
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("primeira tarefa deve entrar no /token");

    let oauth_b = oauth.clone();
    let task_b = tokio::spawn(async move { oauth_b.refresh_access_token("figma-rot").await });
    // Confirma em janela limitada que task_b fica bloqueada no lock sem entrar em /token antes de A liberar.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        server_state.refresh_calls.load(Ordering::SeqCst),
        1,
        "sem serialização, task_b teria entrado em /token enquanto task_a segurava o gate"
    );
    allow_first.notify_one();

    let (res_a, res_b) = tokio::join!(task_a, task_b);
    let tok_a = res_a.unwrap().expect("refresh A deve suceder");
    let tok_b = res_b
        .unwrap()
        .expect("refresh B deve suceder sem invalid_grant");
    assert_eq!(tok_a, "access-token-v2");
    assert_eq!(tok_b, "access-token-v3");
    assert_eq!(
        server_state.max_concurrent_refresh.load(Ordering::SeqCst),
        1,
        "refresh concorrente na mesma integração deve ser estritamente serializado"
    );
    assert_eq!(
        *server_state.refresh_tokens_seen.lock().await,
        vec!["refresh-token-v1".to_owned(), "refresh-token-v2".to_owned()]
    );
    let after_rot = env.secrets.get("figma-rot").await.unwrap().unwrap();
    let after_oauth = after_rot.oauth.as_ref().unwrap();
    assert_eq!(after_oauth.access_token.as_str(), "access-token-v3");
    assert_eq!(
        after_oauth.refresh_token.as_deref().map(String::as_str),
        Some("refresh-token-v3")
    );

    // Parte 2: McpClientManager recebe 401 com token obsoleto, OAuthManager renova e o retry passa!
    *server_state.active_access_token.write().await = "token-expirado-no-servidor".to_owned();
    let client_mgr = env.client_manager(oauth.clone());
    let out = client_mgr
        .call_tool("figma-rot", "echo_oauth", json!({"msg": "pos-refresh"}))
        .await
        .expect("McpClientManager deve renovar via OAuthManager após 401 e concluir o retry");
    assert_eq!(out.text, "ok:pos-refresh");
    client_mgr.shutdown().await;

    // Parte 3: invalid_grant no refresh marca auth_expired = true, preserva config e permite Reconectar.
    server_state
        .force_invalid_grant
        .store(true, Ordering::SeqCst);
    let err = oauth.refresh_access_token("figma-rot").await;
    assert!(
        matches!(err, Err(OAuthError::InvalidGrant(_))),
        "esperava InvalidGrant, veio {err:?}"
    );
    let expired_secrets = env.secrets.get("figma-rot").await.unwrap().unwrap();
    assert!(expired_secrets.oauth.as_ref().unwrap().auth_expired);
    assert_eq!(
        compute_status(&cfg, Some(&expired_secrets)),
        IntegrationStatus::AuthExpired
    );
    assert!(env.registry.get("figma-rot").unwrap().is_some());

    // Reconexão restaura para Ready.
    server_state
        .force_invalid_grant
        .store(false, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl {
        authorize_url: recon_url,
        ..
    } = oauth
        .start(
            "figma-rot",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl na reconexão");
    };
    let (code_rec, state_rec) = simulate_browser_authorize(&recon_url).await;
    let rec_res = oauth
        .handle_callback(&state_rec, Some(&code_rec), None, None)
        .await
        .unwrap();
    assert_eq!(rec_res.status, IntegrationStatus::Ready);
    let restored = env.secrets.get("figma-rot").await.unwrap().unwrap();
    assert!(!restored.oauth.as_ref().unwrap().auth_expired);
    assert_eq!(
        compute_status(&cfg, Some(&restored)),
        IntegrationStatus::Ready
    );

    server_handle.abort();
}

#[tokio::test]
async fn discovery_without_rfc9728_falls_back_to_origin_and_guides_when_dcr_absent() {
    #[derive(Clone)]
    struct OriginOnlyState {
        base_url: String,
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = OriginOnlyState {
        base_url: format!("http://{addr}"),
    };

    // Servidor sem /.well-known/oauth-protected-resource e sem registration_endpoint.
    let app = Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<OriginOnlyState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": st.base_url,
                        "authorization_endpoint": format!("{}/auth", st.base_url),
                        "token_endpoint": format!("{}/tok", st.base_url),
                        "code_challenge_methods_supported": ["S256"]
                    }),
                )
            }),
        )
        .route("/mcp/v1", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "no-dcr",
            format!("http://{addr}/mcp/v1"),
            None,
            AuthConfig::OAuthDcr,
        ))
        .unwrap();

    let oauth = env.oauth_manager();
    let err = oauth
        .start(
            "no-dcr",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .expect_err("sem DCR e sem CIMD deve orientar oauth_manual_client");
    match err {
        OAuthError::RegistrationFailed(msg) => {
            assert!(
                msg.contains("oauth_manual_client"),
                "mensagem deve orientar oauth_manual_client: {msg}"
            );
        }
        other => panic!("esperava RegistrationFailed, veio {other:?}"),
    }

    server_handle.abort();
}

#[tokio::test]
async fn refresh_preserves_non_rotating_token_marks_missing_rt_expired_and_redacts_secrets() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        dcr_supported: true,
        rotate_refresh_tokens: false,
        cimd_supported: true,
        ..Default::default()
    })
    .await;

    let env = TestEnv::new();
    let cfg = remote_config(
        "non-rot",
        format!("http://{addr}/mcp"),
        Some("linear"),
        AuthConfig::OAuthDcr,
    );
    env.registry.create(cfg.clone()).unwrap();

    let oauth = env.oauth_manager();
    let cimd_url = "https://hangar.dev.br/oauth/client.json";
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "non-rot",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: Some(cimd_url.to_owned()),
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .unwrap();

    // Refresh não rotativo após CIMD preserva o refresh_token inicial e atualiza access_token.
    let tok2 = oauth.refresh_access_token("non-rot").await.unwrap();
    assert_eq!(tok2, "access-token-v2");
    let tok3 = oauth.refresh_access_token("non-rot").await.unwrap();
    assert_eq!(tok3, "access-token-v3");
    let stored = env.secrets.get("non-rot").await.unwrap().unwrap();
    assert_eq!(
        stored
            .oauth
            .as_ref()
            .unwrap()
            .refresh_token
            .as_deref()
            .map(String::as_str),
        Some("refresh-token-v1")
    );

    // Reconectar sem cimd_client_id_url não reutiliza URL CIMD como DCR; faz DCR real.
    let OAuthStartOutcome::OpenUrl {
        authorize_url: dcr_url,
        ..
    } = oauth
        .start(
            "non-rot",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert_eq!(server_state.register_calls.load(Ordering::SeqCst), 1);

    // Login que não retorna refresh_token -> tentativa de refresh marca auth_expired = true.
    server_state
        .omit_refresh_token_on_issue
        .store(true, Ordering::SeqCst);
    let (code2, state2) = simulate_browser_authorize(&dcr_url).await;
    oauth
        .handle_callback(&state2, Some(&code2), None, None)
        .await
        .unwrap();
    let err_no_rt = oauth.refresh_access_token("non-rot").await.unwrap_err();
    assert!(matches!(err_no_rt, OAuthError::InvalidGrant(_)));
    let expired = env.secrets.get("non-rot").await.unwrap().unwrap();
    assert!(expired.oauth.as_ref().unwrap().auth_expired);
    assert_eq!(
        compute_status(&cfg, Some(&expired)),
        IntegrationStatus::AuthExpired
    );

    // Erro com credenciais manuais inválidas não vaza client_secret nem código na mensagem.
    let secret_sentinel = "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encial-999";
    let code_sentinel = "codigo-secreto-invalido-888";
    env.registry
        .create(remote_config(
            "leak-check",
            format!("http://{addr}/mcp"),
            Some("google-gmail"),
            AuthConfig::OAuthManualClient {
                client_id: "cid-nao-existe".to_owned(),
                scopes: vec![],
            },
        ))
        .unwrap();
    env.secrets
        .set(
            "leak-check",
            IntegrationSecrets {
                client_secret: Some(Zeroizing::new(secret_sentinel.to_owned())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let OAuthStartOutcome::OpenUrl {
        authorize_url: leak_auth_url,
        ..
    } = oauth
        .start(
            "leak-check",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let leak_state = query_params(&leak_auth_url).remove("state").unwrap();
    let err = oauth
        .handle_callback(&leak_state, Some(code_sentinel), None, None)
        .await
        .unwrap_err();
    let rendered = format!("{err} | {err:?}");
    assert!(!rendered.contains(secret_sentinel));
    assert!(!rendered.contains(code_sentinel));

    server_handle.abort();
}

#[tokio::test]
async fn refresh_in_flight_does_not_overwrite_or_expire_concurrent_manual_reconnect() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        dcr_supported: true,
        rotate_refresh_tokens: true,
        ..Default::default()
    })
    .await;

    let env = TestEnv::new();
    let cfg = remote_config(
        "race-id",
        format!("http://{addr}/mcp"),
        Some("notion"),
        AuthConfig::OAuthDcr,
    );
    env.registry.create(cfg.clone()).unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "race-id",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code1, state1) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state1, Some(&code1), None, None)
        .await
        .unwrap();
    // Agora temos access-token-v1 / refresh-token-v1.

    // Subcaso A: refresh em voo retorna invalid_grant depois que handle_callback já reconectou ->
    // não pode marcar auth_expired = true nem apagar os tokens novos!
    server_state
        .force_invalid_grant
        .store(true, Ordering::SeqCst);
    let gate_a_started = Arc::new(Notify::new());
    let gate_a_allow = Arc::new(Notify::new());
    *server_state.refresh_gate.lock().await = Some((gate_a_started.clone(), gate_a_allow.clone()));

    let oauth_a = oauth.clone();
    let refresh_task_a = tokio::spawn(async move { oauth_a.refresh_access_token("race-id").await });
    tokio::time::timeout(Duration::from_secs(2), gate_a_started.notified())
        .await
        .expect("refresh A deve entrar no /token e bloquear no gate");

    let OAuthStartOutcome::OpenUrl {
        authorize_url: recon_url_a,
        ..
    } = oauth
        .start(
            "race-id",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code2, state2) = simulate_browser_authorize(&recon_url_a).await;
    oauth
        .handle_callback(&state2, Some(&code2), None, None)
        .await
        .unwrap();
    // Agora secrets.bin tem access-token-v2 / refresh-token-v2.

    gate_a_allow.notify_one();
    let _ = refresh_task_a.await.unwrap();

    let after_a = env.secrets.get("race-id").await.unwrap().unwrap();
    let oauth_after_a = after_a.oauth.as_ref().unwrap();
    assert_eq!(oauth_after_a.access_token.as_str(), "access-token-v2");
    assert_eq!(
        oauth_after_a.refresh_token.as_deref().map(String::as_str),
        Some("refresh-token-v2")
    );
    assert!(
        !oauth_after_a.auth_expired,
        "refresh atrasado com invalid_grant não pode marcar reconexão manual mais nova como expirada"
    );
    assert_eq!(
        compute_status(&cfg, Some(&after_a)),
        IntegrationStatus::Ready
    );

    // Subcaso B: refresh em voo gera access-token-v3 no servidor, mas antes de concluir no cliente
    // uma reconexão manual salva access-token-v4 -> refresh antigo preserva v4 e retorna v4!
    server_state
        .force_invalid_grant
        .store(false, Ordering::SeqCst);
    let gate_b_started = Arc::new(Notify::new());
    let gate_b_allow = Arc::new(Notify::new());
    *server_state.refresh_gate.lock().await = Some((gate_b_started.clone(), gate_b_allow.clone()));

    let oauth_b = oauth.clone();
    let refresh_task_b = tokio::spawn(async move { oauth_b.refresh_access_token("race-id").await });
    tokio::time::timeout(Duration::from_secs(2), gate_b_started.notified())
        .await
        .expect("refresh B deve emitir v3 e bloquear no gate");

    let OAuthStartOutcome::OpenUrl {
        authorize_url: recon_url_b,
        ..
    } = oauth
        .start(
            "race-id",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code4, state4) = simulate_browser_authorize(&recon_url_b).await;
    oauth
        .handle_callback(&state4, Some(&code4), None, None)
        .await
        .unwrap();
    // Agora secrets.bin tem access-token-v4 / refresh-token-v4.

    gate_b_allow.notify_one();
    let returned_b = refresh_task_b
        .await
        .unwrap()
        .expect("refresh B deve retornar o access_token mais novo em secrets.bin");
    assert_eq!(
        returned_b, "access-token-v4",
        "refresh B não pode retornar nem gravar v3 por cima de v4"
    );

    let after_b = env.secrets.get("race-id").await.unwrap().unwrap();
    let oauth_after_b = after_b.oauth.as_ref().unwrap();
    assert_eq!(oauth_after_b.access_token.as_str(), "access-token-v4");
    assert_eq!(
        oauth_after_b.refresh_token.as_deref().map(String::as_str),
        Some("refresh-token-v4")
    );
    assert!(!oauth_after_b.auth_expired);

    server_handle.abort();
}

#[tokio::test]
async fn discovery_prefers_all_path_aware_candidates_before_root_fallback() {
    #[derive(Clone)]
    struct TenantOrderState {
        base_url: String,
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = TenantOrderState {
        base_url: format!("http://{addr}"),
    };

    // O issuer possui path `/tenant-b`; a raiz responde 200 (armadilha), o oauth-authorization-server/tenant-b
    // dá 404 e o openid-configuration/tenant-b responde 200 — o path-aware OIDC deve vencer a raiz.
    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(|State(st): State<TenantOrderState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "resource": format!("{}/mcp", st.base_url),
                        "authorization_servers": [format!("{}/tenant-b", st.base_url)]
                    }),
                )
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<TenantOrderState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": st.base_url,
                        "authorization_endpoint": format!("{}/authorize-root-errado", st.base_url),
                        "token_endpoint": format!("{}/token-root", st.base_url),
                        "registration_endpoint": format!("{}/register", st.base_url)
                    }),
                )
            }),
        )
        .route(
            "/.well-known/openid-configuration/tenant-b",
            get(|State(st): State<TenantOrderState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": format!("{}/tenant-b", st.base_url),
                        "authorization_endpoint": format!("{}/authorize-tenant-b", st.base_url),
                        "token_endpoint": format!("{}/token-tenant-b", st.base_url)
                    }),
                )
            }),
        )
        .route("/mcp", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "tenant-order",
            format!("http://{addr}/mcp"),
            Some("google-gmail"),
            AuthConfig::OAuthManualClient {
                client_id: "cid".to_owned(),
                scopes: vec![],
            },
        ))
        .unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "tenant-order",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert!(
        authorize_url.starts_with(&format!("http://{addr}/authorize-tenant-b")),
        "candidato path-aware deve ter prioridade sobre raiz: {authorize_url}"
    );

    server_handle.abort();
}

#[tokio::test]
async fn refresh_invalid_client_marks_auth_expired_and_dcr_reconnect_registers_fresh_client() {
    let (addr, server_state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig {
        dcr_supported: true,
        rotate_refresh_tokens: true,
        ..Default::default()
    })
    .await;

    let env = TestEnv::new();
    let cfg = remote_config(
        "dcr-invalid-client",
        format!("http://{addr}/mcp"),
        Some("linear"),
        AuthConfig::OAuthDcr,
    );
    env.registry.create(cfg.clone()).unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "dcr-invalid-client",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .unwrap();
    assert_eq!(server_state.register_calls.load(Ordering::SeqCst), 1);

    // Servidor remove dcr-client-1 para simular cliente DCR expirado/revogado -> retorna 401 invalid_client.
    server_state.clients.lock().await.remove("dcr-client-1");
    let err = oauth
        .refresh_access_token("dcr-invalid-client")
        .await
        .expect_err("invalid_client no refresh deve retornar InvalidGrant");
    assert!(
        matches!(err, OAuthError::InvalidGrant(ref msg) if msg.contains("invalid_client")),
        "esperava InvalidGrant(invalid_client), veio {err:?}"
    );

    let expired = env
        .secrets
        .get("dcr-invalid-client")
        .await
        .unwrap()
        .unwrap();
    assert!(expired.oauth.as_ref().unwrap().auth_expired);
    assert_eq!(
        compute_status(&cfg, Some(&expired)),
        IntegrationStatus::AuthExpired
    );

    // Ao clicar em Reconectar (start) após auth_expired=true, registra novo client_id via DCR.
    let OAuthStartOutcome::OpenUrl {
        authorize_url: recon_url,
        ..
    } = oauth
        .start(
            "dcr-invalid-client",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert_eq!(
        server_state.register_calls.load(Ordering::SeqCst),
        2,
        "após auth_expired=true, OAuthDcr deve registrar novo cliente DCR"
    );
    let (code2, state2) = simulate_browser_authorize(&recon_url).await;
    let rec_res = oauth
        .handle_callback(&state2, Some(&code2), None, None)
        .await
        .unwrap();
    assert_eq!(rec_res.status, IntegrationStatus::Ready);

    let restored = env
        .secrets
        .get("dcr-invalid-client")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restored
            .oauth
            .as_ref()
            .unwrap()
            .registered_client_id
            .as_deref(),
        Some("dcr-client-2")
    );
    assert!(!restored.oauth.as_ref().unwrap().auth_expired);

    server_handle.abort();
}

#[tokio::test]
async fn pending_authorize_url_helpers_track_latest_and_clear_on_callback_or_expiry() {
    let (addr, _, server_handle) = spawn_mock_oauth_server(MockOAuthConfig::default()).await;
    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "pending-helpers",
            format!("http://{addr}/mcp"),
            Some("notion"),
            AuthConfig::OAuthDcr,
        ))
        .unwrap();

    let oauth = env.oauth_manager();
    assert!(!oauth.is_pending_authorize_url("pending-helpers", "https://example.com"));
    assert_eq!(oauth.latest_pending_authorize_url("pending-helpers"), None);

    let OAuthStartOutcome::OpenUrl {
        authorize_url: url1,
        ..
    } = oauth
        .start(
            "pending-helpers",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert!(oauth.is_pending_authorize_url("pending-helpers", &url1));
    assert!(!oauth.is_pending_authorize_url("outra-integracao", &url1));
    assert_eq!(
        oauth.latest_pending_authorize_url("pending-helpers"),
        Some(url1.clone())
    );

    let OAuthStartOutcome::OpenUrl {
        authorize_url: url2,
        ..
    } = oauth
        .start(
            "pending-helpers",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert!(oauth.is_pending_authorize_url("pending-helpers", &url1));
    assert!(oauth.is_pending_authorize_url("pending-helpers", &url2));
    assert_eq!(
        oauth.latest_pending_authorize_url("pending-helpers"),
        Some(url2.clone())
    );

    let (code2, state2) = simulate_browser_authorize(&url2).await;
    oauth
        .handle_callback(&state2, Some(&code2), None, None)
        .await
        .unwrap();
    assert!(!oauth.is_pending_authorize_url("pending-helpers", &url2));
    assert_eq!(
        oauth.latest_pending_authorize_url("pending-helpers"),
        Some(url1.clone())
    );

    let (code1, state1) = simulate_browser_authorize(&url1).await;
    oauth
        .handle_callback(&state1, Some(&code1), None, None)
        .await
        .unwrap();
    assert!(!oauth.is_pending_authorize_url("pending-helpers", &url1));
    assert_eq!(oauth.latest_pending_authorize_url("pending-helpers"), None);

    // Tentativa já expirada não aparece nos helpers.
    let expired_mgr =
        OAuthManager::new(env.registry.clone(), env.secrets.clone()).with_state_ttl(Duration::ZERO);
    let OAuthStartOutcome::OpenUrl {
        authorize_url: url_exp,
        ..
    } = expired_mgr
        .start(
            "pending-helpers",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert!(!expired_mgr.is_pending_authorize_url("pending-helpers", &url_exp));
    assert_eq!(
        expired_mgr.latest_pending_authorize_url("pending-helpers"),
        None
    );

    server_handle.abort();
}

#[tokio::test]
async fn oauth_sends_canonical_rfc8707_resource_across_authorize_exchange_and_refresh() {
    let (addr, state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig::default()).await;
    let expected_resource = format!("http://{addr}/mcp?project_ref=abc");

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "res-bound",
            format!("http://{addr}/mcp/?project_ref=abc"),
            None,
            AuthConfig::OAuthDcr,
        ))
        .unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "res-bound",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert_eq!(
        query_params(&authorize_url).get("resource"),
        Some(&expected_resource)
    );

    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    assert_eq!(
        *state.authorize_resources.lock().await,
        vec![expected_resource.clone()]
    );

    oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .unwrap();
    oauth.refresh_access_token("res-bound").await.unwrap();

    assert_eq!(
        *state.token_resources.lock().await,
        vec![
            ("authorization_code".to_owned(), expected_resource.clone()),
            ("refresh_token".to_owned(), expected_resource),
        ]
    );

    server_handle.abort();
}

#[tokio::test]
async fn oauth_callback_aborts_without_persisting_if_target_config_changes_in_flight() {
    #[derive(Clone)]
    struct GateState {
        base_url: String,
        token_gate: Arc<Mutex<Option<(Arc<Notify>, Arc<Notify>)>>>,
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let token_started = Arc::new(Notify::new());
    let token_allow = Arc::new(Notify::new());
    let gate_state = GateState {
        base_url: base_url.clone(),
        token_gate: Arc::new(Mutex::new(Some((
            token_started.clone(),
            token_allow.clone(),
        )))),
    };

    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(|State(st): State<GateState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "resource": format!("{}/mcp", st.base_url),
                        "authorization_servers": [st.base_url]
                    }),
                )
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<GateState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": st.base_url,
                        "authorization_endpoint": format!("{}/authorize", st.base_url),
                        "token_endpoint": format!("{}/token", st.base_url)
                    }),
                )
            }),
        )
        .route(
            "/authorize",
            get(|Query(params): Query<HashMap<String, String>>| async move {
                let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
                let state_param = params.get("state").cloned().unwrap_or_default();
                let location = format!("{redirect_uri}?code=gate-code-1&state={state_param}");
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/token",
            post(|State(st): State<GateState>| async move {
                if let Some((started, allow)) = st.token_gate.lock().await.take() {
                    started.notify_one();
                    let _ = tokio::time::timeout(Duration::from_secs(5), allow.notified()).await;
                }
                json_response(
                    StatusCode::OK,
                    json!({
                        "access_token": "tok-tardio",
                        "token_type": "Bearer",
                        "refresh_token": "rt-tardio",
                        "expires_in": 3600
                    }),
                )
            }),
        )
        .route("/mcp", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(gate_state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "target-change",
            format!("{base_url}/mcp"),
            None,
            AuthConfig::OAuthManualClient {
                client_id: "cid-1".to_owned(),
                scopes: vec![],
            },
        ))
        .unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "target-change",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;

    let oauth_bg = oauth.clone();
    let cb_task = tokio::spawn(async move {
        oauth_bg
            .handle_callback(&state_param, Some(&code), None, None)
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), token_started.notified())
        .await
        .expect("POST /token deve iniciar em até 5s");

    // Altera o destino da integração durante a troca de código antes de gravar tokens.
    env.registry
        .update(
            "target-change",
            IntegrationUpdate {
                transport: Some(Transport::RemoteHttp {
                    url: format!("{base_url}/mcp-v2"),
                    header_names: vec![],
                }),
                ..Default::default()
            },
        )
        .unwrap();

    token_allow.notify_one();
    let err = cb_task
        .await
        .unwrap()
        .expect_err("callback deve abortar quando a integração muda em voo");
    assert!(matches!(err, OAuthError::Validation(_)), "erro: {err:?}");
    assert!(
        env.secrets.get("target-change").await.unwrap().is_none(),
        "nenhum segredo pode ser gravado após alteração da URL de destino"
    );

    server_handle.abort();
}

#[tokio::test]
async fn discovery_rejects_missing_or_mismatched_prm_resource_and_as_issuer() {
    #[derive(Clone)]
    struct DiscState {
        base_url: String,
        mode: Arc<AtomicUsize>,
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let mode = Arc::new(AtomicUsize::new(0));
    let state = DiscState {
        base_url: base_url.clone(),
        mode: mode.clone(),
    };

    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(|State(st): State<DiscState>| async move {
                match st.mode.load(Ordering::SeqCst) {
                    0 => json_response(
                        StatusCode::OK,
                        json!({"authorization_servers": [st.base_url]}),
                    ),
                    1 => json_response(
                        StatusCode::OK,
                        json!({
                            "resource": format!("{}/outro-recurso", st.base_url),
                            "authorization_servers": [st.base_url]
                        }),
                    ),
                    _ => json_response(
                        StatusCode::OK,
                        json!({
                            "resource": format!("{}/mcp", st.base_url),
                            "authorization_servers": [st.base_url]
                        }),
                    ),
                }
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<DiscState>| async move {
                match st.mode.load(Ordering::SeqCst) {
                    2 => json_response(
                        StatusCode::OK,
                        json!({
                            "authorization_endpoint": format!("{}/authorize", st.base_url),
                            "token_endpoint": format!("{}/token", st.base_url)
                        }),
                    ),
                    _ => json_response(
                        StatusCode::OK,
                        json!({
                            "issuer": format!("{}/emissor-errado", st.base_url),
                            "authorization_endpoint": format!("{}/authorize", st.base_url),
                            "token_endpoint": format!("{}/token", st.base_url)
                        }),
                    ),
                }
            }),
        )
        .route("/mcp", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "disc-validate",
            format!("{base_url}/mcp"),
            None,
            AuthConfig::OAuthManualClient {
                client_id: "cid".to_owned(),
                scopes: vec![],
            },
        ))
        .unwrap();
    let oauth = env.oauth_manager();
    let input = OAuthStartInput {
        origin_client: OAuthOrigin::Desktop,
        local_port: 8765,
        public_base_url: None,
        cimd_client_id_url: None,
    };

    for m in 0..4 {
        mode.store(m, Ordering::SeqCst);
        let err = oauth
            .start("disc-validate", input.clone())
            .await
            .expect_err(&format!("modo {m} deveria falhar validação RFC 9728/8414"));
        assert!(
            matches!(err, OAuthError::DiscoveryFailed(_)),
            "modo {m} retornou erro inesperado: {err:?}"
        );
    }

    server_handle.abort();
}

#[tokio::test]
async fn oauth_streams_and_rejects_oversized_chunked_response_without_content_length() {
    #[derive(Clone)]
    struct BigState {
        base_url: String,
        oversize_prm: Arc<AtomicBool>,
        eof_gate: Arc<Notify>,
    }

    fn chunked_over_limit_open(eof_gate: Arc<Notify>) -> Response {
        // Envia >256 KiB de JSON válido em andamento e mantém o stream aberto até `eof_gate`.
        let stream = futures_util::stream::unfold((0usize, eof_gate), |(step, gate)| async move {
            match step {
                0 => Some((
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(b"{\"pad\":\"")),
                    (1, gate),
                )),
                1..=35 => Some((
                    Ok::<_, std::convert::Infallible>(Bytes::from(vec![b'a'; 8 * 1024])),
                    (step + 1, gate),
                )),
                36 => {
                    gate.notified().await;
                    Some((
                        Ok::<_, std::convert::Infallible>(Bytes::from_static(b"\"}")),
                        (37, gate),
                    ))
                }
                _ => None,
            }
        });
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let oversize_prm = Arc::new(AtomicBool::new(true));
    let eof_gate = Arc::new(Notify::new());
    let state = BigState {
        base_url: base_url.clone(),
        oversize_prm: oversize_prm.clone(),
        eof_gate: eof_gate.clone(),
    };

    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(|State(st): State<BigState>| async move {
                if st.oversize_prm.load(Ordering::SeqCst) {
                    chunked_over_limit_open(st.eof_gate.clone())
                } else {
                    // Controle: JSON válido abaixo de 256 KiB (com preenchimento de ~128 KiB).
                    json_response(
                        StatusCode::OK,
                        json!({
                            "resource": format!("{}/mcp", st.base_url),
                            "authorization_servers": [st.base_url],
                            "pad": "b".repeat(128 * 1024)
                        }),
                    )
                }
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<BigState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": st.base_url,
                        "authorization_endpoint": format!("{}/authorize", st.base_url),
                        "token_endpoint": format!("{}/token", st.base_url)
                    }),
                )
            }),
        )
        .route(
            "/authorize",
            get(|Query(params): Query<HashMap<String, String>>| async move {
                let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
                let state_param = params.get("state").cloned().unwrap_or_default();
                let location = format!("{redirect_uri}?code=big-code-1&state={state_param}");
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/token",
            post(|State(st): State<BigState>| async move {
                chunked_over_limit_open(st.eof_gate.clone())
            }),
        )
        .route("/mcp", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "oversize-test",
            format!("{base_url}/mcp"),
            None,
            AuthConfig::OAuthManualClient {
                client_id: "cid".to_owned(),
                scopes: vec![],
            },
        ))
        .unwrap();
    let oauth = env.oauth_manager();
    let start_input = OAuthStartInput {
        origin_client: OAuthOrigin::Desktop,
        local_port: 8765,
        public_base_url: None,
        cimd_client_id_url: None,
    };

    // O erro deve chegar ANTES de `eof_gate` liberar o fim do stream, provando corte incremental.
    let prm_err = tokio::time::timeout(
        Duration::from_secs(3),
        oauth.start("oversize-test", start_input.clone()),
    )
    .await
    .expect("corte incremental do PRM não pode aguardar EOF")
    .expect_err("PRM em streaming >256 KiB deve abortar");
    assert!(
        matches!(prm_err, OAuthError::DiscoveryFailed(ref m) if m.contains("limite")),
        "esperava erro de limite no PRM, veio {prm_err:?}"
    );

    oversize_prm.store(false, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } =
        oauth.start("oversize-test", start_input).await.unwrap()
    else {
        panic!("esperava OpenUrl com controle JSON <256 KiB");
    };
    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    let token_err = tokio::time::timeout(
        Duration::from_secs(3),
        oauth.handle_callback(&state_param, Some(&code), None, None),
    )
    .await
    .expect("corte incremental do /token não pode aguardar EOF")
    .expect_err("resposta /token em streaming >256 KiB deve abortar");
    assert!(
        matches!(token_err, OAuthError::Network(ref m) if m.contains("limite")),
        "esperava erro de limite no /token, veio {token_err:?}"
    );

    eof_gate.notify_waiters();
    server_handle.abort();
}

#[tokio::test]
async fn oauth_sanitizes_provider_error_descriptions_and_unknown_codes_across_flows() {
    const LEAK_SENTINEL: &str = "segredo-interno-vazado-42";
    const UNKNOWN_CODE: &str = "codigo_customizado_xpto";

    #[derive(Clone)]
    struct ErrState {
        base_url: String,
        dcr_error: Arc<AtomicBool>,
        token_mode: Arc<AtomicUsize>,
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let dcr_error = Arc::new(AtomicBool::new(true));
    let token_mode = Arc::new(AtomicUsize::new(0));
    let state = ErrState {
        base_url: base_url.clone(),
        dcr_error: dcr_error.clone(),
        token_mode: token_mode.clone(),
    };

    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(|State(st): State<ErrState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "resource": format!("{}/mcp", st.base_url),
                        "authorization_servers": [st.base_url]
                    }),
                )
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<ErrState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": st.base_url,
                        "authorization_endpoint": format!("{}/authorize", st.base_url),
                        "token_endpoint": format!("{}/token", st.base_url),
                        "registration_endpoint": format!("{}/register", st.base_url)
                    }),
                )
            }),
        )
        .route(
            "/register",
            post(|State(st): State<ErrState>| async move {
                if st.dcr_error.load(Ordering::SeqCst) {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        json!({"error": UNKNOWN_CODE, "error_description": LEAK_SENTINEL}),
                    );
                }
                json_response(
                    StatusCode::CREATED,
                    json!({"client_id": "dcr-err-1", "token_endpoint_auth_method": "none"}),
                )
            }),
        )
        .route(
            "/authorize",
            get(|Query(params): Query<HashMap<String, String>>| async move {
                let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
                let state_param = params.get("state").cloned().unwrap_or_default();
                let location = format!("{redirect_uri}?code=err-code-1&state={state_param}");
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/token",
            post(|State(st): State<ErrState>| async move {
                match st.token_mode.load(Ordering::SeqCst) {
                    0 => json_response(
                        StatusCode::OK,
                        json!({
                            "access_token": "tok-err-1",
                            "token_type": "Bearer",
                            "refresh_token": "rt-err-1",
                            "expires_in": 3600
                        }),
                    ),
                    1 => json_response(
                        StatusCode::BAD_REQUEST,
                        json!({"error": "invalid_grant", "error_description": LEAK_SENTINEL}),
                    ),
                    _ => json_response(
                        StatusCode::BAD_REQUEST,
                        json!({"error": UNKNOWN_CODE, "error_description": LEAK_SENTINEL}),
                    ),
                }
            }),
        )
        .route("/mcp", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "err-sanitized",
            format!("{base_url}/mcp"),
            None,
            AuthConfig::OAuthDcr,
        ))
        .unwrap();
    let oauth = env.oauth_manager();
    let input = OAuthStartInput {
        origin_client: OAuthOrigin::Desktop,
        local_port: 8765,
        public_base_url: None,
        cimd_client_id_url: None,
    };

    // 1) Erro no DCR não vaza código desconhecido nem error_description.
    let reg_err = oauth.start("err-sanitized", input.clone()).await.unwrap_err();
    let reg_text = format!("{reg_err} | {reg_err:?}");
    assert!(!reg_text.contains(LEAK_SENTINEL));
    assert!(!reg_text.contains(UNKNOWN_CODE));
    assert!(reg_text.contains("HTTP 400"));

    // 2) Erro no callback direto sanitiza error_description e código desconhecido.
    dcr_error.store(false, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } =
        oauth.start("err-sanitized", input.clone()).await.unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let state_cb = query_params(&authorize_url).get("state").cloned().unwrap();
    let cb_err = oauth
        .handle_callback(&state_cb, None, Some(UNKNOWN_CODE), Some(LEAK_SENTINEL))
        .await
        .unwrap_err();
    let cb_text = format!("{cb_err} | {cb_err:?}");
    assert!(!cb_text.contains(LEAK_SENTINEL));
    assert!(!cb_text.contains(UNKNOWN_CODE));

    // 3) Erro na troca /token com código conhecido ignora error_description.
    token_mode.store(1, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } =
        oauth.start("err-sanitized", input.clone()).await.unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code, state_tok) = simulate_browser_authorize(&authorize_url).await;
    let tok_err = oauth
        .handle_callback(&state_tok, Some(&code), None, None)
        .await
        .unwrap_err();
    let tok_text = format!("{tok_err} | {tok_err:?}");
    assert!(!tok_text.contains(LEAK_SENTINEL));
    assert!(matches!(tok_err, OAuthError::InvalidGrant(_)));

    // 4) Conclui login válido e valida sanitização no TokenRefresher::refresh.
    token_mode.store(0, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } =
        oauth.start("err-sanitized", input).await.unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code_ok, state_ok) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state_ok, Some(&code_ok), None, None)
        .await
        .unwrap();

    token_mode.store(2, Ordering::SeqCst);
    let ref_err = TokenRefresher::refresh(oauth.as_ref(), "err-sanitized")
        .await
        .unwrap_err();
    let ref_text = format!("{ref_err} | {ref_err:?}");
    assert!(!ref_text.contains(LEAK_SENTINEL));
    assert!(!ref_text.contains(UNKNOWN_CODE));

    server_handle.abort();
}

#[tokio::test]
async fn oauth_dcr_and_token_posts_do_not_follow_redirects() {
    #[derive(Clone)]
    struct RedirState {
        base_url: String,
        sink_url: String,
        redirect_dcr: Arc<AtomicBool>,
        redirect_token: Arc<AtomicBool>,
    }

    let sink_hits = Arc::new(AtomicUsize::new(0));
    let sink_hits_srv = sink_hits.clone();
    let sink_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = sink_listener.local_addr().unwrap();
    let sink_url = format!("http://{sink_addr}/roubado");
    let sink_app = Router::new().route(
        "/roubado",
        post(move || {
            let hits = sink_hits_srv.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }),
    );
    let sink_handle = tokio::spawn(async move {
        axum::serve(sink_listener, sink_app).await.unwrap();
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let redirect_dcr = Arc::new(AtomicBool::new(true));
    let redirect_token = Arc::new(AtomicBool::new(true));
    let state = RedirState {
        base_url: base_url.clone(),
        sink_url,
        redirect_dcr: redirect_dcr.clone(),
        redirect_token: redirect_token.clone(),
    };

    let app = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(|State(st): State<RedirState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "resource": format!("{}/mcp", st.base_url),
                        "authorization_servers": [st.base_url]
                    }),
                )
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(st): State<RedirState>| async move {
                json_response(
                    StatusCode::OK,
                    json!({
                        "issuer": st.base_url,
                        "authorization_endpoint": format!("{}/authorize", st.base_url),
                        "token_endpoint": format!("{}/token", st.base_url),
                        "registration_endpoint": format!("{}/register", st.base_url)
                    }),
                )
            }),
        )
        .route(
            "/register",
            post(|State(st): State<RedirState>| async move {
                if st.redirect_dcr.load(Ordering::SeqCst) {
                    return Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, st.sink_url)
                        .body(Body::empty())
                        .unwrap();
                }
                json_response(
                    StatusCode::CREATED,
                    json!({"client_id": "dcr-redir-1", "token_endpoint_auth_method": "none"}),
                )
            }),
        )
        .route(
            "/authorize",
            get(|Query(params): Query<HashMap<String, String>>| async move {
                let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
                let state_param = params.get("state").cloned().unwrap_or_default();
                let location = format!("{redirect_uri}?code=redir-code-1&state={state_param}");
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/token",
            post(|State(st): State<RedirState>| async move {
                if st.redirect_token.load(Ordering::SeqCst) {
                    return Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, st.sink_url)
                        .body(Body::empty())
                        .unwrap();
                }
                json_response(
                    StatusCode::OK,
                    json!({
                        "access_token": "tok-redir-1",
                        "token_type": "Bearer",
                        "refresh_token": "rt-redir-1",
                        "expires_in": 3600
                    }),
                )
            }),
        )
        .route("/mcp", post(|| async { StatusCode::BAD_REQUEST }))
        .with_state(state);
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let env = TestEnv::new();
    env.registry
        .create(remote_config(
            "no-redir",
            format!("{base_url}/mcp"),
            None,
            AuthConfig::OAuthDcr,
        ))
        .unwrap();
    let oauth = env.oauth_manager();
    let input = OAuthStartInput {
        origin_client: OAuthOrigin::Desktop,
        local_port: 8765,
        public_base_url: None,
        cimd_client_id_url: None,
    };

    // 1) POST /register com 307 não pode seguir redirect.
    let dcr_err = oauth.start("no-redir", input.clone()).await.unwrap_err();
    assert!(matches!(dcr_err, OAuthError::RegistrationFailed(_)));
    assert_eq!(sink_hits.load(Ordering::SeqCst), 0);

    // 2) POST /token (exchange) com 307 não pode seguir redirect.
    redirect_dcr.store(false, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } =
        oauth.start("no-redir", input.clone()).await.unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    let exchange_err = oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .unwrap_err();
    assert!(matches!(exchange_err, OAuthError::Network(_)));
    assert_eq!(sink_hits.load(Ordering::SeqCst), 0);

    // 3) POST /token (refresh) com 307 também não pode seguir redirect.
    redirect_token.store(false, Ordering::SeqCst);
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } =
        oauth.start("no-redir", input).await.unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    let (code_ok, state_ok) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state_ok, Some(&code_ok), None, None)
        .await
        .unwrap();

    redirect_token.store(true, Ordering::SeqCst);
    let refresh_err = oauth.refresh_access_token("no-redir").await.unwrap_err();
    assert!(matches!(refresh_err, OAuthError::Network(_)));
    assert_eq!(sink_hits.load(Ordering::SeqCst), 0);

    server_handle.abort();
    sink_handle.abort();
}

#[tokio::test]
async fn oauth_manual_client_id_is_trimmed_consistently_in_start_callback_and_refresh() {
    let (addr, state, server_handle) = spawn_mock_oauth_server(MockOAuthConfig::default()).await;
    state
        .add_static_client(
            "manual-trim",
            Some("manual-secret"),
            vec![format!("http://127.0.0.1:8765{OAUTH_CALLBACK_PATH}")],
        )
        .await;

    let env = TestEnv::new();
    let stored = env
        .registry
        .create(remote_config(
            "manual-trim-test",
            format!("http://{addr}/mcp"),
            Some("google-gmail"),
            AuthConfig::OAuthManualClient {
                client_id: "  manual-trim  ".to_owned(),
                scopes: vec!["openid".to_owned()],
            },
        ))
        .unwrap();
    assert_eq!(
        stored.auth,
        AuthConfig::OAuthManualClient {
            client_id: "manual-trim".to_owned(),
            scopes: vec!["openid".to_owned()],
        }
    );

    env.secrets
        .set(
            "manual-trim-test",
            IntegrationSecrets {
                client_secret: Some(Zeroizing::new("manual-secret".to_owned())),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let oauth = env.oauth_manager();
    let OAuthStartOutcome::OpenUrl { authorize_url, .. } = oauth
        .start(
            "manual-trim-test",
            OAuthStartInput {
                origin_client: OAuthOrigin::Desktop,
                local_port: 8765,
                public_base_url: None,
                cimd_client_id_url: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("esperava OpenUrl");
    };
    assert_eq!(
        query_params(&authorize_url).get("client_id"),
        Some(&"manual-trim".to_owned())
    );

    let (code, state_param) = simulate_browser_authorize(&authorize_url).await;
    oauth
        .handle_callback(&state_param, Some(&code), None, None)
        .await
        .unwrap();
    let renewed = oauth.refresh_access_token("manual-trim-test").await.unwrap();
    assert!(renewed.starts_with("access-token-"));

    server_handle.abort();
}
