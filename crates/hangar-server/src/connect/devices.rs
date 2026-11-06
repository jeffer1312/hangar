//! Credenciais de dono por aparelho (`~/.hangar/connect/devices.json`).
//!
//! Só hashes vão ao disco; o token cru existe só na resposta que o entrega, uma vez. Este processo
//! é o único escritor, então o estado em memória é a verdade e o disco é a cópia.
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::login_guests::store as credentials;

pub const MAX_PENDING: usize = 5;
pub const PENDING_TTL: u64 = 10 * 60;

pub type DeviceId = String;
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// O que o serviço do Connect informou ao ligar esta máquina.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceIdentity {
    pub public_key: [u8; 32],
    pub machine_id: String,
    pub account: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status { Pending, Approved, Denied }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Device {
    id: DeviceId,
    name: String,
    status: Status,
    created_at: u64,
    /// Recusado fica até o prazo, para o aparelho ler `denied` em vez de um erro.
    #[serde(default)]
    denied_at: Option<u64>,
    /// `sha256_hex(secret)`; ausente no aparelho criado com a senha do dono, que não consulta estado.
    #[serde(default)]
    verifier: Option<String>,
    #[serde(default)]
    token_hash: Option<String>,
    #[serde(default)]
    last_seen: Option<u64>,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
struct State {
    devices: Vec<Device>,
    /// Nonce gasto e até quando o bilhete dele valeria: depois disso a validade já o recusa.
    nonces: HashMap<String, u64>,
}

#[derive(Deserialize)]
struct Ticket {
    machine_id: String,
    account: String,
    device_name: String,
    verifier: String,
    nonce: String,
    expires_at: u64,
}

#[derive(Debug)]
pub enum Refusal {
    NotLinked,
    /// Toda recusa do bilhete é igual: quem não tem o segredo não aprende qual conferência falhou.
    Ticket,
    TooManyPending,
    Credential,
    Unknown,
    Io(io::Error),
}

impl From<io::Error> for Refusal {
    fn from(error: io::Error) -> Self { Self::Io(error) }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    Denied,
    /// O token sai uma vez só: nas consultas seguintes vem `None`.
    Approved(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub device_id: DeviceId,
    pub name: String,
    pub status: Status,
    pub last_seen: Option<u64>,
}

pub struct Store {
    path: PathBuf,
    identity: Option<ServiceIdentity>,
    clock: Clock,
    state: Mutex<State>,
}

impl Store {
    pub fn new(dir: PathBuf, identity: Option<ServiceIdentity>) -> Self {
        let clock: Clock = Arc::new(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
        });
        let path = dir.join("devices.json");
        let state = load(&path, clock());
        Self { path, identity, clock, state: Mutex::new(state) }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Grava antes de valer: nonce gasto ou token entregue nunca ficam só na memória.
    fn update<T>(&self, change: impl FnOnce(&mut State, u64) -> Result<T, Refusal>) -> Result<T, Refusal> {
        let now = (self.clock)();
        let mut state = self.lock();
        let mut next = state.clone();
        prune(&mut next, now);
        let value = change(&mut next, now)?;
        if next != *state {
            write(&self.path, &next)?;
            *state = next;
        }
        Ok(value)
    }

    /// `signature` confere os bytes de `ticket` como vieram: o bilhete nunca é serializado de novo.
    pub fn redeem(&self, ticket: &str, signature: &str, secret: &str) -> Result<DeviceId, Refusal> {
        let identity = self.identity.as_ref().ok_or(Refusal::NotLinked)?;
        let signature = base64::engine::general_purpose::STANDARD.decode(signature).map_err(|_| Refusal::Ticket)?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, identity.public_key)
            .verify(ticket.as_bytes(), &signature).map_err(|_| Refusal::Ticket)?;
        let ticket: Ticket = serde_json::from_str(ticket).map_err(|_| Refusal::Ticket)?;
        if ticket.machine_id != identity.machine_id || ticket.account != identity.account {
            return Err(Refusal::Ticket);
        }
        self.update(|state, now| {
            if ticket.expires_at <= now || state.nonces.contains_key(&ticket.nonce) {
                return Err(Refusal::Ticket);
            }
            let verifier = credentials::hash(secret.as_bytes());
            if !same(&verifier, &ticket.verifier) {
                return Err(Refusal::Ticket);
            }
            if state.devices.iter().filter(|d| d.status == Status::Pending).count() >= MAX_PENDING {
                return Err(Refusal::TooManyPending);
            }
            // Só aqui: quem tem o bilhete sem o segredo, ou bateu no limite, não queima o nonce.
            state.nonces.insert(ticket.nonce.clone(), ticket.expires_at);
            let id = credentials::id();
            state.devices.push(Device { id: id.clone(), name: display_name(&ticket.device_name), status: Status::Pending,
                created_at: now, denied_at: None, verifier: Some(verifier), token_hash: None, last_seen: None });
            Ok(id)
        })
    }

    pub fn status(&self, id: &str, secret: &str) -> Result<Poll, Refusal> {
        let presented = credentials::hash(secret.as_bytes());
        self.update(|state, _| {
            let device = state.devices.iter_mut().find(|d| d.id == id).ok_or(Refusal::Credential)?;
            if !device.verifier.as_deref().is_some_and(|v| same(v, &presented)) {
                return Err(Refusal::Credential);
            }
            Ok(match device.status {
                Status::Pending => Poll::Pending,
                Status::Denied => Poll::Denied,
                Status::Approved if device.token_hash.is_some() => Poll::Approved(None),
                Status::Approved => {
                    let token = credentials::token();
                    device.token_hash = Some(credentials::hash(token.as_bytes()));
                    Poll::Approved(Some(token))
                }
            })
        })
    }

    pub fn approve(&self, id: &str) -> Result<(), Refusal> {
        self.update(|state, _| {
            let device = state.devices.iter_mut().find(|d| d.id == id && d.status != Status::Denied).ok_or(Refusal::Unknown)?;
            device.status = Status::Approved;
            Ok(())
        })
    }

    /// Recusa quem ainda não recebeu token; quem já recebeu perde a credencial na hora.
    pub fn remove(&self, id: &str) -> Result<(), Refusal> {
        self.update(|state, now| {
            let index = state.devices.iter().position(|d| d.id == id).ok_or(Refusal::Unknown)?;
            if state.devices[index].token_hash.is_some() {
                state.devices.remove(index);
            } else {
                deny(&mut state.devices[index], now);
            }
            Ok(())
        })
    }

    /// Aparelho sem aprovação: quem chama já provou a senha do dono.
    pub fn enroll(&self, name: &str) -> Result<(DeviceId, String), Refusal> {
        self.update(|state, now| {
            let (id, token) = (credentials::id(), credentials::token());
            state.devices.push(Device { id: id.clone(), name: display_name(name), status: Status::Approved, created_at: now,
                denied_at: None, verifier: None, token_hash: Some(credentials::hash(token.as_bytes())), last_seen: None });
            Ok((id, token))
        })
    }

    pub fn list(&self) -> Vec<Row> {
        let now = (self.clock)();
        let mut state = self.lock();
        prune(&mut state, now);
        state.devices.iter().map(|d| Row { device_id: d.id.clone(), name: d.name.clone(), status: d.status, last_seen: d.last_seen }).collect()
    }

    /// shortcut: `last_seen` só vai ao disco com a próxima gravação; gravar por pedido custaria um fsync a cada um.
    pub fn verify(&self, token: &[u8]) -> Option<DeviceId> {
        let presented = credentials::hash(token);
        let now = (self.clock)();
        let mut state = self.lock();
        let device = state.devices.iter_mut().find(|d| d.status == Status::Approved
            && d.token_hash.as_deref().is_some_and(|hash| same(hash, &presented)))?;
        device.last_seen = Some(now);
        Some(device.id.clone())
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.proof_key(id).is_some()
    }

    /// O hash guardado é a chave da prova: a máquina mostra que conhece o token sem guardá-lo.
    pub fn proof_key(&self, id: &str) -> Option<String> {
        self.lock().devices.iter().find(|d| d.id == id && d.status == Status::Approved).and_then(|d| d.token_hash.clone())
    }
}

fn same(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

fn display_name(raw: &str) -> String {
    raw.trim().chars().take(80).collect()
}

fn deny(device: &mut Device, now: u64) {
    device.status = Status::Denied;
    device.denied_at.get_or_insert(now);
}

fn prune(state: &mut State, now: u64) {
    state.nonces.retain(|_, expires_at| *expires_at > now);
    for device in state.devices.iter_mut().filter(|d| d.status == Status::Pending && now >= d.created_at + PENDING_TTL) {
        deny(device, now);
    }
    state.devices.retain(|d| d.denied_at.is_none_or(|at| now < at + PENDING_TTL));
}

fn write(path: &Path, state: &State) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::runtime::queue::atomic_write(path, &serde_json::to_vec_pretty(state).map_err(io::Error::other)?)
}

fn load(path: &Path, now: u64) -> State {
    let parsed = std::fs::read(path).map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice::<State>(&bytes).map_err(|e| e.to_string()));
    match parsed {
        Ok(state) => state,
        Err(_) if !path.exists() => State::default(),
        Err(error) => {
            // Ilegível vira "nenhum aparelho": perde acessos, e vai pro lado para a próxima gravação não apagá-lo.
            tracing::warn!(code = "connect_devices_unreadable", "{} ilegível, começando vazio: {error}", path.display());
            let bad = path.with_extension(format!("bad-{now}"));
            if let Err(e) = std::fs::rename(path, &bad) {
                tracing::warn!(code = "connect_devices_unreadable", "não consegui guardar {} ilegível: {e}", path.display());
            }
            State::default()
        }
    }
}
