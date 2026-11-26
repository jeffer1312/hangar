//! Rotas HTTP `/api/integrations/*`: catálogo, CRUD sem exposição de segredos, OAuth 2.1, teste e aprovações.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use hangar_core::http::{detail, json_response};
use hangar_integrations::{
    ApprovalDecision, ApprovalError, AuthConfig, CancelOutcome, ClientError, IntegrationConfig,
    IntegrationSecrets, IntegrationStatus, IntegrationUpdate, OAuthError, OAuthOrigin,
    OAuthStartInput, OAuthStartOutcome, PermissionPolicy, RegistryError, SecretKeyError,
    SecretStoreError, ToolsCache, Transport, config_fingerprint, validate_config_and_secrets,
};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use zeroize::{Zeroize, Zeroizing};

use crate::routes::AppState;

const MAX_BODY: usize = 256 * 1024;
const RESERVED_HTTP_HEADERS: [&str; 5] = [
    "authorization",
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

enum Route {
    Collection,
    Catalog,
    OAuthCallback,
    Approval(String),
    Item(String),
    OAuthStart(String),
    OAuthOpenOnDesktop(String),
    Test(String),
    Tools(String),
}

impl Route {
    fn accepts(&self, method: &Method) -> bool {
        match self {
            Self::Collection => matches!(*method, Method::GET | Method::POST),
            Self::Catalog | Self::OAuthCallback | Self::Tools(_) => *method == Method::GET,
            Self::Approval(_) => matches!(*method, Method::POST | Method::DELETE),
            Self::Item(_) => matches!(*method, Method::PATCH | Method::DELETE),
            Self::OAuthStart(_) | Self::OAuthOpenOnDesktop(_) | Self::Test(_) => {
                *method == Method::POST
            }
        }
    }
}

/// Classifica o caminho das rotas de integrações usando a mesma regra na borda e no despacho.
/// Divide os segmentos antes de decodificar `%XX` para impedir que `%2F` crie segmentos extras.
fn classify_path(path: &str) -> Option<Route> {
    let segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(|part| {
            percent_encoding::percent_decode_str(part)
                .decode_utf8_lossy()
                .into_owned()
        })
        .collect();
    let segs: Vec<&str> = segments.iter().map(String::as_str).collect();
    let reserved = |id: &str| matches!(id, "catalog" | "oauth" | "approvals");
    match segs.as_slice() {
        ["api", "integrations"] => Some(Route::Collection),
        ["api", "integrations", "catalog"] => Some(Route::Catalog),
        ["api", "integrations", "oauth", "callback"] => Some(Route::OAuthCallback),
        ["api", "integrations", "approvals", id] if !id.is_empty() => {
            Some(Route::Approval((*id).to_owned()))
        }
        ["api", "integrations", id] if !id.is_empty() && !reserved(id) => {
            Some(Route::Item((*id).to_owned()))
        }
        ["api", "integrations", id, "oauth", "start"] if !id.is_empty() && !reserved(id) => {
            Some(Route::OAuthStart((*id).to_owned()))
        }
        ["api", "integrations", id, "oauth", "open-on-desktop"]
            if !id.is_empty() && !reserved(id) =>
        {
            Some(Route::OAuthOpenOnDesktop((*id).to_owned()))
        }
        ["api", "integrations", id, "test"] if !id.is_empty() && !reserved(id) => {
            Some(Route::Test((*id).to_owned()))
        }
        ["api", "integrations", id, "tools"] if !id.is_empty() && !reserved(id) => {
            Some(Route::Tools((*id).to_owned()))
        }
        _ => None,
    }
}

/// Indica se o método e caminho pertencem às rotas nativas de integrações do Rust.
pub fn matches(method: &Method, path: &str) -> bool {
    classify_path(path).is_some_and(|route| route.accepts(method))
}

/// Roteador público das rotas de integrações MCP.
pub fn router<S: Clone + Send + Sync + 'static>(state: Arc<AppState>) -> Router<S> {
    Router::new()
        .route("/api/integrations", any(public))
        .route("/api/integrations/{*rest}", any(public))
        .with_state(state)
}

async fn public(
    State(state): State<Arc<AppState>>,
    peer: Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>,
    request: Request,
) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    let headers = request.headers().clone();
    let mut response = reply(&state, peer.ok().map(|p| p.0), request).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    response
}

fn is_guest_request(request: &Request) -> bool {
    crate::routes::on_guest_port(request)
        || request.headers().contains_key("x-hangar-guest")
        || matches!(
            hangar_core::guest::caller(request),
            Some(hangar_core::guest::Caller::Guest(_))
        )
        || request
            .extensions()
            .get::<crate::login_guests::gate::Login>()
            .is_some()
}

async fn reply(state: &AppState, peer: Option<SocketAddr>, request: Request) -> Response {
    // Convidado (porta convidado, cabeçalho ou login) nunca administra integrações nem acessa callback.
    if is_guest_request(&request) {
        return detail(
            StatusCode::FORBIDDEN,
            "integration_owner_only",
            "só o dono administra integrações",
            json!({}),
        );
    }

    let route = classify_path(request.uri().path());
    let method = request.method().clone();

    if method == Method::GET && matches!(route, Some(Route::OAuthCallback)) {
        return handle_oauth_callback(state, request).await;
    }

    let socket = peer.unwrap_or_else(|| "0.0.0.0:0".parse().expect("endereço estático válido"));
    if let Err(response) = state
        .edge()
        .owner_only(
            socket,
            &request,
            ("integration_owner_only", "só o dono administra integrações"),
        )
        .await
    {
        return response;
    }

    let Some(route) = route else {
        return detail(StatusCode::NOT_FOUND, "not_found", "rota desconhecida", json!({}));
    };
    if !route.accepts(&method) {
        return detail(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "método não aceito nesta rota",
            json!({}),
        );
    }

    match (method, route) {
        (Method::GET, Route::Catalog) => {
            json_response(StatusCode::OK, json!({"presets": hangar_integrations::catalog()}))
        }
        (Method::GET, Route::Collection) => list_integrations(state).await,
        (Method::POST, Route::Collection) => create_integration(state, request).await,
        (Method::PATCH, Route::Item(id)) => patch_integration(state, &id, request).await,
        (Method::DELETE, Route::Item(id)) => delete_integration(state, &id).await,
        (Method::POST, Route::OAuthStart(id)) => oauth_start(state, &id, request).await,
        (Method::POST, Route::OAuthOpenOnDesktop(id)) => {
            oauth_open_on_desktop(state, &id, request).await
        }
        (Method::POST, Route::Test(id)) => test_integration(state, &id).await,
        (Method::GET, Route::Tools(id)) => list_tools(state, &id),
        (Method::POST, Route::Approval(request_id)) => {
            answer_approval(state, &request_id, request).await
        }
        (Method::DELETE, Route::Approval(request_id)) => cancel_approval(state, &request_id),
        _ => detail(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "método não aceito nesta rota",
            json!({}),
        ),
    }
}

// ---------------------------------------------------------------------------
// Tipos de visualização e entrada JSON (sem nunca derivar Debug com segredos)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct IntegrationView {
    #[serde(flatten)]
    config: IntegrationConfig,
    status: IntegrationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

fn build_view(
    config: IntegrationConfig,
    secrets: Option<&IntegrationSecrets>,
    last_error: Option<String>,
) -> IntegrationView {
    let mut status = hangar_integrations::compute_status(&config, secrets);
    // Só rebaixa para Error quando a configuração parecia pronta; AuthRequired/AuthExpired/Disabled prevalecem.
    if status == IntegrationStatus::Ready && last_error.is_some() {
        status = IntegrationStatus::Error;
    }
    IntegrationView {
        config,
        status,
        last_error,
    }
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

#[derive(Default, Deserialize)]
struct SecretsPatch {
    #[serde(default, deserialize_with = "present")]
    bearer_token: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    headers: Option<Option<BTreeMap<String, Option<String>>>>,
    #[serde(default, deserialize_with = "present")]
    oauth_client_secret: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    client_secret: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    env: Option<Option<BTreeMap<String, Option<String>>>>,
}

#[derive(Deserialize)]
struct CreateIntegrationBody {
    id: String,
    name: String,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
    transport: Transport,
    auth: AuthConfig,
    #[serde(default)]
    policy: PermissionPolicy,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    projects: Vec<String>,
    #[serde(default)]
    allowed_tools: BTreeSet<String>,
    #[serde(default)]
    secrets: Option<SecretsPatch>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Default, Deserialize)]
struct PatchIntegrationBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    preset: Option<Option<String>>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    transport: Option<Transport>,
    #[serde(default)]
    auth: Option<AuthConfig>,
    #[serde(default)]
    policy: Option<PermissionPolicy>,
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    projects: Option<Vec<String>>,
    #[serde(default)]
    allowed_tools: Option<BTreeSet<String>>,
    #[serde(default)]
    add_allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    remove_allowed_tools: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    secrets: Option<Option<SecretsPatch>>,
}

#[derive(Deserialize)]
struct OAuthStartBody {
    origin_client: OAuthOrigin,
    #[serde(default)]
    public_base_url: Option<String>,
    #[serde(default)]
    cimd_client_id_url: Option<String>,
}

#[derive(Default, Deserialize)]
struct OpenDesktopBody {
    #[serde(default)]
    authorize_url: Option<String>,
}

#[derive(Deserialize)]
struct ApprovalAnswerBody {
    interaction_id: String,
    decision: ApprovalDecision,
}

// ---------------------------------------------------------------------------
// Leitura protegida de corpo e validação de segredos
// ---------------------------------------------------------------------------

async fn read_body_bytes(request: Request) -> Result<Zeroizing<Vec<u8>>, Response> {
    let mut body = request.into_body();
    let mut raw = Zeroizing::new(Vec::with_capacity(MAX_BODY.min(16 * 1024)));
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return Err(invalid_body());
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let fits = raw.len() + data.len() <= MAX_BODY;
        if fits {
            raw.extend_from_slice(&data);
        }
        if let Ok(mut owned) = data.try_into_mut() {
            owned.as_mut().zeroize();
        }
        if !fits {
            return Err(detail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "corpo excede o limite de 256 KiB",
                json!({}),
            ));
        }
    }
    Ok(raw)
}

async fn read_json_body<T: DeserializeOwned>(request: Request) -> Result<T, Response> {
    let raw = read_body_bytes(request).await?;
    // Erro genérico para nunca ecoar campos secretos vindos em JSON malformado.
    serde_json::from_slice::<T>(&raw).map_err(|_| invalid_body())
}

async fn read_optional_json_body<T: DeserializeOwned + Default>(
    request: Request,
) -> Result<T, Response> {
    let raw = read_body_bytes(request).await?;
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice::<T>(&raw).map_err(|_| invalid_body())
}

fn validate_secret_field(value: &str) -> Result<(), Response> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(invalid_body());
    }
    Ok(())
}

fn validate_header_entry(name: &str, value: &str) -> Result<String, Response> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 256 || value.contains('\0') {
        return Err(invalid_body());
    }
    let parsed = HeaderName::from_bytes(trimmed.as_bytes()).map_err(|_| invalid_body())?;
    let lower = parsed.as_str();
    if RESERVED_HTTP_HEADERS.contains(&lower) {
        return Err(invalid_body());
    }
    HeaderValue::from_str(value).map_err(|_| invalid_body())?;
    Ok(lower.to_owned())
}

fn validate_env_key_entry(key: &str, value: &str) -> Result<String, Response> {
    let trimmed = key.trim();
    if trimmed.is_empty()
        || trimmed.len() > 256
        || trimmed.contains('=')
        || trimmed.contains('\0')
        || value.contains('\0')
    {
        return Err(invalid_body());
    }
    Ok(trimmed.to_owned())
}

fn apply_secrets_patch(
    slot: &mut IntegrationSecrets,
    patch: SecretsPatch,
) -> Result<(), Response> {
    if let Some(bearer_opt) = patch.bearer_token {
        match bearer_opt {
            None => slot.bearer_token = None,
            Some(tok) => {
                validate_secret_field(&tok)?;
                slot.bearer_token = Some(Zeroizing::new(tok));
            }
        }
    }

    let secret_patch = patch.oauth_client_secret.or(patch.client_secret);
    if let Some(cs_opt) = secret_patch {
        match cs_opt {
            None => slot.client_secret = None,
            Some(sec) => {
                validate_secret_field(&sec)?;
                slot.client_secret = Some(Zeroizing::new(sec));
            }
        }
    }

    if let Some(headers_opt) = patch.headers {
        match headers_opt {
            None => slot.headers.clear(),
            Some(map) => {
                for (raw_name, val_opt) in map {
                    match val_opt {
                        None => {
                            let lower = raw_name.trim().to_ascii_lowercase();
                            slot.headers.remove(&lower);
                        }
                        Some(val) => {
                            let lower = validate_header_entry(&raw_name, &val)?;
                            slot.headers.insert(lower, Zeroizing::new(val));
                        }
                    }
                }
            }
        }
    }

    if let Some(env_opt) = patch.env {
        match env_opt {
            None => slot.env.clear(),
            Some(map) => {
                for (raw_key, val_opt) in map {
                    match val_opt {
                        None => {
                            slot.env.remove(raw_key.trim());
                        }
                        Some(val) => {
                            let key = validate_env_key_entry(&raw_key, &val)?;
                            slot.env.insert(key, Zeroizing::new(val));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_integrations(state: &AppState) -> Response {
    let configs = match state.integrations.registry.list() {
        Ok(list) => list,
        Err(err) => return registry_error(err),
    };
    let errors = state.integrations.last_errors_snapshot();
    let mut views = Vec::with_capacity(configs.len());
    for cfg in configs {
        let secrets = match state.integrations.secrets.get(&cfg.id).await {
            Ok(sec) => sec,
            Err(err) => return secret_store_error(err),
        };
        let last_err = errors.get(&cfg.id).cloned();
        views.push(build_view(cfg, secrets.as_ref(), last_err));
    }
    json_response(StatusCode::OK, json!({"integrations": views}))
}

async fn create_integration(state: &AppState, request: Request) -> Response {
    let body: CreateIntegrationBody = match read_json_body(request).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };

    let mut initial_secrets = IntegrationSecrets::default();
    if let Some(patch) = body.secrets
        && let Err(resp) = apply_secrets_patch(&mut initial_secrets, patch)
    {
        return resp;
    }

    let now = now_unix_secs();
    let config = IntegrationConfig {
        id: body.id,
        name: body.name,
        preset: body.preset,
        enabled: body.enabled,
        transport: body.transport,
        auth: body.auth,
        policy: body.policy,
        pinned: body.pinned,
        projects: body.projects,
        allowed_tools: body.allowed_tools,
        created_at: now,
        updated_at: now,
    };
    if let Err(err) = config.validate() {
        return registry_error(RegistryError::Validation(err));
    }
    if let Err(msg) = validate_config_and_secrets(&config, &initial_secrets) {
        return detail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "erro_corpo_invalido",
            &msg,
            json!({}),
        );
    }

    let lock_id = config.id.clone();
    let _guard = state.integrations.lock_integration(&lock_id).await;

    // Confere duplicidade sob a trava antes de tocar em segredos para nunca apagar credenciais de um ID existente.
    match state.integrations.registry.get(&lock_id) {
        Ok(Some(_)) => return registry_error(RegistryError::AlreadyExists(lock_id)),
        Ok(None) => {}
        Err(err) => return registry_error(err),
    }

    // Nunca herda segredos órfãos de um DELETE parcial anterior ao recriar um ID livre sem `secrets`.
    if initial_secrets.is_empty()
        && let Err(err) = state.integrations.secrets.delete(&lock_id).await
    {
        return secret_store_error(err);
    }

    let created = match state.integrations.registry.create(config) {
        Ok(cfg) => cfg,
        Err(err) => return registry_error(err),
    };

    let stored_secrets = if initial_secrets.is_empty() {
        None
    } else {
        if let Err(err) = state
            .integrations
            .secrets
            .set(&created.id, initial_secrets.clone())
            .await
        {
            // Desfaz gravação em config.json para nunca deixar criação parcial sem o segredo pedido.
            let _ = state.integrations.registry.delete(&created.id);
            return secret_store_error(err);
        }
        Some(initial_secrets)
    };

    state.integrations.clear_error(&created.id);
    state.integrations.clear_cancel_warning(&created.id);
    let view = build_view(created, stored_secrets.as_ref(), None);
    json_response(StatusCode::CREATED, json!({"integration": view}))
}

fn remote_url(transport: &Transport) -> Option<&str> {
    match transport {
        Transport::RemoteHttp { url, .. } => Some(url.as_str()),
        Transport::LocalStdio { .. } => None,
    }
}

fn clear_remote_endpoint_secrets(
    slot: &mut Option<IntegrationSecrets>,
    keep_oauth_fp: Option<[u8; 32]>,
) {
    if let Some(entry) = slot.as_mut() {
        entry.bearer_token = None;
        entry.client_secret = None;
        let keep_oauth = keep_oauth_fp.is_some_and(|expected| {
            entry
                .oauth
                .as_ref()
                .and_then(|tok| tok.config_fingerprint)
                == Some(expected)
        });
        if !keep_oauth {
            entry.oauth = None;
        }
        entry.headers.clear();
        if entry.is_empty() {
            *slot = None;
        }
    }
}

async fn invalidate_integration_runtime(state: &AppState, id: &str) {
    state.integrations.clients.invalidate(id).await;
    state.integrations.tools_cache.invalidate(id);
    state.integrations.oauth.cancel_pending_for(id);
    state.integrations.approvals.cancel_for_integration(id);
    state.integrations.clear_voice_confirmations(id);
    state.integrations.clear_error(id);
    state.integrations.clear_cancel_warning(id);
}

async fn patch_integration(state: &AppState, id: &str, request: Request) -> Response {
    let body: PatchIntegrationBody = match read_json_body(request).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };

    if body.allowed_tools.is_some()
        && (body.add_allowed_tools.is_some() || body.remove_allowed_tools.is_some())
    {
        return invalid_body();
    }

    // Valida previamente uma amostra de patch de segredos antes de travar ou mutar o config.json.
    if let Some(Some(ref patch)) = body.secrets {
        let mut probe = IntegrationSecrets::default();
        let probe_patch = SecretsPatch {
            bearer_token: patch.bearer_token.clone(),
            headers: patch.headers.clone(),
            oauth_client_secret: patch.oauth_client_secret.clone(),
            client_secret: patch.client_secret.clone(),
            env: patch.env.clone(),
        };
        if let Err(resp) = apply_secrets_patch(&mut probe, probe_patch) {
            return resp;
        }
    }

    let _guard = state.integrations.lock_integration(id).await;

    let existing = match state.integrations.registry.get(id) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return not_found_integration(id),
        Err(err) => return registry_error(err),
    };

    let previous_secrets = match state.integrations.secrets.get(id).await {
        Ok(sec) => sec,
        Err(err) => return secret_store_error(err),
    };

    let has_field_updates = body.name.is_some()
        || body.preset.is_some()
        || body.enabled.is_some()
        || body.transport.is_some()
        || body.auth.is_some()
        || body.policy.is_some()
        || body.pinned.is_some()
        || body.projects.is_some()
        || body.allowed_tools.is_some();
    let has_allowed_delta =
        body.add_allowed_tools.is_some() || body.remove_allowed_tools.is_some();
    let add_tools = body.add_allowed_tools.unwrap_or_default();
    let remove_tools = body.remove_allowed_tools.unwrap_or_default();
    let explicit_allowed_tools = body.allowed_tools.is_some();

    // Pré-valida a configuração efetiva e os segredos resultantes antes de gravar no disco.
    let mut candidate_config = existing.clone();
    if let Some(ref name) = body.name {
        candidate_config.name = name.clone();
    }
    if let Some(ref preset) = body.preset {
        candidate_config.preset = preset.clone();
    }
    if let Some(enabled) = body.enabled {
        candidate_config.enabled = enabled;
    }
    if let Some(ref transport) = body.transport {
        if &candidate_config.transport != transport && body.allowed_tools.is_none() {
            candidate_config.allowed_tools.clear();
        }
        candidate_config.transport = transport.clone();
    }
    if let Some(ref auth) = body.auth {
        candidate_config.auth = auth.clone();
    }
    if let Some(policy) = body.policy {
        candidate_config.policy = policy;
    }
    if let Some(pinned) = body.pinned {
        candidate_config.pinned = pinned;
    }
    if let Some(ref projects) = body.projects {
        candidate_config.projects = projects.clone();
    }
    if let Some(ref allowed) = body.allowed_tools {
        candidate_config.allowed_tools = allowed.clone();
    }
    for tool in &add_tools {
        candidate_config.allowed_tools.insert(tool.clone());
    }
    for tool in &remove_tools {
        candidate_config.allowed_tools.remove(tool);
    }

    let remote_url_changed = remote_url(&existing.transport) != remote_url(&candidate_config.transport);
    let destination_changed = existing.transport != candidate_config.transport
        || existing.auth != candidate_config.auth;
    let should_invalidate =
        body.enabled == Some(false) || destination_changed || body.secrets.is_some();

    let candidate_secrets = match &body.secrets {
        None => {
            let mut next = previous_secrets.clone();
            if remote_url_changed {
                clear_remote_endpoint_secrets(&mut next, None);
            }
            next
        }
        Some(None) => None,
        Some(Some(patch)) => {
            let mut next = previous_secrets.clone();
            if remote_url_changed {
                clear_remote_endpoint_secrets(&mut next, None);
            }
            let clone_patch = SecretsPatch {
                bearer_token: patch.bearer_token.clone(),
                headers: patch.headers.clone(),
                oauth_client_secret: patch.oauth_client_secret.clone(),
                client_secret: patch.client_secret.clone(),
                env: patch.env.clone(),
            };
            let entry = next.get_or_insert_with(IntegrationSecrets::default);
            if let Err(resp) = apply_secrets_patch(entry, clone_patch) {
                return resp;
            }
            if entry.is_empty() {
                next = None;
            }
            next
        }
    };
    if let Err(msg) = validate_config_and_secrets(
        &candidate_config,
        &candidate_secrets.clone().unwrap_or_default(),
    ) {
        return detail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "erro_corpo_invalido",
            &msg,
            json!({}),
        );
    }

    // Invalida conexões e cartões antes de publicar novo destino/auth para impedir reutilização cruzada.
    if destination_changed || body.enabled == Some(false) {
        state.integrations.clients.invalidate(id).await;
        state.integrations.tools_cache.invalidate(id);
        state.integrations.oauth.cancel_pending_for(id);
        state.integrations.approvals.cancel_for_integration(id);
        state.integrations.clear_voice_confirmations(id);
    }

    // Troca de URL/destino limpa segredos remotos ANTES de gravar o novo destino em config.json,
    // impedindo que `ensure_connected` combine URL nova com credenciais do servidor anterior.
    let mut cleared_old_remote_secrets = false;
    if remote_url_changed && previous_secrets.is_some() {
        if let Err(err) = state
            .integrations
            .secrets
            .update_with(id, move |slot| {
                clear_remote_endpoint_secrets(slot, None);
                Ok(())
            })
            .await
        {
            return secret_store_error(err);
        }
        cleared_old_remote_secrets = true;
    }

    let rollback_update = IntegrationUpdate {
        name: Some(existing.name.clone()),
        preset: Some(existing.preset.clone()),
        enabled: Some(existing.enabled),
        transport: Some(existing.transport.clone()),
        auth: Some(existing.auth.clone()),
        policy: Some(existing.policy),
        pinned: Some(existing.pinned),
        projects: Some(existing.projects.clone()),
        // Restaura `allowed_tools` quando o PATCH substituiu, limpou ou aplicou delta na lista (sob `lock_integration`).
        allowed_tools: if explicit_allowed_tools || destination_changed || has_allowed_delta {
            Some(existing.allowed_tools.clone())
        } else {
            None
        },
        updated_at: Some(existing.updated_at),
    };

    async fn restore_secrets(state: &AppState, id: String, prev: Option<IntegrationSecrets>) {
        match prev {
            Some(sec) => {
                let _ = state.integrations.secrets.set(&id, sec).await;
            }
            None => {
                let _ = state.integrations.secrets.delete(&id).await;
            }
        }
    }

    let mut did_registry_update = false;
    let mut updated = if !has_field_updates && has_allowed_delta {
        existing.clone()
    } else {
        let update = IntegrationUpdate {
            name: body.name,
            preset: body.preset,
            enabled: body.enabled,
            transport: body.transport,
            auth: body.auth,
            policy: body.policy,
            pinned: body.pinned,
            projects: body.projects,
            allowed_tools: body.allowed_tools,
            updated_at: None,
        };
        match state.integrations.registry.update(id, update) {
            Ok(cfg) => {
                did_registry_update = true;
                cfg
            }
            Err(err) => {
                if cleared_old_remote_secrets {
                    restore_secrets(state, id.to_owned(), previous_secrets).await;
                }
                return registry_error(err);
            }
        }
    };

    if has_allowed_delta {
        match state
            .integrations
            .registry
            .patch_allowed_tools(id, &add_tools, &remove_tools)
        {
            Ok(cfg) => {
                did_registry_update = true;
                updated = cfg;
            }
            Err(err) => {
                if did_registry_update {
                    let _ = state.integrations.registry.update(id, rollback_update);
                }
                if cleared_old_remote_secrets {
                    restore_secrets(state, id.to_owned(), previous_secrets).await;
                }
                return registry_error(err);
            }
        }
    }

    let secrets_after = match body.secrets {
        None => {
            if remote_url_changed {
                // Segunda limpeza pós-publicação remove segredos antigos preservando eventual OAuth já emitido para a nova configuração.
                let published_fp = config_fingerprint(&updated);
                match state
                    .integrations
                    .secrets
                    .update_with(id, move |slot| {
                        clear_remote_endpoint_secrets(slot, Some(published_fp));
                        Ok(slot.clone())
                    })
                    .await
                {
                    Ok(sec) => sec,
                    Err(err) => {
                        if did_registry_update {
                            let _ = state.integrations.registry.update(id, rollback_update);
                        }
                        if cleared_old_remote_secrets {
                            restore_secrets(state, id.to_owned(), previous_secrets).await;
                        }
                        return secret_store_error(err);
                    }
                }
            } else {
                candidate_secrets
            }
        }
        Some(None) => {
            if let Err(err) = state.integrations.secrets.delete(id).await {
                if did_registry_update {
                    let _ = state.integrations.registry.update(id, rollback_update);
                }
                if cleared_old_remote_secrets {
                    restore_secrets(state, id.to_owned(), previous_secrets).await;
                }
                return secret_store_error(err);
            }
            None
        }
        Some(Some(patch)) => {
            let mut patch_slot = Some(patch);
            let published_fp = config_fingerprint(&updated);
            let res = state
                .integrations
                .secrets
                .update_with(id, move |slot| {
                    let patch = patch_slot.take().unwrap_or_default();
                    if remote_url_changed {
                        clear_remote_endpoint_secrets(slot, Some(published_fp));
                    }
                    let entry = slot.get_or_insert_with(IntegrationSecrets::default);
                    if apply_secrets_patch(entry, patch).is_err() {
                        return Err(SecretStoreError::InvalidId);
                    }
                    if entry.is_empty() {
                        *slot = None;
                    }
                    Ok(slot.clone())
                })
                .await;
            match res {
                Ok(sec) => sec,
                Err(SecretStoreError::InvalidId) => {
                    if did_registry_update {
                        let _ = state.integrations.registry.update(id, rollback_update);
                    }
                    if cleared_old_remote_secrets {
                        restore_secrets(state, id.to_owned(), previous_secrets).await;
                    }
                    return invalid_body();
                }
                Err(err) => {
                    if did_registry_update {
                        let _ = state.integrations.registry.update(id, rollback_update);
                    }
                    if cleared_old_remote_secrets {
                        restore_secrets(state, id.to_owned(), previous_secrets).await;
                    }
                    return secret_store_error(err);
                }
            }
        }
    };

    state.integrations.clear_error(id);
    if should_invalidate {
        state.integrations.clients.invalidate(id).await;
        state.integrations.tools_cache.invalidate(id);
        state.integrations.clear_voice_confirmations(id);
    }
    if destination_changed {
        state.integrations.oauth.cancel_pending_for(id);
        state.integrations.clear_cancel_warning(id);
    }

    let view = build_view(updated, secrets_after.as_ref(), None);
    json_response(StatusCode::OK, json!({"integration": view}))
}

async fn delete_integration(state: &AppState, id: &str) -> Response {
    let _guard = state.integrations.lock_integration(id).await;

    match state.integrations.registry.delete(id) {
        Ok(_) => {
            invalidate_integration_runtime(state, id).await;
            if let Err(err) = state.integrations.secrets.delete(id).await {
                return secret_store_error(err);
            }
            json_response(StatusCode::OK, json!({"ok": true, "id": id}))
        }
        // Permite concluir DELETE anterior que removeu config.json mas falhou ao apagar o segredo.
        Err(RegistryError::NotFound(_)) => match state.integrations.secrets.get(id).await {
            Ok(Some(_)) => {
                invalidate_integration_runtime(state, id).await;
                if let Err(err) = state.integrations.secrets.delete(id).await {
                    return secret_store_error(err);
                }
                json_response(StatusCode::OK, json!({"ok": true, "id": id}))
            }
            Ok(None) => not_found_integration(id),
            Err(err) => secret_store_error(err),
        },
        Err(err) => registry_error(err),
    }
}

async fn oauth_start(state: &AppState, id: &str, request: Request) -> Response {
    let fallback_host_port = parse_host_port(request.headers());
    let body: OAuthStartBody = match read_json_body(request).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };

    let local_port = match state.integrations.local_port() {
        port if port > 0 => port,
        _ => fallback_host_port.unwrap_or_else(|| state.cfg.listen.port()),
    };

    let outcome = state
        .integrations
        .oauth
        .start(
            id,
            OAuthStartInput {
                origin_client: body.origin_client,
                local_port,
                public_base_url: body.public_base_url,
                cimd_client_id_url: body.cimd_client_id_url,
            },
        )
        .await;

    match outcome {
        Ok(OAuthStartOutcome::OpenUrl {
            authorize_url,
            redirect_uri,
        }) => {
            state.integrations.clear_error(id);
            json_response(
                StatusCode::OK,
                json!({
                    "mode": "open_url",
                    "authorize_url": authorize_url,
                    "redirect_uri": redirect_uri,
                }),
            )
        }
        Ok(OAuthStartOutcome::HandoffDesktop {
            authorize_url,
            redirect_uri,
            reason,
        }) => {
            state.integrations.clear_error(id);
            json_response(
                StatusCode::OK,
                json!({
                    "mode": "handoff_desktop",
                    "authorize_url": authorize_url,
                    "redirect_uri": redirect_uri,
                    "reason": reason,
                }),
            )
        }
        Err(err) => {
            if !matches!(err, OAuthError::NotFound(_)) {
                state.integrations.record_error(id, err.to_string());
            }
            oauth_error(err)
        }
    }
}

async fn oauth_open_on_desktop(state: &AppState, id: &str, request: Request) -> Response {
    match state.integrations.registry.get(id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_integration(id),
        Err(err) => return registry_error(err),
    }

    let body: OpenDesktopBody = match read_optional_json_body(request).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };

    let authorize_url = match body.authorize_url {
        Some(url) => {
            // Nunca abre URL arbitrária no navegador do dono sem corresponder a um fluxo OAuth pendente.
            if !state.integrations.oauth.is_pending_authorize_url(id, &url) {
                return detail(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "erro_integracao_oauth_invalido",
                    "link de autorização OAuth desconhecido ou expirado",
                    json!({}),
                );
            }
            url
        }
        None => match state.integrations.oauth.latest_pending_authorize_url(id) {
            Some(url) => url,
            None => {
                return detail(
                    StatusCode::CONFLICT,
                    "integration_oauth_no_pending",
                    "nenhum fluxo OAuth pendente para abrir no desktop",
                    json!({}),
                );
            }
        },
    };

    match state.login_browser.open_default_url(&authorize_url).await {
        Ok(()) => json_response(StatusCode::OK, json!({"ok": true})),
        Err(err) => {
            let status = StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_GATEWAY);
            detail(status, err.code, &err.message, err.params)
        }
    }
}

async fn handle_oauth_callback(state: &AppState, request: Request) -> Response {
    let mut oauth_state = None;
    let mut code = None;
    let mut error = None;
    let mut error_description = None;

    if let Some(query) = request.uri().query() {
        for (k, v) in form_urlencoded::parse(query.as_bytes()) {
            match k.as_ref() {
                "state" => oauth_state = Some(v.into_owned()),
                "code" => code = Some(v.into_owned()),
                "error" => error = Some(v.into_owned()),
                "error_description" => error_description = Some(v.into_owned()),
                _ => {}
            }
        }
    }

    let res = state
        .integrations
        .oauth
        .handle_callback(
            oauth_state.as_deref().unwrap_or(""),
            code.as_deref(),
            error.as_deref(),
            error_description.as_deref(),
        )
        .await;

    match res {
        Ok(ok) => {
            state.integrations.clients.invalidate(&ok.integration_id).await;
            state.integrations.clear_error(&ok.integration_id);
            let safe_id = escape_html(&ok.integration_id);
            let html = render_callback_html(
                "Integração conectada",
                &format!(
                    "A integração <strong>{safe_id}</strong> foi autorizada com sucesso. Você já pode fechar esta aba e voltar para o Hangar."
                ),
            );
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response()
        }
        Err(err) => {
            let status = match &err {
                OAuthError::InvalidState
                | OAuthError::ExpiredState
                | OAuthError::ProviderError { .. }
                | OAuthError::InvalidGrant(_)
                | OAuthError::Validation(_) => StatusCode::BAD_REQUEST,
                OAuthError::NotFound(_) => StatusCode::NOT_FOUND,
                OAuthError::UnsupportedAuth => StatusCode::UNPROCESSABLE_ENTITY,
                OAuthError::DiscoveryFailed(_)
                | OAuthError::RegistrationFailed(_)
                | OAuthError::Network(_) => StatusCode::BAD_GATEWAY,
                OAuthError::Secrets(_) => StatusCode::SERVICE_UNAVAILABLE,
                OAuthError::Registry(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let safe_msg = escape_html(&err.to_string());
            let html = render_callback_html("Falha ao conectar integração", &safe_msg);
            (
                status,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response()
        }
    }
}

async fn test_integration(state: &AppState, id: &str) -> Response {
    let config = match state.integrations.registry.get(id) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return not_found_integration(id),
        Err(err) => return registry_error(err),
    };

    match state.integrations.clients.list_tools(id).await {
        Ok(tools) => {
            state.integrations.clear_error(id);
            let summaries = ToolsCache::into_tool_summaries(&config, tools);
            json_response(
                StatusCode::OK,
                json!({
                    "status": "ready",
                    "tools": summaries,
                }),
            )
        }
        Err(err) => client_error(state, id, err),
    }
}

fn list_tools(state: &AppState, id: &str) -> Response {
    let config = match state.integrations.registry.get(id) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return not_found_integration(id),
        Err(err) => return registry_error(err),
    };
    let summaries = state
        .integrations
        .tools_cache
        .summaries(&config)
        .unwrap_or_default();
    json_response(StatusCode::OK, json!({"tools": summaries}))
}

async fn answer_approval(state: &AppState, request_id: &str, request: Request) -> Response {
    let body: ApprovalAnswerBody = match read_json_body(request).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };

    let claim = match state.integrations.approvals.claim_answer(
        request_id,
        &body.interaction_id,
        body.decision,
    ) {
        Ok(claim) => claim,
        Err(err) => return approval_error(err),
    };

    let integration_id = claim.integration_id().to_owned();
    // Serializa gravação de `AllowAlways` com PATCH/DELETE da mesma integração.
    let _guard = state.integrations.lock_integration(&integration_id).await;

    if !claim.is_live() {
        drop(claim);
        return approval_error(ApprovalError::Lost);
    }

    if let Some(expected_fp) = claim.config_fingerprint() {
        match state.integrations.registry.get(&integration_id) {
            Ok(Some(cfg)) if config_fingerprint(&cfg) == expected_fp => {}
            Ok(_) => {
                drop(claim);
                state
                    .integrations
                    .approvals
                    .cancel_for_integration(&integration_id);
                return approval_error(ApprovalError::Lost);
            }
            Err(err) => {
                drop(claim);
                return registry_error(err);
            }
        }
    }

    let mut added_allow_always_tool: Option<String> = None;
    if claim.decision() == ApprovalDecision::AllowAlways {
        let already_allowed = state
            .integrations
            .registry
            .get(&integration_id)
            .ok()
            .flatten()
            .is_some_and(|cfg| cfg.allowed_tools.contains(claim.tool()));
        let registry = state.integrations.registry.clone();
        let target_id = integration_id.clone();
        let tool = claim.tool().to_owned();
        let tool_for_rollback = tool.clone();
        let persisted = tokio::task::spawn_blocking(move || {
            registry.add_allowed_tool(&target_id, &tool)
        })
        .await;
        match persisted {
            Ok(Ok(_)) => {
                if !already_allowed {
                    added_allow_always_tool = Some(tool_for_rollback);
                }
            }
            Ok(Err(RegistryError::NotFound(_))) => {}
            Ok(Err(err)) => {
                drop(claim);
                return registry_error(err);
            }
            Err(_) => {
                drop(claim);
                return registry_error(RegistryError::Io(
                    "tarefa de gravação de ferramenta liberada interrompida".into(),
                ));
            }
        }
    }

    let answered = match claim.commit() {
        Ok(ans) => ans,
        Err(err) => {
            // Se o chamador caiu durante a gravação de `AllowAlways`, desfaz a liberação para alinhar estado e resposta.
            if let Some(tool) = added_allow_always_tool {
                let registry = state.integrations.registry.clone();
                let target_id = integration_id.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    registry.patch_allowed_tools(&target_id, &[], &[tool])
                })
                .await;
            }
            return approval_error(err);
        }
    };

    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "decision": answered.decision,
        }),
    )
}

fn cancel_approval(state: &AppState, request_id: &str) -> Response {
    match state.integrations.approvals.try_cancel(request_id) {
        CancelOutcome::Cancelled => json_response(StatusCode::OK, json!({"ok": true})),
        CancelOutcome::InProgress => detail(
            StatusCode::CONFLICT,
            "approval_in_progress",
            "uma resposta para este pedido está em andamento",
            json!({}),
        ),
        CancelOutcome::NotFound => detail(
            StatusCode::NOT_FOUND,
            "approval_not_found",
            "pedido de aprovação não encontrado ou já encerrado",
            json!({}),
        ),
    }
}

// ---------------------------------------------------------------------------
// Auxiliares e mapeamentos de erro
// ---------------------------------------------------------------------------

fn invalid_body() -> Response {
    detail(
        StatusCode::UNPROCESSABLE_ENTITY,
        "erro_corpo_invalido",
        "corpo inválido",
        json!({}),
    )
}

fn not_found_integration(id: &str) -> Response {
    detail(
        StatusCode::NOT_FOUND,
        "integration_not_found",
        "integração não encontrada",
        json!({"id": id}),
    )
}

fn registry_error(err: RegistryError) -> Response {
    match err {
        RegistryError::Validation(v) => detail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "integration_invalid",
            &v.to_string(),
            json!({}),
        ),
        RegistryError::AlreadyExists(_) => detail(
            StatusCode::CONFLICT,
            "integration_already_exists",
            "já existe uma integração com este identificador",
            json!({}),
        ),
        RegistryError::NotFound(id) => not_found_integration(&id),
        RegistryError::Corrupted(_) => detail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "integration_registry_corrupted",
            "catálogo de integrações ilegível",
            json!({}),
        ),
        RegistryError::Io(_) => detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration_registry_io",
            "não foi possível ler ou gravar o catálogo de integrações",
            json!({}),
        ),
    }
}

fn secret_store_error(err: SecretStoreError) -> Response {
    match err {
        SecretStoreError::InvalidId => invalid_body(),
        SecretStoreError::KeyLost
        | SecretStoreError::Key(SecretKeyError::Absent | SecretKeyError::Lost) => detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration_secrets_key_lost",
            "chave de segredos das integrações ausente no chaveiro",
            json!({}),
        ),
        SecretStoreError::Key(
            SecretKeyError::Locked | SecretKeyError::Bus | SecretKeyError::Unsupported,
        ) => detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration_secrets_unavailable",
            "chaveiro de segredos das integrações indisponível",
            json!({}),
        ),
        SecretStoreError::Corrupted => detail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "integration_secrets_corrupted",
            "arquivo de segredos das integrações ilegível",
            json!({}),
        ),
        SecretStoreError::Io(_) => detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration_secrets_io",
            "não foi possível ler ou gravar os segredos das integrações",
            json!({}),
        ),
    }
}

fn oauth_error(err: OAuthError) -> Response {
    match err {
        OAuthError::NotFound(id) => not_found_integration(&id),
        OAuthError::UnsupportedAuth | OAuthError::Validation(_) => detail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "integration_oauth_invalid",
            &err.to_string(),
            json!({}),
        ),
        OAuthError::DiscoveryFailed(_)
        | OAuthError::RegistrationFailed(_)
        | OAuthError::Network(_) => detail(
            StatusCode::BAD_GATEWAY,
            "integration_oauth_upstream",
            &err.to_string(),
            json!({}),
        ),
        OAuthError::InvalidGrant(_)
        | OAuthError::ProviderError { .. }
        | OAuthError::InvalidState
        | OAuthError::ExpiredState => detail(
            StatusCode::BAD_REQUEST,
            "integration_oauth_rejected",
            &err.to_string(),
            json!({}),
        ),
        OAuthError::Registry(e) => registry_error(e),
        OAuthError::Secrets(e) => secret_store_error(e),
    }
}

fn client_error(state: &AppState, id: &str, err: ClientError) -> Response {
    match err {
        ClientError::Disabled => detail(
            StatusCode::CONFLICT,
            "integration_disabled",
            "integração desabilitada",
            json!({}),
        ),
        // Nunca devolve HTTP 401 no /test para evitar confundir falha externa com token do Hangar.
        ClientError::AuthRequired(reason) => {
            state.integrations.record_error(id, reason.clone());
            detail(StatusCode::FAILED_DEPENDENCY, "auth_required", &reason, json!({}))
        }
        ClientError::AuthExpired(reason) => {
            state.integrations.record_error(id, reason.clone());
            detail(StatusCode::FAILED_DEPENDENCY, "auth_expired", &reason, json!({}))
        }
        ClientError::Timeout(reason) => {
            state.integrations.record_error(id, reason.clone());
            detail(StatusCode::GATEWAY_TIMEOUT, "integration_timeout", &reason, json!({}))
        }
        ClientError::Transport(_) => {
            let msg = "falha de transporte ao comunicar com o servidor MCP";
            state.integrations.record_error(id, msg);
            detail(
                StatusCode::BAD_GATEWAY,
                "integration_upstream_error",
                msg,
                json!({}),
            )
        }
        ClientError::Protocol(_) => {
            let msg = "falha de protocolo ao comunicar com o servidor MCP";
            state.integrations.record_error(id, msg);
            detail(
                StatusCode::BAD_GATEWAY,
                "integration_upstream_error",
                msg,
                json!({}),
            )
        }
        ClientError::ToolNotFound(tool) => {
            let msg = format!("ferramenta '{tool}' não encontrada no servidor MCP");
            state.integrations.record_error(id, msg.clone());
            detail(
                StatusCode::BAD_GATEWAY,
                "integration_upstream_error",
                &msg,
                json!({}),
            )
        }
        ClientError::RemoteError { code, .. } => {
            // Não armazena nem devolve texto bruto do servidor remoto em `last_error`/`/test` para evitar eco de segredos.
            let msg = format!("erro remoto MCP ({code}) retornado pelo servidor externo");
            state.integrations.record_error(id, msg.clone());
            detail(
                StatusCode::BAD_GATEWAY,
                "integration_upstream_error",
                &msg,
                json!({}),
            )
        }
        ClientError::Registry(e) => registry_error(e),
        ClientError::Secrets(e) => secret_store_error(e),
    }
}

fn approval_error(err: ApprovalError) -> Response {
    match err {
        ApprovalError::Lost => detail(
            StatusCode::NOT_FOUND,
            "approval_not_found",
            &err.to_string(),
            json!({}),
        ),
        ApprovalError::Claimed => detail(
            StatusCode::CONFLICT,
            "approval_in_progress",
            &err.to_string(),
            json!({}),
        ),
        ApprovalError::Stale | ApprovalError::Answered => detail(
            StatusCode::CONFLICT,
            "approval_conflict",
            &err.to_string(),
            json!({}),
        ),
        ApprovalError::Invalid(reason) => detail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "erro_corpo_invalido",
            &reason,
            json!({}),
        ),
        ApprovalError::TooMany | ApprovalError::Cooldown => detail(
            StatusCode::TOO_MANY_REQUESTS,
            "approval_rate_limited",
            &err.to_string(),
            json!({}),
        ),
    }
}

fn parse_host_port(headers: &HeaderMap) -> Option<u16> {
    let raw = headers.get(header::HOST)?.to_str().ok()?.trim();
    if let Some((_, rest)) = raw.rsplit_once("]:") {
        return rest.parse::<u16>().ok().filter(|p| *p > 0);
    }
    if raw.starts_with('[') {
        return None;
    }
    raw.rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|p| *p > 0)
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_callback_html(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"pt-BR\"><head><meta charset=\"utf-8\"><title>{title} — Hangar</title>\
<style>body{{font-family:system-ui,sans-serif;max-width:520px;margin:64px auto;padding:0 20px;color:#111;line-height:1.5;}}\
h1{{font-size:1.25rem;margin-bottom:12px;}}</style></head>\
<body><h1>{title}</h1><p>{body}</p></body></html>"
    )
}

fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::escape_html;

    #[test]
    fn escape_html_neutralizes_tags_quotes_and_ampersands() {
        assert_eq!(
            escape_html("<script>\"'&</script>"),
            "&lt;script&gt;&quot;&#x27;&amp;&lt;/script&gt;"
        );
    }
}
