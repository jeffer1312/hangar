//! Ticket da sessão, leitura da revisão (uma por vez por sessão), resumo do chip e envio das decisões.
use crate::assets::{self, AssetCache};
use hangar_connectors::host::{ConnectorStatus, Host};
use hangar_connectors::protocol::{ConnectorError, ErrorCode};
use hangar_connectors::registry::Entry;
use hangar_connectors::work_review::{self, Answer, Asset, AssetRefs, Badge, DecideParams, Review, ReviewState};
use hangar_connectors::work_tracking::GetParams;
use hangar_connectors::{AdminError, AdminKind, CallError};
use hangar_mission::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Semaphore};

pub type Task<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Branch que não é ticket volta a ser procurada depois disto: o ticket pode ser criado depois.
const BRANCH_MISS_TTL: Duration = Duration::from_secs(5 * 60);

/// Quem a sessão é agora; quem monta é o servidor, com a mesma identidade da tool `mission`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionRef {
    pub name: String,
    pub lifecycle_id: Option<String>,
    /// `None`: a sessão não tem chave da vida; o vínculo vale como na tool `mission`.
    pub key: Option<String>,
    pub group_id: Option<String>,
    pub branch: Option<String>,
    /// A chave existe mas não foi lida: o vínculo gravado não confere sem ela; só a branch acha o ticket.
    pub key_unreadable: bool,
}

pub trait Sessions: Send + Sync + 'static {
    /// `Ok(None)`: sessão desconhecida (a revisão some, sem erro). `Err(código)`: a leitura da lista
    /// ou da identidade falhou, e isso não pode virar "sem vínculo".
    fn resolve(&self, name: String) -> Task<Result<Option<SessionRef>, String>>;
}

/// Diário de uso. `detail` é código de erro, nunca texto da conversa ou do conector.
pub trait Diag: Send + Sync + 'static {
    fn report(&self, code: &str, session: &str, detail: &str);
}

#[derive(Clone, Debug)]
pub enum Failure {
    Call(CallError),
    Mission(hangar_mission::store::Error),
    NotLinked,
}

impl Failure {
    pub fn code(&self) -> &str {
        match self {
            Failure::Call(error) => error.code(),
            Failure::Mission(error) => error.code,
            Failure::NotLinked => "work_review_not_linked",
        }
    }
}

impl From<CallError> for Failure { fn from(error: CallError) -> Self { Failure::Call(error) } }
impl From<ConnectorError> for Failure { fn from(error: ConnectorError) -> Self { Failure::Call(error.into()) } }

fn invalid(message: impl Into<String>) -> Failure { ConnectorError::invalid_params(message).into() }

fn unreadable(code: String) -> Failure {
    ConnectorError::new(ErrorCode::Unavailable, format!("não foi possível conferir a sessão: {code}")).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via { Ticket, Branch }

/// O ticket da sessão e por onde ele foi achado.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Target { pub via: Via, pub connector: String, pub ticket_id: String, pub key: String }

/// O catálogo dos conectores, lido uma vez por leitura.
type Catalog = Result<Vec<ConnectorStatus>, AdminError>;

/// O conector está ligado e oferece a revisão.
fn offers_review(entry: &Entry) -> bool { entry.enabled && entry.capabilities.contains_key(work_review::CAPABILITY) }

/// Resposta da rota da sessão ligada.
#[derive(Clone, Debug, Serialize)]
pub struct Linked {
    #[serde(flatten)]
    pub target: Target,
    /// O conector do vínculo oferece `work_review` e está ligado.
    pub supported: bool,
    pub review: Option<Review>,
    pub fetched_at: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideBody { pub review_id: String, pub decisions: Vec<Answer> }

/// O que o chip do card mostra; só existe com revisão.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct Summary { review_id: String, state: ReviewState, badge: Option<Badge>, updated_at: String }

impl Summary {
    fn of(review: &Review) -> Self {
        Self { review_id: review.id.clone(), state: review.state, badge: review.badge.clone(), updated_at: review.updated_at.clone() }
    }
}

#[derive(Default)]
struct Slot {
    life: Option<String>,
    branch: Option<String>,
    target: Option<Target>,
    summary: Option<Summary>,
    running: bool,
    /// Pedido que chegou durante a leitura: relê uma vez no fim, com a identidade mais nova.
    again: Option<SessionRef>,
    /// Sobe a cada escrita: leitura começada antes dela não grava.
    generation: u64,
    published: u64,
}

/// Resultado da busca pela branch e quando foi feita.
struct BranchHit { target: Option<Target>, at: Instant }

#[derive(Default)]
struct State {
    slots: HashMap<String, Slot>,
    /// Busca pela branch por `(conector, branch)`, inclusive o negativo; erro não entra.
    branches: HashMap<(String, String), BranchHit>,
    seq: u64,
}

impl State {
    /// Esquece as sessões fora de `current` e as buscas pela branch que só elas usavam.
    fn prune(&mut self, current: &[(&str, Option<&str>)]) {
        self.slots.retain(|name, slot| slot.running || current.iter().any(|(current, _)| current == name));
        // As de fechadas e renomeadas não voltam a ser pedidas.
        let live: std::collections::HashSet<String> = self.slots.values().filter_map(|slot| slot.branch.clone()).collect();
        self.branches.retain(|(_, branch), _| live.contains(branch));
    }

    /// O achado vale até a branch mudar; o negativo, só por `BRANCH_MISS_TTL`.
    fn cached_branch(&self, key: &(String, String), now: Instant) -> Option<Option<Target>> {
        let hit = self.branches.get(key)?;
        (hit.target.is_some() || now.saturating_duration_since(hit.at) < BRANCH_MISS_TTL).then(|| hit.target.clone())
    }
}

#[derive(Clone, Copy)]
struct Turn { seq: u64, generation: u64 }

pub struct Engine {
    host: Arc<Host>,
    mission: Arc<Store>,
    sessions: Arc<dyn Sessions>,
    diag: Arc<dyn Diag>,
    assets: AssetCache,
    state: Mutex<State>,
    queues: Mutex<HashMap<String, Arc<Semaphore>>>,
    changed: Notify,
    /// Sobe junto com cada `changed`, sob a trava do estado.
    version: AtomicU64,
}

impl Engine {
    pub fn new(host: Arc<Host>, mission: Arc<Store>, sessions: Arc<dyn Sessions>, diag: Arc<dyn Diag>, cache_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { host, mission, sessions, diag, assets: AssetCache::new(&cache_dir, assets::LIMIT),
            state: Mutex::default(), queues: Mutex::default(), changed: Notify::new(), version: AtomicU64::new(0) })
    }

    /// Acorda quando o resumo publicado de alguma sessão muda.
    pub fn changed(&self) -> &Notify { &self.changed }

    /// Muda a cada resumo publicado que muda. Lida antes de `summaries_for`: versão igual e mesmas
    /// linhas dão o mesmo corpo.
    pub fn summaries_version(&self) -> u64 { self.version.load(Ordering::SeqCst) }

    fn notify_changed(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
    }

    /// Algum conector ligado oferece a revisão: sem ele, o servidor nem acompanha os turnos. Catálogo
    /// ilegível é erro, para quem chama registrar em vez de desligar calado.
    pub fn has_review_connector(&self) -> Result<bool, AdminError> {
        Ok(self.host.list()?.iter().any(|status| offers_review(&status.entry)))
    }

    /// O catálogo lê e interpreta um arquivo: fora do runtime assíncrono.
    async fn catalog(&self) -> Catalog {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || host.list()).await
            .unwrap_or_else(|_| Err(AdminError::new(AdminKind::RegistryUnavailable, "leitura do catálogo interrompida")))
    }

    /// Esquece as sessões que saíram da lista. O vigia do fim de turno chama com as linhas dele: sem lista
    /// aberta, `summaries_for` não roda e as vagas criadas pelos turnos ficariam para sempre.
    pub fn prune(&self, current: &[(&str, Option<&str>)]) { self.lock().prune(current); }

    /// Corpo do evento `work_reviews`: só as sessões atuais, na mesma vida, com revisão.
    pub fn summaries_for(&self, current: &[(&str, Option<&str>)]) -> Value {
        let mut state = self.lock();
        state.prune(current);
        let mut reviews = serde_json::Map::new();
        for (name, life) in current {
            if let Some(slot) = state.slots.get(*name) && slot.life.as_deref() == *life && let Some(summary) = &slot.summary {
                reviews.insert((*name).to_owned(), json!(summary));
            }
        }
        json!({"reviews": reviews})
    }

    /// Releitura em segundo plano (fim de turno, tool `mission`); não espera.
    pub fn refresh(self: &Arc<Self>, session: SessionRef) { self.schedule(session, false); }

    /// Primeira vista: só relê quem ainda não tem resumo.
    pub fn refresh_if_unseen(self: &Arc<Self>, session: SessionRef) { self.schedule(session, true); }

    /// Leitura pedida pela pessoa: fura a fila do conector e atualiza o resumo.
    pub async fn session_review(&self, name: &str) -> Result<Option<Linked>, Failure> {
        let Some(session) = self.resolve(name).await? else { return Ok(None) };
        self.read(&session, false).await
    }

    pub async fn asset(&self, name: &str, asset: &str, review_id: &str) -> Result<Asset, Failure> {
        // Antes da sessão: ID fora do formato nunca chega ao disco nem ao conector.
        let refs = AssetRefs::new(review_id, asset)?;
        let session = self.resolve(name).await?.ok_or(Failure::NotLinked)?;
        // O ticket da última leitura: o painel lê a revisão antes das imagens, e reler os Tickets a
        // cada miniatura custaria uma leitura de disco por imagem.
        let slot = |engine: &Self| {
            let state = engine.lock();
            state.slots.get(&session.name).filter(|slot| slot.life == session.lifecycle_id)
                .map(|slot| (slot.target.clone(), slot.summary.as_ref().map(|summary| summary.review_id.clone())))
        };
        let target = match slot(self).and_then(|(target, _)| target) {
            Some(target) => target,
            None => self.locate(&session, false, &self.catalog().await).await?.ok_or(Failure::NotLinked)?,
        };
        if let Some(hit) = self.assets.get(&target.connector, &target.ticket_id, review_id, asset).await { return Ok(hit); }
        let params = refs.for_ticket(target.ticket_id.clone())?;
        let image = self.host.review_asset(&target.connector, params).await?;
        // Só a rodada atual (conferida depois da resposta) vai ao disco: a resposta atrasada de uma
        // rodada antiga recriaria a pasta apagada, e qualquer `review_id` pedido criaria pasta nova.
        // A rodada que troca entre esta conferência e a gravação o cache recusa sob a própria trava.
        let current = slot(self).and_then(|(known, review)| (known.as_ref() == Some(&target)).then_some(review).flatten());
        if current.as_deref() == Some(review_id) {
            self.assets.put(&target.connector, &target.ticket_id, review_id, asset, &image).await;
        }
        Ok(image)
    }

    /// Só chega ao conector o que bate com a revisão relida agora; depois relê. A revisão nova volta
    /// junto, ou `None` se a releitura falhou (a decisão já foi entregue; o segundo plano tenta de novo).
    pub async fn decide(self: &Arc<Self>, name: &str, body: DecideBody) -> Result<Option<Review>, Failure> {
        let session = self.resolve(name).await?.ok_or(Failure::NotLinked)?;
        let linked = self.read(&session, false).await?.ok_or(Failure::NotLinked)?;
        if !linked.supported {
            return Err(CallError::connector(ErrorCode::UnsupportedCapability, "o conector do ticket não oferece work_review").into());
        }
        let params = DecideParams::new(linked.target.ticket_id.clone(), body.review_id, body.decisions)?;
        let Some(review) = linked.review.as_ref().filter(|review| review.id == params.review_id) else {
            return Err(CallError::connector(ErrorCode::StaleReview, "a revisão mudou; releia antes de decidir").into());
        };
        check(review, &params.decisions)?;
        // Em tarefa própria: o cliente que cai no meio (rede do celular) não interrompe o envio, a
        // segunda geração nem a releitura, senão o chip ficaria em `waiting`.
        let (engine, connector) = (self.clone(), linked.target.connector);
        tokio::spawn(async move {
            // Antes e depois: a leitura que correu durante o envio também não grava.
            engine.bump(&session);
            let sent = engine.host.decide_review(&connector, params).await;
            engine.bump(&session);
            sent?;
            match engine.read(&session, false).await {
                Ok(linked) => Ok(linked.and_then(|linked| linked.review)),
                Err(failure) => {
                    engine.warn(&session.name, "work_review_reread_failed", failure.code());
                    engine.refresh(session);
                    Ok(None)
                }
            }
        }).await.unwrap_or_else(|_| Err(CallError::connector(ErrorCode::Unavailable, "envio das decisões interrompido").into()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }

    async fn resolve(&self, name: &str) -> Result<Option<SessionRef>, Failure> {
        self.sessions.resolve(name.to_owned()).await.map_err(unreadable)
    }

    /// A vaga da sessão nesta vida; outra vida com o mesmo nome começa do zero.
    fn slot<'a>(&self, state: &'a mut State, session: &SessionRef) -> &'a mut Slot {
        let slot = state.slots.entry(session.name.clone()).or_default();
        if slot.life != session.lifecycle_id {
            if slot.summary.is_some() { self.notify_changed(); }
            *slot = Slot { life: session.lifecycle_id.clone(), ..Slot::default() };
        }
        slot
    }

    fn schedule(self: &Arc<Self>, session: SessionRef, only_unseen: bool) {
        {
            let mut state = self.lock();
            let slot = self.slot(&mut state, &session);
            if only_unseen && slot.summary.is_some() { return; }
            if slot.running {
                slot.again = Some(session);
                return;
            }
            slot.running = true;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            let mut current = session;
            loop {
                if let Err(failure) = engine.read(&current, true).await {
                    engine.warn(&current.name, "work_review_refresh_failed", failure.code());
                }
                let mut state = engine.lock();
                let Some(slot) = state.slots.get_mut(&current.name).filter(|slot| slot.life == current.lifecycle_id) else { return };
                match slot.again.take() {
                    Some(next) => current = next,
                    None => { slot.running = false; return; }
                }
            }
        });
    }

    fn begin(&self, session: &SessionRef) -> Turn {
        let mut state = self.lock();
        state.seq += 1;
        let seq = state.seq;
        let slot = self.slot(&mut state, session);
        let generation = slot.generation;
        // Branch trocada: o guardado da anterior sai, e voltar a ela refaz a busca.
        let old = (slot.branch != session.branch).then(|| std::mem::replace(&mut slot.branch, session.branch.clone())).flatten();
        if let Some(old) = old { state.branches.retain(|(_, branch), _| *branch != old); }
        Turn { seq, generation }
    }

    fn bump(&self, session: &SessionRef) {
        let mut state = self.lock();
        self.slot(&mut state, session).generation += 1;
    }

    /// Grava o resultado se nenhuma escrita nem leitura mais nova passou na frente; `false` quando não gravou.
    fn publish(&self, session: &SessionRef, turn: Turn, target: Option<&Target>, review: Option<&Review>) -> bool {
        let mut state = self.lock();
        let Some(slot) = state.slots.get_mut(&session.name) else { return false };
        if slot.life != session.lifecycle_id || slot.generation != turn.generation || turn.seq < slot.published { return false; }
        slot.published = turn.seq;
        slot.target = target.cloned();
        let summary = review.map(Summary::of);
        if slot.summary != summary {
            slot.summary = summary;
            self.notify_changed();
        }
        true
    }

    /// Vínculo trocado de ticket: o resumo do anterior sai antes da leitura, que pode falhar.
    fn retarget(&self, session: &SessionRef, turn: Turn, target: &Target) {
        let mut state = self.lock();
        let Some(slot) = state.slots.get_mut(&session.name) else { return };
        if slot.life != session.lifecycle_id || turn.seq < slot.published || slot.target.as_ref().is_none_or(|old| old == target) { return; }
        slot.target = Some(target.clone());
        if slot.summary.take().is_some() { self.notify_changed(); }
    }

    async fn read(&self, session: &SessionRef, background: bool) -> Result<Option<Linked>, Failure> {
        let turn = self.begin(session);
        let catalog = self.catalog().await;
        let Some(target) = self.locate(session, background, &catalog).await? else {
            self.publish(session, turn, None, None);
            return Ok(None);
        };
        self.retarget(session, turn, &target);
        // Catálogo ilegível é falha da leitura: como "sem revisão", o chip e a aba sumiriam calados.
        let supported = catalog.as_ref().map_err(|error| CallError::from(error.clone()))?.iter()
            .find(|status| status.entry.id == target.connector).is_some_and(|status| offers_review(&status.entry));
        let review = if supported {
            let params = GetParams::new(target.ticket_id.clone())?;
            self.queued(background, &target.connector, self.host.get_review(&target.connector, params)).await?
        } else { None };
        // Depois de publicar: a imagem atrasada da rodada anterior já não passa pela conferência de `asset`.
        // A leitura que não gravou deixa a limpeza para a que gravou.
        if self.publish(session, turn, Some(&target), review.as_ref()) && let Some(review) = &review {
            self.assets.keep_only(&target.connector, &target.ticket_id, &review.id).await;
        }
        Ok(Some(Linked { target, supported, review, fetched_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }))
    }

    /// O vínculo do Tickets vence; sem ele, o ticket cujo `key` é a branch, em cada conector com a revisão.
    async fn locate(&self, session: &SessionRef, background: bool, catalog: &Catalog) -> Result<Option<Target>, Failure> {
        // Com a chave ilegível, `link::resolve` aceitaria o vínculo sem chave gravada, que com ela não confere.
        if !session.key_unreadable {
            let store = self.mission.clone();
            let snapshot = tokio::task::spawn_blocking(move || store.snapshot()).await
                .map_err(|_| CallError::connector(ErrorCode::Unavailable, "leitura dos tickets interrompida"))?
                .map_err(Failure::Mission)?;
            let linked = hangar_mission::link::resolve(&snapshot, &session.name, session.key.as_deref(), session.group_id.as_deref())
                .map_err(Failure::Mission)?;
            if let Some(item) = linked {
                return Ok(Some(Target { via: Via::Ticket, connector: item.connector, ticket_id: item.ticket_id, key: item.key }));
            }
        }
        let Some(branch) = session.branch.clone().filter(|branch| !branch.is_empty()) else { return Ok(None) };
        let connectors = catalog.as_ref().map_err(|error| CallError::from(error.clone()))?.iter()
            .filter(|status| offers_review(&status.entry))
            .map(|status| status.entry.id.clone());
        let mut failed = None;
        for connector in connectors {
            let cache_key = (connector.clone(), branch.clone());
            match self.lock().cached_branch(&cache_key, Instant::now()) {
                Some(Some(target)) => return Ok(Some(target)),
                Some(None) => continue,
                None => {}
            }
            let found = match GetParams::new(branch.clone()) {
                Ok(params) => self.queued(background, &connector, self.host.get_ticket(&connector, params)).await,
                Err(error) => Err(error.into()),
            };
            let hit = match found {
                Ok(detail) => (detail.summary.key == branch)
                    .then(|| Target { via: Via::Branch, connector: connector.clone(), ticket_id: detail.summary.id, key: detail.summary.key }),
                // Branch que não é ticket é resposta, não falha.
                Err(CallError::Connector(error)) if matches!(error.code, ErrorCode::NotFound | ErrorCode::InvalidParams) => None,
                Err(error) => { failed.get_or_insert(error); continue; }
            };
            self.lock().branches.insert(cache_key, BranchHit { target: hit.clone(), at: Instant::now() });
            if hit.is_some() { return Ok(hit); }
        }
        failed.map_or(Ok(None), |error| Err(error.into()))
    }

    /// Segundo plano: uma chamada por vez por conector, para o pedido da pessoa esperar no máximo uma.
    async fn queued<T>(&self, background: bool, connector: &str, call: impl Future<Output = Result<T, CallError>>) -> Result<T, CallError> {
        if !background { return call.await; }
        let queue = self.queues.lock().unwrap_or_else(|e| e.into_inner()).entry(connector.to_owned()).or_insert_with(|| Arc::new(Semaphore::new(1))).clone();
        let _turn = queue.acquire_owned().await.expect("o semáforo da fila nunca fecha");
        call.await
    }

    /// Falha em segundo plano: ao diário, uma vez por minuto por sessão e código; a tela não vê.
    fn warn(&self, session: &str, code: &'static str, detail: &str) {
        if !hangar_core::warn_limit::allow(Some(session), &format!("{code}:{detail}")) { return; }
        tracing::warn!(code, session, detail, "work_review: releitura falhou; vale o resumo anterior");
        self.diag.report(code, session, detail);
    }
}

/// Lote em espera com as decisões enviadas: todas as mostradas, escolha que existe, motivo onde é pedido.
/// Fora de `waiting`, ou com nenhuma enviada ainda pendente, é reenvio de lote já aceito: o conector é
/// idempotente e decide.
fn check(review: &Review, answers: &[Answer]) -> Result<(), Failure> {
    if review.state != ReviewState::Waiting { return Ok(()); }
    let present = answers.iter().filter(|answer| review.decisions.iter().any(|decision| decision.id == answer.id)).count();
    if present == 0 { return Ok(()); }
    if present != answers.len() { return Err(invalid("o envio mistura decisões pendentes e já decididas; releia a revisão")); }
    for decision in &review.decisions {
        let Some(answer) = answers.iter().find(|answer| answer.id == decision.id) else {
            return Err(invalid(format!("falta a decisão {}", decision.id)));
        };
        let Some(choice) = decision.choices.iter().find(|choice| choice.id == answer.choice) else {
            return Err(invalid(format!("a escolha {} não existe na decisão {}", answer.choice, decision.id)));
        };
        if choice.requires_reason && answer.reason.as_deref().is_none_or(|reason| reason.trim().is_empty()) {
            return Err(invalid(format!("a decisão {} pede o motivo", decision.id)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoSessions;
    impl Sessions for NoSessions { fn resolve(&self, _: String) -> Task<Result<Option<SessionRef>, String>> { Box::pin(async { Ok(None) }) } }
    struct NoDiag;
    impl Diag for NoDiag { fn report(&self, _: &str, _: &str, _: &str) {} }

    fn review(state: &str) -> Review {
        serde_json::from_value(json!({"id": "r-1", "title": "t", "updated_at": "2026-11-18T10:00:00Z", "state": state})).unwrap()
    }

    /// O alvo achatado sai nos mesmos campos e na mesma ordem de antes, no topo do corpo da rota.
    #[test]
    fn linked_serializes_the_target_flat_in_place() {
        let linked = Linked { target: Target { via: Via::Branch, connector: "help".into(), ticket_id: "1".into(), key: "PM-1".into() },
            supported: true, review: None, fetched_at: "2026-11-18T10:00:00.000Z".into() };
        assert_eq!(serde_json::to_string(&linked).unwrap(),
            r#"{"via":"branch","connector":"help","ticket_id":"1","key":"PM-1","supported":true,"review":null,"fetched_at":"2026-11-18T10:00:00.000Z"}"#);
    }

    #[test]
    fn a_branch_miss_expires_but_a_hit_stays() {
        let mut state = State::default();
        let at = Instant::now();
        let target = Target { via: Via::Branch, connector: "help".into(), ticket_id: "1".into(), key: "PM-1".into() };
        let (hit, miss) = (("help".to_owned(), "PM-1".to_owned()), ("help".to_owned(), "PM-2".to_owned()));
        state.branches.insert(hit.clone(), BranchHit { target: Some(target.clone()), at });
        state.branches.insert(miss.clone(), BranchHit { target: None, at });
        let later = at + BRANCH_MISS_TTL;
        assert_eq!(state.cached_branch(&miss, at), Some(None), "o negativo vale por um tempo");
        assert_eq!(state.cached_branch(&miss, later), None, "depois, a branch é procurada de novo");
        assert_eq!(state.cached_branch(&hit, later), Some(Some(target)), "o achado fica");
    }

    /// O host serializa as chamadas por conector, então a corrida não se monta pela rota: a leitura
    /// começada antes do envio é simulada pela ordem dos passos.
    #[tokio::test]
    async fn read_started_before_a_write_does_not_publish() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(hangar_connectors::registry::Registry::new(dir.path().join("c.json")), Default::default());
        let engine = Engine::new(host, Arc::new(Store::new(dir.path().join("m"))), Arc::new(NoSessions), Arc::new(NoDiag), dir.path().join("cache"));
        let session = SessionRef { name: "s1".into(), lifecycle_id: Some("l".into()), ..Default::default() };
        let summary = |engine: &Engine| engine.summaries_for(&[("s1", Some("l"))])["reviews"]["s1"]["state"].clone();
        let before = engine.begin(&session);
        engine.bump(&session);
        engine.publish(&session, before, None, Some(&review("waiting")));
        assert!(summary(&engine).is_null(), "começou antes do envio: não grava, mesmo sem leitura mais nova");
        let after = engine.begin(&session);
        engine.publish(&session, after, None, Some(&review("applying")));
        assert_eq!(summary(&engine), "applying");
        let older = engine.begin(&session);
        let newer = engine.begin(&session);
        engine.publish(&session, newer, None, Some(&review("done")));
        engine.publish(&session, older, None, Some(&review("applying")));
        assert_eq!(summary(&engine), "done", "a leitura mais nova vence");
    }
}
