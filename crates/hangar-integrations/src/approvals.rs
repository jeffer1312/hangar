//! Fila em memória de aprovações de escrita das integrações MCP (sem segredos).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::Instant;

use hangar_core::ids::random_hex;

use crate::model::{ApprovalDecision, valid_tool_name};

/// Prazo padrão para o dono responder a um pedido de aprovação (10 minutos).
pub const APPROVAL_EXPIRES: Duration = Duration::from_secs(600);
/// Limite global de pedidos abertos simultaneamente em memória.
pub const MAX_OPEN: usize = 32;
/// Limite de pedidos abertos simultaneamente por sessão.
pub const MAX_OPEN_PER_SESSION: usize = 3;
/// Cooldown aplicado à sessão após uma negação explícita do dono (30 segundos).
pub const COOLDOWN: Duration = Duration::from_secs(30);
/// Tamanho máximo em caracteres de `arguments_preview`.
pub const MAX_ARGUMENTS_PREVIEW_CHARS: usize = 2000;
/// Tamanho máximo em caracteres de `reason`.
pub const MAX_REASON_CHARS: usize = 2048;

const DEFAULT_RETENTION: Duration = Duration::from_secs(60);
const MAX_ORIGIN_CHARS: usize = 256;
const MAX_SESSION_KEY_CHARS: usize = 256;
const MAX_INTEGRATION_ID_BYTES: usize = 128;
const MAX_INTEGRATION_NAME_BYTES: usize = 200;

/// Configuração de prazos da fila de aprovações.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovalConfig {
    pub expires: Duration,
    pub cooldown: Duration,
    pub retention: Duration,
}

/// Cartão público de pedido de aprovação exibido na lista SSE do dono.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCard {
    pub id: String,
    pub interaction_id: String,
    pub origin: String,
    pub integration_id: String,
    pub integration_name: String,
    pub tool: String,
    pub arguments_preview: String,
    pub reason: String,
    pub expires_at: f64,
}

/// Snapshot publicado no evento SSE `integrations_approvals`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApprovalFeed {
    pub requests: Vec<ApprovalCard>,
}

/// Desfecho observado pelo chamador que aguarda a aprovação da ferramenta.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolution {
    AllowOnce,
    AllowAlways,
    Denied,
    TimedOut,
    Cancelled,
}

/// Erros de abertura, resposta ou cancelamento na fila de aprovações.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalError {
    Invalid(String),
    Lost,
    Stale,
    Answered,
    TooMany,
    Cooldown,
}

/// Parâmetros de entrada para abrir um pedido de aprovação de escrita.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenApprovalRequest {
    pub origin: String,
    pub session_key: String,
    pub integration_id: String,
    pub integration_name: String,
    pub tool: String,
    pub arguments_preview: String,
    pub reason: String,
}

/// Resultado imediato da resposta do dono para que o chamador persista `AllowAlways`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnsweredApproval {
    pub integration_id: String,
    pub tool: String,
    pub decision: ApprovalDecision,
}

/// Reserva em duas fases de uma resposta ao cartão; devolve o pedido para a fila no `Drop` se não confirmada.
pub struct ClaimedApproval {
    inner: Arc<StoreInner>,
    request_id: String,
    integration_id: String,
    tool: String,
    decision: ApprovalDecision,
    armed: bool,
}

struct PendingRequest {
    card: ApprovalCard,
    seq: u64,
    session_key: String,
    deadline: Instant,
    /// Reserva em curso durante a gravação de `AllowAlways`; segunda resposta recebe `Answered`.
    claimed: bool,
    outcome: Option<ApprovalResolution>,
    tx: watch::Sender<Option<ApprovalResolution>>,
    /// Acorda `run_deadline` ao confirmar ou liberar uma reserva sem mexer em `tx`.
    state_tx: watch::Sender<()>,
}

#[derive(Default)]
struct StoreState {
    requests: HashMap<String, PendingRequest>,
    cooldown: HashMap<String, Instant>,
    seq: u64,
}

struct StoreInner {
    config: ApprovalConfig,
    feed: watch::Sender<Arc<ApprovalFeed>>,
    state: Mutex<StoreState>,
}

/// Armazenamento em memória da fila de aprovações de escrita das integrações MCP.
#[derive(Clone)]
pub struct ApprovalStore {
    inner: Arc<StoreInner>,
}

/// Alça de espera vinculada a um cartão aberto; cancela o cartão no `Drop` se ainda pendente.
pub struct ApprovalWaiter {
    inner: Weak<StoreInner>,
    card: ApprovalCard,
    rx: watch::Receiver<Option<ApprovalResolution>>,
    disarmed: bool,
}

impl ApprovalStore {
    /// Cria uma fila com os prazos padrão de produção.
    pub fn new() -> Self {
        Self::with_config(ApprovalConfig::default())
    }

    /// Cria uma fila com prazos configuráveis (ex.: prazos curtos em testes).
    pub fn with_config(config: ApprovalConfig) -> Self {
        let (feed, _) = watch::channel(Arc::default());
        Self {
            inner: Arc::new(StoreInner {
                config,
                feed,
                state: Mutex::default(),
            }),
        }
    }

    /// Canal reativo publicado sempre que os cartões abertos mudam.
    pub fn feed(&self) -> watch::Receiver<Arc<ApprovalFeed>> {
        self.inner.feed.subscribe()
    }

    /// Quantidade atual de cartões pendentes na fila.
    pub fn open_count(&self) -> usize {
        lock_state(&self.inner.state)
            .requests
            .values()
            .filter(|req| req.outcome.is_none())
            .count()
    }

    /// Abre um cartão na fila e devolve a alça que aguarda o desfecho.
    ///
    /// # Errors
    ///
    /// Retorna [`ApprovalError::Invalid`] quando algum campo viola o contrato ou não há
    /// runtime Tokio ativo para agendar a expiração, [`ApprovalError::TooMany`] se o teto global
    /// ou por sessão foi atingido, ou [`ApprovalError::Cooldown`] se a sessão acabou de ter uma
    /// negação explícita.
    pub fn open_request(
        &self,
        params: OpenApprovalRequest,
    ) -> Result<ApprovalWaiter, ApprovalError> {
        validate_request(&params)?;
        // Garante runtime Tokio antes de publicar o cartão para nunca deixar pedido sem expiração.
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            ApprovalError::Invalid("runtime Tokio indisponível para agendar expiração".into())
        })?;

        let id = random_hex(16);
        let interaction_id = random_hex(16);
        let now = Instant::now();
        let expires_at = now_unix_secs() + self.inner.config.expires.as_secs_f64();
        let deadline = now + self.inner.config.expires;

        let card = ApprovalCard {
            id: id.clone(),
            interaction_id,
            origin: params.origin.trim().to_owned(),
            integration_id: params.integration_id.trim().to_owned(),
            integration_name: params.integration_name.trim().to_owned(),
            // Preserva o nome exato da ferramenta para coincidir com `AllowAlways` na persistência.
            tool: params.tool,
            arguments_preview: truncate_preview_chars(
                &params.arguments_preview.replace('\0', ""),
                MAX_ARGUMENTS_PREVIEW_CHARS,
            ),
            reason: params.reason.trim().to_owned(),
            expires_at,
        };

        let (tx, rx) = watch::channel(None);
        let (state_tx, _) = watch::channel(());

        {
            let mut state = lock_state(&self.inner.state);
            state.cooldown.retain(|_, until| *until > now);

            let open_total = state
                .requests
                .values()
                .filter(|req| req.outcome.is_none())
                .count();
            if open_total >= MAX_OPEN {
                return Err(ApprovalError::TooMany);
            }

            let open_session = state
                .requests
                .values()
                .filter(|req| req.outcome.is_none() && req.session_key == params.session_key)
                .count();
            if open_session >= MAX_OPEN_PER_SESSION {
                return Err(ApprovalError::TooMany);
            }

            if state.cooldown.contains_key(&params.session_key) {
                return Err(ApprovalError::Cooldown);
            }

            state.seq += 1;
            let seq = state.seq;
            state.requests.insert(
                id.clone(),
                PendingRequest {
                    card: card.clone(),
                    seq,
                    session_key: params.session_key,
                    deadline,
                    claimed: false,
                    outcome: None,
                    tx,
                    state_tx,
                },
            );
            publish(&self.inner.feed, &state);
        }

        let inner = Arc::clone(&self.inner);
        handle.spawn(run_deadline(inner, id));

        Ok(ApprovalWaiter {
            inner: Arc::downgrade(&self.inner),
            card,
            rx,
            disarmed: false,
        })
    }

    /// Reserva o cartão para resposta em duas fases antes de persistir `AllowAlways`.
    ///
    /// Não define `outcome`, não acorda o chamador nem remove o cartão do feed; se a guarda
    /// [`ClaimedApproval`] for descartada sem [`ClaimedApproval::commit`], a reserva é liberada.
    ///
    /// # Errors
    ///
    /// Retorna [`ApprovalError::Lost`] se o pedido sumiu, foi cancelado ou já expirou,
    /// [`ApprovalError::Answered`] se já foi respondido ou possui outra resposta em voo, e
    /// [`ApprovalError::Stale`] se o `interaction_id` não confere com o cartão atual.
    pub fn claim_answer(
        &self,
        request_id: &str,
        interaction_id: &str,
        decision: ApprovalDecision,
    ) -> Result<ClaimedApproval, ApprovalError> {
        let mut state = lock_state(&self.inner.state);
        let Some(req) = state.requests.get_mut(request_id) else {
            return Err(ApprovalError::Lost);
        };
        if req.outcome.is_some() || req.claimed {
            return Err(ApprovalError::Answered);
        }
        // Recusa resposta se o prazo venceu mesmo antes da task `run_deadline` marcar `TimedOut`.
        if Instant::now() >= req.deadline {
            return Err(ApprovalError::Lost);
        }
        if req.card.interaction_id != interaction_id {
            return Err(ApprovalError::Stale);
        }

        req.claimed = true;
        Ok(ClaimedApproval {
            inner: Arc::clone(&self.inner),
            request_id: request_id.to_owned(),
            integration_id: req.card.integration_id.clone(),
            tool: req.card.tool.clone(),
            decision,
            armed: true,
        })
    }

    /// Registra a decisão do dono para o cartão aberto.
    ///
    /// # Errors
    ///
    /// Retorna [`ApprovalError::Lost`] se o pedido sumiu, foi cancelado ou já expirou,
    /// [`ApprovalError::Answered`] se já foi respondido ou possui outra resposta em voo, e
    /// [`ApprovalError::Stale`] se o `interaction_id` não confere com o cartão atual.
    pub fn answer(
        &self,
        request_id: &str,
        interaction_id: &str,
        decision: ApprovalDecision,
    ) -> Result<AnsweredApproval, ApprovalError> {
        self.claim_answer(request_id, interaction_id, decision)?
            .commit()
    }

    /// Cancela um pedido ainda aberto e remove o cartão da fila.
    pub fn cancel(&self, request_id: &str) -> bool {
        cancel_pending(&self.inner, request_id)
    }

    /// Cancela todos os pedidos abertos vinculados a uma integração removida.
    pub fn cancel_for_integration(&self, integration_id: &str) {
        let mut state = lock_state(&self.inner.state);
        let ids: Vec<String> = state
            .requests
            .iter()
            .filter(|(_, req)| req.outcome.is_none() && req.card.integration_id == integration_id)
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        for id in ids {
            if let Some(mut req) = state.requests.remove(&id) {
                req.outcome = Some(ApprovalResolution::Cancelled);
                let _ = req.tx.send_replace(Some(ApprovalResolution::Cancelled));
            }
        }
        publish(&self.inner.feed, &state);
    }
}

impl Default for ApprovalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            expires: APPROVAL_EXPIRES,
            cooldown: COOLDOWN,
            retention: DEFAULT_RETENTION,
        }
    }
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "pedido de aprovação inválido: {reason}"),
            Self::Lost => write!(f, "pedido de aprovação não encontrado ou expirado"),
            Self::Stale => write!(f, "o cartão de aprovação foi atualizado; tente novamente"),
            Self::Answered => write!(f, "este pedido de aprovação já foi respondido"),
            Self::TooMany => write!(f, "muitos pedidos de aprovação aguardando resposta"),
            Self::Cooldown => {
                write!(
                    f,
                    "o dono acabou de negar um pedido desta sessão; aguarde antes de tentar de novo"
                )
            }
        }
    }
}

impl std::error::Error for ApprovalError {}

impl ClaimedApproval {
    /// Identificador da integração do pedido reservado.
    pub fn integration_id(&self) -> &str {
        &self.integration_id
    }

    /// Nome da ferramenta do pedido reservado.
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Decisão validada e reservada para confirmação.
    pub fn decision(&self) -> ApprovalDecision {
        self.decision
    }

    /// Confirma a resposta reservada, aplica cooldown, acorda o chamador e atualiza o feed.
    ///
    /// # Errors
    ///
    /// Retorna [`ApprovalError::Lost`] se o cartão foi cancelado durante a persistência, ou
    /// [`ApprovalError::Answered`] se já havia sido encerrado.
    pub fn commit(mut self) -> Result<AnsweredApproval, ApprovalError> {
        self.armed = false;
        let mut state = lock_state(&self.inner.state);
        let Some(req) = state.requests.get_mut(&self.request_id) else {
            return Err(ApprovalError::Lost);
        };
        if req.outcome.is_some() {
            req.claimed = false;
            return Err(ApprovalError::Answered);
        }

        let resolution = match self.decision {
            ApprovalDecision::AllowOnce => ApprovalResolution::AllowOnce,
            ApprovalDecision::AllowAlways => ApprovalResolution::AllowAlways,
            ApprovalDecision::Deny => ApprovalResolution::Denied,
        };
        req.claimed = false;
        req.outcome = Some(resolution);
        let _ = req.tx.send_replace(Some(resolution));
        let _ = req.state_tx.send_replace(());

        let answered = AnsweredApproval {
            integration_id: req.card.integration_id.clone(),
            tool: req.card.tool.clone(),
            decision: self.decision,
        };
        let session_key = req.session_key.clone();

        if self.decision == ApprovalDecision::Deny && self.inner.config.cooldown > Duration::ZERO {
            state
                .cooldown
                .insert(session_key, Instant::now() + self.inner.config.cooldown);
        }

        publish(&self.inner.feed, &state);
        Ok(answered)
    }
}

impl Drop for ClaimedApproval {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let mut state = lock_state(&self.inner.state);
        if let Some(req) = state.requests.get_mut(&self.request_id) {
            req.claimed = false;
            let _ = req.state_tx.send_replace(());
        }
    }
}

impl ApprovalWaiter {
    /// Cartão público aberto na fila para exibição ou notificação.
    pub fn card(&self) -> &ApprovalCard {
        &self.card
    }

    /// Identificador único deste pedido de aprovação.
    pub fn request_id(&self) -> &str {
        &self.card.id
    }

    /// Aguarda até o pedido receber uma decisão, expirar ou ser cancelado.
    pub async fn wait(mut self) -> ApprovalResolution {
        loop {
            if let Some(resolution) = *self.rx.borrow_and_update() {
                self.disarmed = true;
                return resolution;
            }
            if self.rx.changed().await.is_err() {
                self.disarmed = true;
                return ApprovalResolution::Cancelled;
            }
        }
    }
}

impl Drop for ApprovalWaiter {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        self.disarmed = true;
        if let Some(inner) = self.inner.upgrade() {
            cancel_pending(&inner, &self.card.id);
        }
    }
}

/// Formata a origem no padrão `<server_id>::<session>`.
pub fn format_origin(server_id: &str, session: &str) -> String {
    format!("{server_id}::{session}")
}

/// Corta `text` em até `max_chars` caracteres respeitando fronteiras UTF-8 válidas.
pub fn truncate_preview_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if let Some((byte_idx, _)) = text.char_indices().nth(max_chars) {
        // Recua um caractere para incluir `…` sem ultrapassar `max_chars`.
        let prefix_end = text[..byte_idx]
            .char_indices()
            .next_back()
            .map_or(0, |(idx, _)| idx);
        let mut out = String::with_capacity(prefix_end + '…'.len_utf8());
        out.push_str(&text[..prefix_end]);
        out.push('…');
        out
    } else {
        text.to_owned()
    }
}

/// Formata argumentos JSON em prévia legível truncada com segurança.
pub fn format_arguments_preview(arguments: &serde_json::Value) -> String {
    let raw = serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string());
    truncate_preview_chars(&raw, MAX_ARGUMENTS_PREVIEW_CHARS)
}

fn validate_request(params: &OpenApprovalRequest) -> Result<(), ApprovalError> {
    if !valid_text(&params.origin, MAX_ORIGIN_CHARS) {
        return Err(ApprovalError::Invalid("origem inválida".into()));
    }
    if !valid_text(&params.session_key, MAX_SESSION_KEY_CHARS) {
        return Err(ApprovalError::Invalid("chave de sessão inválida".into()));
    }
    if params.integration_id.trim().is_empty()
        || params.integration_id.len() > MAX_INTEGRATION_ID_BYTES
        || params.integration_id.contains('\0')
    {
        return Err(ApprovalError::Invalid(
            "identificador da integração inválido".into(),
        ));
    }
    if params.integration_name.trim().is_empty()
        || params.integration_name.len() > MAX_INTEGRATION_NAME_BYTES
        || params.integration_name.contains('\0')
    {
        return Err(ApprovalError::Invalid("nome da integração inválido".into()));
    }
    if !valid_tool_name(&params.tool) {
        return Err(ApprovalError::Invalid("nome da ferramenta inválido".into()));
    }
    if !valid_text(&params.reason, MAX_REASON_CHARS) {
        return Err(ApprovalError::Invalid(
            "motivo vazio ou longo demais".into(),
        ));
    }
    Ok(())
}

fn valid_text(text: &str, max_chars: usize) -> bool {
    !text.trim().is_empty() && !text.contains('\0') && text.chars().count() <= max_chars
}

fn cancel_pending(inner: &StoreInner, request_id: &str) -> bool {
    let mut state = lock_state(&inner.state);
    let Some(entry) = state.requests.get(request_id) else {
        return false;
    };
    if entry.outcome.is_some() {
        return false;
    }
    if let Some(mut req) = state.requests.remove(request_id) {
        req.outcome = Some(ApprovalResolution::Cancelled);
        let _ = req.tx.send_replace(Some(ApprovalResolution::Cancelled));
        publish(&inner.feed, &state);
        return true;
    }
    false
}

fn publish(feed: &watch::Sender<Arc<ApprovalFeed>>, state: &StoreState) {
    let mut open: Vec<&PendingRequest> = state
        .requests
        .values()
        .filter(|req| req.outcome.is_none())
        .collect();
    open.sort_by_key(|req| req.seq);
    let next = ApprovalFeed {
        requests: open.into_iter().map(|req| req.card.clone()).collect(),
    };
    feed.send_if_modified(|current| {
        if **current == next {
            false
        } else {
            *current = Arc::new(next);
            true
        }
    });
}

async fn run_deadline(inner: Arc<StoreInner>, id: String) {
    loop {
        let (wake_at, mut state_rx) = {
            let mut state = lock_state(&inner.state);
            let Some(req) = state.requests.get_mut(&id) else {
                return;
            };
            // Mesmo se já respondido antes do primeiro poll, segue para a retenção e remoção.
            if req.outcome.is_some() {
                break;
            }
            if req.claimed {
                (None, req.state_tx.subscribe())
            } else if Instant::now() >= req.deadline {
                req.outcome = Some(ApprovalResolution::TimedOut);
                let _ = req.tx.send_replace(Some(ApprovalResolution::TimedOut));
                publish(&inner.feed, &state);
                break;
            } else {
                (Some(req.deadline), req.state_tx.subscribe())
            }
        };

        match wake_at {
            Some(deadline) => {
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {}
                    res = state_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                }
            }
            None => {
                if state_rx.changed().await.is_err() {
                    return;
                }
            }
        }
    }

    // Mantém o registro resolvido por uma janela curta para distinguir duplo clique (`Answered`) de `Lost`.
    if inner.config.retention > Duration::ZERO {
        tokio::time::sleep(inner.config.retention).await;
    }
    let mut state = lock_state(&inner.state);
    if state
        .requests
        .get(&id)
        .is_some_and(|req| req.outcome.is_some())
    {
        state.requests.remove(&id);
    }
}

fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample_request(session_key: &str, tool: &str) -> OpenApprovalRequest {
        OpenApprovalRequest {
            origin: format_origin("mac-1", "sessao-a"),
            session_key: session_key.to_owned(),
            integration_id: "github".to_owned(),
            integration_name: "GitHub".to_owned(),
            tool: tool.to_owned(),
            arguments_preview: r#"{"title":"Issue 1"}"#.to_owned(),
            reason: "Criar issue solicitada pelo usuário".to_owned(),
        }
    }

    #[tokio::test]
    async fn allow_once_and_allow_always_resolve_waiter_and_clear_feed() {
        let store = ApprovalStore::new();
        let feed = store.feed();
        assert!(feed.borrow().requests.is_empty());

        let waiter = store
            .open_request(sample_request("k1", "create_issue"))
            .expect("deve abrir primeiro pedido");
        let card = waiter.card().clone();
        assert!(!card.id.is_empty());
        assert!(!card.interaction_id.is_empty());
        assert_eq!(card.origin, "mac-1::sessao-a");
        assert_eq!(card.integration_id, "github");
        assert_eq!(card.tool, "create_issue");
        assert_eq!(feed.borrow().requests.len(), 1);

        let answered = store
            .answer(&card.id, &card.interaction_id, ApprovalDecision::AllowAlways)
            .expect("allow_always deve responder pedido aberto");
        assert_eq!(answered.integration_id, "github");
        assert_eq!(answered.tool, "create_issue");
        assert_eq!(answered.decision, ApprovalDecision::AllowAlways);
        assert!(feed.borrow().requests.is_empty());

        assert_eq!(waiter.wait().await, ApprovalResolution::AllowAlways);

        let w2 = store
            .open_request(sample_request("k1", "merge_pr"))
            .expect("deve abrir segundo pedido");
        let c2 = w2.card().clone();
        store
            .answer(&c2.id, &c2.interaction_id, ApprovalDecision::AllowOnce)
            .expect("allow_once deve responder");
        assert_eq!(w2.wait().await, ApprovalResolution::AllowOnce);
    }

    #[tokio::test(start_paused = true)]
    async fn deny_resolves_waiter_and_triggers_session_cooldown() {
        let store = ApprovalStore::with_config(ApprovalConfig {
            expires: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
            retention: Duration::from_secs(5),
        });
        let waiter = store
            .open_request(sample_request("sess-cooldown", "delete_repo"))
            .expect("deve abrir pedido");
        let card = waiter.card().clone();

        store
            .answer(&card.id, &card.interaction_id, ApprovalDecision::Deny)
            .expect("deny deve responder");
        assert_eq!(waiter.wait().await, ApprovalResolution::Denied);

        let blocked = store.open_request(sample_request("sess-cooldown", "delete_repo"));
        assert_eq!(blocked.err(), Some(ApprovalError::Cooldown));

        // Outra sessão não é bloqueada pelo cooldown da primeira.
        let other = store
            .open_request(sample_request("outra-sessao", "delete_repo"))
            .expect("outra sessão segue livre");
        drop(other);

        tokio::time::advance(Duration::from_secs(31)).await;
        let recovered = store
            .open_request(sample_request("sess-cooldown", "delete_repo"))
            .expect("após cooldown a sessão pode pedir novamente");
        drop(recovered);
    }

    #[tokio::test]
    async fn stale_interaction_id_and_double_answer_are_rejected() {
        let store = ApprovalStore::new();
        let waiter = store
            .open_request(sample_request("k1", "write_file"))
            .expect("deve abrir");
        let card = waiter.card().clone();

        assert_eq!(
            store.answer(&card.id, "errado", ApprovalDecision::AllowOnce),
            Err(ApprovalError::Stale)
        );
        assert_eq!(store.feed().borrow().requests.len(), 1);

        store
            .answer(&card.id, &card.interaction_id, ApprovalDecision::AllowOnce)
            .expect("primeira resposta vence");
        assert_eq!(
            store.answer(&card.id, &card.interaction_id, ApprovalDecision::Deny),
            Err(ApprovalError::Answered)
        );
        assert_eq!(waiter.wait().await, ApprovalResolution::AllowOnce);
    }

    #[tokio::test]
    async fn enforces_max_open_per_session_and_global_max_open() {
        let store = ApprovalStore::new();
        let mut held = Vec::new();
        for idx in 0..MAX_OPEN_PER_SESSION {
            held.push(
                store
                    .open_request(sample_request("sess-1", &format!("tool_{idx}")))
                    .expect("dentro do teto por sessão"),
            );
        }
        assert_eq!(
            store
                .open_request(sample_request("sess-1", "tool_extra"))
                .err(),
            Some(ApprovalError::TooMany)
        );

        for sess in 2..15 {
            for idx in 0..MAX_OPEN_PER_SESSION {
                if held.len() == MAX_OPEN {
                    break;
                }
                held.push(
                    store
                        .open_request(sample_request(
                            &format!("sess-{sess}"),
                            &format!("t_{idx}"),
                        ))
                        .expect("dentro do teto global"),
                );
            }
        }
        assert_eq!(held.len(), MAX_OPEN);
        assert_eq!(
            store
                .open_request(sample_request("sess-livre", "tool_z"))
                .err(),
            Some(ApprovalError::TooMany)
        );
        drop(held);
        assert!(store.feed().borrow().requests.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn expires_after_configured_ttl_and_clears_feed() {
        let store = ApprovalStore::with_config(ApprovalConfig {
            expires: Duration::from_secs(10),
            cooldown: Duration::from_secs(30),
            retention: Duration::from_secs(5),
        });
        let feed = store.feed();
        let waiter = store
            .open_request(sample_request("sess-ttl", "send_email"))
            .expect("deve abrir");
        let id = waiter.request_id().to_owned();
        assert_eq!(feed.borrow().requests.len(), 1);

        assert_eq!(waiter.wait().await, ApprovalResolution::TimedOut);
        assert!(feed.borrow().requests.is_empty());
        assert!(!store.cancel(&id));
    }

    #[tokio::test]
    async fn dropping_waiter_cancels_pending_request_immediately_and_updates_feed() {
        let store = ApprovalStore::new();
        let mut feed = store.feed();

        let waiter = store
            .open_request(sample_request("sess-cancel", "send_email"))
            .expect("deve abrir");
        let id = waiter.request_id().to_owned();
        assert_eq!(feed.borrow_and_update().requests.len(), 1);

        // Simula o cancelamento do cliente MCP num `tokio::select!` abandonando `waiter.wait()`.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::select! {
            _ = waiter.wait() => panic!("wait não deveria completar antes do cancelamento"),
            _ = async {
                let _ = tx.send(());
                let _ = rx.await;
            } => {}
        }

        tokio::time::timeout(Duration::from_millis(200), feed.changed())
            .await
            .expect("feed deve notificar imediatamente após Drop do waiter")
            .expect("canal do feed aberto");
        assert!(feed.borrow().requests.is_empty());
        assert_eq!(
            store.answer(&id, "any", ApprovalDecision::AllowOnce),
            Err(ApprovalError::Lost)
        );
    }

    #[test]
    fn truncates_arguments_preview_on_utf8_char_boundary() {
        let emoji = "🚀".repeat(MAX_ARGUMENTS_PREVIEW_CHARS + 50);
        let req = OpenApprovalRequest {
            arguments_preview: emoji,
            ..sample_request("sess-utf8", "send_msg")
        };
        let truncated = truncate_preview_chars(&req.arguments_preview, MAX_ARGUMENTS_PREVIEW_CHARS);
        assert_eq!(truncated.chars().count(), MAX_ARGUMENTS_PREVIEW_CHARS);
        assert!(truncated.ends_with('…'));

        let short = truncate_preview_chars("olá mundo", MAX_ARGUMENTS_PREVIEW_CHARS);
        assert_eq!(short, "olá mundo");
    }

    #[test]
    fn rejects_invalid_inputs() {
        let store = ApprovalStore::new();

        let mut empty_reason = sample_request("k1", "create_issue");
        empty_reason.reason = "   ".to_owned();
        assert!(matches!(
            store.open_request(empty_reason),
            Err(ApprovalError::Invalid(_))
        ));

        let mut huge_reason = sample_request("k1", "create_issue");
        huge_reason.reason = "a".repeat(MAX_REASON_CHARS + 1);
        assert!(matches!(
            store.open_request(huge_reason),
            Err(ApprovalError::Invalid(_))
        ));

        let invalid_tool = sample_request("k1", "");
        assert!(matches!(
            store.open_request(invalid_tool),
            Err(ApprovalError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn claim_answer_rollback_on_drop_keeps_card_and_allows_retry() {
        let store = ApprovalStore::new();
        let feed = store.feed();
        let waiter = store
            .open_request(sample_request("sess-claim", "create_issue"))
            .expect("deve abrir pedido");
        let card = waiter.card().clone();

        let wait_handle = tokio::spawn(async move { waiter.wait().await });
        tokio::task::yield_now().await;

        let claim = store
            .claim_answer(&card.id, &card.interaction_id, ApprovalDecision::AllowAlways)
            .unwrap_or_else(|err| panic!("reserva deve funcionar: {err}"));
        assert_eq!(claim.integration_id(), "github");
        assert_eq!(claim.tool(), "create_issue");
        assert_eq!(claim.decision(), ApprovalDecision::AllowAlways);

        // Enquanto reservado: continua no feed, waiter não acorda e segunda tentativa recebe Answered.
        assert_eq!(feed.borrow().requests.len(), 1);
        assert_eq!(store.open_count(), 1);
        assert_eq!(
            store
                .claim_answer(&card.id, &card.interaction_id, ApprovalDecision::AllowAlways)
                .err(),
            Some(ApprovalError::Answered)
        );
        tokio::task::yield_now().await;
        assert!(
            !wait_handle.is_finished(),
            "waiter não pode acordar antes do commit da reserva"
        );

        // Drop sem confirmar libera a reserva para nova tentativa.
        drop(claim);
        tokio::task::yield_now().await;
        assert_eq!(feed.borrow().requests.len(), 1);
        assert_eq!(store.open_count(), 1);
        assert!(!wait_handle.is_finished());

        let retried = store
            .claim_answer(&card.id, &card.interaction_id, ApprovalDecision::AllowAlways)
            .unwrap_or_else(|err| panic!("nova reserva após drop deve funcionar: {err}"));
        let answered = retried
            .commit()
            .expect("commit deve resolver após persistência");
        assert_eq!(answered.decision, ApprovalDecision::AllowAlways);
        assert!(feed.borrow().requests.is_empty());
        assert_eq!(wait_handle.await.unwrap(), ApprovalResolution::AllowAlways);
    }

    #[tokio::test(start_paused = true)]
    async fn claim_answer_does_not_timeout_while_claimed_and_times_out_after_rollback() {
        let store = ApprovalStore::with_config(ApprovalConfig {
            expires: Duration::from_secs(10),
            cooldown: Duration::from_secs(30),
            retention: Duration::from_secs(5),
        });
        let feed = store.feed();
        let waiter = store
            .open_request(sample_request("sess-claim-ttl", "create_issue"))
            .expect("deve abrir pedido");
        let card = waiter.card().clone();
        let wait_handle = tokio::spawn(async move { waiter.wait().await });
        tokio::task::yield_now().await;

        let claim = store
            .claim_answer(&card.id, &card.interaction_id, ApprovalDecision::AllowAlways)
            .unwrap_or_else(|err| panic!("reserva deve funcionar: {err}"));

        // Avança além do prazo enquanto a reserva está ativa: não pode expirar no meio da gravação.
        tokio::time::advance(Duration::from_secs(20)).await;
        tokio::task::yield_now().await;
        assert_eq!(feed.borrow().requests.len(), 1);
        assert!(!wait_handle.is_finished());

        // Ao descartar a reserva vencida sem confirmar, `run_deadline` acorda e expira o cartão.
        drop(claim);
        let resolution = tokio::time::timeout(Duration::from_secs(2), wait_handle)
            .await
            .expect("waiter deve expirar após drop da reserva vencida")
            .unwrap();
        assert_eq!(resolution, ApprovalResolution::TimedOut);
        assert!(feed.borrow().requests.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_answer_before_deadline_task_runs_still_cleans_up_requests() {
        let store = ApprovalStore::with_config(ApprovalConfig {
            expires: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
            retention: Duration::from_secs(5),
        });
        let waiter = store
            .open_request(sample_request("sess-leak", "write_file"))
            .expect("deve abrir");
        let card = waiter.card().clone();

        // Responde antes do primeiro `yield_now`, quando `run_deadline` ainda não foi executada.
        store
            .answer(&card.id, &card.interaction_id, ApprovalDecision::AllowOnce)
            .expect("resposta imediata deve suceder");
        tokio::task::yield_now().await;
        assert_eq!(waiter.wait().await, ApprovalResolution::AllowOnce);

        // Após a janela de retenção, `run_deadline` deve remover o registro de `state.requests`.
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(
            lock_state(&store.inner.state).requests.is_empty(),
            "run_deadline vazou pedido respondido antes do primeiro poll"
        );
    }

    #[test]
    fn open_request_outside_tokio_runtime_fails_without_inserting_or_publishing() {
        let store = ApprovalStore::new();
        let feed = store.feed();
        let err = match store.open_request(sample_request("sess-no-rt", "create_issue")) {
            Ok(_) => panic!("deve falhar sem runtime Tokio"),
            Err(err) => err,
        };
        assert!(matches!(err, ApprovalError::Invalid(_)), "{err:?}");
        assert_eq!(store.open_count(), 0);
        assert!(feed.borrow().requests.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn claim_answer_rejects_expired_before_deadline_task_while_prior_claim_commits() {
        let store = ApprovalStore::with_config(ApprovalConfig {
            expires: Duration::from_secs(10),
            cooldown: Duration::from_secs(30),
            retention: Duration::from_secs(5),
        });
        let waiter_a = store
            .open_request(sample_request("sess-exp-1", "write_a"))
            .expect("deve abrir A");
        let card_a = waiter_a.card().clone();
        let waiter_b = store
            .open_request(sample_request("sess-exp-2", "write_b"))
            .expect("deve abrir B");
        let card_b = waiter_b.card().clone();
        tokio::task::yield_now().await;

        // Reserva B antes do vencimento.
        let claim_b = store
            .claim_answer(&card_b.id, &card_b.interaction_id, ApprovalDecision::AllowAlways)
            .expect("reserva de B antes do prazo deve suceder");

        // Simula A já vencido enquanto `run_deadline` ainda não acordou para marcar `TimedOut`.
        {
            let mut state = lock_state(&store.inner.state);
            let req_a = state.requests.get_mut(&card_a.id).expect("A ainda na fila");
            req_a.deadline = Instant::now();
        }
        assert_eq!(
            store
                .claim_answer(&card_a.id, &card_a.interaction_id, ApprovalDecision::AllowOnce)
                .err(),
            Some(ApprovalError::Lost),
            "claim_answer após o prazo deve falhar com Lost mesmo antes do run_deadline"
        );

        // Avança o relógio além do prazo original de B: a reserva prévia ainda pode confirmar.
        tokio::time::advance(Duration::from_secs(15)).await;
        let answered_b = claim_b
            .commit()
            .expect("reserva feita antes do prazo deve confirmar mesmo após o prazo");
        assert_eq!(answered_b.decision, ApprovalDecision::AllowAlways);
        assert_eq!(waiter_b.wait().await, ApprovalResolution::AllowAlways);
        assert_eq!(waiter_a.wait().await, ApprovalResolution::TimedOut);
    }

    #[tokio::test]
    async fn open_request_preserves_exact_tool_name_with_surrounding_spaces() {
        let store = ApprovalStore::new();
        let waiter = store
            .open_request(sample_request("sess-space", "publish "))
            .expect("deve abrir pedido com espaço no nome");
        let card = waiter.card().clone();
        assert_eq!(card.tool, "publish ");

        let answered = store
            .answer(&card.id, &card.interaction_id, ApprovalDecision::AllowAlways)
            .expect("deve responder");
        assert_eq!(answered.tool, "publish ");
        assert_eq!(waiter.wait().await, ApprovalResolution::AllowAlways);
    }
}
