//! Motor OAuth 2.1 local com descoberta RFC 9728/8414, DCR, CIMD, PKCE S256 e refresh serializado.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::future::BoxFuture;
use http::Uri;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use zeroize::Zeroizing;

use hangar_core::net::http_client;

use crate::catalog::find_preset;
use crate::client::{ClientError, TokenRefresher};
use crate::model::{
    AuthConfig, IntegrationConfig, IntegrationStatus, Transport, config_fingerprint,
};
use crate::registry::{IntegrationRegistry, RegistryError};
use crate::secrets::{IntegrationSecrets, OAuthTokens, SecretStore, SecretStoreError};

/// Caminho padrão de callback OAuth atendido localmente pelo servidor do Hangar.
pub const OAUTH_CALLBACK_PATH: &str = "/api/integrations/oauth/callback";

/// Tempo padrão de validade de uma tentativa OAuth (`state`) em memória (10 minutos).
pub const DEFAULT_STATE_TTL: Duration = Duration::from_secs(600);

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_URL_BYTES: usize = 4096;
const MAX_OAUTH_RESPONSE_BYTES: usize = 256 * 1024;

/// Origem do cliente que iniciou o fluxo OAuth (define política de `redirect_uri` e handoff).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthOrigin {
    Desktop,
    RemoteDesktop,
    Mobile,
}

/// Parâmetros recebidos para iniciar um fluxo OAuth 2.1 para uma integração cadastrada.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthStartInput {
    pub origin_client: OAuthOrigin,
    pub local_port: u16,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default)]
    pub cimd_client_id_url: Option<String>,
}

/// Resultado do início do fluxo OAuth: abertura direta ou handoff para o navegador do desktop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OAuthStartOutcome {
    OpenUrl {
        authorize_url: String,
        redirect_uri: String,
    },
    HandoffDesktop {
        authorize_url: String,
        redirect_uri: String,
        reason: String,
    },
}

/// Resultado de conclusão bem-sucedida do callback OAuth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthCallbackResult {
    pub integration_id: String,
    pub status: IntegrationStatus,
}

/// Erros do motor OAuth 2.1 local (nunca carregam tokens, segredos ou `code_verifier`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthError {
    UnsupportedAuth,
    NotFound(String),
    DiscoveryFailed(String),
    RegistrationFailed(String),
    InvalidState,
    ExpiredState,
    ProviderError {
        error: String,
        description: Option<String>,
    },
    InvalidGrant(String),
    Network(String),
    Registry(RegistryError),
    Secrets(SecretStoreError),
    Validation(String),
}

/// Gerenciador local de autorização, callback e renovação de tokens OAuth 2.1.
pub struct OAuthManager {
    registry: Arc<IntegrationRegistry>,
    secrets: Arc<SecretStore>,
    http: reqwest::Client,
    no_redirect_http: reqwest::Client,
    pending: StdMutex<HashMap<String, PendingAuth>>,
    refresh_locks: StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    state_ttl: Duration,
}

/// Tentativa pendente em memória; não implementa `Debug` para proteger `code_verifier` e `client_secret`.
struct PendingAuth {
    integration_id: String,
    config_fingerprint: [u8; 32],
    resource_uri: String,
    authorize_url: String,
    code_verifier: Zeroizing<String>,
    redirect_uri: String,
    token_endpoint: String,
    client_id: String,
    client_secret: Option<Zeroizing<String>>,
    scopes: Vec<String>,
    /// Verdadeiro para `OAuthDcr` (DCR ou CIMD), persistindo `client_id` para futuros refreshes.
    persist_client_id: bool,
    /// Verdadeiro apenas para registro DCR (não CIMD), evitando reutilizar URL CIMD como DCR.
    is_dcr: bool,
    created_at: Instant,
}

struct DiscoveredMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    client_id_metadata_document_supported: bool,
    www_authenticate_scope: Option<String>,
}

#[derive(Default, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Deserialize)]
struct AuthorizationServerMetadata {
    #[serde(default)]
    issuer: Option<String>,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    client_id_metadata_document_supported: Option<bool>,
    #[serde(default)]
    code_challenge_methods_supported: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct DcrRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<f64>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct OAuthErrorPayload {
    error: String,
}

struct ParsedHttpUrl {
    scheme: String,
    origin: String,
    path: String,
}

struct ResolvedRedirect {
    redirect_uri: String,
    handoff_reason: Option<String>,
}

struct ClientCredentials {
    client_id: String,
    client_secret: Option<Zeroizing<String>>,
    scopes: Vec<String>,
    persist_client_id: bool,
    is_dcr: bool,
}

impl OAuthManager {
    /// Cria um novo gerenciador OAuth ligado ao catálogo e ao repositório cifrado de segredos.
    pub fn new(registry: Arc<IntegrationRegistry>, secrets: Arc<SecretStore>) -> Self {
        Self {
            registry,
            secrets,
            http: build_http_client(reqwest::redirect::Policy::limited(5)),
            no_redirect_http: build_http_client(reqwest::redirect::Policy::none()),
            pending: StdMutex::new(HashMap::new()),
            refresh_locks: StdMutex::new(HashMap::new()),
            state_ttl: DEFAULT_STATE_TTL,
        }
    }

    /// Ajusta o TTL de tentativas pendentes de `state` (útil em testes de expiração).
    pub fn with_state_ttl(mut self, state_ttl: Duration) -> Self {
        self.state_ttl = state_ttl;
        self
    }

    /// Verifica se existe tentativa pendente não expirada para `integration_id` com esta `authorize_url`.
    pub fn is_pending_authorize_url(&self, integration_id: &str, authorize_url: &str) -> bool {
        let mut guard = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let ttl = self.state_ttl;
        guard.retain(|_, pending| pending.created_at.elapsed() <= ttl);
        guard.values().any(|pending| {
            pending.integration_id == integration_id && pending.authorize_url == authorize_url
        })
    }

    /// Retorna a `authorize_url` pendente e não expirada mais recente de `integration_id`.
    pub fn latest_pending_authorize_url(&self, integration_id: &str) -> Option<String> {
        let mut guard = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let ttl = self.state_ttl;
        guard.retain(|_, pending| pending.created_at.elapsed() <= ttl);
        guard
            .values()
            .filter(|pending| pending.integration_id == integration_id)
            .max_by_key(|pending| pending.created_at)
            .map(|pending| pending.authorize_url.clone())
    }

    /// Cancela todas as tentativas OAuth pendentes vinculadas a `integration_id`.
    pub fn cancel_pending_for(&self, integration_id: &str) {
        let mut guard = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        guard.retain(|_, pending| pending.integration_id != integration_id);
    }

    /// Inicia o fluxo OAuth 2.1 para `integration_id`, descobrindo endpoints e montando a URL.
    ///
    /// # Errors
    ///
    /// Retorna [`OAuthError`] se a integração não existir, não usar transporte HTTP com OAuth,
    /// falhar na descoberta RFC 9728/8414 ou no registro dinâmico (DCR), ou receber entrada inválida.
    pub async fn start(
        &self,
        integration_id: &str,
        input: OAuthStartInput,
    ) -> Result<OAuthStartOutcome, OAuthError> {
        let config = self
            .registry
            .get(integration_id)?
            .ok_or_else(|| OAuthError::NotFound(integration_id.to_owned()))?;
        let mcp_url = match &config.transport {
            Transport::RemoteHttp { url, .. } => url.as_str(),
            Transport::LocalStdio { .. } => return Err(OAuthError::UnsupportedAuth),
        };
        if !matches!(
            config.auth,
            AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. }
        ) {
            return Err(OAuthError::UnsupportedAuth);
        }

        let config_fp = config_fingerprint(&config);
        let resource_uri = canonical_resource_uri(mcp_url, false)?;
        let redirect = resolve_redirect(&config, &input)?;
        let metadata = self.discover_metadata(mcp_url).await?;
        let existing_secrets = self.secrets.get(integration_id).await?;
        let creds = self
            .resolve_client_credentials(
                &config,
                existing_secrets.as_ref(),
                &metadata,
                &input,
                &redirect.redirect_uri,
            )
            .await?;

        let code_verifier = Zeroizing::new(random_base64url_32()?);
        let code_challenge = pkce_s256_challenge(&code_verifier);
        let state = random_base64url_32()?;
        let scope_str = (!creds.scopes.is_empty()).then(|| creds.scopes.join(" "));

        let mut query_params: Vec<(&str, &str)> = vec![
            ("response_type", "code"),
            ("client_id", &creds.client_id),
            ("redirect_uri", &redirect.redirect_uri),
            ("state", &state),
            ("code_challenge", &code_challenge),
            ("code_challenge_method", "S256"),
            ("resource", &resource_uri),
        ];
        if let Some(scope) = scope_str.as_deref() {
            query_params.push(("scope", scope));
        }
        if is_google_accounts_url(&metadata.authorization_endpoint) {
            query_params.push(("access_type", "offline"));
            query_params.push(("prompt", "consent"));
        }
        let authorize_url = build_authorize_url(&metadata.authorization_endpoint, &query_params);

        let entry = PendingAuth {
            integration_id: integration_id.to_owned(),
            config_fingerprint: config_fp,
            resource_uri,
            authorize_url: authorize_url.clone(),
            code_verifier,
            redirect_uri: redirect.redirect_uri.clone(),
            token_endpoint: metadata.token_endpoint,
            client_id: creds.client_id,
            client_secret: creds.client_secret,
            scopes: creds.scopes,
            persist_client_id: creds.persist_client_id,
            is_dcr: creds.is_dcr,
            created_at: Instant::now(),
        };
        {
            let mut guard = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            let ttl = self.state_ttl;
            guard.retain(|_, pending| pending.created_at.elapsed() <= ttl);
            guard.insert(state, entry);
        }

        match redirect.handoff_reason {
            Some(reason) => Ok(OAuthStartOutcome::HandoffDesktop {
                authorize_url,
                redirect_uri: redirect.redirect_uri,
                reason,
            }),
            None => Ok(OAuthStartOutcome::OpenUrl {
                authorize_url,
                redirect_uri: redirect.redirect_uri,
            }),
        }
    }

    /// Conclui o callback OAuth validando `state` de uso único e trocando `code` por tokens.
    ///
    /// # Errors
    ///
    /// Retorna [`OAuthError::InvalidState`] para `state` desconhecido ou já utilizado,
    /// [`OAuthError::ExpiredState`] se o `state` tiver expirado, [`OAuthError::NotFound`] ou
    /// [`OAuthError::UnsupportedAuth`] se a integração foi removida ou deixou de usar OAuth,
    /// [`OAuthError::ProviderError`] se o provedor retornou erro, ou falhas de troca de código/armazenamento.
    pub async fn handle_callback(
        &self,
        state: &str,
        code: Option<&str>,
        error: Option<&str>,
        _error_description: Option<&str>,
    ) -> Result<OAuthCallbackResult, OAuthError> {
        if state.trim().is_empty() {
            return Err(OAuthError::InvalidState);
        }
        // Remove o `state` atomicamente na primeira leitura para impedir replay concorrente.
        let pending = {
            let mut guard = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = guard.remove(state);
            let ttl = self.state_ttl;
            guard.retain(|_, item| item.created_at.elapsed() <= ttl);
            entry
        };
        let Some(entry) = pending else {
            return Err(OAuthError::InvalidState);
        };
        if entry.created_at.elapsed() > self.state_ttl {
            return Err(OAuthError::ExpiredState);
        }

        if let Some(err_code) = error.map(str::trim).filter(|e| !e.is_empty()) {
            // Nunca ecoa `error_description` nem códigos desconhecidos recebidos no callback.
            let (safe_code, safe_msg) = map_oauth_error_code(err_code, None);
            return Err(OAuthError::ProviderError {
                error: safe_code,
                description: Some(safe_msg),
            });
        }

        let auth_code = code
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| OAuthError::Validation("código de autorização ausente".into()))?;

        // Evita trocar o código se a integração já foi removida ou alterada desde o início do login.
        self.ensure_oauth_integration(&entry.integration_id, entry.config_fingerprint)?;

        let mut form_params: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", auth_code),
            ("redirect_uri", &entry.redirect_uri),
            ("client_id", &entry.client_id),
            ("code_verifier", &entry.code_verifier),
            ("resource", &entry.resource_uri),
        ];
        if let Some(secret) = entry.client_secret.as_ref() {
            form_params.push(("client_secret", secret.as_str()));
        }

        let token = self
            .post_token_request(&entry.token_endpoint, &form_params, false)
            .await?;

        let expires_at = compute_expires_at(token.expires_in);
        let fallback_scope = (!entry.scopes.is_empty()).then(|| entry.scopes.join(" "));
        let scope = token
            .scope
            .filter(|s| !s.trim().is_empty())
            .or(fallback_scope);
        let integration_id = entry.integration_id.clone();
        let expected_fp = entry.config_fingerprint;
        let token_endpoint = entry.token_endpoint;
        let persist_client_id = entry.persist_client_id;
        let is_dcr = entry.is_dcr;
        let client_id = entry.client_id;
        let client_secret = entry.client_secret;
        let redirect_uri = entry.redirect_uri;

        // Revalida antes e sob a escrita de segredos para impedir gravação órfã ou em destino alterado.
        self.ensure_oauth_integration(&integration_id, expected_fp)?;

        let registry = Arc::clone(&self.registry);
        let check_id = integration_id.clone();
        let persist_res = self
            .secrets
            .update_with(&integration_id, move |slot| {
                let current_cfg = match registry.get(&check_id) {
                    Ok(Some(cfg)) => cfg,
                    Ok(None) => return Ok(Err(OAuthError::NotFound(check_id))),
                    Err(err) => return Ok(Err(OAuthError::Registry(err))),
                };
                if let Err(err) = ensure_oauth_config_matches(&current_cfg, expected_fp) {
                    return Ok(Err(err));
                }
                let secrets = slot.get_or_insert_with(IntegrationSecrets::default);
                secrets.oauth = Some(OAuthTokens {
                    access_token: Zeroizing::new(token.access_token),
                    refresh_token: token
                        .refresh_token
                        .filter(|rt| !rt.trim().is_empty())
                        .map(Zeroizing::new),
                    expires_at,
                    token_type: token.token_type,
                    scope,
                    registered_client_id: persist_client_id.then_some(client_id),
                    registered_client_secret: if is_dcr { client_secret } else { None },
                    registered_redirect_uri: is_dcr.then_some(redirect_uri),
                    token_endpoint: Some(token_endpoint),
                    auth_expired: false,
                });
                Ok(Ok(()))
            })
            .await?;
        persist_res?;

        Ok(OAuthCallbackResult {
            integration_id,
            status: IntegrationStatus::Ready,
        })
    }

    /// Renova o `access_token` de `integration_id` sob trava exclusiva por integração.
    ///
    /// # Errors
    ///
    /// Retorna [`OAuthError::InvalidGrant`] quando não há `refresh_token` ou o servidor rejeita o
    /// refresh (marcando `auth_expired = true` sem apagar a configuração), ou erros de rede/registro.
    pub async fn refresh_access_token(&self, integration_id: &str) -> Result<String, OAuthError> {
        let lock = self.refresh_lock_for(integration_id);
        let _guard = lock.lock().await;

        let config = self
            .registry
            .get(integration_id)?
            .ok_or_else(|| OAuthError::NotFound(integration_id.to_owned()))?;
        let mcp_url = match &config.transport {
            Transport::RemoteHttp { url, .. } => url.as_str(),
            Transport::LocalStdio { .. } => return Err(OAuthError::UnsupportedAuth),
        };
        if !matches!(
            config.auth,
            AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. }
        ) {
            return Err(OAuthError::UnsupportedAuth);
        }

        let initial_fp = config_fingerprint(&config);
        let resource_uri = canonical_resource_uri(mcp_url, false)?;
        let mcp_is_https = parse_http_url(mcp_url, false)?.scheme == "https";
        let existing_secrets = self.secrets.get(integration_id).await?;
        let Some(oauth) = existing_secrets.as_ref().and_then(|s| s.oauth.as_ref()) else {
            return Err(OAuthError::InvalidGrant(
                "credenciais OAuth não configuradas".into(),
            ));
        };

        let expected_access = oauth.access_token.clone();
        let expected_refresh = oauth.refresh_token.clone();

        let (client_id, client_secret) = match &config.auth {
            AuthConfig::None | AuthConfig::Bearer => return Err(OAuthError::UnsupportedAuth),
            AuthConfig::OAuthManualClient { client_id, .. } => {
                let cid = client_id.trim();
                if cid.is_empty() {
                    return Err(OAuthError::Validation(
                        "client_id manual não pode estar vazio".into(),
                    ));
                }
                (
                    cid.to_owned(),
                    existing_secrets
                        .as_ref()
                        .and_then(|s| s.client_secret.clone())
                        .filter(|v| !v.trim().is_empty()),
                )
            }
            AuthConfig::OAuthDcr => {
                let Some(cid) = oauth
                    .registered_client_id
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                else {
                    self.mark_auth_expired_if_unchanged(
                        integration_id,
                        &expected_access,
                        expected_refresh.as_ref(),
                    )
                    .await?;
                    return Err(OAuthError::InvalidGrant(
                        "client_id OAuth ausente; reconecte a integração".into(),
                    ));
                };
                let secret = oauth
                    .registered_client_secret
                    .clone()
                    .or_else(|| {
                        existing_secrets
                            .as_ref()
                            .and_then(|s| s.client_secret.clone())
                    })
                    .filter(|v| !v.trim().is_empty());
                (cid.to_owned(), secret)
            }
        };

        let Some(refresh_token) = oauth
            .refresh_token
            .as_ref()
            .filter(|rt| !rt.trim().is_empty())
            .cloned()
        else {
            self.mark_auth_expired_if_unchanged(
                integration_id,
                &expected_access,
                expected_refresh.as_ref(),
            )
            .await?;
            return Err(OAuthError::InvalidGrant(
                "sem refresh_token disponível".into(),
            ));
        };

        let token_endpoint = match oauth.token_endpoint.as_deref() {
            Some(ep) if parse_http_url(ep, mcp_is_https).is_ok() => ep.to_owned(),
            _ => self.discover_metadata(mcp_url).await?.token_endpoint,
        };

        let mut form_params: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", &client_id),
            ("resource", resource_uri.as_str()),
        ];
        if let Some(secret) = client_secret.as_ref() {
            form_params.push(("client_secret", secret.as_str()));
        }

        let token = match self
            .post_token_request(&token_endpoint, &form_params, true)
            .await
        {
            Ok(tok) => tok,
            Err(OAuthError::InvalidGrant(reason)) => {
                // Só marca a integração como expirada se ela ainda apontar para a mesma configuração.
                if self
                    .ensure_oauth_integration(integration_id, initial_fp)
                    .is_ok()
                {
                    self.mark_auth_expired_if_unchanged(
                        integration_id,
                        &expected_access,
                        expected_refresh.as_ref(),
                    )
                    .await?;
                }
                return Err(OAuthError::InvalidGrant(reason));
            }
            Err(err) => return Err(err),
        };

        self.ensure_oauth_integration(integration_id, initial_fp)?;

        let fresh_access = token.access_token;
        let fresh_refresh = token
            .refresh_token
            .filter(|rt| !rt.trim().is_empty())
            .map(Zeroizing::new);
        let new_expires_at = compute_expires_at(token.expires_in);
        let new_token_type = token.token_type;
        let new_scope = token.scope;

        let registry = Arc::clone(&self.registry);
        let check_id = integration_id.to_owned();
        let returned_access = self
            .secrets
            .update_with(integration_id, move |slot| {
                let current_cfg = match registry.get(&check_id) {
                    Ok(Some(cfg)) => cfg,
                    Ok(None) => return Ok(Err(OAuthError::NotFound(check_id))),
                    Err(err) => return Ok(Err(OAuthError::Registry(err))),
                };
                if let Err(err) = ensure_oauth_config_matches(&current_cfg, initial_fp) {
                    return Ok(Err(err));
                }
                let Some(secrets) = slot.as_mut() else {
                    return Ok(Ok(fresh_access));
                };
                let current_oauth = secrets.oauth.get_or_insert_with(OAuthTokens::default);
                // Reconexão manual paralela via `handle_callback` vence o refresh antigo em voo.
                if !oauth_tokens_match(current_oauth, &expected_access, expected_refresh.as_ref()) {
                    if !current_oauth.access_token.trim().is_empty() {
                        return Ok(Ok(current_oauth.access_token.as_str().to_owned()));
                    }
                    return Ok(Ok(fresh_access));
                }
                current_oauth.access_token = Zeroizing::new(fresh_access.clone());
                // Preserva o refresh_token atual quando o servidor não rotaciona na resposta.
                if let Some(rotated) = fresh_refresh {
                    current_oauth.refresh_token = Some(rotated);
                }
                current_oauth.expires_at = new_expires_at;
                if let Some(tt) = new_token_type {
                    current_oauth.token_type = Some(tt);
                }
                if let Some(sc) = new_scope {
                    current_oauth.scope = Some(sc);
                }
                current_oauth.token_endpoint = Some(token_endpoint);
                current_oauth.auth_expired = false;
                Ok(Ok(fresh_access))
            })
            .await??;

        Ok(returned_access)
    }

    fn refresh_lock_for(&self, integration_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        locks
            .entry(integration_id.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    fn ensure_oauth_integration(
        &self,
        integration_id: &str,
        expected_fingerprint: [u8; 32],
    ) -> Result<(), OAuthError> {
        let config = self
            .registry
            .get(integration_id)?
            .ok_or_else(|| OAuthError::NotFound(integration_id.to_owned()))?;
        ensure_oauth_config_matches(&config, expected_fingerprint)
    }

    async fn mark_auth_expired_if_unchanged(
        &self,
        integration_id: &str,
        expected_access: &Zeroizing<String>,
        expected_refresh: Option<&Zeroizing<String>>,
    ) -> Result<bool, OAuthError> {
        let expected_access = expected_access.clone();
        let expected_refresh = expected_refresh.cloned();
        let marked = self
            .secrets
            .update_with(integration_id, move |slot| {
                if let Some(secrets) = slot.as_mut()
                    && let Some(oauth) = secrets.oauth.as_mut()
                {
                    if !oauth_tokens_match(oauth, &expected_access, expected_refresh.as_ref()) {
                        return Ok(false);
                    }
                    oauth.auth_expired = true;
                    return Ok(true);
                }
                Ok(false)
            })
            .await?;
        Ok(marked)
    }

    async fn discover_metadata(&self, mcp_url: &str) -> Result<DiscoveredMetadata, OAuthError> {
        let parsed_mcp = parse_http_url(mcp_url, false)?;
        let require_https = parsed_mcp.scheme == "https";
        let expected_resource = parsed_mcp.canonical_uri();

        let (prm_hint_url, www_scope) = self.probe_www_authenticate(mcp_url, require_https).await?;
        let mut prm_candidates = Vec::new();
        if let Some(hint) = prm_hint_url {
            prm_candidates.push(hint);
        }
        if !parsed_mcp.path.is_empty() {
            prm_candidates.push(format!(
                "{}/.well-known/oauth-protected-resource{}",
                parsed_mcp.origin, parsed_mcp.path
            ));
        }
        prm_candidates.push(format!(
            "{}/.well-known/oauth-protected-resource",
            parsed_mcp.origin
        ));

        let mut discovered_issuer = None;
        for candidate in prm_candidates {
            if let Some(prm) = self
                .fetch_protected_resource_metadata(&candidate, &expected_resource, require_https)
                .await?
                && let Some(first_server) = prm
                    .authorization_servers
                    .into_iter()
                    .find(|s| !s.trim().is_empty())
            {
                validate_discovered_url(
                    "authorization_server em metadados do recurso",
                    &first_server,
                    require_https,
                )?;
                discovered_issuer = Some(first_server);
                break;
            }
        }

        let issuer_url = discovered_issuer.unwrap_or_else(|| parsed_mcp.origin.clone());
        let parsed_issuer = validate_discovered_url("issuer", &issuer_url, require_https)?;
        let expected_issuer = parsed_issuer.canonical_uri();
        let as_candidates = build_auth_server_metadata_urls(&parsed_issuer);

        for candidate in as_candidates {
            if let Some(as_meta) = self.fetch_auth_server_metadata(&candidate).await? {
                validate_discovered_as_metadata(&as_meta, &expected_issuer, require_https)?;
                return Ok(DiscoveredMetadata {
                    authorization_endpoint: as_meta.authorization_endpoint,
                    token_endpoint: as_meta.token_endpoint,
                    registration_endpoint: as_meta.registration_endpoint,
                    client_id_metadata_document_supported: as_meta
                        .client_id_metadata_document_supported
                        .unwrap_or(false),
                    www_authenticate_scope: www_scope,
                });
            }
        }

        Err(OAuthError::DiscoveryFailed(
            "não foi possível descobrir authorization_endpoint e token_endpoint (RFC 9728/8414)"
                .into(),
        ))
    }

    async fn probe_www_authenticate(
        &self,
        mcp_url: &str,
        require_https: bool,
    ) -> Result<(Option<String>, Option<String>), OAuthError> {
        let post_resp = self
            .http
            .post(mcp_url)
            .timeout(PROBE_HTTP_TIMEOUT)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .await;

        let resp = match post_resp {
            Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => Some(resp),
            Ok(resp) if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED => self
                .http
                .get(mcp_url)
                .timeout(PROBE_HTTP_TIMEOUT)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
                .ok(),
            Ok(resp) => Some(resp),
            Err(_) => None,
        };

        let Some(resp) = resp else {
            return Ok((None, None));
        };
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok((None, None));
        }
        for val in resp.headers().get_all(reqwest::header::WWW_AUTHENTICATE) {
            if let Ok(raw) = val.to_str() {
                let (meta_url, scope) = parse_www_authenticate_bearer(raw);
                if let Some(url) = meta_url {
                    validate_discovered_url(
                        "resource_metadata em WWW-Authenticate",
                        &url,
                        require_https,
                    )?;
                    return Ok((Some(url), scope));
                }
                if scope.is_some() {
                    return Ok((None, scope));
                }
            }
        }
        Ok((None, None))
    }

    async fn fetch_protected_resource_metadata(
        &self,
        url: &str,
        expected_resource: &str,
        require_https: bool,
    ) -> Result<Option<ProtectedResourceMetadata>, OAuthError> {
        let Ok(resp) = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
        else {
            return Ok(None);
        };
        if !resp.status().is_success() {
            return Ok(None);
        }
        let bytes = read_bounded_body(resp, MAX_OAUTH_RESPONSE_BYTES)
            .await
            .map_err(|reason| {
                OAuthError::DiscoveryFailed(format!(
                    "falha ao ler metadados do recurso protegido: {reason}"
                ))
            })?;
        let prm = serde_json::from_slice::<ProtectedResourceMetadata>(&bytes).map_err(|_| {
            OAuthError::DiscoveryFailed("JSON de metadados do recurso protegido inválido".into())
        })?;
        // RFC 9728 exige que o documento identifique exatamente o recurso consultado antes de confiar em authorization_servers.
        if !prm_resource_matches(prm.resource.as_deref(), expected_resource, require_https) {
            return Err(OAuthError::DiscoveryFailed(
                "campo resource nos metadados do recurso ausente ou divergente".into(),
            ));
        }
        Ok(Some(prm))
    }

    async fn fetch_auth_server_metadata(
        &self,
        url: &str,
    ) -> Result<Option<AuthorizationServerMetadata>, OAuthError> {
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|err| {
                OAuthError::DiscoveryFailed(format!("falha ao consultar metadados OAuth: {err}"))
            })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND
            || resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(OAuthError::DiscoveryFailed(format!(
                "endpoint de metadados OAuth retornou HTTP {}",
                resp.status().as_u16()
            )));
        }
        let bytes = read_bounded_body(resp, MAX_OAUTH_RESPONSE_BYTES)
            .await
            .map_err(|reason| {
                OAuthError::DiscoveryFailed(format!("falha ao ler metadados OAuth: {reason}"))
            })?;
        let parsed =
            serde_json::from_slice::<AuthorizationServerMetadata>(&bytes).map_err(|_| {
                OAuthError::DiscoveryFailed("JSON de metadados do servidor OAuth inválido".into())
            })?;
        Ok(Some(parsed))
    }

    async fn resolve_client_credentials(
        &self,
        config: &IntegrationConfig,
        existing_secrets: Option<&IntegrationSecrets>,
        metadata: &DiscoveredMetadata,
        input: &OAuthStartInput,
        redirect_uri: &str,
    ) -> Result<ClientCredentials, OAuthError> {
        let preset_scopes = config
            .preset
            .as_deref()
            .and_then(find_preset)
            .map(|p| p.default_scopes.clone())
            .unwrap_or_default();

        match &config.auth {
            AuthConfig::OAuthManualClient { client_id, scopes } => {
                let cid = client_id.trim();
                if cid.is_empty() {
                    return Err(OAuthError::Validation(
                        "client_id manual não pode estar vazio".into(),
                    ));
                }
                let secret = existing_secrets
                    .and_then(|s| s.client_secret.clone())
                    .filter(|v| !v.trim().is_empty());
                let effective_scopes = if !scopes.is_empty() {
                    scopes.clone()
                } else {
                    preset_scopes
                };
                Ok(ClientCredentials {
                    client_id: cid.to_owned(),
                    client_secret: secret,
                    scopes: effective_scopes,
                    persist_client_id: false,
                    is_dcr: false,
                })
            }
            AuthConfig::OAuthDcr => {
                let effective_scopes = if !preset_scopes.is_empty() {
                    preset_scopes
                } else if let Some(scope) = &metadata.www_authenticate_scope {
                    scope.split_whitespace().map(str::to_owned).collect()
                } else {
                    vec![]
                };

                if metadata.client_id_metadata_document_supported
                    && let Some(raw_cimd) = input
                        .cimd_client_id_url
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                {
                    parse_http_url(raw_cimd, true).map_err(|err| {
                        OAuthError::Validation(format!("cimd_client_id_url inválida: {err}"))
                    })?;
                    return Ok(ClientCredentials {
                        client_id: raw_cimd.to_owned(),
                        client_secret: None,
                        scopes: effective_scopes,
                        persist_client_id: true,
                        is_dcr: false,
                    });
                }

                if let Some(oauth) = existing_secrets.and_then(|s| s.oauth.as_ref())
                    && !oauth.auth_expired
                    && let Some(saved_cid) = oauth
                        .registered_client_id
                        .as_deref()
                        .filter(|cid| !cid.trim().is_empty() && !cid.starts_with("https://"))
                {
                    let same_endpoint =
                        oauth.token_endpoint.as_deref() == Some(metadata.token_endpoint.as_str());
                    let compatible_redirect = oauth
                        .registered_redirect_uri
                        .as_deref()
                        .is_none_or(|saved| saved == redirect_uri);
                    if same_endpoint && compatible_redirect {
                        let secret = oauth
                            .registered_client_secret
                            .clone()
                            .or_else(|| existing_secrets.and_then(|s| s.client_secret.clone()))
                            .filter(|v| !v.trim().is_empty());
                        return Ok(ClientCredentials {
                            client_id: saved_cid.to_owned(),
                            client_secret: secret,
                            scopes: effective_scopes,
                            persist_client_id: true,
                            is_dcr: true,
                        });
                    }
                }

                let Some(reg_endpoint) = metadata.registration_endpoint.as_deref() else {
                    return Err(OAuthError::RegistrationFailed(
                        "servidor OAuth não suporta registro dinâmico (DCR) nem CIMD; use oauth_manual_client"
                            .into(),
                    ));
                };

                let (client_id, client_secret) =
                    self.register_dcr_client(reg_endpoint, redirect_uri).await?;
                Ok(ClientCredentials {
                    client_id,
                    client_secret,
                    scopes: effective_scopes,
                    persist_client_id: true,
                    is_dcr: true,
                })
            }
            AuthConfig::None | AuthConfig::Bearer => Err(OAuthError::UnsupportedAuth),
        }
    }

    async fn register_dcr_client(
        &self,
        registration_endpoint: &str,
        redirect_uri: &str,
    ) -> Result<(String, Option<Zeroizing<String>>), OAuthError> {
        let payload = serde_json::json!({
            "client_name": "Hangar",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        });
        let body = serde_json::to_vec(&payload)
            .map_err(|err| OAuthError::RegistrationFailed(format!("serialização DCR: {err}")))?;

        let resp = self
            .no_redirect_http
            .post(registration_endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|err| {
                OAuthError::RegistrationFailed(format!("falha ao registrar cliente DCR: {err}"))
            })?;

        let status = resp.status();
        let bytes = read_bounded_body(resp, MAX_OAUTH_RESPONSE_BYTES)
            .await
            .map_err(|reason| {
                OAuthError::RegistrationFailed(format!("falha ao ler resposta DCR: {reason}"))
            })?;
        if !status.is_success() {
            let reason = extract_safe_oauth_error(&bytes, status.as_u16())
                .map(|(err, msg)| format!("HTTP {} ({err}: {msg})", status.as_u16()))
                .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
            return Err(OAuthError::RegistrationFailed(reason));
        }

        let reg: DcrRegistrationResponse = serde_json::from_slice(&bytes).map_err(|_| {
            OAuthError::RegistrationFailed("resposta JSON de registro DCR inválida".into())
        })?;
        if reg.client_id.trim().is_empty() {
            return Err(OAuthError::RegistrationFailed(
                "servidor DCR retornou client_id vazio".into(),
            ));
        }
        let secret = reg
            .client_secret
            .filter(|s| !s.trim().is_empty())
            .map(Zeroizing::new);
        Ok((reg.client_id, secret))
    }

    async fn post_token_request(
        &self,
        token_endpoint: &str,
        params: &[(&str, &str)],
        treat_invalid_grant_on_error: bool,
    ) -> Result<TokenResponse, OAuthError> {
        let body = encode_form_body(params);

        let resp = self
            .no_redirect_http
            .post(token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body.as_str().to_owned())
            .send()
            .await
            .map_err(|err| OAuthError::Network(format!("requisição de token falhou: {err}")))?;

        let status = resp.status();
        let bytes = read_bounded_body(resp, MAX_OAUTH_RESPONSE_BYTES)
            .await
            .map_err(|reason| OAuthError::Network(format!("falha ao ler token: {reason}")))?;

        if !status.is_success() {
            if let Some((err_code, safe_msg)) = extract_safe_oauth_error(&bytes, status.as_u16()) {
                if (status == reqwest::StatusCode::BAD_REQUEST
                    || status == reqwest::StatusCode::UNAUTHORIZED)
                    && matches!(
                        err_code.as_str(),
                        "invalid_grant"
                            | "invalid_token"
                            | "unauthorized_client"
                            | "invalid_client"
                    )
                    && (treat_invalid_grant_on_error || err_code == "invalid_grant")
                {
                    return Err(OAuthError::InvalidGrant(format!("{err_code}: {safe_msg}")));
                }
                return Err(OAuthError::ProviderError {
                    error: err_code,
                    description: Some(safe_msg),
                });
            }
            return Err(OAuthError::Network(format!(
                "endpoint de token retornou HTTP {}",
                status.as_u16()
            )));
        }

        let parsed: TokenResponse = serde_json::from_slice(&bytes).map_err(|_| {
            OAuthError::Network("resposta JSON do endpoint de token inválida".into())
        })?;
        if parsed.access_token.trim().is_empty() {
            return Err(OAuthError::Network(
                "servidor OAuth retornou access_token vazio".into(),
            ));
        }
        Ok(parsed)
    }
}

impl TokenRefresher for OAuthManager {
    fn refresh<'a>(
        &'a self,
        integration_id: &'a str,
    ) -> BoxFuture<'a, Result<String, ClientError>> {
        Box::pin(async move {
            self.refresh_access_token(integration_id)
                .await
                .map_err(|err| match err {
                    OAuthError::InvalidGrant(msg)
                    | OAuthError::NotFound(msg)
                    | OAuthError::Validation(msg) => ClientError::AuthExpired(msg),
                    OAuthError::UnsupportedAuth => {
                        ClientError::AuthExpired("integração não usa autenticação OAuth".into())
                    }
                    OAuthError::ProviderError { error, description } => {
                        let (safe_code, safe_msg) = sanitize_provider_error(&error, description.as_deref());
                        ClientError::AuthExpired(format!("{safe_code}: {safe_msg}"))
                    }
                    OAuthError::InvalidState | OAuthError::ExpiredState => {
                        ClientError::AuthExpired("estado OAuth inválido".into())
                    }
                    OAuthError::Network(msg)
                    | OAuthError::DiscoveryFailed(msg)
                    | OAuthError::RegistrationFailed(msg) => ClientError::Transport(msg),
                    OAuthError::Registry(err) => ClientError::Registry(err),
                    OAuthError::Secrets(err) => ClientError::Secrets(err),
                })
        })
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedAuth => write!(f, "integração não usa autenticação OAuth"),
            Self::NotFound(id) => write!(f, "integração '{id}' não encontrada"),
            Self::DiscoveryFailed(reason) => write!(f, "falha na descoberta OAuth: {reason}"),
            Self::RegistrationFailed(reason) => write!(f, "falha no registro OAuth: {reason}"),
            Self::InvalidState => write!(f, "parâmetro state OAuth inválido ou já utilizado"),
            Self::ExpiredState => write!(f, "tentativa de login OAuth expirada"),
            Self::ProviderError { error, description } => {
                let (safe_code, safe_msg) = sanitize_provider_error(error, description.as_deref());
                write!(f, "erro do provedor OAuth ({safe_code}): {safe_msg}")
            }
            Self::InvalidGrant(reason) => write!(f, "sessão OAuth expirada: {reason}"),
            Self::Network(reason) => write!(f, "erro de comunicação OAuth: {reason}"),
            Self::Registry(err) => write!(f, "{err}"),
            Self::Secrets(err) => write!(f, "{err}"),
            Self::Validation(reason) => write!(f, "dados OAuth inválidos: {reason}"),
        }
    }
}

impl std::error::Error for OAuthError {}

impl From<RegistryError> for OAuthError {
    fn from(err: RegistryError) -> Self {
        Self::Registry(err)
    }
}

impl From<SecretStoreError> for OAuthError {
    fn from(err: SecretStoreError) -> Self {
        Self::Secrets(err)
    }
}

/// Calcula o status operacional público da integração sem expor segredos.
pub fn compute_status(
    config: &IntegrationConfig,
    secrets: Option<&IntegrationSecrets>,
) -> IntegrationStatus {
    if !config.enabled {
        return IntegrationStatus::Disabled;
    }
    match &config.auth {
        AuthConfig::None => IntegrationStatus::Ready,
        AuthConfig::Bearer => {
            let has_token = secrets
                .and_then(|s| s.bearer_token.as_ref())
                .is_some_and(|tok| !tok.trim().is_empty());
            if has_token {
                IntegrationStatus::Ready
            } else {
                IntegrationStatus::AuthRequired
            }
        }
        AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. } => {
            let Some(oauth) = secrets.and_then(|s| s.oauth.as_ref()) else {
                return IntegrationStatus::AuthRequired;
            };
            if oauth.auth_expired {
                IntegrationStatus::AuthExpired
            } else if oauth.access_token.trim().is_empty() {
                IntegrationStatus::AuthRequired
            } else {
                IntegrationStatus::Ready
            }
        }
    }
}

impl ParsedHttpUrl {
    fn canonical_uri(&self) -> String {
        format!("{}{}", self.origin, self.path)
    }
}

fn ensure_oauth_config_matches(
    config: &IntegrationConfig,
    expected_fingerprint: [u8; 32],
) -> Result<(), OAuthError> {
    if !matches!(config.transport, Transport::RemoteHttp { .. })
        || !matches!(
            config.auth,
            AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. }
        )
    {
        return Err(OAuthError::UnsupportedAuth);
    }
    if config_fingerprint(config) != expected_fingerprint {
        return Err(OAuthError::Validation(
            "configuração da integração foi alterada durante o login OAuth; inicie a conexão novamente"
                .into(),
        ));
    }
    Ok(())
}

fn build_http_client(redirect_policy: reqwest::redirect::Policy) -> reqwest::Client {
    http_client()
        .timeout(DEFAULT_HTTP_TIMEOUT)
        .redirect(redirect_policy)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn resolve_redirect(
    config: &IntegrationConfig,
    input: &OAuthStartInput,
) -> Result<ResolvedRedirect, OAuthError> {
    if input.local_port == 0 {
        return Err(OAuthError::Validation(
            "porta local inválida para callback OAuth".into(),
        ));
    }
    let loopback_uri = format!("http://127.0.0.1:{}{OAUTH_CALLBACK_PATH}", input.local_port);

    let requires_loopback = config
        .preset
        .as_deref()
        .and_then(find_preset)
        .is_some_and(|preset| preset.requires_loopback_redirect);

    match input.origin_client {
        OAuthOrigin::Desktop => Ok(ResolvedRedirect {
            redirect_uri: loopback_uri,
            handoff_reason: None,
        }),
        OAuthOrigin::Mobile | OAuthOrigin::RemoteDesktop => {
            if requires_loopback {
                return Ok(ResolvedRedirect {
                    redirect_uri: loopback_uri,
                    handoff_reason: Some(
                        "este provedor exige redirecionamento em localhost (127.0.0.1); conclua o login no navegador do computador servidor"
                            .into(),
                    ),
                });
            }
            let Some(raw_base) = input
                .public_base_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
            else {
                return Ok(ResolvedRedirect {
                    redirect_uri: loopback_uri,
                    handoff_reason: Some(
                        "endereço HTTPS público não configurado; conclua o login no navegador do computador servidor"
                            .into(),
                    ),
                });
            };
            let trimmed = raw_base.trim_end_matches('/');
            let Ok(parsed) = parse_http_url(trimmed, true) else {
                return Ok(ResolvedRedirect {
                    redirect_uri: loopback_uri,
                    handoff_reason: Some(
                        "o callback remoto exige URL HTTPS pública válida; conclua o login no navegador do computador servidor"
                            .into(),
                    ),
                });
            };
            let base = format!("{}{}", parsed.origin, parsed.path);
            Ok(ResolvedRedirect {
                redirect_uri: format!("{base}{OAUTH_CALLBACK_PATH}"),
                handoff_reason: None,
            })
        }
    }
}

fn canonical_resource_uri(raw: &str, require_https: bool) -> Result<String, OAuthError> {
    Ok(parse_http_url(raw, require_https)?.canonical_uri())
}

fn normalize_authority(raw_authority: &str, scheme: &str) -> String {
    let default_port = if scheme == "https" { "443" } else { "80" };
    let lower = raw_authority.to_ascii_lowercase();
    if let Some(after_bracket) = lower.strip_prefix('[')
        && let Some((ipv6, tail)) = after_bracket.split_once(']')
    {
        if tail
            .strip_prefix(':')
            .is_some_and(|port| port == default_port)
        {
            return format!("[{ipv6}]");
        }
        return lower;
    }
    if let Some((host, port)) = lower.rsplit_once(':')
        && port == default_port
    {
        return host.to_owned();
    }
    lower
}

fn parse_http_url(raw: &str, require_https: bool) -> Result<ParsedHttpUrl, OAuthError> {
    if raw.is_empty()
        || raw.len() > MAX_URL_BYTES
        || raw.contains('#')
        || raw.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(OAuthError::Validation("URL OAuth malformada".into()));
    }
    let uri: Uri = raw
        .parse()
        .map_err(|_| OAuthError::Validation("URL OAuth inválida".into()))?;
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| OAuthError::Validation("URL OAuth sem esquema".into()))?;
    if require_https && scheme != "https" {
        return Err(OAuthError::Validation(
            "URL OAuth deve usar https://".into(),
        ));
    }
    if scheme != "http" && scheme != "https" {
        return Err(OAuthError::Validation(
            "URL OAuth deve usar http:// ou https://".into(),
        ));
    }
    let authority = uri
        .authority()
        .ok_or_else(|| OAuthError::Validation("URL OAuth sem host".into()))?;
    // Bloqueia userinfo (`user:pass@host`) em endpoints descobertos ou callbacks.
    if authority.as_str().contains('@') || authority.host().is_empty() {
        return Err(OAuthError::Validation(
            "URL OAuth não pode conter credenciais embutidas".into(),
        ));
    }
    let origin = format!(
        "{scheme}://{}",
        normalize_authority(authority.as_str(), &scheme)
    );
    let path = match uri.path() {
        "" | "/" => String::new(),
        other => other.trim_end_matches('/').to_owned(),
    };
    Ok(ParsedHttpUrl {
        scheme,
        origin,
        path,
    })
}

fn prm_resource_matches(
    prm_resource: Option<&str>,
    expected_canonical: &str,
    require_https: bool,
) -> bool {
    let Some(raw) = prm_resource.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    // RFC 9728 proíbe query e fragmento no identificador de recurso protegido.
    if raw.contains('?') || raw.contains('#') {
        return false;
    }
    canonical_resource_uri(raw, require_https)
        .is_ok_and(|actual| actual == expected_canonical)
}

fn validate_discovered_url(
    label: &str,
    url: &str,
    require_https: bool,
) -> Result<ParsedHttpUrl, OAuthError> {
    parse_http_url(url, require_https)
        .map_err(|err| OAuthError::DiscoveryFailed(format!("{label} inválido: {err}")))
}

fn validate_discovered_as_metadata(
    as_meta: &AuthorizationServerMetadata,
    expected_issuer: &str,
    require_https: bool,
) -> Result<(), OAuthError> {
    let raw_issuer = as_meta
        .issuer
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            OAuthError::DiscoveryFailed("metadados do servidor OAuth sem campo issuer".into())
        })?;
    if raw_issuer.contains('?') {
        return Err(OAuthError::DiscoveryFailed(
            "campo issuer dos metadados OAuth não pode conter query".into(),
        ));
    }
    let parsed_issuer = validate_discovered_url("issuer", raw_issuer, require_https)?;
    if parsed_issuer.canonical_uri() != expected_issuer {
        return Err(OAuthError::DiscoveryFailed(
            "campo issuer dos metadados OAuth diverge do emissor esperado".into(),
        ));
    }

    validate_discovered_url(
        "authorization_endpoint",
        &as_meta.authorization_endpoint,
        require_https,
    )?;
    validate_discovered_url("token_endpoint", &as_meta.token_endpoint, require_https)?;
    if let Some(reg_ep) = as_meta.registration_endpoint.as_deref() {
        validate_discovered_url("registration_endpoint", reg_ep, require_https)?;
    }
    if let Some(methods) = &as_meta.code_challenge_methods_supported
        && !methods.iter().any(|m| m == "S256")
    {
        return Err(OAuthError::DiscoveryFailed(
            "servidor OAuth não suporta PKCE S256".into(),
        ));
    }
    Ok(())
}

async fn read_bounded_body(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(len) = resp.content_length()
        && len > max_bytes as u64
    {
        return Err(format!("resposta excede o limite de {max_bytes} bytes"));
    }
    let initial = resp
        .content_length()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(4096)
        .min(max_bytes);
    let mut buf = Zeroizing::new(Vec::with_capacity(initial));
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|err| format!("falha ao ler corpo HTTP: {err}"))?
    {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("resposta excede o limite de {max_bytes} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn oauth_tokens_match(
    current: &OAuthTokens,
    expected_access: &Zeroizing<String>,
    expected_refresh: Option<&Zeroizing<String>>,
) -> bool {
    let same_access = current.access_token.as_str() == expected_access.as_str();
    let same_refresh = match expected_refresh {
        Some(exp) => current
            .refresh_token
            .as_ref()
            .is_some_and(|cur| cur.as_str() == exp.as_str()),
        None => current.refresh_token.is_none(),
    };
    same_access && same_refresh
}

fn build_auth_server_metadata_urls(issuer: &ParsedHttpUrl) -> Vec<String> {
    let mut urls = Vec::new();
    let _ = &issuer.scheme;
    if issuer.path.is_empty() {
        urls.push(format!(
            "{}/.well-known/oauth-authorization-server",
            issuer.origin
        ));
        urls.push(format!(
            "{}/.well-known/openid-configuration",
            issuer.origin
        ));
    } else {
        urls.push(format!(
            "{}/.well-known/oauth-authorization-server{}",
            issuer.origin, issuer.path
        ));
        urls.push(format!(
            "{}/.well-known/openid-configuration{}",
            issuer.origin, issuer.path
        ));
        urls.push(format!(
            "{}{}/.well-known/openid-configuration",
            issuer.origin, issuer.path
        ));
        urls.push(format!(
            "{}/.well-known/oauth-authorization-server",
            issuer.origin
        ));
        urls.push(format!(
            "{}/.well-known/openid-configuration",
            issuer.origin
        ));
    }
    urls
}

fn parse_www_authenticate_bearer(header_value: &str) -> (Option<String>, Option<String>) {
    let lower = header_value.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(pos) = lower[search_from..].find("bearer") {
        let start = search_from + pos;
        let before_ok =
            start == 0 || matches!(header_value.as_bytes()[start - 1], b' ' | b',' | b'\t');
        let after = start + "bearer".len();
        let after_ok =
            after < header_value.len() && matches!(header_value.as_bytes()[after], b' ' | b'\t');
        if before_ok && after_ok {
            let params_str = &header_value[after..];
            let resource_metadata = extract_auth_param(params_str, "resource_metadata");
            let scope = extract_auth_param(params_str, "scope");
            return (resource_metadata, scope);
        }
        search_from = after;
    }
    (None, None)
}

fn extract_auth_param(params: &str, key: &str) -> Option<String> {
    let lower = params.to_ascii_lowercase();
    let mut idx = 0usize;
    while let Some(found) = lower[idx..].find(key) {
        let pos = idx + found;
        let before_ok = pos == 0 || matches!(params.as_bytes()[pos - 1], b' ' | b',' | b'\t');
        let after_key = pos + key.len();
        if !before_ok {
            idx = after_key;
            continue;
        }
        let rest = params[after_key..].trim_start();
        let Some(after_eq) = rest.strip_prefix('=') else {
            idx = after_key;
            continue;
        };
        let val_start = after_eq.trim_start();
        if let Some(quoted) = val_start.strip_prefix('"') {
            let end = quoted.find('"')?;
            return Some(quoted[..end].to_owned());
        }
        let end = val_start
            .find(|c: char| c == ',' || c.is_whitespace())
            .unwrap_or(val_start.len());
        return (!val_start[..end].is_empty()).then(|| val_start[..end].to_owned());
    }
    None
}

fn random_base64url_32() -> Result<String, OAuthError> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(&mut bytes[..])
        .map_err(|_| OAuthError::Network("falha ao gerar bytes aleatórios OAuth".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(&bytes[..]))
}

fn pkce_s256_challenge(verifier: &str) -> String {
    let hashed = digest(&SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hashed.as_ref())
}

fn build_authorize_url(authorization_endpoint: &str, params: &[(&str, &str)]) -> String {
    let query = encode_form_body(params);
    let sep = if authorization_endpoint.contains('?') {
        if authorization_endpoint.ends_with('?') || authorization_endpoint.ends_with('&') {
            ""
        } else {
            "&"
        }
    } else {
        "?"
    };
    format!("{authorization_endpoint}{sep}{}", query.as_str())
}

fn encode_form_body(params: &[(&str, &str)]) -> Zeroizing<String> {
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for (k, v) in params {
        ser.append_pair(k, v);
    }
    Zeroizing::new(ser.finish())
}

fn is_google_accounts_url(url: &str) -> bool {
    parse_http_url(url, false).is_ok_and(|parsed| parsed.origin == "https://accounts.google.com")
}

fn map_oauth_error_code(raw_code: &str, status: Option<u16>) -> (String, String) {
    let normalized = raw_code.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "invalid_grant" => (
            normalized,
            "autorização expirada ou revogada pelo provedor".to_owned(),
        ),
        "invalid_token" => (
            normalized,
            "token OAuth expirado ou inválido".to_owned(),
        ),
        "invalid_client" => (
            normalized,
            "credenciais do cliente OAuth recusadas pelo provedor".to_owned(),
        ),
        "invalid_scope" => (
            normalized,
            "escopo OAuth recusado ou inválido".to_owned(),
        ),
        "access_denied" => (
            normalized,
            "acesso negado pelo usuário ou provedor".to_owned(),
        ),
        "unauthorized_client" => (
            normalized,
            "cliente OAuth não autorizado para este fluxo".to_owned(),
        ),
        "unsupported_grant_type" => (
            normalized,
            "tipo de concessão OAuth não suportado pelo provedor".to_owned(),
        ),
        "invalid_request" => (
            normalized,
            "requisição OAuth recusada como inválida".to_owned(),
        ),
        "server_error" => (
            normalized,
            "erro interno no provedor OAuth".to_owned(),
        ),
        "temporarily_unavailable" => (
            normalized,
            "provedor OAuth temporariamente indisponível".to_owned(),
        ),
        _ => {
            let msg = match status {
                Some(code) => format!("erro do provedor OAuth (HTTP {code})"),
                None => "erro retornado pelo provedor OAuth".to_owned(),
            };
            ("unknown_error".to_owned(), msg)
        }
    }
}

fn sanitize_provider_error(raw_code: &str, description: Option<&str>) -> (String, String) {
    let (safe_code, default_msg) = map_oauth_error_code(raw_code, None);
    if safe_code == "unknown_error"
        && let Some(desc) = description
        && let Some(rest) = desc.strip_prefix("erro do provedor OAuth (HTTP ")
        && let Some(code_str) = rest.strip_suffix(')')
        && code_str.bytes().all(|b| b.is_ascii_digit())
    {
        return (safe_code, desc.to_owned());
    }
    (safe_code, default_msg)
}

fn extract_safe_oauth_error(bytes: &[u8], status: u16) -> Option<(String, String)> {
    let payload: OAuthErrorPayload = serde_json::from_slice(bytes).ok()?;
    let error = payload.error.trim();
    if error.is_empty() {
        return None;
    }
    Some(map_oauth_error_code(error, Some(status)))
}

fn compute_expires_at(expires_in: Option<f64>) -> Option<f64> {
    let secs = expires_in.filter(|s| s.is_finite() && *s > 0.0)?;
    Some(now_unix_secs() + secs)
}

fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::model::{
        AuthConfig, IntegrationConfig, IntegrationStatus, PermissionPolicy, Transport,
    };
    use crate::secrets::{IntegrationSecrets, OAuthTokens};

    use super::{
        OAuthError, compute_status, parse_http_url, parse_www_authenticate_bearer,
        validate_discovered_as_metadata, validate_discovered_url,
    };
    use zeroize::Zeroizing;

    fn sample_cfg(enabled: bool, auth: AuthConfig) -> IntegrationConfig {
        IntegrationConfig {
            id: "sample".to_owned(),
            name: "Sample".to_owned(),
            preset: None,
            enabled,
            transport: Transport::RemoteHttp {
                url: "https://mcp.example.com/mcp".to_owned(),
                header_names: vec![],
            },
            auth,
            policy: PermissionPolicy::ReadFreeAskWrite,
            pinned: false,
            projects: vec![],
            allowed_tools: BTreeSet::new(),
            created_at: 1.0,
            updated_at: 1.0,
        }
    }

    #[test]
    fn compute_status_covers_all_auth_and_enabled_states() {
        let disabled = sample_cfg(false, AuthConfig::None);
        assert_eq!(compute_status(&disabled, None), IntegrationStatus::Disabled);

        let no_auth = sample_cfg(true, AuthConfig::None);
        assert_eq!(compute_status(&no_auth, None), IntegrationStatus::Ready);

        let bearer = sample_cfg(true, AuthConfig::Bearer);
        assert_eq!(
            compute_status(&bearer, None),
            IntegrationStatus::AuthRequired
        );
        assert_eq!(
            compute_status(
                &bearer,
                Some(&IntegrationSecrets {
                    bearer_token: Some(Zeroizing::new("   ".to_owned())),
                    ..Default::default()
                })
            ),
            IntegrationStatus::AuthRequired
        );
        assert_eq!(
            compute_status(
                &bearer,
                Some(&IntegrationSecrets {
                    bearer_token: Some(Zeroizing::new("pat-123".to_owned())),
                    ..Default::default()
                })
            ),
            IntegrationStatus::Ready
        );

        let oauth = sample_cfg(true, AuthConfig::OAuthDcr);
        assert_eq!(
            compute_status(&oauth, None),
            IntegrationStatus::AuthRequired
        );
        assert_eq!(
            compute_status(
                &oauth,
                Some(&IntegrationSecrets {
                    oauth: Some(OAuthTokens {
                        access_token: Zeroizing::new("tok".to_owned()),
                        auth_expired: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            ),
            IntegrationStatus::AuthExpired
        );
        assert_eq!(
            compute_status(
                &oauth,
                Some(&IntegrationSecrets {
                    oauth: Some(OAuthTokens {
                        access_token: Zeroizing::new("tok".to_owned()),
                        auth_expired: false,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            ),
            IntegrationStatus::Ready
        );
    }

    #[test]
    fn parses_www_authenticate_bearer_and_rejects_userinfo_urls() {
        let header = r#"Bearer realm="mcp", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource", scope="read write""#;
        let (meta, scope) = parse_www_authenticate_bearer(header);
        assert_eq!(
            meta.as_deref(),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource")
        );
        assert_eq!(scope.as_deref(), Some("read write"));

        assert!(parse_http_url("https://user:pass@mcp.example.com/auth", false).is_err());
        assert!(parse_http_url("ftp://mcp.example.com/auth", false).is_err());
        assert!(parse_http_url("http://mcp.example.com/auth", true).is_err());
        assert!(parse_http_url("https://mcp.example.com/auth#frag", false).is_err());
    }

    #[test]
    fn https_mcp_discovery_rejects_plaintext_http_oauth_endpoints() {
        let mcp_is_https = parse_http_url("https://mcp.example.com/mcp", false)
            .unwrap()
            .scheme
            == "https";
        let expected_issuer = "https://auth.example.com";
        let meta = super::AuthorizationServerMetadata {
            issuer: Some(expected_issuer.to_owned()),
            authorization_endpoint: "http://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            client_id_metadata_document_supported: None,
            code_challenge_methods_supported: None,
        };
        let err = validate_discovered_as_metadata(&meta, expected_issuer, mcp_is_https)
            .expect_err("MCP https:// deve rejeitar authorization_endpoint http://");
        assert!(matches!(err, OAuthError::DiscoveryFailed(_)), "{err:?}");

        let meta_http_token = super::AuthorizationServerMetadata {
            issuer: Some(expected_issuer.to_owned()),
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "http://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            client_id_metadata_document_supported: None,
            code_challenge_methods_supported: None,
        };
        assert!(matches!(
            validate_discovered_as_metadata(&meta_http_token, expected_issuer, mcp_is_https),
            Err(OAuthError::DiscoveryFailed(_))
        ));

        let meta_http_reg = super::AuthorizationServerMetadata {
            issuer: Some(expected_issuer.to_owned()),
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: Some("http://auth.example.com/register".to_owned()),
            client_id_metadata_document_supported: None,
            code_challenge_methods_supported: None,
        };
        assert!(matches!(
            validate_discovered_as_metadata(&meta_http_reg, expected_issuer, mcp_is_https),
            Err(OAuthError::DiscoveryFailed(_))
        ));

        // Emissor divergente ou ausente também deve ser recusado (RFC 8414).
        let meta_wrong_issuer = super::AuthorizationServerMetadata {
            issuer: Some("https://auth.example.com/tenant-a".to_owned()),
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            client_id_metadata_document_supported: None,
            code_challenge_methods_supported: None,
        };
        assert!(matches!(
            validate_discovered_as_metadata(
                &meta_wrong_issuer,
                "https://auth.example.com/tenant-b",
                mcp_is_https
            ),
            Err(OAuthError::DiscoveryFailed(_))
        ));

        let meta_missing_issuer = super::AuthorizationServerMetadata {
            issuer: None,
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            client_id_metadata_document_supported: None,
            code_challenge_methods_supported: None,
        };
        assert!(matches!(
            validate_discovered_as_metadata(&meta_missing_issuer, expected_issuer, mcp_is_https),
            Err(OAuthError::DiscoveryFailed(_))
        ));

        assert!(matches!(
            validate_discovered_url("resource_metadata", "http://evil.example/prm", mcp_is_https),
            Err(OAuthError::DiscoveryFailed(_))
        ));
        assert!(matches!(
            validate_discovered_url(
                "authorization_server",
                "http://evil.example/issuer",
                mcp_is_https
            ),
            Err(OAuthError::DiscoveryFailed(_))
        ));
        let meta_local = super::AuthorizationServerMetadata {
            issuer: Some("http://auth.example.com".to_owned()),
            authorization_endpoint: "http://auth.example.com/authorize".to_owned(),
            token_endpoint: "http://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            client_id_metadata_document_supported: None,
            code_challenge_methods_supported: None,
        };
        assert!(
            validate_discovered_as_metadata(&meta_local, "http://auth.example.com", false).is_ok(),
            "MCP http:// local deve continuar aceitando endpoints http://"
        );
    }

    fn insert_pending_for_test(
        oauth: &super::OAuthManager,
        state: &str,
        integration_id: &str,
        authorize_url: &str,
        token_endpoint: &str,
    ) {
        let mut cfg = sample_cfg(true, AuthConfig::OAuthDcr);
        cfg.id = integration_id.to_owned();
        let mut guard = oauth
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(
            state.to_owned(),
            super::PendingAuth {
                integration_id: integration_id.to_owned(),
                config_fingerprint: crate::model::config_fingerprint(&cfg),
                resource_uri: "https://mcp.example.com/mcp".to_owned(),
                authorize_url: authorize_url.to_owned(),
                code_verifier: Zeroizing::new("verifier-teste-123".to_owned()),
                redirect_uri: "http://127.0.0.1:8765/api/integrations/oauth/callback".to_owned(),
                token_endpoint: token_endpoint.to_owned(),
                client_id: "cid-teste".to_owned(),
                client_secret: None,
                scopes: vec![],
                persist_client_id: true,
                is_dcr: true,
                created_at: std::time::Instant::now(),
            },
        );
    }

    #[tokio::test]
    async fn cancel_pending_for_clears_only_target_integration_states() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = std::sync::Arc::new(crate::registry::IntegrationRegistry::new(dir.path()));
        let secrets = std::sync::Arc::new(crate::secrets::SecretStore::new(
            dir.path(),
            std::sync::Arc::new(crate::secrets::MemoryKeyStore::new()),
        ));
        let oauth = super::OAuthManager::new(registry, secrets);

        insert_pending_for_test(
            &oauth,
            "state-a1",
            "integ-a",
            "https://auth.example.com/authorize?state=state-a1",
            "https://auth.example.com/token",
        );
        insert_pending_for_test(
            &oauth,
            "state-a2",
            "integ-a",
            "https://auth.example.com/authorize?state=state-a2",
            "https://auth.example.com/token",
        );
        insert_pending_for_test(
            &oauth,
            "state-b1",
            "integ-b",
            "https://auth.example.com/authorize?state=state-b1",
            "https://auth.example.com/token",
        );

        oauth.cancel_pending_for("integ-a");

        assert!(!oauth.is_pending_authorize_url(
            "integ-a",
            "https://auth.example.com/authorize?state=state-a1"
        ));
        assert_eq!(oauth.latest_pending_authorize_url("integ-a"), None);
        assert!(oauth.is_pending_authorize_url(
            "integ-b",
            "https://auth.example.com/authorize?state=state-b1"
        ));
        assert_eq!(
            oauth.latest_pending_authorize_url("integ-b").as_deref(),
            Some("https://auth.example.com/authorize?state=state-b1")
        );

        let err = oauth
            .handle_callback("state-a1", Some("code-1"), None, None)
            .await
            .expect_err("state cancelado não pode ser aceito");
        assert_eq!(err, OAuthError::InvalidState);
    }

    #[tokio::test]
    async fn handle_callback_after_delete_or_auth_change_rejects_without_writing_secret() {
        use axum::extract::State;
        use axum::http::StatusCode;
        use axum::response::{IntoResponse, Response};
        use axum::{Router, routing::post};
        use std::sync::Arc;

        async fn delete_during_token(State(reg): State<Arc<crate::registry::IntegrationRegistry>>) -> Response {
            let _ = reg.delete("integ-race");
            let body = serde_json::to_vec(&serde_json::json!({
                "access_token": "tok-orphan-race",
                "token_type": "Bearer",
                "expires_in": 3600
            }))
            .expect("serializar token");
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Arc::new(crate::registry::IntegrationRegistry::new(dir.path()));
        let secrets = Arc::new(crate::secrets::SecretStore::new(
            dir.path(),
            Arc::new(crate::secrets::MemoryKeyStore::new()),
        ));
        let oauth = super::OAuthManager::new(registry.clone(), secrets.clone());

        // 1. Integração apagada antes da chegada do callback.
        let mut cfg_del = sample_cfg(true, AuthConfig::OAuthDcr);
        cfg_del.id = "integ-del".to_owned();
        registry.create(cfg_del).unwrap();
        insert_pending_for_test(
            &oauth,
            "state-del",
            "integ-del",
            "https://auth.example.com/authorize?state=state-del",
            "http://127.0.0.1:1/token",
        );
        registry.delete("integ-del").unwrap();

        let err_del = oauth
            .handle_callback("state-del", Some("code-del"), None, None)
            .await
            .expect_err("callback após delete deve ser rejeitado");
        assert_eq!(err_del, OAuthError::NotFound("integ-del".to_owned()));
        assert!(secrets.get("integ-del").await.unwrap().is_none());

        // 2. Integração trocou de OAuth para Bearer antes do callback.
        let mut cfg_bearer = sample_cfg(true, AuthConfig::OAuthDcr);
        cfg_bearer.id = "integ-bearer".to_owned();
        registry.create(cfg_bearer).unwrap();
        insert_pending_for_test(
            &oauth,
            "state-bearer",
            "integ-bearer",
            "https://auth.example.com/authorize?state=state-bearer",
            "http://127.0.0.1:1/token",
        );
        registry
            .update(
                "integ-bearer",
                crate::registry::IntegrationUpdate {
                    auth: Some(AuthConfig::Bearer),
                    ..Default::default()
                },
            )
            .unwrap();

        let err_auth = oauth
            .handle_callback("state-bearer", Some("code-bearer"), None, None)
            .await
            .expect_err("callback após troca para Bearer deve falhar");
        assert_eq!(err_auth, OAuthError::UnsupportedAuth);
        assert!(secrets.get("integ-bearer").await.unwrap().is_none());

        // 3. Integração apagada durante a troca de código em /token também não grava segredo órfão.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/token", post(delete_during_token))
            .with_state(registry.clone());
        let srv_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut cfg_race = sample_cfg(true, AuthConfig::OAuthDcr);
        cfg_race.id = "integ-race".to_owned();
        registry.create(cfg_race).unwrap();
        insert_pending_for_test(
            &oauth,
            "state-race",
            "integ-race",
            "https://auth.example.com/authorize?state=state-race",
            &format!("http://{addr}/token"),
        );

        let err_race = oauth
            .handle_callback("state-race", Some("code-race"), None, None)
            .await
            .expect_err("delete durante /token deve impedir gravação órfã");
        assert_eq!(err_race, OAuthError::NotFound("integ-race".to_owned()));
        assert!(secrets.get("integ-race").await.unwrap().is_none());

        srv_handle.abort();
    }
}
