//! Credenciais de dono por aparelho (`~/.hangar/connect/devices.json`).
//!
//! Só hashes vão ao disco; o token cru existe só na resposta que o entrega, uma vez. Este processo
//! é o único escritor, então o estado em memória é a verdade e o disco é a cópia.
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use base64::Engine;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use hangar_core::credentials;

pub const MAX_PENDING: usize = 5;
pub const PENDING_TTL: u64 = 10 * 60;
/// O serviço assina bilhetes de um minuto: validade longa é chave vazada ou relógio errado.
pub const MAX_TICKET_AHEAD: u64 = 5 * 60;

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
    /// Hash do token novo em entrega: vale junto com o antigo até a entrega se confirmar, senão quem
    /// está fora do ar perderia o acesso sem saber.
    #[serde(default)]
    next_token_hash: Option<String>,
    #[serde(default)]
    last_seen: Option<u64>,
    /// Aparelho que aprovou este; `None` é o dono. Revogar um leva junto tudo que ele aprovou.
    #[serde(default)]
    approved_by: Option<DeviceId>,
    #[serde(default)]
    approved_at: Option<u64>,
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
    /// Aparelho pedindo para revogar outro que não aprovou.
    NotYours,
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
    identity: RwLock<Option<ServiceIdentity>>,
    clock: Clock,
    state: Mutex<State>,
    /// `devices.json` ficou no lugar sem ser lido: gravar por cima perderia os aparelhos que ele guarda.
    fault: Option<String>,
}

impl Store {
    pub fn new(dir: PathBuf, identity: Option<ServiceIdentity>) -> Self {
        let clock: Clock = Arc::new(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
        });
        let path = dir.join("devices.json");
        let (state, fault) = load(&path, clock());
        Self { path, identity: RwLock::new(identity), clock, state: Mutex::new(state), fault }
    }

    /// Vai para `status().error`. shortcut: lido só na subida, a recusa fica até reiniciar o servidor; reler a cada pedido custaria disco no porteiro.
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// A ligação chega com o servidor de pé, na resposta de `POST /v1/machines`.
    pub fn set_identity(&self, identity: Option<ServiceIdentity>) {
        *self.identity.write().unwrap_or_else(|e| e.into_inner()) = identity;
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
            if let Some(fault) = &self.fault {
                return Err(Refusal::Io(io::Error::other(fault.clone())));
            }
            write(&self.path, &next)?;
            *state = next;
        }
        Ok(value)
    }

    /// `signature` confere os bytes de `ticket` como vieram: o bilhete nunca é serializado de novo.
    pub fn redeem(&self, ticket: &str, signature: &str, secret: &str) -> Result<DeviceId, Refusal> {
        let identity = self.identity.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or(Refusal::NotLinked)?;
        let signature = base64::engine::general_purpose::STANDARD.decode(signature).map_err(|_| Refusal::Ticket)?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, identity.public_key)
            .verify(ticket.as_bytes(), &signature).map_err(|_| Refusal::Ticket)?;
        let ticket: Ticket = serde_json::from_str(ticket).map_err(|_| Refusal::Ticket)?;
        if ticket.machine_id != identity.machine_id || ticket.account != identity.account {
            return Err(Refusal::Ticket);
        }
        self.update(|state, now| {
            if ticket.expires_at <= now || ticket.expires_at > now + MAX_TICKET_AHEAD || state.nonces.contains_key(&ticket.nonce) {
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
                created_at: now, denied_at: None, verifier: Some(verifier), token_hash: None, next_token_hash: None, last_seen: None,
                approved_by: None, approved_at: None });
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

    /// `by`: o aparelho que aprova; `None` é o dono. Quem já foi aprovado não muda: reaprovar trocaria
    /// `approved_by` e tiraria o aparelho da cascata de quem o aprovou.
    pub fn approve(&self, id: &str, by: Option<&str>) -> Result<(), Refusal> {
        self.update(|state, now| {
            // Revogado entre a camada e aqui não aprova: o aprovado nasceria fora da cascata.
            if by.is_some_and(|by| !state.devices.iter().any(|d| d.id == by && d.status == Status::Approved)) {
                return Err(Refusal::Credential);
            }
            let device = state.devices.iter_mut().find(|d| d.id == id && d.status != Status::Denied).ok_or(Refusal::Unknown)?;
            if device.status == Status::Pending {
                device.status = Status::Approved;
                device.approved_by = by.map(str::to_owned);
                device.approved_at = Some(now);
            }
            Ok(())
        })
    }

    /// Recusa quem ainda não recebeu token; quem já recebeu perde a credencial na hora. Vale também
    /// para tudo que este aparelho aprovou, senão um aparelho roubado sairia com outra credencial.
    pub fn remove(&self, id: &str) -> Result<(), Refusal> {
        self.revoke(id, None)
    }

    /// `remove` pedido por um aparelho: só ele mesmo, o que ele aprovou e pedido que ainda espera aprovação,
    /// que ele poderia aprovar. Sem isso, um aparelho roubado derrubaria os outros e o dono junto.
    pub fn remove_by(&self, id: &str, by: &str) -> Result<(), Refusal> {
        self.revoke(id, Some(by))
    }

    fn revoke(&self, id: &str, by: Option<&str>) -> Result<(), Refusal> {
        self.update(|state, now| {
            let Some(target) = state.devices.iter().find(|d| d.id == id) else { return Err(Refusal::Unknown) };
            if by.is_some_and(|by| target.status != Status::Pending && !cascade(state, by).contains(id)) {
                return Err(Refusal::NotYours);
            }
            let revoked = cascade(state, id);
            state.devices.retain_mut(|device| {
                if !revoked.contains(&device.id) { return true; }
                if device.token_hash.is_some() { return false; }
                deny(device, now);
                true
            });
            Ok(())
        })
    }

    /// Aparelho sem aprovação: quem chama já provou a senha do dono.
    pub fn enroll(&self, name: &str) -> Result<(DeviceId, String), Refusal> {
        self.update(|state, now| {
            let (id, token) = (credentials::id(), credentials::token());
            state.devices.push(Device { id: id.clone(), name: display_name(name), status: Status::Approved, created_at: now,
                denied_at: None, verifier: None, token_hash: Some(credentials::hash(token.as_bytes())), next_token_hash: None, last_seen: None,
                approved_by: None, approved_at: Some(now) });
            Ok((id, token))
        })
    }

    /// Credencial que esta máquina entrega a outra da mesma conta. Sem cartão aqui: a ligação foi
    /// pedida pelo dono desta máquina, e o cartão aprovado é o da outra.
    pub fn issue_local(&self, name: &str) -> Result<(DeviceId, String), Refusal> {
        self.enroll(name)
    }

    /// Token novo para o mesmo aparelho, a entregar de novo a quem o guarda: o cru nunca vai ao disco.
    pub fn rotate(&self, id: &str) -> Result<String, Refusal> {
        self.update(|state, _| {
            let device = state.devices.iter_mut().find(|d| d.id == id && d.status == Status::Approved && d.token_hash.is_some())
                .ok_or(Refusal::Unknown)?;
            let token = credentials::token();
            device.next_token_hash = Some(credentials::hash(token.as_bytes()));
            Ok(token)
        })
    }

    /// A entrega de `token` se confirmou: o antigo deixa de valer. Só o token em entrega muda alguma
    /// coisa: dois anúncios juntos não promovem o que a outra ponta não recebeu.
    pub fn settle(&self, id: &str, token: &str) -> Result<(), Refusal> {
        let delivered = credentials::hash(token.as_bytes());
        self.update(|state, _| {
            if let Some(device) = state.devices.iter_mut().find(|d| d.id == id)
                && device.next_token_hash.as_deref().is_some_and(|next| same(next, &delivered)) {
                device.token_hash = device.next_token_hash.take();
            }
            Ok(())
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
            && [&d.token_hash, &d.next_token_hash].into_iter().flatten().any(|hash| same(hash, &presented)))?;
        // Usar o token novo prova a entrega, mesmo com a resposta dela perdida no caminho.
        if device.next_token_hash.as_deref().is_some_and(|next| same(next, &presented)) {
            device.token_hash = device.next_token_hash.take();
        }
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

/// O aparelho e tudo que nasceu da aprovação dele, direta ou por outro.
fn cascade(state: &State, id: &str) -> HashSet<DeviceId> {
    let mut found = HashSet::from([id.to_owned()]);
    loop {
        let before = found.len();
        for device in &state.devices {
            if device.approved_by.as_ref().is_some_and(|by| found.contains(by)) {
                found.insert(device.id.clone());
            }
        }
        if found.len() == before { return found; }
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
    for device in &mut state.devices {
        // Token não buscado no prazo vence: segredo vazado meses depois não recolhe nada.
        let waiting_since = match device.status {
            Status::Pending => Some(device.created_at),
            Status::Approved if device.token_hash.is_none() => device.approved_at,
            _ => None,
        };
        if waiting_since.is_some_and(|since| now >= since + PENDING_TTL) {
            deny(device, now);
        }
    }
    state.devices.retain(|d| d.denied_at.is_none_or(|at| now < at + PENDING_TTL));
}

fn write(path: &Path, state: &State) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    hangar_core::fs::atomic_write(path, &serde_json::to_vec_pretty(state).map_err(io::Error::other)?)
}

/// Falha de leitura não diz nada do conteúdo: o arquivo fica. Só o que abriu e não é JSON de aparelhos vai para o lado.
fn load(path: &Path, now: u64) -> (State, Option<String>) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (State::default(), None),
        Err(error) => {
            tracing::warn!(code = "connect_devices_unreadable", "{} não foi lido: {error}", path.display());
            return (State::default(), Some(format!("os aparelhos do Connect não foram lidos ({}): reinicie o Hangar depois de resolver", error.kind())));
        }
    };
    match serde_json::from_slice::<State>(&bytes) {
        Ok(state) => (state, None),
        Err(error) => {
            // Ilegível vira "nenhum aparelho": perde acessos, e vai pro lado para a próxima gravação não apagá-lo.
            tracing::warn!(code = "connect_devices_unreadable", "{} ilegível, começando vazio: {error}", path.display());
            let bad = path.with_extension(format!("bad-{now}"));
            let fault = std::fs::rename(path, &bad).err().map(|e| {
                tracing::warn!(code = "connect_devices_unreadable", "não consegui guardar {} ilegível: {e}", path.display());
                format!("os aparelhos do Connect estão ilegíveis e o arquivo não saiu do lugar ({}): reinicie o Hangar depois de resolver", e.kind())
            });
            (State::default(), fault)
        }
    }
}
