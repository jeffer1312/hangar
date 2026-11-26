//! Serviço de integrações MCP externas no servidor (catálogo, segredos, OAuth, cliente e aprovações).

pub mod http;
pub mod keyring;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hangar_integrations::{
    ApprovalConfig, ApprovalStore, ClientConfig, IntegrationRegistry, McpClientManager,
    OAuthManager, SecretKeyStore, SecretStore, ToolsCache,
};

const VOICE_CONFIRMATION_TTL: Duration = Duration::from_secs(120);

type VoiceConfirmationKey = (String, Option<String>, String, String, String);

/// Serviço compartilhado em [`crate::routes::AppState`] para gerenciar integrações MCP externas.
pub struct IntegrationsService {
    pub registry: Arc<IntegrationRegistry>,
    pub secrets: Arc<SecretStore>,
    pub tools_cache: Arc<ToolsCache>,
    pub oauth: Arc<OAuthManager>,
    pub clients: Arc<McpClientManager>,
    pub approvals: Arc<ApprovalStore>,
    /// Porta local real vinculada no listener (0 quando ainda não servida via `serve_with_guest`).
    local_port: AtomicU16,
    /// Última falha operacional em memória por integração (limpa ao reconectar/atualizar/testar).
    last_errors: Mutex<HashMap<String, String>>,
    /// Último aviso de cancelamento em voo por integração (ex.: chamada cujo efeito remoto é desconhecido).
    last_cancel_warnings: Mutex<HashMap<String, String>>,
    /// Travas por integração para serializar mutações administrativas (`create`, `patch`, `delete`, `AllowAlways`).
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Confirmações de uso único emitidas para chamadas de escrita pela voz, indexadas por
    /// `(voice_token, target_session, integration_id, tool_name, args_hash)`.
    voice_confirmations: Mutex<HashMap<VoiceConfirmationKey, Instant>>,
}

impl IntegrationsService {
    /// Cria o serviço usando o chaveiro isolado do sistema operacional.
    pub fn system(hangar_dir: &Path) -> Self {
        Self::with_key_store(hangar_dir, keyring::system())
    }

    /// Cria o serviço usando um [`SecretKeyStore`] explícito (ex.: memória em testes).
    pub fn with_key_store(hangar_dir: &Path, key_store: Arc<dyn SecretKeyStore>) -> Self {
        Self::with_options(
            hangar_dir,
            key_store,
            ClientConfig::default(),
            hangar_integrations::DEFAULT_STATE_TTL,
            ApprovalConfig::default(),
        )
    }

    /// Cria o serviço com parâmetros configuráveis de cliente, OAuth e aprovações.
    pub fn with_options(
        hangar_dir: &Path,
        key_store: Arc<dyn SecretKeyStore>,
        client_config: ClientConfig,
        oauth_state_ttl: Duration,
        approval_config: ApprovalConfig,
    ) -> Self {
        let registry = Arc::new(IntegrationRegistry::new(hangar_dir));
        let secrets = Arc::new(SecretStore::new(hangar_dir, key_store));
        let tools_cache = Arc::new(ToolsCache::new());
        let oauth = Arc::new(
            OAuthManager::new(registry.clone(), secrets.clone()).with_state_ttl(oauth_state_ttl),
        );
        let clients = Arc::new(
            McpClientManager::new(
                registry.clone(),
                secrets.clone(),
                tools_cache.clone(),
                client_config,
            )
            .with_refresher(oauth.clone()),
        );
        let approvals = Arc::new(ApprovalStore::with_config(approval_config));
        Self {
            registry,
            secrets,
            tools_cache,
            oauth,
            clients,
            approvals,
            local_port: AtomicU16::new(0),
            last_errors: Mutex::new(HashMap::new()),
            last_cancel_warnings: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            voice_confirmations: Mutex::new(HashMap::new()),
        }
    }

    /// Registra a porta real vinculada após `listener.local_addr()`.
    pub fn set_local_port(&self, port: u16) {
        self.local_port.store(port, Ordering::Relaxed);
    }

    /// Retorna a porta local real registrada (ou `0` se ainda não vinculada).
    pub fn local_port(&self) -> u16 {
        self.local_port.load(Ordering::Relaxed)
    }

    /// Serializa mutações administrativas (`create`, `patch`, `delete`, `AllowAlways`) por identificador.
    pub async fn lock_integration(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut guard = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
            guard
                .entry(id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    /// Registra autorização pendente de voz vinculada ao token de voz, sessão alvo, integração, ferramenta e argumentos.
    pub fn record_voice_confirmation(
        &self,
        voice_token: &str,
        target_session: Option<&str>,
        integration_id: &str,
        tool_name: &str,
        args_hash: String,
    ) {
        let now = Instant::now();
        let mut guard = self
            .voice_confirmations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.retain(|_, expires_at| *expires_at > now);
        guard.insert(
            (
                voice_token.to_owned(),
                target_session.map(str::to_owned),
                integration_id.to_owned(),
                tool_name.to_owned(),
                args_hash,
            ),
            now + VOICE_CONFIRMATION_TTL,
        );
    }

    /// Consome atomicamente uma confirmação de voz prévia se presente para o mesmo escopo e dentro do prazo.
    pub fn consume_voice_confirmation(
        &self,
        voice_token: &str,
        target_session: Option<&str>,
        integration_id: &str,
        tool_name: &str,
        args_hash: &str,
    ) -> bool {
        let now = Instant::now();
        let mut guard = self
            .voice_confirmations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.retain(|_, expires_at| *expires_at > now);
        let key = (
            voice_token.to_owned(),
            target_session.map(str::to_owned),
            integration_id.to_owned(),
            tool_name.to_owned(),
            args_hash.to_owned(),
        );
        guard.remove(&key).is_some()
    }

    /// Remove confirmações de voz pendentes da integração após alteração sensível ou exclusão.
    pub fn clear_voice_confirmations(&self, integration_id: &str) {
        self.voice_confirmations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(_, _, id, _, _), _| id != integration_id);
    }

    /// Registra a última mensagem de falha operacional em memória (sem segredos).
    pub fn record_error(&self, id: &str, message: impl Into<String>) {
        self.last_errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), message.into());
    }

    /// Limpa o erro operacional em memória registrado para `id` (sem apagar aviso de cancelamento em voo).
    pub fn clear_error(&self, id: &str) {
        self.last_errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    /// Retorna uma cópia instantânea dos erros em memória por integração.
    pub fn last_errors_snapshot(&self) -> HashMap<String, String> {
        self.last_errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Registra aviso de chamada externa cancelada em voo com efeito remoto desconhecido.
    pub fn record_cancel_warning(&self, id: &str, tool: &str) {
        let msg = format!(
            "integration_call_cancelled_in_flight: a última chamada externa ('{tool}') foi cancelada em voo e o efeito remoto é desconhecido; confira antes de repetir"
        );
        self.last_cancel_warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), msg);
    }

    /// Limpa o aviso de cancelamento em voo registrado para `id`.
    pub fn clear_cancel_warning(&self, id: &str) {
        self.last_cancel_warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    /// Retorna uma cópia instantânea dos avisos de cancelamento em voo por integração.
    pub fn cancel_warnings_snapshot(&self) -> HashMap<String, String> {
        self.last_cancel_warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Encerra todas as conexões MCP ativas e aguarda limpeza dos processos filhos.
    pub async fn shutdown(&self) {
        self.clients.shutdown().await;
    }
}
