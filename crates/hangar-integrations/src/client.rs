//! Cliente MCP Streamable HTTP + `stdio` e gerenciador de ciclo de vida das integrações.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use hangar_core::terminal_process::{
    CommandTree, PRIVATE_ENV_KEYS, die_with_parent, finish, leader_exited,
};
use http::{HeaderName, HeaderValue};
use ring::digest::{Context as DigestContext, SHA256};
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig as McpClientInfo,
    ContentBlock, Implementation, PaginatedRequestParams, ResourceContents, Tool,
};
use rmcp::service::{
    ClientCacheConfig, ClientInitializeError, Peer, RoleClient, RunningService, ServiceError,
};
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    StreamableHttpError,
};
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, Notify, watch};
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::model::{AuthConfig, IntegrationConfig, Transport, config_fingerprint};
use crate::registry::{IntegrationRegistry, RegistryError};
use crate::secrets::{IntegrationSecrets, SecretStore, SecretStoreError};
use crate::tools_cache::{DiscoveredTool, ToolsCache};

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(300);
pub const DEFAULT_EXIT_GRACE: Duration = Duration::from_secs(3);

const RESERVED_HTTP_HEADERS: [&str; 5] = [
    "authorization",
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

/// Parâmetros de tempo e ciclo de vida para conexões MCP gerenciadas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientConfig {
    pub connect_timeout: Duration,
    pub call_timeout: Duration,
    pub idle_ttl: Duration,
    pub exit_grace: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            idle_ttl: DEFAULT_IDLE_TTL,
            exit_grace: DEFAULT_EXIT_GRACE,
        }
    }
}

/// Renovador de tokens OAuth chamado quando uma integração HTTP retorna 401.
pub trait TokenRefresher: Send + Sync {
    fn refresh<'a>(&'a self, integration_id: &'a str)
    -> BoxFuture<'a, Result<String, ClientError>>;
}

/// Erros operacionais do cliente MCP e do gerenciador de ciclo de vida.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    Disabled,
    AuthRequired(String),
    AuthExpired(String),
    Timeout(String),
    Transport(String),
    Protocol(String),
    ToolNotFound(String),
    RemoteError { code: i32, message: String },
    Registry(RegistryError),
    Secrets(SecretStoreError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "integração desabilitada"),
            Self::AuthRequired(reason) => write!(f, "autenticação necessária: {reason}"),
            Self::AuthExpired(reason) => write!(f, "autenticação expirada: {reason}"),
            Self::Timeout(reason) => write!(f, "tempo esgotado: {reason}"),
            Self::Transport(reason) => write!(f, "erro de transporte MCP: {reason}"),
            Self::Protocol(reason) => write!(f, "erro de protocolo MCP: {reason}"),
            Self::ToolNotFound(name) => write!(f, "ferramenta não encontrada: {name}"),
            Self::RemoteError { code, message } => {
                write!(f, "erro remoto MCP ({code}): {message}")
            }
            Self::Registry(error) => write!(f, "{error}"),
            Self::Secrets(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<RegistryError> for ClientError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<SecretStoreError> for ClientError {
    fn from(error: SecretStoreError) -> Self {
        Self::Secrets(error)
    }
}

/// Saída estruturada de uma chamada `tools/call` no servidor MCP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpCallOutput {
    pub text: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,
}

/// Gerenciador de conexões MCP sob demanda para transportes `stdio` e Streamable HTTP.
pub struct McpClientManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    registry: Arc<IntegrationRegistry>,
    secrets: Arc<SecretStore>,
    tools_cache: Arc<ToolsCache>,
    config: ClientConfig,
    refresher: Option<Arc<dyn TokenRefresher>>,
    slots: StdMutex<HashMap<String, Arc<Slot>>>,
    retired_cleanups: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    shutting_down: AtomicBool,
    shutdown_watch: watch::Sender<bool>,
}

#[derive(Clone)]
struct OAuthOverride {
    config_fingerprint: [u8; 32],
    stale_token: Zeroizing<String>,
    fresh_token: Zeroizing<String>,
}

#[derive(Default)]
struct Slot {
    connect_lock: AsyncMutex<()>,
    refresh_lock: AsyncMutex<()>,
    active: StdMutex<Option<Arc<ActiveSession>>>,
    /// Token renovado vinculado ao fingerprint da config caso o renovador não tenha atualizado `secrets.bin`.
    oauth_override: StdMutex<Option<OAuthOverride>>,
    /// Incrementado a cada refresh OAuth bem-sucedido para deduplicar retries concorrentes.
    refresh_epoch: AtomicU64,
    next_generation: AtomicU64,
    reaper_active: AtomicBool,
    stop_notify: Notify,
}

struct StdioOwned {
    child: Child,
    tree: CommandTree,
}

struct PendingStdioGuard {
    pending: Arc<StdMutex<Option<StdioOwned>>>,
}

impl PendingStdioGuard {
    fn new() -> Self {
        Self {
            pending: Arc::new(StdMutex::new(None)),
        }
    }

    fn clone_slot(&self) -> Arc<StdMutex<Option<StdioOwned>>> {
        Arc::clone(&self.pending)
    }

    fn take(&self) -> Option<StdioOwned> {
        take_pending_stdio(&self.pending)
    }
}

impl Drop for PendingStdioGuard {
    fn drop(&mut self) {
        if let Some(owned) = self.take() {
            spawn_stdio_cleanup(owned, Duration::ZERO, true);
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SessionFingerprint {
    transport: Transport,
    auth: AuthConfig,
    secrets_digest: [u8; 32],
}

#[derive(Clone)]
struct OAuthAttempt {
    epoch: u64,
    token: Option<Zeroizing<String>>,
}

struct ActiveSession {
    generation: u64,
    fingerprint: SessionFingerprint,
    oauth_epoch: u64,
    oauth_token: Option<Zeroizing<String>>,
    peer: Peer<RoleClient>,
    running: AsyncMutex<Option<RunningService<RoleClient, McpClientInfo>>>,
    stdio: StdMutex<Option<StdioOwned>>,
    in_flight: AtomicUsize,
    last_used: StdMutex<Instant>,
    idle_notify: Notify,
    cancel: watch::Sender<bool>,
}

struct InFlightGuard {
    session: Arc<ActiveSession>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        *self
            .session
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
        if self.session.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.session.idle_notify.notify_waiters();
        }
    }
}

enum ConnectFailure {
    Http401(String),
    Other(ClientError),
}

enum OpError {
    Unauthorized(String),
    Timeout(String),
    FatalTransport(String),
    Client(ClientError),
}

impl McpClientManager {
    /// Cria o gerenciador sem iniciar nenhum processo filho nem conexão remota.
    pub fn new(
        registry: Arc<IntegrationRegistry>,
        secrets: Arc<SecretStore>,
        tools_cache: Arc<ToolsCache>,
        config: ClientConfig,
    ) -> Self {
        let (shutdown_watch, _) = watch::channel(false);
        Self {
            inner: Arc::new(ManagerInner {
                registry,
                secrets,
                tools_cache,
                config,
                refresher: None,
                slots: StdMutex::new(HashMap::new()),
                retired_cleanups: StdMutex::new(Vec::new()),
                shutting_down: AtomicBool::new(false),
                shutdown_watch,
            }),
        }
    }

    /// Configura o renovador OAuth usado após resposta HTTP 401.
    pub fn with_refresher(mut self, refresher: Arc<dyn TokenRefresher>) -> Self {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("with_refresher deve ser chamado antes de compartilhar o gerenciador");
        inner.refresher = Some(refresher);
        self
    }

    /// Lista os `integration_id` com conexão atualmente ativa em memória.
    pub fn running_ids(&self) -> Vec<String> {
        let slots = self
            .inner
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut ids: Vec<String> = slots
            .iter()
            .filter_map(|(id, slot)| {
                let active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
                active
                    .as_ref()
                    .filter(|session| !session.is_closed())
                    .map(|_| id.clone())
            })
            .collect();
        ids.sort();
        ids
    }

    /// Encerra e descarta qualquer conexão ativa de `integration_id`.
    pub async fn invalidate(&self, integration_id: &str) {
        let Some(slot) = self.existing_slot(integration_id) else {
            return;
        };
        let _connect_guard = slot.connect_lock.lock().await;
        *slot
            .oauth_override
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let session = slot
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        slot.stop_notify.notify_waiters();
        if let Some(session) = session {
            session.shutdown(self.inner.config.exit_grace, false).await;
        }
    }

    /// Encerra todas as conexões ativas e aguarda o fim das árvores de processos filhos.
    pub async fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        let _ = self.inner.shutdown_watch.send_replace(true);
        let slots: Vec<Arc<Slot>> = self
            .inner
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let grace = self.inner.config.exit_grace;
        futures_util::future::join_all(slots.into_iter().map(|slot| async move {
            let _connect_guard = slot.connect_lock.lock().await;
            let session = slot
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            slot.stop_notify.notify_waiters();
            if let Some(session) = session {
                session.shutdown(grace, false).await;
            }
        }))
        .await;
        // Aguarda tarefas de encerramento de sessões aposentadas em 401 antes de concluir `shutdown`.
        let pending_cleanups = {
            let mut cleanups = self
                .inner
                .retired_cleanups
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut *cleanups)
        };
        for handle in pending_cleanups {
            let _ = handle.await;
        }
    }

    /// Descobre todas as ferramentas da integração (com paginação) e atualiza o cache.
    pub async fn list_tools(
        &self,
        integration_id: &str,
    ) -> Result<Vec<DiscoveredTool>, ClientError> {
        // Registra a geração antes da descoberta para não regravar cache após invalidação concorrente.
        let expected_gen = self.inner.tools_cache.generation(integration_id);
        let tools = self
            .execute_with_oauth_retry(integration_id, |session, _config| async move {
                let raw_tools = list_all_tools_paginated(&session.peer).await?;
                Ok(raw_tools
                    .into_iter()
                    .map(discovered_from_rmcp)
                    .collect::<Vec<_>>())
            })
            .await?;
        let _ = self
            .inner
            .tools_cache
            .set_if_generation(integration_id, expected_gen, tools.clone());
        Ok(tools)
    }

    /// Invoca uma ferramenta na integração respeitando `call_timeout` e renovação OAuth.
    pub async fn call_tool(
        &self,
        integration_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpCallOutput, ClientError> {
        self.call_tool_tracked(integration_id, tool_name, arguments, None, None)
            .await
    }

    /// Invoca uma ferramenta sinalizando `external_dispatched` somente no momento exato do envio ao transporte.
    ///
    /// Quando `expected_config_fp` é informado, rejeita o envio se o destino ou autenticação da integração
    /// mudou entre a admissão e a conexão/envio (inclusive em retry após renovação OAuth).
    pub async fn call_tool_tracked(
        &self,
        integration_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        expected_config_fp: Option<[u8; 32]>,
        external_dispatched: Option<Arc<AtomicBool>>,
    ) -> Result<McpCallOutput, ClientError> {
        self.call_tool_guarded(
            integration_id,
            tool_name,
            arguments,
            expected_config_fp,
            external_dispatched,
            || async { Ok(()) },
        )
        .await
    }

    /// Igual a [`Self::call_tool_tracked`], mas executa `before_dispatch` após a conexão estar pronta e
    /// imediatamente antes de enviar `tools/call` ao transporte (fechando corridas durante `ensure_connected`).
    pub async fn call_tool_guarded<G, GFut>(
        &self,
        integration_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        expected_config_fp: Option<[u8; 32]>,
        external_dispatched: Option<Arc<AtomicBool>>,
        before_dispatch: G,
    ) -> Result<McpCallOutput, ClientError>
    where
        G: Fn() -> GFut,
        GFut: Future<Output = Result<(), ClientError>>,
    {
        let params = build_call_tool_params(tool_name, arguments)?;
        let tool_owned = tool_name.to_owned();
        self.execute_with_oauth_retry_for(
            integration_id,
            expected_config_fp,
            move |session, _config| {
                let params = params.clone();
                let tool_name = tool_owned.clone();
                let external_dispatched = external_dispatched.clone();
                let guard_fut = before_dispatch();
                async move {
                    guard_fut.await.map_err(OpError::Client)?;
                    if let Some(flag) = external_dispatched.as_ref() {
                        flag.store(true, Ordering::SeqCst);
                    }
                    let result = session
                        .peer
                        .call_tool(params)
                        .await
                        .map_err(|err| map_service_error(err, Some(&tool_name)))?;
                    Ok(extract_call_output(result))
                }
            },
        )
        .await
    }

    fn slot(&self, integration_id: &str) -> Arc<Slot> {
        self.inner
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(integration_id.to_owned())
            .or_default()
            .clone()
    }

    fn existing_slot(&self, integration_id: &str) -> Option<Arc<Slot>> {
        self.inner
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(integration_id)
            .cloned()
    }

    async fn execute_with_oauth_retry<T, F, Fut>(
        &self,
        integration_id: &str,
        operation: F,
    ) -> Result<T, ClientError>
    where
        F: Fn(Arc<ActiveSession>, IntegrationConfig) -> Fut,
        Fut: Future<Output = Result<T, OpError>>,
    {
        self.execute_with_oauth_retry_for(integration_id, None, operation)
            .await
    }

    async fn execute_with_oauth_retry_for<T, F, Fut>(
        &self,
        integration_id: &str,
        admitted_config_fp: Option<[u8; 32]>,
        operation: F,
    ) -> Result<T, ClientError>
    where
        F: Fn(Arc<ActiveSession>, IntegrationConfig) -> Fut,
        Fut: Future<Output = Result<T, OpError>>,
    {
        let slot = self.slot(integration_id);
        let first_attempt = self
            .run_single_attempt(&slot, integration_id, None, admitted_config_fp, &operation)
            .await;

        let (reason, maybe_auth, maybe_fp, failed_attempt) = match first_attempt {
            Ok(value) => return Ok(value),
            Err((OpError::Unauthorized(reason), maybe_auth, maybe_fp, attempt)) => {
                (reason, maybe_auth, maybe_fp, attempt)
            }
            Err((OpError::Timeout(msg), _, _, _)) => return Err(ClientError::Timeout(msg)),
            Err((OpError::FatalTransport(msg), _, _, _)) => return Err(ClientError::Transport(msg)),
            Err((OpError::Client(err), _, _, _)) => return Err(err),
        };

        let auth = match maybe_auth {
            Some(auth) => auth,
            None => self.load_enabled_config(integration_id)?.auth,
        };
        if !is_oauth_auth(&auth) {
            return Err(ClientError::AuthRequired(reason));
        }

        let Some(refresher) = &self.inner.refresher else {
            return Err(ClientError::AuthExpired(reason));
        };

        let (new_token, refresh_fp) = {
            let _refresh_guard = slot.refresh_lock.lock().await;
            let refresh_cfg = self.load_enabled_config(integration_id)?;
            let refresh_fp = config_fingerprint(&refresh_cfg);
            let expected_fp = admitted_config_fp.or(maybe_fp);
            if let Some(expected_fp) = expected_fp
                && expected_fp != refresh_fp
            {
                *slot
                    .oauth_override
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
                return Err(ClientError::AuthExpired(
                    "configuração da integração alterada durante tentativa OAuth".into(),
                ));
            }
            if !is_oauth_auth(&refresh_cfg.auth) {
                return Err(ClientError::AuthRequired(reason));
            }

            let current_epoch = slot.refresh_epoch.load(Ordering::Acquire);
            let current_secrets = self
                .inner
                .secrets
                .get(integration_id)
                .await?
                .unwrap_or_default();
            let current_effective = resolve_effective_oauth_token(
                &slot,
                refresh_fp,
                &refresh_cfg.auth,
                &current_secrets,
                None,
            );

            let already_refreshed = failed_attempt.as_ref().is_some_and(|attempt| {
                current_epoch != attempt.epoch
                    || current_effective.as_ref().is_some_and(|cur| {
                        attempt
                            .token
                            .as_ref()
                            .is_none_or(|used| cur.as_str() != used.as_str())
                    })
            });

            if already_refreshed {
                let Some(reusable) = current_effective else {
                    return Err(ClientError::AuthRequired(reason));
                };
                (reusable, refresh_fp)
            } else {
                let stale_token = current_secrets
                    .oauth
                    .as_ref()
                    .map(|o| o.access_token.clone())
                    .unwrap_or_default();
                // Limpa override antigo antes de renovar para nunca preservar token velho em caso de erro.
                *slot
                    .oauth_override
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;

                let refreshed = refresher.refresh(integration_id).await?;
                if refreshed.trim().is_empty() {
                    return Err(ClientError::AuthExpired("token renovado vazio".into()));
                }

                // Se URL ou auth mudaram durante o refresh em voo, descarta o token antigo sem tentar o novo destino.
                let post_refresh_cfg = self.load_enabled_config(integration_id)?;
                if config_fingerprint(&post_refresh_cfg) != refresh_fp {
                    *slot
                        .oauth_override
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = None;
                    return Err(ClientError::AuthExpired(
                        "configuração da integração alterada durante renovação OAuth".into(),
                    ));
                }

                let refreshed_secrets = self
                    .inner
                    .secrets
                    .get(integration_id)
                    .await?
                    .unwrap_or_default();
                let current_secret_token = refreshed_secrets
                    .oauth
                    .as_ref()
                    .map(|o| o.access_token.as_str())
                    .unwrap_or("");
                if current_secret_token == stale_token.as_str() {
                    *slot
                        .oauth_override
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(OAuthOverride {
                        config_fingerprint: refresh_fp,
                        stale_token,
                        fresh_token: Zeroizing::new(refreshed.clone()),
                    });
                }
                slot.refresh_epoch.fetch_add(1, Ordering::AcqRel);
                (Zeroizing::new(refreshed), refresh_fp)
            }
        };

        match self
            .run_single_attempt(
                &slot,
                integration_id,
                Some(new_token.as_str()),
                Some(refresh_fp),
                &operation,
            )
            .await
        {
            Ok(value) => Ok(value),
            Err((OpError::Unauthorized(reason), _, _, _)) => {
                let mut guard = slot
                    .oauth_override
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if guard
                    .as_ref()
                    .is_some_and(|entry| entry.fresh_token.as_str() == new_token.as_str())
                {
                    *guard = None;
                }
                Err(ClientError::AuthExpired(reason))
            }
            Err((OpError::Timeout(msg), _, _, _)) => Err(ClientError::Timeout(msg)),
            Err((OpError::FatalTransport(msg), _, _, _)) => Err(ClientError::Transport(msg)),
            Err((OpError::Client(err), _, _, _)) => Err(err),
        }
    }

    async fn run_single_attempt<T, F, Fut>(
        &self,
        slot: &Arc<Slot>,
        integration_id: &str,
        override_oauth_token: Option<&str>,
        expected_config_fp: Option<[u8; 32]>,
        operation: &F,
    ) -> Result<T, (OpError, Option<AuthConfig>, Option<[u8; 32]>, Option<OAuthAttempt>)>
    where
        F: Fn(Arc<ActiveSession>, IntegrationConfig) -> Fut,
        Fut: Future<Output = Result<T, OpError>>,
    {
        let (config, session, _in_flight, oauth_attempt) = match self
            .ensure_connected(
                slot,
                integration_id,
                override_oauth_token,
                expected_config_fp,
            )
            .await
        {
            Ok(tuple) => tuple,
            Err((ConnectFailure::Http401(reason), auth, fp, attempt)) => {
                return Err((OpError::Unauthorized(reason), auth, fp, attempt));
            }
            Err((ConnectFailure::Other(err), auth, fp, attempt)) => {
                return Err((OpError::Client(err), auth, fp, attempt));
            }
        };

        let mut cancel_rx = session.cancel.subscribe();
        let call_timeout = self.inner.config.call_timeout;
        let auth = config.auth.clone();
        let config_fp = config_fingerprint(&config);

        let outcome = if *cancel_rx.borrow_and_update() {
            Err(OpError::FatalTransport(
                "conexão invalidada durante a chamada".into(),
            ))
        } else {
            let work = operation(session.clone(), config);
            tokio::select! {
                biased;
                _ = cancel_rx.changed() => {
                    Err(OpError::FatalTransport("conexão invalidada durante a chamada".into()))
                }
                timed = tokio::time::timeout(call_timeout, work) => match timed {
                    Ok(res) => res,
                    Err(_) => Err(OpError::Timeout(format!(
                        "chamada à integração '{integration_id}' excedeu {:?}",
                        call_timeout
                    ))),
                }
            }
        };
        drop(_in_flight);

        match outcome {
            Ok(value) => Ok(value),
            Err(err) => {
                match &err {
                    OpError::Timeout(_) => {
                        self.discard_session(slot, session.generation, true).await;
                    }
                    OpError::FatalTransport(_) => {
                        self.discard_session(slot, session.generation, false).await;
                    }
                    OpError::Unauthorized(_) => {
                        self.retire_session_on_401(slot, session.generation).await;
                    }
                    OpError::Client(_) => {}
                }
                Err((err, Some(auth), Some(config_fp), Some(oauth_attempt)))
            }
        }
    }

    async fn discard_session(&self, slot: &Arc<Slot>, generation: u64, force_kill: bool) {
        let _connect_guard = slot.connect_lock.lock().await;
        let stale = {
            let mut active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
            active.take_if(|session| session.generation == generation)
        };
        if let Some(session) = stale {
            slot.stop_notify.notify_waiters();
            session
                .shutdown(self.inner.config.exit_grace, force_kill)
                .await;
        }
    }

    async fn retire_session_on_401(&self, slot: &Arc<Slot>, generation: u64) {
        let stale = {
            let mut active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
            active.take_if(|session| session.generation == generation)
        };
        let Some(session) = stale else {
            return;
        };
        slot.stop_notify.notify_waiters();
        let grace = self.inner.config.exit_grace;
        if session.in_flight.load(Ordering::Acquire) == 0 {
            session.shutdown(grace, false).await;
            return;
        }
        // Drena chamadas irmãs antes de fechar o transporte para não abortar outro 401 concorrente.
        let mut shutdown_rx = self.inner.shutdown_watch.subscribe();
        let cleanup_handle = tokio::spawn(async move {
            if !*shutdown_rx.borrow_and_update() {
                loop {
                    let idle_notified = session.idle_notify.notified();
                    tokio::pin!(idle_notified);
                    let _ = idle_notified.as_mut().enable();
                    if session.in_flight.load(Ordering::Acquire) == 0 {
                        break;
                    }
                    tokio::select! {
                        _ = &mut idle_notified => {}
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow_and_update() {
                                break;
                            }
                        }
                    }
                }
            }
            session.shutdown(grace, false).await;
        });
        let mut cleanups = self
            .inner
            .retired_cleanups
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cleanups.retain(|h| !h.is_finished());
        cleanups.push(cleanup_handle);
    }

    fn load_enabled_config(&self, integration_id: &str) -> Result<IntegrationConfig, ClientError> {
        let Some(config) = self.inner.registry.get(integration_id)? else {
            return Err(ClientError::Registry(RegistryError::NotFound(
                integration_id.to_owned(),
            )));
        };
        if !config.enabled {
            return Err(ClientError::Disabled);
        }
        Ok(config)
    }

    async fn ensure_connected(
        &self,
        slot: &Arc<Slot>,
        integration_id: &str,
        override_oauth_token: Option<&str>,
        expected_config_fp: Option<[u8; 32]>,
    ) -> Result<
        (
            IntegrationConfig,
            Arc<ActiveSession>,
            InFlightGuard,
            OAuthAttempt,
        ),
        (
            ConnectFailure,
            Option<AuthConfig>,
            Option<[u8; 32]>,
            Option<OAuthAttempt>,
        ),
    > {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err((
                ConnectFailure::Other(ClientError::Transport(
                    "gerenciador de clientes encerrado".into(),
                )),
                None,
                None,
                None,
            ));
        }

        let config = self
            .load_enabled_config(integration_id)
            .map_err(|err| (ConnectFailure::Other(err), None, None, None))?;
        let config_fp = config_fingerprint(&config);
        if expected_config_fp.is_some_and(|fp| fp != config_fp) {
            *slot
                .oauth_override
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
            return Err((
                ConnectFailure::Other(ClientError::AuthExpired(
                    "configuração da integração alterada durante retry OAuth".into(),
                )),
                Some(config.auth),
                Some(config_fp),
                None,
            ));
        }
        let secrets = self
            .inner
            .secrets
            .get(integration_id)
            .await
            .map_err(|err| {
                (
                    ConnectFailure::Other(ClientError::Secrets(err)),
                    Some(config.auth.clone()),
                    Some(config_fp),
                    None,
                )
            })?
            .unwrap_or_default();
        // Confirma que a configuração não trocou de destino entre as leituras de config e segredos.
        let confirmed_cfg = self
            .load_enabled_config(integration_id)
            .map_err(|err| (ConnectFailure::Other(err), None, None, None))?;
        let confirmed_fp = config_fingerprint(&confirmed_cfg);
        if confirmed_fp != config_fp {
            return Err((
                ConnectFailure::Other(ClientError::Transport(
                    "configuração da integração alterada durante conexão; tente novamente".into(),
                )),
                Some(confirmed_cfg.auth),
                Some(confirmed_fp),
                None,
            ));
        }
        crate::model::validate_config_and_secrets(&config, &secrets).map_err(|msg| {
            (
                ConnectFailure::Other(ClientError::Protocol(msg)),
                Some(config.auth.clone()),
                Some(config_fp),
                None,
            )
        })?;
        let effective_oauth =
            resolve_effective_oauth_override(slot, config_fp, &secrets, override_oauth_token);
        let effective_oauth_str = effective_oauth.as_deref().map(String::as_str);
        let fingerprint = SessionFingerprint::compute(&config, &secrets, effective_oauth_str);

        {
            let active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(session) = active.as_ref()
                && session.fingerprint == fingerprint
                && !session.is_closed()
            {
                let in_flight = session.enter_call();
                let attempt = OAuthAttempt {
                    epoch: session.oauth_epoch,
                    token: session.oauth_token.clone(),
                };
                return Ok((config, session.clone(), in_flight, attempt));
            }
        }

        let _connect_guard = slot.connect_lock.lock().await;
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err((
                ConnectFailure::Other(ClientError::Transport(
                    "gerenciador de clientes encerrado".into(),
                )),
                Some(config.auth),
                Some(config_fp),
                None,
            ));
        }

        // Re-lê config e segredos após adquirir a trava para refletir edições concorrentes.
        let config = self
            .load_enabled_config(integration_id)
            .map_err(|err| (ConnectFailure::Other(err), None, None, None))?;
        let config_fp = config_fingerprint(&config);
        if expected_config_fp.is_some_and(|fp| fp != config_fp) {
            *slot
                .oauth_override
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
            return Err((
                ConnectFailure::Other(ClientError::AuthExpired(
                    "configuração da integração alterada durante retry OAuth".into(),
                )),
                Some(config.auth),
                Some(config_fp),
                None,
            ));
        }
        let secrets = self
            .inner
            .secrets
            .get(integration_id)
            .await
            .map_err(|err| {
                (
                    ConnectFailure::Other(ClientError::Secrets(err)),
                    Some(config.auth.clone()),
                    Some(config_fp),
                    None,
                )
            })?
            .unwrap_or_default();
        // Confirma que a configuração não trocou de destino entre as leituras de config e segredos.
        let confirmed_cfg = self
            .load_enabled_config(integration_id)
            .map_err(|err| (ConnectFailure::Other(err), None, None, None))?;
        let confirmed_fp = config_fingerprint(&confirmed_cfg);
        if confirmed_fp != config_fp {
            return Err((
                ConnectFailure::Other(ClientError::Transport(
                    "configuração da integração alterada durante conexão; tente novamente".into(),
                )),
                Some(confirmed_cfg.auth),
                Some(confirmed_fp),
                None,
            ));
        }
        crate::model::validate_config_and_secrets(&config, &secrets).map_err(|msg| {
            (
                ConnectFailure::Other(ClientError::Protocol(msg)),
                Some(config.auth.clone()),
                Some(config_fp),
                None,
            )
        })?;
        let effective_oauth =
            resolve_effective_oauth_override(slot, config_fp, &secrets, override_oauth_token);
        let effective_oauth_str = effective_oauth.as_deref().map(String::as_str);
        let fingerprint = SessionFingerprint::compute(&config, &secrets, effective_oauth_str);

        let previous = {
            let mut active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(session) = active.as_ref()
                && session.fingerprint == fingerprint
                && !session.is_closed()
            {
                let in_flight = session.enter_call();
                let attempt = OAuthAttempt {
                    epoch: session.oauth_epoch,
                    token: session.oauth_token.clone(),
                };
                return Ok((config, session.clone(), in_flight, attempt));
            }
            active.take()
        };
        if let Some(old) = previous {
            slot.stop_notify.notify_waiters();
            old.shutdown(self.inner.config.exit_grace, false).await;
        }

        let oauth_epoch = slot.refresh_epoch.load(Ordering::Acquire);
        let used_oauth_token = resolve_effective_oauth_token(
            slot,
            config_fp,
            &config.auth,
            &secrets,
            override_oauth_token,
        );
        let session = self
            .open_session(
                slot,
                &config,
                &secrets,
                fingerprint,
                effective_oauth_str,
                oauth_epoch,
                used_oauth_token.clone(),
            )
            .await
            .map_err(|err| {
                (
                    err,
                    Some(config.auth.clone()),
                    Some(config_fp),
                    Some(OAuthAttempt {
                        epoch: oauth_epoch,
                        token: used_oauth_token.clone(),
                    }),
                )
            })?;

        // `enter_call()` dentro de `slot.active.lock()` evita janela com `in_flight == 0` para o reaper.
        let (in_flight, should_spawn_reaper) = {
            let mut active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
            let in_flight = session.enter_call();
            *active = Some(session.clone());
            let should_spawn = !slot.reaper_active.swap(true, Ordering::AcqRel);
            (in_flight, should_spawn)
        };
        if should_spawn_reaper {
            self.spawn_reaper(slot);
        }
        Ok((
            config,
            session,
            in_flight,
            OAuthAttempt {
                epoch: oauth_epoch,
                token: used_oauth_token,
            },
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_session(
        &self,
        slot: &Slot,
        config: &IntegrationConfig,
        secrets: &IntegrationSecrets,
        fingerprint: SessionFingerprint,
        override_oauth_token: Option<&str>,
        oauth_epoch: u64,
        oauth_token: Option<Zeroizing<String>>,
    ) -> Result<Arc<ActiveSession>, ConnectFailure> {
        let connect_timeout = self.inner.config.connect_timeout;
        let pending_stdio = PendingStdioGuard::new();

        let connect_fut = connect_transport(
            config,
            secrets,
            override_oauth_token,
            pending_stdio.clone_slot(),
        );

        let (running, stdio_owned) = match tokio::time::timeout(connect_timeout, connect_fut).await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(failure)) => {
                if let Some(owned) = pending_stdio.take() {
                    finish_stdio_owned(owned, self.inner.config.exit_grace, true).await;
                }
                return Err(failure);
            }
            Err(_) => {
                if let Some(owned) = pending_stdio.take() {
                    finish_stdio_owned(owned, self.inner.config.exit_grace, true).await;
                }
                return Err(ConnectFailure::Other(ClientError::Timeout(format!(
                    "conexão à integração '{}' excedeu {:?}",
                    config.id, connect_timeout
                ))));
            }
        };

        // Desabilita cache interno de respostas do rmcp para que ToolsCache seja fonte única
        // e falhas/401 nunca sejam mascarados por respostas expiradas em cache.
        running
            .peer()
            .set_response_cache_config(ClientCacheConfig::disabled())
            .await;

        let peer = running.peer().clone();
        let generation = slot.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let (cancel, _) = watch::channel(false);

        Ok(Arc::new(ActiveSession {
            generation,
            fingerprint,
            oauth_epoch,
            oauth_token,
            peer,
            running: AsyncMutex::new(Some(running)),
            stdio: StdMutex::new(stdio_owned),
            in_flight: AtomicUsize::new(0),
            last_used: StdMutex::new(Instant::now()),
            idle_notify: Notify::new(),
            cancel,
        }))
    }

    fn spawn_reaper(&self, slot: &Arc<Slot>) {
        let weak_slot: Weak<Slot> = Arc::downgrade(slot);
        let weak_inner: Weak<ManagerInner> = Arc::downgrade(&self.inner);
        let idle_ttl = self.inner.config.idle_ttl;
        let exit_grace = self.inner.config.exit_grace;

        tokio::spawn(async move {
            loop {
                let Some(inner) = weak_inner.upgrade() else {
                    if let Some(slot) = weak_slot.upgrade() {
                        let _active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
                        slot.reaper_active.store(false, Ordering::Release);
                    }
                    return;
                };
                let shutting_down = inner.shutting_down.load(Ordering::Acquire);
                drop(inner);

                let Some(slot) = weak_slot.upgrade() else {
                    return;
                };
                if shutting_down {
                    let _active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
                    slot.reaper_active.store(false, Ordering::Release);
                    return;
                }

                // Registra e habilita `stop_notify` antes de inspecionar `slot.active` para não
                // perder `notify_waiters()` disparado antes do `select!`.
                let stop_notified = slot.stop_notify.notified();
                tokio::pin!(stop_notified);
                let _ = stop_notified.as_mut().enable();

                // A verificação de ativo e o desligamento da flag `reaper_active` compartilham o mesmo mutex.
                let session = {
                    let active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
                    let Some(session) = active.as_ref().cloned() else {
                        slot.reaper_active.store(false, Ordering::Release);
                        return;
                    };
                    session
                };

                let idle_notified = session.idle_notify.notified();
                tokio::pin!(idle_notified);
                let _ = idle_notified.as_mut().enable();
                if session.in_flight.load(Ordering::Acquire) > 0 {
                    tokio::select! {
                        _ = &mut idle_notified => {}
                        _ = &mut stop_notified => {}
                    }
                    continue;
                }

                let deadline = {
                    let last = *session
                        .last_used
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    last + idle_ttl
                };
                if Instant::now() < deadline {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => {}
                        _ = &mut stop_notified => {}
                    }
                    continue;
                }

                // Re-verifica e remove no mesmo guard de `slot.active` após adquirir `connect_lock`.
                let _connect_guard = slot.connect_lock.lock().await;
                let removed = {
                    let mut active = slot.active.lock().unwrap_or_else(PoisonError::into_inner);
                    let still_idle = active.as_ref().is_some_and(|cur| {
                        cur.generation == session.generation
                            && cur.in_flight.load(Ordering::Acquire) == 0
                            && (*cur.last_used.lock().unwrap_or_else(PoisonError::into_inner)
                                + idle_ttl
                                <= Instant::now())
                    });
                    if still_idle { active.take() } else { None }
                };
                if let Some(removed) = removed {
                    removed.shutdown(exit_grace, false).await;
                }
            }
        });
    }
}

impl Drop for ManagerInner {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::Release);
        let _ = self.shutdown_watch.send_replace(true);
        let slots = self.slots.get_mut().unwrap_or_else(PoisonError::into_inner);
        for slot in slots.values() {
            slot.stop_notify.notify_waiters();
            let Some(session) = slot
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            else {
                continue;
            };
            let _ = session.cancel.send_replace(true);
            let owned = session
                .stdio
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(owned) = owned {
                spawn_stdio_cleanup(owned, Duration::ZERO, true);
            }
        }
    }
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        let _ = self.cancel.send_replace(true);
        let owned = self
            .stdio
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(owned) = owned {
            spawn_stdio_cleanup(owned, Duration::ZERO, true);
        }
    }
}

impl ActiveSession {
    fn is_closed(&self) -> bool {
        self.peer.is_transport_closed()
    }

    fn enter_call(self: &Arc<Self>) -> InFlightGuard {
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        *self
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
        InFlightGuard {
            session: Arc::clone(self),
        }
    }

    async fn shutdown(&self, grace: Duration, force_kill: bool) {
        let _ = self.cancel.send_replace(true);
        let close_wait = if force_kill {
            Duration::from_millis(20)
        } else {
            grace
        };
        if let Some(mut running) = self.running.lock().await.take() {
            let _ = running.close_with_timeout(close_wait).await;
        }
        let owned = self
            .stdio
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(owned) = owned {
            let _ = tokio::spawn(finish_stdio_owned(owned, grace, force_kill)).await;
        }
    }
}

fn spawn_stdio_cleanup(mut owned: StdioOwned, grace: Duration, force_kill: bool) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(finish_stdio_owned(owned, grace, force_kill));
        }
        Err(_) => {
            let _ = owned.child.start_kill();
        }
    }
}

fn take_pending_stdio(pending: &StdMutex<Option<StdioOwned>>) -> Option<StdioOwned> {
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

async fn finish_stdio_owned(mut owned: StdioOwned, grace: Duration, force_kill: bool) {
    if !force_kill && grace > Duration::ZERO {
        let _ = tokio::time::timeout(grace, leader_exited(&mut owned.child)).await;
    }
    finish(&mut owned.tree).await;
    let _ = owned.child.wait().await;
}

fn update_len_prefixed(ctx: &mut DigestContext, bytes: &[u8]) {
    ctx.update(&(bytes.len() as u64).to_le_bytes());
    ctx.update(bytes);
}

impl SessionFingerprint {
    fn compute(
        config: &IntegrationConfig,
        secrets: &IntegrationSecrets,
        override_oauth_token: Option<&str>,
    ) -> Self {
        let mut ctx = DigestContext::new(&SHA256);
        match &config.transport {
            Transport::RemoteHttp { url, header_names } => {
                update_len_prefixed(&mut ctx, b"remote_http");
                update_len_prefixed(&mut ctx, url.as_bytes());
                ctx.update(&(header_names.len() as u64).to_le_bytes());
                for name in header_names {
                    update_len_prefixed(&mut ctx, name.as_bytes());
                }
            }
            Transport::LocalStdio {
                command,
                cwd,
                env_keys,
            } => {
                update_len_prefixed(&mut ctx, b"local_stdio");
                ctx.update(&(command.len() as u64).to_le_bytes());
                for arg in command {
                    update_len_prefixed(&mut ctx, arg.as_bytes());
                }
                match cwd {
                    Some(dir) => {
                        ctx.update(&[1]);
                        update_len_prefixed(&mut ctx, dir.as_os_str().as_encoded_bytes());
                    }
                    None => ctx.update(&[0]),
                }
                ctx.update(&(env_keys.len() as u64).to_le_bytes());
                for key in env_keys {
                    update_len_prefixed(&mut ctx, key.as_bytes());
                }
            }
        }
        match &config.auth {
            AuthConfig::None => update_len_prefixed(&mut ctx, b"auth_none"),
            AuthConfig::Bearer => update_len_prefixed(&mut ctx, b"auth_bearer"),
            AuthConfig::OAuthDcr => update_len_prefixed(&mut ctx, b"auth_oauth_dcr"),
            AuthConfig::OAuthManualClient { client_id, scopes } => {
                update_len_prefixed(&mut ctx, b"auth_oauth_manual");
                update_len_prefixed(&mut ctx, client_id.trim().as_bytes());
                ctx.update(&(scopes.len() as u64).to_le_bytes());
                for scope in scopes {
                    update_len_prefixed(&mut ctx, scope.as_bytes());
                }
            }
        }
        match &secrets.bearer_token {
            Some(bearer) => {
                ctx.update(&[1]);
                update_len_prefixed(&mut ctx, bearer.as_bytes());
            }
            None => ctx.update(&[0]),
        }
        if let Some(token) = override_oauth_token {
            ctx.update(&[1]);
            update_len_prefixed(&mut ctx, token.as_bytes());
        } else if let Some(oauth) = &secrets.oauth {
            ctx.update(&[2]);
            update_len_prefixed(&mut ctx, oauth.access_token.as_bytes());
        } else {
            ctx.update(&[0]);
        }
        ctx.update(&(secrets.headers.len() as u64).to_le_bytes());
        for (k, v) in &secrets.headers {
            update_len_prefixed(&mut ctx, k.as_bytes());
            update_len_prefixed(&mut ctx, v.as_bytes());
        }
        ctx.update(&(secrets.env.len() as u64).to_le_bytes());
        for (k, v) in &secrets.env {
            update_len_prefixed(&mut ctx, k.as_bytes());
            update_len_prefixed(&mut ctx, v.as_bytes());
        }
        let digest = ctx.finish();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(digest.as_ref());
        Self {
            transport: config.transport.clone(),
            auth: config.auth.clone(),
            secrets_digest: bytes,
        }
    }
}

fn client_info() -> McpClientInfo {
    McpClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("hangar-integrations", env!("CARGO_PKG_VERSION")),
    )
}

fn resolve_effective_oauth_override(
    slot: &Slot,
    config_fp: [u8; 32],
    secrets: &IntegrationSecrets,
    explicit_override: Option<&str>,
) -> Option<Zeroizing<String>> {
    if let Some(token) = explicit_override.filter(|t| !t.is_empty()) {
        return Some(Zeroizing::new(token.to_owned()));
    }
    let current_secret = secrets
        .oauth
        .as_ref()
        .map(|o| o.access_token.as_str())
        .unwrap_or("");
    let mut guard = slot
        .oauth_override
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(entry) = guard.as_ref() {
        if entry.config_fingerprint == config_fp && entry.stale_token.as_str() == current_secret {
            return Some(entry.fresh_token.clone());
        }
        // O segredo em disco ou a configuração da integração mudou; descarta o override antigo.
        *guard = None;
    }
    None
}

fn resolve_effective_oauth_token(
    slot: &Slot,
    config_fp: [u8; 32],
    auth: &AuthConfig,
    secrets: &IntegrationSecrets,
    explicit_override: Option<&str>,
) -> Option<Zeroizing<String>> {
    if !is_oauth_auth(auth) {
        return None;
    }
    if let Some(override_tok) =
        resolve_effective_oauth_override(slot, config_fp, secrets, explicit_override)
    {
        return Some(override_tok);
    }
    secrets
        .oauth
        .as_ref()
        .filter(|o| {
            !o.access_token.is_empty()
                && o.config_fingerprint.is_none_or(|fp| fp == config_fp)
        })
        .map(|o| o.access_token.clone())
}

async fn connect_transport(
    config: &IntegrationConfig,
    secrets: &IntegrationSecrets,
    override_oauth_token: Option<&str>,
    pending_stdio: Arc<StdMutex<Option<StdioOwned>>>,
) -> Result<
    (
        RunningService<RoleClient, McpClientInfo>,
        Option<StdioOwned>,
    ),
    ConnectFailure,
> {
    match &config.transport {
        Transport::LocalStdio { command, cwd, .. } => {
            connect_stdio(command, cwd.as_deref(), secrets, pending_stdio).await
        }
        Transport::RemoteHttp { url, .. } => {
            let config_fp = config_fingerprint(config);
            connect_http(url, &config.auth, config_fp, secrets, override_oauth_token).await
        }
    }
}

async fn connect_stdio(
    command: &[String],
    cwd: Option<&std::path::Path>,
    secrets: &IntegrationSecrets,
    pending_stdio: Arc<StdMutex<Option<StdioOwned>>>,
) -> Result<
    (
        RunningService<RoleClient, McpClientInfo>,
        Option<StdioOwned>,
    ),
    ConnectFailure,
> {
    if command.is_empty() {
        return Err(ConnectFailure::Other(ClientError::Protocol(
            "comando stdio vazio".into(),
        )));
    }
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    // O filho não herda credenciais internas do servidor mesmo se passadas em secrets.env.
    for key in PRIVATE_ENV_KEYS {
        cmd.env_remove(key);
    }
    for (key, value) in &secrets.env {
        if PRIVATE_ENV_KEYS
            .iter()
            .any(|private| private.eq_ignore_ascii_case(key))
        {
            continue;
        }
        cmd.env(key, value.as_str());
    }

    let mut tree = CommandTree::configure(&mut cmd).map_err(|err| {
        ConnectFailure::Other(ClientError::Transport(format!(
            "falha ao configurar contenção do processo: {:?}",
            err.kind()
        )))
    })?;
    die_with_parent(&mut cmd);

    let mut child = cmd.spawn().map_err(|err| {
        ConnectFailure::Other(ClientError::Transport(format!(
            "falha ao iniciar processo stdio: {:?}",
            err.kind()
        )))
    })?;
    if let Err(err) = tree.attach(&child) {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(ConnectFailure::Other(ClientError::Transport(format!(
            "falha ao anexar processo à contenção: {:?}",
            err.kind()
        ))));
    }
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let mut owned = StdioOwned { child, tree };
        finish(&mut owned.tree).await;
        let _ = owned.child.wait().await;
        return Err(ConnectFailure::Other(ClientError::Transport(
            "processo stdio sem entrada ou saída padrão".into(),
        )));
    };

    *pending_stdio.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(StdioOwned { child, tree });

    let transport = AsyncRwTransport::new_client(stdout, stdin);
    let running = client_info()
        .serve(transport)
        .await
        .map_err(map_init_error)?;
    let owned = take_pending_stdio(&pending_stdio);
    Ok((running, owned))
}

async fn connect_http(
    url: &str,
    auth: &AuthConfig,
    config_fp: [u8; 32],
    secrets: &IntegrationSecrets,
    override_oauth_token: Option<&str>,
) -> Result<
    (
        RunningService<RoleClient, McpClientInfo>,
        Option<StdioOwned>,
    ),
    ConnectFailure,
> {
    let auth_header = resolve_http_auth_header(auth, config_fp, secrets, override_oauth_token)
        .map_err(ConnectFailure::Other)?;
    let custom_headers = build_custom_headers(secrets).map_err(ConnectFailure::Other)?;

    let mut http_config =
        StreamableHttpClientTransportConfig::with_uri(url).custom_headers(custom_headers);
    if let Some(token) = auth_header {
        http_config = http_config.auth_header(token);
    }

    let transport = StreamableHttpClientTransport::from_config(http_config);
    let running = client_info()
        .serve(transport)
        .await
        .map_err(map_init_error)?;
    Ok((running, None))
}

fn is_oauth_auth(auth: &AuthConfig) -> bool {
    matches!(
        auth,
        AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. }
    )
}

fn resolve_http_auth_header(
    auth: &AuthConfig,
    config_fp: [u8; 32],
    secrets: &IntegrationSecrets,
    override_oauth_token: Option<&str>,
) -> Result<Option<String>, ClientError> {
    match auth {
        AuthConfig::None => Ok(None),
        AuthConfig::Bearer => {
            let Some(token) = secrets.bearer_token.as_ref().filter(|t| !t.is_empty()) else {
                return Err(ClientError::AuthRequired(
                    "token Bearer não configurado".into(),
                ));
            };
            Ok(Some(token.as_str().to_owned()))
        }
        AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. } => {
            if let Some(token) = override_oauth_token.filter(|t| !t.is_empty()) {
                return Ok(Some(token.to_owned()));
            }
            let Some(oauth) = secrets
                .oauth
                .as_ref()
                .filter(|t| !t.access_token.is_empty())
            else {
                return Err(ClientError::AuthRequired(
                    "autenticação OAuth necessária".into(),
                ));
            };
            // Rejeita token OAuth vinculado a outro destino/auth mesmo antes da limpeza pós-PATCH.
            if oauth.config_fingerprint.is_some_and(|fp| fp != config_fp) {
                return Err(ClientError::AuthRequired(
                    "token OAuth pertence a outra configuração da integração".into(),
                ));
            }
            Ok(Some(oauth.access_token.as_str().to_owned()))
        }
    }
}

fn build_custom_headers(
    secrets: &IntegrationSecrets,
) -> Result<HashMap<HeaderName, HeaderValue>, ClientError> {
    let mut headers = HashMap::with_capacity(secrets.headers.len());
    for (raw_name, raw_value) in &secrets.headers {
        let name = HeaderName::from_bytes(raw_name.as_bytes()).map_err(|err| {
            ClientError::Protocol(format!(
                "nome de cabeçalho HTTP inválido '{raw_name}': {err}"
            ))
        })?;
        if RESERVED_HTTP_HEADERS
            .iter()
            .any(|reserved| name.as_str().eq_ignore_ascii_case(reserved))
        {
            return Err(ClientError::Protocol(format!(
                "cabeçalho HTTP reservado não permitido em headers extras: {raw_name}"
            )));
        }
        let value = HeaderValue::from_str(raw_value.as_str()).map_err(|err| {
            ClientError::Protocol(format!(
                "valor de cabeçalho HTTP inválido para '{raw_name}': {err}"
            ))
        })?;
        headers.insert(name, value);
    }
    Ok(headers)
}

async fn list_all_tools_paginated(peer: &Peer<RoleClient>) -> Result<Vec<Tool>, OpError> {
    let mut tools = Vec::new();
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    loop {
        let params = cursor
            .take()
            .map(|c| PaginatedRequestParams::default().with_cursor(Some(c)));
        let page = peer
            .list_tools(params)
            .await
            .map_err(|err| map_service_error(err, None))?;
        tools.extend(page.tools);
        match page.next_cursor {
            Some(next) if !next.is_empty() => {
                if !seen_cursors.insert(next.clone()) {
                    return Err(OpError::Client(ClientError::Protocol(format!(
                        "cursor de paginação repetido: {next}"
                    ))));
                }
                cursor = Some(next);
            }
            _ => break,
        }
    }
    Ok(tools)
}

fn discovered_from_rmcp(tool: Tool) -> DiscoveredTool {
    DiscoveredTool {
        name: tool.name.into_owned(),
        description: tool.description.map(|d| d.into_owned()),
        input_schema: serde_json::Value::Object(tool.input_schema.as_ref().clone()),
        read_only_hint: tool
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
    }
}

fn build_call_tool_params(
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<CallToolRequestParams, ClientError> {
    let mut params = CallToolRequestParams::new(tool_name.to_owned());
    match arguments {
        serde_json::Value::Null => {}
        serde_json::Value::Object(map) => {
            params = params.with_arguments(map);
        }
        _ => {
            return Err(ClientError::Protocol(
                "argumentos da ferramenta devem ser um objeto JSON".into(),
            ));
        }
    }
    Ok(params)
}

fn extract_call_output(result: CallToolResult) -> McpCallOutput {
    let mut parts = Vec::with_capacity(result.content.len());
    for block in result.content {
        match block {
            ContentBlock::Text(text) => {
                if !text.text.is_empty() {
                    parts.push(text.text);
                }
            }
            ContentBlock::Image(image) => {
                parts.push(format!(
                    "[imagem {} ({} bytes base64)]",
                    image.mime_type,
                    image.data.len()
                ));
            }
            ContentBlock::Audio(audio) => {
                parts.push(format!(
                    "[áudio {} ({} bytes base64)]",
                    audio.mime_type,
                    audio.data.len()
                ));
            }
            ContentBlock::Resource(embedded) => match embedded.resource {
                ResourceContents::TextResourceContents { text, uri, .. } => {
                    if !text.is_empty() {
                        parts.push(text);
                    } else {
                        parts.push(format!("[recurso {uri}]"));
                    }
                }
                ResourceContents::BlobResourceContents { uri, mime_type, .. } => {
                    parts.push(format!(
                        "[recurso binário {uri} ({})]",
                        mime_type.as_deref().unwrap_or("application/octet-stream")
                    ));
                }
                _ => parts.push("[recurso]".to_owned()),
            },
            ContentBlock::ResourceLink(link) => {
                parts.push(format!("[link {}: {}]", link.name, link.uri));
            }
            _ => {}
        }
    }
    let text = if parts.is_empty() {
        result
            .structured_content
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_default()
    } else {
        parts.join("\n")
    };
    McpCallOutput {
        text,
        is_error: result.is_error.unwrap_or(false),
        structured_content: result.structured_content,
    }
}

fn map_init_error(error: ClientInitializeError) -> ConnectFailure {
    match error {
        ClientInitializeError::TransportError { error, context } => {
            if is_http_401_error(error.error.as_ref()) {
                ConnectFailure::Http401(format!("{context}: {}", error.error))
            } else {
                ConnectFailure::Other(ClientError::Transport(format!(
                    "{context}: {}",
                    error.error
                )))
            }
        }
        ClientInitializeError::JsonRpcError(data) => {
            ConnectFailure::Other(ClientError::RemoteError {
                code: data.code.0,
                message: data.message.into_owned(),
            })
        }
        ClientInitializeError::ConnectionClosed(msg) => {
            ConnectFailure::Other(ClientError::Transport(msg))
        }
        ClientInitializeError::Cancelled => {
            ConnectFailure::Other(ClientError::Transport("inicialização cancelada".into()))
        }
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => map_init_error(*fallback),
        other => ConnectFailure::Other(ClientError::Protocol(other.to_string())),
    }
}

fn map_service_error(error: ServiceError, tool_name: Option<&str>) -> OpError {
    match error {
        ServiceError::McpError(data) => {
            let lower = data.message.to_ascii_lowercase();
            if let Some(name) = tool_name
                && (data.code == rmcp::model::ErrorCode::INVALID_PARAMS
                    || data.code == rmcp::model::ErrorCode::METHOD_NOT_FOUND)
                && (lower.contains("tool not found") || lower.contains("unknown tool"))
            {
                return OpError::Client(ClientError::ToolNotFound(name.to_owned()));
            }
            OpError::Client(ClientError::RemoteError {
                code: data.code.0,
                message: data.message.into_owned(),
            })
        }
        ServiceError::TransportSend(dyn_err) => {
            if is_http_401_error(dyn_err.error.as_ref()) {
                OpError::Unauthorized(dyn_err.to_string())
            } else {
                OpError::FatalTransport(dyn_err.to_string())
            }
        }
        ServiceError::TransportClosed => {
            OpError::FatalTransport("transporte encerrado pelo servidor".into())
        }
        ServiceError::Timeout { timeout } => {
            OpError::Timeout(format!("tempo esgotado após {:?}", timeout))
        }
        ServiceError::Cancelled { reason } => {
            OpError::FatalTransport(reason.unwrap_or_else(|| "operação cancelada".into()))
        }
        other => OpError::Client(ClientError::Protocol(other.to_string())),
    }
}

fn is_http_401_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(err) = current {
        if err.is::<AuthRequiredError>() {
            return true;
        }
        if let Some(http_err) = err.downcast_ref::<StreamableHttpError<reqwest::Error>>() {
            match http_err {
                StreamableHttpError::AuthRequired(_) => return true,
                StreamableHttpError::Client(req_err)
                    if req_err.status() == Some(reqwest::StatusCode::UNAUTHORIZED) =>
                {
                    return true;
                }
                StreamableHttpError::UnexpectedServerResponse(msg)
                    if msg.starts_with("HTTP 401") =>
                {
                    return true;
                }
                _ => {}
            }
        }
        if let Some(req_err) = err.downcast_ref::<reqwest::Error>()
            && req_err.status() == Some(reqwest::StatusCode::UNAUTHORIZED)
        {
            return true;
        }
        current = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use zeroize::Zeroizing;

    use super::SessionFingerprint;
    use crate::model::{AuthConfig, IntegrationConfig, PermissionPolicy, Transport};
    use crate::secrets::IntegrationSecrets;

    fn base_stdio_config(cmd: Vec<&str>, cwd: Option<&str>) -> IntegrationConfig {
        IntegrationConfig {
            id: "fp-test".to_owned(),
            name: "FP Test".to_owned(),
            preset: None,
            enabled: true,
            transport: Transport::LocalStdio {
                command: cmd.into_iter().map(str::to_owned).collect(),
                cwd: cwd.map(PathBuf::from),
                env_keys: vec!["A".to_owned(), "B".to_owned()],
            },
            auth: AuthConfig::None,
            policy: PermissionPolicy::AllowAll,
            pinned: false,
            projects: vec![],
            allowed_tools: BTreeSet::new(),
            created_at: 1.0,
            updated_at: 1.0,
        }
    }

    #[test]
    fn session_fingerprint_length_prefixes_variable_fields_against_boundary_collisions() {
        let cfg = base_stdio_config(vec!["node", "srv.js"], Some("/tmp/a"));
        let sec_a = IntegrationSecrets {
            env: BTreeMap::from([
                ("a".to_owned(), Zeroizing::new("xenv:b=y".to_owned())),
                ("b".to_owned(), Zeroizing::new("z".to_owned())),
            ]),
            headers: BTreeMap::from([
                ("x-a".to_owned(), Zeroizing::new("vhdr:x-b=1".to_owned())),
                ("x-b".to_owned(), Zeroizing::new("2".to_owned())),
            ]),
            bearer_token: Some(Zeroizing::new("tok-1".to_owned())),
            ..Default::default()
        };
        let sec_b = IntegrationSecrets {
            env: BTreeMap::from([
                ("a".to_owned(), Zeroizing::new("x".to_owned())),
                ("b".to_owned(), Zeroizing::new("yenv:b=z".to_owned())),
            ]),
            headers: BTreeMap::from([
                ("x-a".to_owned(), Zeroizing::new("v".to_owned())),
                ("x-b".to_owned(), Zeroizing::new("1hdr:x-b=2".to_owned())),
            ]),
            bearer_token: Some(Zeroizing::new("tok-1".to_owned())),
            ..Default::default()
        };
        assert!(SessionFingerprint::compute(&cfg, &sec_a, None) == SessionFingerprint::compute(&cfg, &sec_a, None));
        assert!(SessionFingerprint::compute(&cfg, &sec_a, None) != SessionFingerprint::compute(&cfg, &sec_b, None));

        let cmd_a = base_stdio_config(vec!["ab", "c"], None);
        let cmd_b = base_stdio_config(vec!["a", "bc"], None);
        assert!(
            SessionFingerprint::compute(&cmd_a, &IntegrationSecrets::default(), None)
                != SessionFingerprint::compute(&cmd_b, &IntegrationSecrets::default(), None)
        );
    }
}
