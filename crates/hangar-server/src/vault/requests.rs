//! Pedidos do cofre na dona: aprovação de uso (`approve`) e cadastro pelo cartão (`entry`). Só na
//! memória: reiniciar o servidor perde os pedidos, as entradas temporárias e as liberações, e quem
//! pediu recebe `erro_cofre_pedido_perdido`. Nenhum tipo daqui que leva valor tem `Debug`.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, sync::{Arc, Mutex, MutexGuard}, time::Duration};
use tokio::{sync::watch, time::Instant};
use zeroize::Zeroizing;

use super::{KeyError, NewEntry, Policy, State, Vault, VaultError, remote::{UseStart, WaitStatus}, wire::{AskReq, UseKind, UseReq}};
use crate::routes::AppState;

/// Prazo para o dono responder.
pub const EXPIRES: Duration = Duration::from_secs(10 * 60);
/// Sem nenhum `wait` nesse tempo, quem pediu foi embora (reiniciou ou desistiu).
pub const IDLE: Duration = Duration::from_secs(60);
/// Vida da entrada "só desta vez" e da liberação do primeiro uso depois do cadastro.
pub const TEMPORARY: Duration = Duration::from_secs(24 * 60 * 60);
/// Um peer com o token não enche a memória nem a tela do dono.
const MAX_OPEN: usize = 32;
/// Uma sessão sozinha não enche a tela do dono de cartões.
const MAX_OPEN_PER_SESSION: usize = 3;
pub const COOLDOWN: Duration = Duration::from_secs(30);
const MAX_TEXT: usize = 2048;
const MAX_KEY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind { Approve, Entry }

/// Um pedido como o cartão o mostra (evento `vault`). Nada aqui é segredo.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Card {
    pub id: String,
    pub interaction_id: String,
    pub kind: Kind,
    /// `servidor::sessao`, como quem pediu declarou.
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub reason: String,
    #[serde(rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_kind: Option<UseKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    pub expires_at: f64,
}

/// O que o evento `vault` publica. `state` é o último estado do cofre que esta máquina viu; `None`
/// antes da primeira leitura (abrir o cofre só para o evento tocaria o chaveiro).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Feed { pub state: Option<State>, pub requests: Vec<Card> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Vault(VaultError),
    /// `erro_cofre_pedido_perdido`
    Lost,
    /// `erro_interacao_desatualizada`
    Stale,
    /// `erro_cofre_pedido_respondido`
    Answered,
    /// `erro_cofre_pedidos_demais`
    TooMany,
    /// `erro_cofre_pedidos_demais` também: o dono recusou esta sessão há menos de `COOLDOWN`.
    Cooldown,
}

impl From<VaultError> for RequestError {
    fn from(error: VaultError) -> Self { Self::Vault(error) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision { Allow, Deny, Save, Decline }

/// `POST /api/vault/requests/{id}`. Campos planos, sem `tag`: o serde não copia a senha num buffer
/// intermediário que não zera.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub interaction_id: String,
    pub decision: Decision,
    #[serde(default)]
    pub password: Option<Zeroizing<String>>,
    #[serde(default)]
    pub keep: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub policy: Option<Policy>,
}

/// O resultado de `start_use`: o valor na hora ou o pedido aberto (o cartão, para o aviso).
pub enum Start { Value(Zeroizing<String>), Opened(Card) }

struct Request {
    card: Card,
    seq: u64,
    session_key: String,
    deadline: Instant,
    idle: Instant,
    /// Uma resposta em curso (lendo ou gravando o cofre): a segunda recebe `Answered`.
    claimed: bool,
    outcome: Option<WaitStatus>,
    /// Anda a cada mudança e cai junto com o pedido: acorda o `wait` e o prazo.
    changed: watch::Sender<()>,
}

struct Temporary { session_key: String, name: String, value: Zeroizing<String>, until: Instant }

struct Grant { session_key: String, name: String, until: Instant }

#[derive(Default)]
struct Inner { requests: HashMap<String, Request>, temporary: Vec<Temporary>, grants: Vec<Grant>, state: Option<State>, seq: u64,
    /// Sessão que o dono acabou de recusar não abre outro cartão antes disso.
    cooldown: HashMap<String, Instant> }

/// Um por servidor (`AppState.vault_requests`); o `ListHub` assina o `feed`.
pub struct Requests { inner: Mutex<Inner>, feed: watch::Sender<Arc<Feed>> }

impl Default for Requests {
    fn default() -> Self { Self { inner: Mutex::default(), feed: watch::channel(Arc::default()).0 } }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

fn text_ok(text: &str, max: usize) -> bool { !text.trim().is_empty() && text.chars().count() <= max }

fn origin(server: &str, session: &str) -> String { format!("{server}::{session}") }

fn use_ok(req: &UseReq) -> bool {
    super::config::valid_server_id(&req.origin_server) && text_ok(&req.session, 128) && text_ok(&req.session_key, MAX_KEY)
        && super::name_ok(&req.entry) && text_ok(&req.reason, MAX_TEXT) && req.target.chars().count() <= MAX_TEXT
}

fn ask_ok(req: &AskReq) -> bool {
    super::config::valid_server_id(&req.origin_server) && text_ok(&req.session, 128) && text_ok(&req.session_key, MAX_KEY)
        && super::name_ok(&req.label) && text_ok(&req.reason, MAX_TEXT) && super::field_ok(&req.placeholder, MAX_TEXT)
}

/// Decisão que cabe no tipo do pedido, com os campos dela e só eles.
fn answer_ok(kind: Kind, a: &Answer) -> bool {
    match (kind, a.decision) {
        (Kind::Approve, Decision::Allow | Decision::Deny) | (Kind::Entry, Decision::Decline) =>
            a.password.is_none() && a.keep.is_none() && a.name.is_none() && a.policy.is_none(),
        (Kind::Entry, Decision::Save) =>
            a.password.as_deref().is_some_and(|p| super::secret_ok(p)) && a.keep.is_some() && super::field_ok(&a.name, super::MAX_NAME),
        _ => false,
    }
}

/// O estado do cofre que uma operação nele revelou; falha de disco não diz nada sobre ele.
fn seen(result: Result<(), VaultError>) -> Option<State> {
    match result {
        Ok(()) | Err(VaultError::UnknownEntry | VaultError::DuplicateName) => Some(State::Open),
        Err(VaultError::Key(KeyError::Locked | KeyError::Bus)) => Some(State::Locked),
        Err(VaultError::Key(KeyError::Lost | KeyError::Absent)) => Some(State::KeyLost),
        Err(VaultError::Absent | VaultError::Key(KeyError::Unsupported)) => Some(State::Absent),
        Err(VaultError::Unreadable | VaultError::Io | VaultError::Invalid) => None,
    }
}

fn copy(status: &WaitStatus) -> WaitStatus {
    match status {
        WaitStatus::Pending => WaitStatus::Pending,
        WaitStatus::Allowed(value) => WaitStatus::Allowed(value.clone()),
        WaitStatus::Saved { entry, temporary } => WaitStatus::Saved { entry: entry.clone(), temporary: *temporary },
        WaitStatus::Denied => WaitStatus::Denied,
        WaitStatus::Declined => WaitStatus::Declined,
        WaitStatus::TimedOut => WaitStatus::TimedOut,
        WaitStatus::Cancelled => WaitStatus::Cancelled,
    }
}

fn prune(inner: &mut Inner, now: Instant) {
    inner.temporary.retain(|t| t.until > now);
    inner.grants.retain(|g| g.until > now);
}

/// Resposta que perdeu a conexão no meio da leitura ou gravação devolve o pedido ao dono.
struct Claim<'a> { requests: &'a Requests, id: &'a str, armed: bool }

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.armed { return; }
        let mut inner = lock(&self.requests.inner);
        if let Some(request) = inner.requests.get_mut(self.id) {
            request.claimed = false;
            request.changed.send_replace(());
        }
    }
}

impl Requests {
    pub fn feed(&self) -> watch::Receiver<Arc<Feed>> { self.feed.subscribe() }

    /// Estado lido pela rota de estado entra no evento.
    pub fn note_state(&self, state: State) {
        let mut inner = lock(&self.inner);
        inner.state = Some(state);
        self.publish(&inner);
    }

    fn note(inner: &mut Inner, result: Result<(), VaultError>) {
        if let Some(state) = seen(result) { inner.state = Some(state); }
    }

    /// Sai só quando muda: pedidos abertos na ordem em que chegaram.
    fn publish(&self, inner: &Inner) {
        let mut open: Vec<&Request> = inner.requests.values().filter(|r| r.outcome.is_none()).collect();
        open.sort_by_key(|r| r.seq);
        let next = Feed { state: inner.state, requests: open.into_iter().map(|r| r.card.clone()).collect() };
        self.feed.send_if_modified(|current| if **current == next { false } else { *current = Arc::new(next); true });
    }

    fn open(self: &Arc<Self>, mut card: Card, session_key: String) -> Result<Card, RequestError> {
        let (id, interaction_id) = (super::new_id()?, super::new_id()?);
        let now = Instant::now();
        let mut inner = lock(&self.inner);
        if inner.requests.len() >= MAX_OPEN { return Err(RequestError::TooMany); }
        let mine = inner.requests.values().filter(|r| r.outcome.is_none() && r.session_key == session_key).count();
        if mine >= MAX_OPEN_PER_SESSION { return Err(RequestError::TooMany); }
        inner.cooldown.retain(|_, until| *until > now);
        if inner.cooldown.contains_key(&session_key) { return Err(RequestError::Cooldown); }
        card.id = id.clone();
        card.interaction_id = interaction_id;
        card.expires_at = super::now() + EXPIRES.as_secs_f64();
        inner.seq += 1;
        let seq = inner.seq;
        inner.requests.insert(id.clone(), Request { card: card.clone(), seq, session_key, deadline: now + EXPIRES, idle: now + IDLE,
            claimed: false, outcome: None, changed: watch::channel(()).0 });
        self.publish(&inner);
        drop(inner);
        tokio::spawn(self.clone().deadlines(id));
        Ok(card)
    }

    /// Entrada temporária da sessão, senha liberada ou primeiro uso depois do cadastro pelo cartão
    /// devolvem o valor; o resto vira pedido de aprovação. Sem aviso: quem avisa é `use_entry`.
    pub async fn start_use(self: &Arc<Self>, vault: &Vault, hangar_dir: &Path, req: UseReq) -> Result<Start, RequestError> {
        if !use_ok(&req) { return Err(VaultError::Invalid.into()); }
        {
            let mut inner = lock(&self.inner);
            prune(&mut inner, Instant::now());
            if let Some(t) = inner.temporary.iter().find(|t| t.session_key == req.session_key && super::same_name(&t.name, &req.entry)) {
                return Ok(Start::Value(t.value.clone()));
            }
        }
        let found = vault.by_name(hangar_dir, &req.entry).await;
        let name = {
            let mut inner = lock(&self.inner);
            Self::note(&mut inner, found.as_ref().map(drop).map_err(|e| *e));
            self.publish(&inner);
            let entry = found?;
            if entry.policy == Policy::Free { return Ok(Start::Value(entry.password)); }
            let grant = inner.grants.iter().position(|g| g.session_key == req.session_key && super::same_name(&g.name, &entry.name));
            if let Some(at) = grant {
                inner.grants.remove(at);
                return Ok(Start::Value(entry.password));
            }
            entry.name.clone()
        };
        let card = Card { id: String::new(), interaction_id: String::new(), kind: Kind::Approve, origin: origin(&req.origin_server, &req.session),
            entry_name: Some(name), label: None, reason: req.reason, use_kind: Some(req.kind), target: Some(req.target), placeholder: None,
            expires_at: 0.0 };
        self.open(card, req.session_key).map(Start::Opened)
    }

    /// Cadastro pelo cartão; sem cofre aqui não haveria onde guardar.
    pub fn start_ask(self: &Arc<Self>, hangar_dir: &Path, req: AskReq) -> Result<Card, RequestError> {
        if !ask_ok(&req) { return Err(VaultError::Invalid.into()); }
        if !super::store::exists(&super::store::path(hangar_dir)) { return Err(VaultError::Absent.into()); }
        let card = Card { id: String::new(), interaction_id: String::new(), kind: Kind::Entry, origin: origin(&req.origin_server, &req.session),
            entry_name: None, label: Some(req.label.trim().to_owned()), reason: req.reason, use_kind: None, target: None,
            placeholder: req.placeholder, expires_at: 0.0 };
        self.open(card, req.session_key)
    }

    /// Consulta longa: o desfecho assim que houver, ou `Pending` depois de `hold`. O desfecho
    /// vale para toda espera até 60 s depois da última; aí o id é `Lost`. Cada chamada renova os 60 s.
    pub async fn wait(&self, id: &str, hold: Duration) -> Result<WaitStatus, RequestError> {
        let until = Instant::now() + hold;
        loop {
            let mut rx = {
                let mut inner = lock(&self.inner);
                let request = inner.requests.get_mut(id).ok_or(RequestError::Lost)?;
                request.idle = request.idle.max(Instant::now() + IDLE);
                // O desfecho fica até 60 s depois da última espera: a espera antiga que ainda segurava a
                // consulta não o tira da nova, que quem pediu abriu depois de uma falha de rede.
                match &request.outcome {
                    None => request.changed.subscribe(),
                    Some(outcome) => return Ok(copy(outcome)),
                }
            };
            if tokio::time::timeout_at(until, rx.changed()).await.is_err() {
                let mut inner = lock(&self.inner);
                let request = inner.requests.get_mut(id).ok_or(RequestError::Lost)?;
                // Os 60 s contam do fim desta espera, não do começo.
                request.idle = request.idle.max(Instant::now() + IDLE);
                match &request.outcome {
                    None => return Ok(WaitStatus::Pending),
                    Some(outcome) => return Ok(copy(outcome)),
                }
            }
        }
    }

    /// Quem pediu desistiu: o pedido fecha e o cartão some.
    pub fn cancel(&self, id: &str) -> Result<(), RequestError> {
        let mut inner = lock(&self.inner);
        inner.requests.remove(id).ok_or(RequestError::Lost)?;
        self.publish(&inner);
        Ok(())
    }

    /// A resposta do dono. Só a primeira vale; falha do cofre (chaveiro trancado, nome repetido)
    /// devolve o erro e deixa o pedido aberto. "Guardar" grava no cofre e libera o primeiro uso
    /// daquela sessão; sem guardar, a senha fica só na memória, para aquela sessão, por 24 h.
    pub async fn answer(&self, vault: &Vault, hangar_dir: &Path, id: &str, answer: Answer) -> Result<(), RequestError> {
        let (session_key, entry_name, label) = {
            let mut inner = lock(&self.inner);
            let request = inner.requests.get_mut(id).ok_or(RequestError::Lost)?;
            if request.outcome.is_some() || request.claimed { return Err(RequestError::Answered); }
            if request.card.interaction_id != answer.interaction_id { return Err(RequestError::Stale); }
            if !answer_ok(request.card.kind, &answer) { return Err(VaultError::Invalid.into()); }
            request.claimed = true;
            (request.session_key.clone(), request.card.entry_name.clone(), request.card.label.clone())
        };
        let mut claim = Claim { requests: self, id, armed: true };
        let name = answer.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).or(label).unwrap_or_default();
        if answer.decision == Decision::Save && !super::name_ok(&name) { return Err(VaultError::Invalid.into()); }
        let mut password = answer.password;
        let (result, touched) = match answer.decision {
            Decision::Deny => (Ok(WaitStatus::Denied), false),
            Decision::Decline => (Ok(WaitStatus::Declined), false),
            Decision::Allow => {
                let found = vault.by_name(hangar_dir, entry_name.as_deref().unwrap_or_default()).await;
                (found.map(|e| WaitStatus::Allowed(e.password)), true)
            }
            Decision::Save if answer.keep == Some(true) => {
                let Some(secret) = password.take() else { return Err(VaultError::Invalid.into()) };
                let new = NewEntry { name: name.clone(), username: None, password: secret, url: None, notes: None,
                    policy: answer.policy.unwrap_or_default() };
                (vault.add(hangar_dir, new).await.map(|_| WaitStatus::Saved { entry: name.clone(), temporary: false }), true)
            }
            Decision::Save => (Ok(WaitStatus::Saved { entry: name.clone(), temporary: true }), false),
        };
        claim.armed = false;
        let mut guard = lock(&self.inner);
        let inner = &mut *guard;
        if touched { Self::note(inner, result.as_ref().map(drop).map_err(|e| *e)); }
        let Some(request) = inner.requests.get_mut(id) else {
            // Quem pediu desistiu durante a gravação: a senha guardada fica, sem liberação.
            self.publish(inner);
            return match result { Ok(WaitStatus::Saved { temporary: false, .. }) => Ok(()), _ => Err(RequestError::Lost) };
        };
        request.claimed = false;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                request.changed.send_replace(());
                self.publish(inner);
                return Err(error.into());
            }
        };
        let now = Instant::now();
        match &outcome {
            WaitStatus::Saved { entry, temporary: false } =>
                inner.grants.push(Grant { session_key, name: entry.clone(), until: now + TEMPORARY }),
            WaitStatus::Saved { entry, temporary: true } => {
                inner.temporary.retain(|t| !(t.session_key == session_key && super::same_name(&t.name, entry)));
                if let Some(value) = password.take() {
                    inner.temporary.push(Temporary { session_key, name: entry.clone(), value, until: now + TEMPORARY });
                }
            }
            WaitStatus::Denied | WaitStatus::Declined => { inner.cooldown.insert(session_key, now + COOLDOWN); }
            _ => {}
        }
        request.outcome = Some(outcome);
        request.idle = request.idle.max(now + IDLE);
        request.changed.send_replace(());
        self.publish(inner);
        Ok(())
    }

    /// Um temporizador por pedido, dormindo até o próximo prazo: 10 min fecha como `timed_out`;
    /// 60 s sem `wait` some (quem pediu foi embora); desfecho não buscado em 60 s também some.
    async fn deadlines(self: Arc<Self>, id: String) {
        loop {
            let (wake, mut rx) = {
                let mut guard = lock(&self.inner);
                let inner = &mut *guard;
                let now = Instant::now();
                let Some(request) = inner.requests.get_mut(&id) else { return };
                let wake = if request.claimed {
                    None
                } else if request.outcome.is_some() || now < request.deadline {
                    if now >= request.idle {
                        inner.requests.remove(&id);
                        self.publish(inner);
                        return;
                    }
                    Some(if request.outcome.is_some() { request.idle } else { request.deadline.min(request.idle) })
                } else {
                    request.outcome = Some(WaitStatus::TimedOut);
                    request.idle = request.idle.max(now + IDLE);
                    request.changed.send_replace(());
                    self.publish(inner);
                    continue;
                };
                (wake, request.changed.subscribe())
            };
            match wake {
                Some(at) => tokio::select! {
                    () = tokio::time::sleep_until(at) => {}
                    _ = rx.changed() => {}
                },
                None => { let _ = rx.changed().await; }
            }
        }
    }
}

/// `POST /api/vault/use` e a tool da Task 4 nesta máquina: abre o pedido e avisa o celular.
pub async fn use_entry(st: &Arc<AppState>, req: UseReq) -> Result<UseStart, RequestError> {
    let session = req.session.clone();
    match st.vault_requests.start_use(&st.vault, &st.hangar_dir, req).await? {
        Start::Value(value) => Ok(UseStart::Value(value)),
        Start::Opened(card) => {
            announce(st, &session, &card);
            Ok(UseStart::Request(card.id))
        }
    }
}

/// `POST /api/vault/ask-entry` e `request_secret` nesta máquina.
pub fn ask_entry(st: &Arc<AppState>, req: AskReq) -> Result<String, RequestError> {
    let session = req.session.clone();
    let card = st.vault_requests.start_ask(&st.hangar_dir, req)?;
    announce(st, &session, &card);
    Ok(card.id)
}

fn announce(st: &Arc<AppState>, session: &str, card: &Card) {
    // Teste nunca manda aviso real ao celular do dono.
    if cfg!(test) { return; }
    let label = card.entry_name.clone().or_else(|| card.label.clone()).unwrap_or_default();
    let notice = crate::push::send::Notice::VaultRequest { server_id: st.server_id.clone(), request_id: card.id.clone(),
        origin: card.origin.clone(), label };
    crate::push::send::Sender::new(st.clone(), crate::push::server_env()).vault_request(session, notice);
}
