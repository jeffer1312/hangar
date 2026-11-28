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

pub use hangar_connect_client::install_code;

/// Chave que junta os pedidos do mesmo celular no cartão de quem aprova, sem expor o id. Prefixo próprio: não coincide
/// com o hash de onde sai o código.
fn install_group(install_id: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, format!("hangar-install-group\0{install_id}").as_bytes());
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn system_clock() -> Clock {
    Arc::new(|| hangar_core::clock::wall_now() as u64)
}

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
    /// Entregas anteriores ainda sem confirmação: a outra ponta pode ter gravado uma delas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unconfirmed_token_hashes: Vec<String>,
    #[serde(default)]
    last_seen: Option<u64>,
    /// O `last_seen` que está no disco, ou a última tentativa de levá-lo: só memória.
    #[serde(skip)]
    last_seen_saved: Option<u64>,
    /// Aparelho que aprovou este; `None` é o dono. Revogar um leva junto tudo que ele aprovou.
    #[serde(default)]
    approved_by: Option<DeviceId>,
    #[serde(default)]
    approved_at: Option<u64>,
    /// Mesmo valor nos pedidos de um celular a várias máquinas: só agrupa o cartão, não dá acesso.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_id: Option<String>,
    /// Aparelho da conta que o serviço liberou: revogá-lo lá tira este daqui.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_device: Option<String>,
}

/// Entregas sem confirmação que seguem valendo; a mais velha sai, senão um par fora do ar acumularia tokens.
const MAX_UNCONFIRMED: usize = 3;
/// O uso do aparelho vai ao disco no máximo uma vez neste intervalo: um fsync por pedido custaria caro.
pub const LAST_SEEN_SAVE_EVERY: u64 = 60 * 60;

impl Device {
    fn accepts(&self, hash: &str) -> bool {
        self.token_hash.iter().chain(&self.next_token_hash).chain(&self.unconfirmed_token_hashes).any(|known| same(known, hash))
    }

    /// Só o token mais novo em entrega é promovido: com dois anúncios juntos, promover um anterior
    /// tiraria o que a outra ponta gravou por último.
    fn promote_if_next(&mut self, hash: &str) {
        if self.next_token_hash.as_deref().is_some_and(|next| same(next, hash)) {
            self.token_hash = self.next_token_hash.take();
            self.unconfirmed_token_hashes.clear();
        }
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
struct State {
    devices: Vec<Device>,
    /// Nonce gasto e até quando o bilhete dele valeria: depois disso a validade já o recusa.
    nonces: HashMap<String, u64>,
    /// Desde quando o `last_seen` vai ao disco: antes disso, `last_seen` nulo não prova que a credencial nunca foi usada.
    #[serde(default)]
    tracked_since: Option<u64>,
}

#[derive(Deserialize)]
struct Ticket {
    machine_id: String,
    account: String,
    device_name: String,
    verifier: String,
    nonce: String,
    expires_at: u64,
    /// Ausentes no serviço antigo: o aparelho espera o cartão desta máquina.
    #[serde(default)]
    preapproved: bool,
    #[serde(default)]
    account_device: Option<String>,
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
    InstallId,
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
    pub created_at: u64,
    /// O mesmo em toda linha: `last_seen` nulo só diz "sem uso" a partir daqui.
    pub tracked_since: u64,
    /// Só no pedido esperando, e só de celular que mandou o `install_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_group: Option<String>,
    /// O aparelho da conta por onde ele entrou: a tela desta máquina mostra só os que não têm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_device: Option<String>,
}

pub struct Store {
    path: PathBuf,
    identity: RwLock<Option<ServiceIdentity>>,
    clock: Clock,
    state: Mutex<State>,
    /// `devices.json` ficou no lugar sem ser lido: gravar por cima perderia os aparelhos que ele guarda.
    fault: Option<String>,
    /// A conta recusou a leitura a esta máquina: revogação lá não chegaria aqui. `Some` são os aparelhos da conta e o
    /// que eles aprovaram aqui, refeito a cada mudança no registro. Só memória: a próxima leitura boa desfaz.
    /// Sempre travado depois de `state`.
    suspended: Mutex<Option<HashSet<DeviceId>>>,
    /// `tracked_since` saiu do relógio no carregamento, não do arquivo.
    tracked_since_loaded: bool,
}

impl Store {
    pub fn new(dir: PathBuf, identity: Option<ServiceIdentity>) -> Self {
        let clock = system_clock();
        let path = dir.join("devices.json");
        let (mut state, fault) = load(&path, clock());
        // Arquivo de antes do uso ir ao disco: a contagem começa agora. Só vai ao disco na próxima gravação; até lá,
        // cada subida recomeça, o que só adia a limpeza.
        let tracked_since_loaded = state.tracked_since.is_none();
        state.tracked_since.get_or_insert_with(|| clock());
        Self { path, identity: RwLock::new(identity), clock, state: Mutex::new(state), fault, suspended: Mutex::new(None), tracked_since_loaded }
    }

    /// Vai para `status().error`. shortcut: lido só na subida, a recusa fica até reiniciar o servidor; reler a cada pedido custaria disco no porteiro.
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        // O relógio chega depois do carregamento: o começo que saiu do relógio segue o novo.
        if self.tracked_since_loaded {
            self.state.get_mut().unwrap_or_else(|e| e.into_inner()).tracked_since = Some(clock());
        }
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

    fn lock_suspended(&self) -> MutexGuard<'_, Option<HashSet<DeviceId>>> {
        self.suspended.lock().unwrap_or_else(|e| e.into_inner())
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
            mark_last_seen_saved(&mut next);
            *state = next;
            if let Some(suspended) = self.lock_suspended().as_mut() {
                *suspended = account_cascade(&state);
            }
        }
        Ok(value)
    }

    /// `signature` confere os bytes de `ticket` como vieram: o bilhete nunca é serializado de novo.
    pub fn redeem(&self, ticket: &str, signature: &str, secret: &str) -> Result<DeviceId, Refusal> {
        self.redeem_from(ticket, signature, secret, None).map(|(id, _)| id)
    }

    /// `install_id` vem do celular e só agrupa os pedidos dele no cartão de aprovação. Bilhete que a conta liberou
    /// nasce [`Status::Approved`]; os outros, [`Status::Pending`].
    pub fn redeem_from(&self, ticket: &str, signature: &str, secret: &str, install_id: Option<&str>) -> Result<(DeviceId, Status), Refusal> {
        if install_id.is_some_and(|id| !valid_install(id)) {
            return Err(Refusal::InstallId);
        }
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
            let name = display_name(&ticket.device_name);
            let account_device = ticket.account_device.clone().filter(|d| ticket.preapproved && !d.is_empty());
            // Nova tentativa do mesmo celular troca o pedido anterior: um cartão só, e tentar de novo não gasta o limite.
            // Só pedido troca pedido: o `install_id` vem do celular, e um liberado apagaria o cartão de outro.
            if let Some(install) = install_id.filter(|_| account_device.is_none()) {
                state.devices.retain(|d| !(d.status == Status::Pending && d.install_id.as_deref() == Some(install) && d.name == name));
            }
            if account_device.is_none() &&state.devices.iter().filter(|d| d.status == Status::Pending).count() >= MAX_PENDING {
                return Err(Refusal::TooManyPending);
            }
            // Só aqui: quem tem o bilhete sem o segredo, ou bateu no limite, não queima o nonce.
            state.nonces.insert(ticket.nonce.clone(), ticket.expires_at);
            let id = credentials::id();
            // Liberado na conta nasce aprovado sem `approved_by`: a cascata local não alcança o que a conta decidiu.
            let status = if account_device.is_some() { Status::Approved } else { Status::Pending };
            state.devices.push(Device { id: id.clone(), name, status,
                created_at: now, denied_at: None, verifier: Some(verifier), token_hash: None, next_token_hash: None, unconfirmed_token_hashes: Vec::new(), last_seen: None,
                last_seen_saved: None, approved_by: None, approved_at: account_device.is_some().then_some(now), install_id: install_id.map(str::to_owned), account_device });
            Ok((id, status))
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
            withdraw(state, now, |device| revoked.contains(&device.id));
            Ok(())
        })
    }

    /// Aparelhos da conta revogados no serviço saem daqui também, com o que eles aprovaram nesta máquina.
    pub fn revoke_account_devices(&self, account_devices: &[String]) -> Result<(), Refusal> {
        self.update(|state, now| {
            let revoked: HashSet<DeviceId> = state.devices.iter()
                .filter(|d| d.account_device.as_ref().is_some_and(|a| account_devices.contains(a)))
                .flat_map(|d| cascade(state, &d.id))
                .collect();
            withdraw(state, now, |device| revoked.contains(&device.id));
            Ok(())
        })
    }

    /// Liga ou desliga a recusa no porteiro dos aparelhos ligados à conta, sem apagar nenhum; devolve o valor anterior.
    pub fn suspend_account_devices(&self, suspended: bool) -> bool {
        if !suspended {
            return self.lock_suspended().take().is_some();
        }
        let state = self.lock();
        // A revogação na conta leva junto o que o aparelho aprovou aqui: a suspensão também.
        self.lock_suspended().replace(account_cascade(&state)).is_some()
    }

    /// Aparelho aprovado aqui passa a ser `account_device` na conta; a ligação nunca troca de dono.
    pub fn bind_account_device(&self, id: &str, account_device: &str) -> Result<(), Refusal> {
        if account_device.is_empty() {
            return Err(Refusal::Unknown);
        }
        self.update(|state, _| {
            let device = state.devices.iter_mut().find(|d| d.id == id && d.status == Status::Approved).ok_or(Refusal::Unknown)?;
            match &device.account_device {
                Some(bound) if bound != account_device => Err(Refusal::NotYours),
                _ => { device.account_device = Some(account_device.to_owned()); Ok(()) }
            }
        })
    }

    /// Aparelho sem aprovação: quem chama já provou a senha do dono.
    pub fn enroll(&self, name: &str) -> Result<(DeviceId, String), Refusal> {
        self.enroll_install(name, None)
    }

    /// Como [`Store::enroll`]. A mesma instalação pedindo de novo recebe token novo na entrada que já tinha:
    /// o app que perdeu a credencial não soma outra linha na lista.
    pub fn enroll_install(&self, name: &str, install_id: Option<&str>) -> Result<(DeviceId, String), Refusal> {
        if install_id.is_some_and(|id| !valid_install(id)) {
            return Err(Refusal::InstallId);
        }
        self.update(|state, now| {
            let token = credentials::token();
            let hash = Some(credentials::hash(token.as_bytes()));
            // Só entrada nascida com a senha: a resgatada por bilhete tem outra prova e outro dono da decisão.
            let mine = install_id.and_then(|install| state.devices.iter_mut().find(|d| d.status == Status::Approved
                && d.verifier.is_none() && d.account_device.is_none() && d.install_id.as_deref() == Some(install)));
            if let Some(device) = mine {
                (device.name, device.token_hash, device.next_token_hash) = (display_name(name), hash, None);
                device.unconfirmed_token_hashes.clear();
                // O uso e a idade eram do token perdido: a entrada nova não pode parecer usada nem velha.
                (device.last_seen, device.last_seen_saved, device.created_at) = (None, None, now);
                return Ok((device.id.clone(), token));
            }
            let id = credentials::id();
            state.devices.push(Device { id: id.clone(), name: display_name(name), status: Status::Approved, created_at: now,
                denied_at: None, verifier: None, token_hash: hash, next_token_hash: None, unconfirmed_token_hashes: Vec::new(), last_seen: None,
                last_seen_saved: None, approved_by: None, approved_at: Some(now), install_id: install_id.map(str::to_owned), account_device: None });
            Ok((id, token))
        })
    }

    /// Token novo para o mesmo aparelho, a entregar de novo a quem o guarda: o cru nunca vai ao disco.
    pub fn rotate(&self, id: &str) -> Result<String, Refusal> {
        self.update(|state, _| {
            let device = state.devices.iter_mut().find(|d| d.id == id && d.status == Status::Approved && d.token_hash.is_some())
                .ok_or(Refusal::Unknown)?;
            let token = credentials::token();
            if let Some(pending) = device.next_token_hash.replace(credentials::hash(token.as_bytes())) {
                device.unconfirmed_token_hashes.push(pending);
                let excess = device.unconfirmed_token_hashes.len().saturating_sub(MAX_UNCONFIRMED);
                device.unconfirmed_token_hashes.drain(..excess);
            }
            Ok(token)
        })
    }

    /// A entrega de `token` se confirmou: o antigo deixa de valer. Só o token em entrega muda alguma
    /// coisa: dois anúncios juntos não promovem o que a outra ponta não recebeu.
    pub fn settle(&self, id: &str, token: &str) -> Result<(), Refusal> {
        let delivered = credentials::hash(token.as_bytes());
        self.update(|state, _| {
            if let Some(device) = state.devices.iter_mut().find(|d| d.id == id) {
                device.promote_if_next(&delivered);
            }
            Ok(())
        })
    }

    pub fn list(&self) -> Vec<Row> {
        let now = (self.clock)();
        let mut state = self.lock();
        prune(&mut state, now);
        let tracked_since = state.tracked_since.unwrap_or(now);
        state.devices.iter().map(|d| {
            let install = d.install_id.as_deref().filter(|_| d.status == Status::Pending);
            Row { device_id: d.id.clone(), name: d.name.clone(), status: d.status, last_seen: d.last_seen, created_at: d.created_at, tracked_since,
                install_code: install.map(install_code), install_group: install.map(install_group),
                account_device: d.account_device.clone() }
        }).collect()
    }

    /// `last_seen` vai ao disco no primeiro uso e depois no máximo uma vez por [`LAST_SEEN_SAVE_EVERY`]; falha
    /// de gravação só vai ao log, nunca recusa quem apresentou o token.
    pub fn verify(&self, token: &[u8]) -> Option<DeviceId> {
        let presented = credentials::hash(token);
        let now = (self.clock)();
        let mut state = self.lock();
        let suspended = self.lock_suspended();
        let index = state.devices.iter()
            .position(|d| d.status == Status::Approved && !suspended.as_ref().is_some_and(|s| s.contains(&d.id)) && d.accepts(&presented))?;
        drop(suspended);
        // Usar o token novo prova a entrega, mesmo com a resposta dela perdida no caminho. Vai ao disco: depois
        // de reiniciar, o antigo voltaria a valer e a prova sairia com ele.
        let promoted = state.devices[index].next_token_hash.as_deref().is_some_and(|next| same(next, &presented));
        let device = &mut state.devices[index];
        if promoted {
            device.promote_if_next(&presented);
        }
        // Sem isto a tela diria "Ainda não apareceu" depois de reiniciar, para credencial em uso.
        let stale = device.last_seen_saved.is_none_or(|saved| now >= saved.saturating_add(LAST_SEEN_SAVE_EVERY));
        device.last_seen = Some(now);
        let id = device.id.clone();
        if promoted || stale {
            let written = match &self.fault { Some(fault) => Err(io::Error::other(fault.clone())), None => write(&self.path, &state) };
            match written {
                Ok(()) => mark_last_seen_saved(&mut state),
                Err(error) if promoted => tracing::warn!(code = "connect_device_promote_failed", device = %id, "token novo aceito sem ir ao disco: {error}"),
                Err(error) => {
                    // Conta como tentativa: disco com defeito não vira um aviso por pedido.
                    state.devices[index].last_seen_saved = Some(now);
                    tracing::warn!(code = "connect_device_last_seen_failed", device = %id, "último uso do aparelho não foi ao disco: {error}");
                }
            }
        }
        Some(id)
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.proof_key(id).is_some()
    }

    /// O hash guardado é a chave da prova: a máquina mostra que conhece o token sem guardá-lo.
    pub fn proof_key(&self, id: &str) -> Option<String> {
        self.lock().devices.iter().find(|d| d.id == id && d.status == Status::Approved).and_then(|d| d.token_hash.clone())
    }
}

/// O estado inteiro acabou de ir ao disco: o `last_seen` de cada aparelho está lá.
fn mark_last_seen_saved(state: &mut State) {
    for device in &mut state.devices {
        device.last_seen_saved = device.last_seen;
    }
}

/// Os aparelhos ligados à conta e o que eles aprovaram aqui.
fn account_cascade(state: &State) -> HashSet<DeviceId> {
    state.devices.iter().filter(|d| d.account_device.is_some()).flat_map(|d| cascade(state, &d.id)).collect()
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

/// Quem já recebeu token perde o aparelho; quem ainda espera lê `denied` em vez de um erro.
fn withdraw(state: &mut State, now: u64, revoked: impl Fn(&Device) -> bool) {
    state.devices.retain_mut(|device| {
        if !revoked(device) { return true; }
        if device.token_hash.is_some() { return false; }
        deny(device, now);
        true
    });
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

fn valid_install(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
        Ok(mut state) => {
            mark_last_seen_saved(&mut state);
            (state, None)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;
    use serde_json::{Value, json};

    const NOW: u64 = 1_000_000;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn dir() -> Dir {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Dir(std::env::temp_dir().join(format!("hangar-connect-devices-{}-{n}", std::process::id())))
    }

    fn key() -> ring::signature::Ed25519KeyPair {
        ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap()
    }

    fn store(dir: &Dir) -> Store {
        let identity = ServiceIdentity { public_key: key().public_key().as_ref().try_into().unwrap(), machine_id: "pc1".into(), account: "acme".into() };
        Store::new(dir.0.clone(), Some(identity)).with_clock(Arc::new(|| NOW))
    }

    /// Bilhete do serviço; `extra` traz os campos da confiança na conta, ausentes no serviço antigo.
    fn ticket(nonce: &str, secret: &str, extra: Value) -> (String, String) {
        let mut body = json!({"machine_id": "pc1", "account": "acme", "device_name": "celular",
            "verifier": credentials::hash(secret.as_bytes()), "nonce": nonce, "expires_at": NOW + 60});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let text = body.to_string();
        let signature = base64::engine::general_purpose::STANDARD.encode(key().sign(text.as_bytes()));
        (text, signature)
    }

    fn redeem(store: &Store, nonce: &str, secret: &str, extra: Value) -> Result<DeviceId, Refusal> {
        let (text, signature) = ticket(nonce, secret, extra);
        store.redeem_from(&text, &signature, secret, None).map(|(id, _)| id)
    }

    #[test]
    fn a_ticket_the_account_released_is_approved_on_the_first_poll() {
        let dir = dir();
        let store = store(&dir);
        let id = redeem(&store, "n1", "s1", json!({"preapproved": true, "account_device": "D"})).unwrap();
        let Poll::Approved(Some(token)) = store.status(&id, "s1").unwrap() else { panic!("não saiu aprovado com token") };
        assert_eq!(store.status(&id, "s1").unwrap(), Poll::Approved(None), "o token sai uma vez");
        assert_eq!(store.verify(token.as_bytes()), Some(id));
        assert!(matches!(redeem(&store, "n1", "s1", json!({"preapproved": true, "account_device": "D"})), Err(Refusal::Ticket)), "nonce gasto");
    }

    #[test]
    fn pending_cards_do_not_block_a_device_the_account_released() {
        let dir = dir();
        let store = store(&dir);
        for n in 0..MAX_PENDING {
            redeem(&store, &format!("p{n}"), "s", json!({})).unwrap();
        }
        assert!(matches!(redeem(&store, "p-extra", "s", json!({})), Err(Refusal::TooManyPending)));
        let id = redeem(&store, "n1", "s1", json!({"preapproved": true, "account_device": "D"})).unwrap();
        assert!(matches!(store.status(&id, "s1").unwrap(), Poll::Approved(Some(_))));
    }

    #[test]
    fn a_ticket_without_the_account_fields_waits_for_the_card() {
        let dir = dir();
        let store = store(&dir);
        let old = redeem(&store, "n1", "s1", json!({})).unwrap();
        assert_eq!(store.status(&old, "s1").unwrap(), Poll::Pending);
        let no_device = redeem(&store, "n2", "s2", json!({"preapproved": true})).unwrap();
        assert_eq!(store.status(&no_device, "s2").unwrap(), Poll::Pending, "pré-aprovado sem aparelho da conta não aprova");
        let not_released = redeem(&store, "n3", "s3", json!({"preapproved": false, "account_device": "D"})).unwrap();
        assert_eq!(store.status(&not_released, "s3").unwrap(), Poll::Pending);
    }

    #[test]
    fn revoking_an_account_device_takes_its_local_devices_off() {
        let dir = dir();
        let store = store(&dir);
        let mut tokens = Vec::new();
        for (nonce, secret, account_device) in [("n1", "s1", "D"), ("n2", "s2", "D"), ("n3", "s3", "E")] {
            let id = redeem(&store, nonce, secret, json!({"preapproved": true, "account_device": account_device})).unwrap();
            let Poll::Approved(Some(token)) = store.status(&id, secret).unwrap() else { panic!("{nonce} sem token") };
            tokens.push((id, token));
        }
        store.revoke_account_devices(&["D".to_owned()]).unwrap();
        assert_eq!(store.verify(tokens[0].1.as_bytes()), None);
        assert_eq!(store.verify(tokens[1].1.as_bytes()), None);
        assert_eq!(store.verify(tokens[2].1.as_bytes()), Some(tokens[2].0.clone()), "outro aparelho da conta fica");
        let reloaded = super::tests::store(&dir);
        assert_eq!(reloaded.list().iter().map(|r| r.device_id.clone()).collect::<Vec<_>>(), vec![tokens[2].0.clone()], "a revogação foi ao disco");
    }

    #[test]
    fn the_list_says_which_devices_came_through_the_account() {
        let dir = dir();
        let store = store(&dir);
        let released = redeem(&store, "n1", "s1", json!({"preapproved": true, "account_device": "D"})).unwrap();
        let local = redeem(&store, "n2", "s2", json!({})).unwrap();
        let rows: Vec<Value> = store.list().iter().map(|row| serde_json::to_value(row).unwrap()).collect();
        let row = |id: &DeviceId| rows.iter().find(|row| row["device_id"] == json!(id)).unwrap().clone();
        assert_eq!(row(&released)["account_device"], "D");
        assert!(row(&local).get("account_device").is_none(), "sem vínculo, o campo não vai: {}", row(&local));
    }

    #[test]
    fn revoking_an_account_device_takes_what_it_approved_here() {
        let dir = dir();
        let store = store(&dir);
        let released = redeem(&store, "n1", "s1", json!({"preapproved": true, "account_device": "D"})).unwrap();
        let child = redeem(&store, "n2", "s2", json!({})).unwrap();
        store.approve(&child, Some(&released)).unwrap();
        let Poll::Approved(Some(token)) = store.status(&child, "s2").unwrap() else { panic!("filho sem token") };
        store.revoke_account_devices(&["D".to_owned()]).unwrap();
        assert_eq!(store.verify(token.as_bytes()), None, "o que D aprovou nesta máquina sai junto");
    }

    #[test]
    fn a_released_ticket_does_not_replace_a_pending_request() {
        let dir = dir();
        let store = store(&dir);
        let (text, signature) = ticket("n1", "s1", json!({}));
        let (pending, status) = store.redeem_from(&text, &signature, "s1", Some("install-1")).unwrap();
        assert_eq!(status, Status::Pending);
        let (text, signature) = ticket("n2", "s2", json!({"preapproved": true, "account_device": "D"}));
        assert_eq!(store.redeem_from(&text, &signature, "s2", Some("install-1")).unwrap().1, Status::Approved, "liberado na conta já nasce aprovado");
        assert_eq!(store.status(&pending, "s1").unwrap(), Poll::Pending, "o pedido pendente continua");
    }

    #[test]
    fn the_suspension_covers_account_devices_that_arrive_while_it_is_on() {
        let dir = dir();
        let store = store(&dir);
        let local = redeem(&store, "n0", "s0", json!({})).unwrap();
        store.approve(&local, None).unwrap();
        let Poll::Approved(Some(local_token)) = store.status(&local, "s0").unwrap() else { panic!("local sem token") };
        assert!(!store.suspend_account_devices(true));
        let released = redeem(&store, "n1", "s1", json!({"preapproved": true, "account_device": "D"})).unwrap();
        let Poll::Approved(Some(token)) = store.status(&released, "s1").unwrap() else { panic!("D sem token") };
        assert_eq!(store.verify(token.as_bytes()), None, "aparelho da conta resgatado com a suspensão ligada fica de fora");
        let child = redeem(&store, "n2", "s2", json!({})).unwrap();
        store.approve(&child, Some(&released)).unwrap();
        let Poll::Approved(Some(child_token)) = store.status(&child, "s2").unwrap() else { panic!("filho sem token") };
        assert_eq!(store.verify(child_token.as_bytes()), None, "o que ele aprovou aqui também");
        assert_eq!(store.verify(local_token.as_bytes()), Some(local), "aprovado nesta máquina não depende da conta");
        assert!(store.suspend_account_devices(false));
        assert_eq!(store.verify(token.as_bytes()), Some(released));
    }

    #[test]
    fn the_same_install_enrolling_again_keeps_one_entry() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let dir = dir();
        let now = Arc::new(AtomicU64::new(NOW));
        let ticking = now.clone();
        let store = store(&dir).with_clock(Arc::new(move || ticking.load(Ordering::Relaxed)));
        let (first, lost) = store.enroll_install("Pixel", Some("install-1")).unwrap();
        assert_eq!(store.verify(lost.as_bytes()), Some(first.clone()));
        now.store(NOW + 500, Ordering::Relaxed);
        let (again, token) = store.enroll_install("Pixel 8", Some("install-1")).unwrap();
        assert_eq!(again, first, "a mesma instalação fica na mesma entrada");
        let row = |store: &Store| store.list().into_iter().find(|row| row.device_id == first).unwrap();
        assert_eq!((row(&store).last_seen, row(&store).created_at), (None, NOW + 500), "não herda o uso nem a idade do token perdido");
        assert_eq!(super::tests::store(&dir).list()[0].last_seen, None, "o uso zerado foi ao disco");
        assert_eq!(store.verify(lost.as_bytes()), None, "o token perdido deixa de valer");
        assert_eq!(store.verify(token.as_bytes()), Some(first.clone()));
        assert_eq!(row(&store).last_seen, Some(NOW + 500));
        let (other, _) = store.enroll_install("Outro", Some("install-2")).unwrap();
        let (old_app, _) = store.enroll("App antigo").unwrap();
        assert!(other != first && old_app != first);
        let rows = store.list();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().find(|row| row.device_id == first).unwrap().name, "Pixel 8");
        assert!(matches!(store.enroll_install("x", Some("com espaço")), Err(Refusal::InstallId)));
    }

    #[test]
    fn enrolling_never_takes_over_a_device_that_came_with_a_ticket() {
        let dir = dir();
        let store = store(&dir);
        let (text, signature) = ticket("n1", "s1", json!({}));
        let (redeemed, _) = store.redeem_from(&text, &signature, "s1", Some("install-1")).unwrap();
        store.approve(&redeemed, None).unwrap();
        let (enrolled, _) = store.enroll_install("celular", Some("install-1")).unwrap();
        assert_ne!(enrolled, redeemed);
        assert!(matches!(store.status(&redeemed, "s1").unwrap(), Poll::Approved(Some(_))), "o resgatado segue com a prova dele");
    }

    #[test]
    fn last_seen_goes_to_disk_at_most_once_an_hour() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let dir = dir();
        let now = Arc::new(AtomicU64::new(NOW));
        let ticking = now.clone();
        let store = store(&dir).with_clock(Arc::new(move || ticking.load(Ordering::Relaxed)));
        let (id, token) = store.enroll("App").unwrap();
        let seen_in = |store: &Store| store.list().into_iter().find(|row| row.device_id == id).unwrap().last_seen;
        let on_disk = || seen_in(&super::tests::store(&dir));
        assert_eq!(on_disk(), None);

        assert_eq!(store.verify(token.as_bytes()), Some(id.clone()));
        assert_eq!(on_disk(), Some(NOW), "o primeiro uso vai ao disco");

        now.store(NOW + 60, Ordering::Relaxed);
        assert_eq!(store.verify(token.as_bytes()), Some(id.clone()));
        assert_eq!(seen_in(&store), Some(NOW + 60), "na memória anda a cada pedido");
        assert_eq!(on_disk(), Some(NOW), "dentro da hora não grava de novo");

        let later = NOW + LAST_SEEN_SAVE_EVERY;
        now.store(later, Ordering::Relaxed);
        assert_eq!(store.verify(token.as_bytes()), Some(id.clone()));
        assert_eq!(on_disk(), Some(later), "passada a hora, grava de novo");
    }

    #[test]
    fn a_file_without_tracked_since_starts_counting_at_the_load() {
        let dir = dir();
        store(&dir).enroll("App").unwrap();
        let file = dir.0.join("devices.json");
        let mut saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["tracked_since"], json!(NOW), "a gravação leva o começo da contagem");
        saved.as_object_mut().unwrap().remove("tracked_since");
        std::fs::write(&file, serde_json::to_vec(&saved).unwrap()).unwrap();

        let old = store(&dir).with_clock(Arc::new(|| NOW + 500));
        assert!(old.list().iter().all(|row| row.tracked_since == NOW + 500), "arquivo antigo conta do carregamento");
        old.enroll("Outro").unwrap();

        let reloaded = store(&dir).with_clock(Arc::new(|| NOW + 900));
        assert!(reloaded.list().iter().all(|row| row.tracked_since == NOW + 500), "com o campo no arquivo, ele fica");
    }

    #[test]
    fn a_restarted_store_does_not_write_last_seen_again_within_the_hour() {
        let dir = dir();
        let (id, token) = store(&dir).enroll("App").unwrap();
        assert_eq!(store(&dir).verify(token.as_bytes()), Some(id.clone()));
        let file = dir.0.join("devices.json");
        let saved = std::fs::read(&file).unwrap();
        let restarted = store(&dir).with_clock(Arc::new(|| NOW + 60));
        assert_eq!(restarted.verify(token.as_bytes()), Some(id));
        assert_eq!(std::fs::read(&file).unwrap(), saved, "o `last_seen` lido do disco conta como gravado");
    }

    #[test]
    fn a_device_bound_to_the_account_falls_with_it() {
        let dir = dir();
        let store = store(&dir);
        let id = redeem(&store, "n1", "s1", json!({})).unwrap();
        assert!(matches!(store.bind_account_device(&id, "D"), Err(Refusal::Unknown)), "pendente não se liga");
        store.approve(&id, None).unwrap();
        let Poll::Approved(Some(token)) = store.status(&id, "s1").unwrap() else { panic!("sem token") };
        assert!(matches!(store.bind_account_device("outro", "D"), Err(Refusal::Unknown)));
        assert!(matches!(store.bind_account_device(&id, ""), Err(Refusal::Unknown)));
        store.bind_account_device(&id, "D").unwrap();
        store.bind_account_device(&id, "D").unwrap();
        assert!(matches!(store.bind_account_device(&id, "E"), Err(Refusal::NotYours)));
        let reloaded = super::tests::store(&dir);
        reloaded.revoke_account_devices(&["D".to_owned()]).unwrap();
        assert_eq!(reloaded.verify(token.as_bytes()), None, "a ligação foi ao disco e cai com a conta");
    }
}
