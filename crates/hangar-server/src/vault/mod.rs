//! Cofre de senhas do dono, nesta máquina: `vault.bin` cifrado com a chave do chaveiro. O valor
//! da senha nunca entra em log, diário, evento nem mensagem de erro.
pub mod config;
pub mod http;
pub mod key;
pub mod remote;
pub mod store;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_remote;
pub mod wire;

use serde::{Deserialize, Deserializer, Serialize};
use std::{path::Path, sync::Arc};
use zeroize::Zeroizing;

pub use key::{Key, KeyError, KeyStore};
pub use store::{Entry, Policy};

const MAX_NAME: usize = 200;
const MAX_FIELD: usize = 2048;
const MAX_SECRET: usize = 16 * 1024;

/// `Remote` e `Offline` só saem da rota de estado (`http::status`), nunca de `Vault::status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role { Owner, Remote, None }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State { Open, Locked, Absent, KeyLost, Offline }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status { pub role: Role, pub state: State }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultError {
    Key(KeyError),
    /// Esta máquina não tem `vault.bin`.
    Absent,
    Unreadable,
    Io,
    UnknownEntry,
    DuplicateName,
    Invalid,
}

impl From<store::StoreError> for VaultError {
    fn from(error: store::StoreError) -> Self {
        match error {
            store::StoreError::Missing => Self::Absent,
            store::StoreError::Unreadable => Self::Unreadable,
            store::StoreError::Io => Self::Io,
        }
    }
}

/// O que `GET /api/vault/entries` mostra: tudo menos a senha.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub policy: Policy,
    pub updated_at: f64,
}

impl From<&Entry> for Summary {
    fn from(e: &Entry) -> Self {
        Self { id: e.id.clone(), name: e.name.clone(), username: e.username.clone(), url: e.url.clone(),
            notes: e.notes.clone(), policy: e.policy, updated_at: e.updated_at }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewEntry {
    pub name: String,
    #[serde(default)]
    pub username: Option<String>,
    pub password: Zeroizing<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub policy: Policy,
}

/// Chave presente com `null` apaga o campo; chave ausente não toca.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryChange {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub username: Option<Option<String>>,
    #[serde(default)]
    pub password: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "present")]
    pub url: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub notes: Option<Option<String>>,
    #[serde(default)]
    pub policy: Option<Policy>,
}

fn field_ok(value: &Option<String>, max: usize) -> bool { value.as_ref().is_none_or(|v| v.chars().count() <= max) }

fn name_ok(name: &str) -> bool { !name.trim().is_empty() && name.chars().count() <= MAX_NAME }

fn secret_ok(password: &str) -> bool { !password.is_empty() && password.len() <= MAX_SECRET }

fn same_name(a: &str, b: &str) -> bool { a.trim().to_lowercase() == b.trim().to_lowercase() }

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or_default()
}

fn new_id() -> Result<String, VaultError> {
    let mut raw = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw).map_err(|_| VaultError::Io)?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// Um por servidor (`AppState.vault`). A trava guarda a chave aberta e serializa as gravações.
pub struct Vault {
    keys: Arc<dyn KeyStore>,
    /// Aberta uma vez, a chave fica até o servidor sair: trancar o chaveiro depois não fecha o cofre.
    key: tokio::sync::Mutex<Option<Key>>,
}

impl Vault {
    pub fn system() -> Self { Self::new(Arc::new(key::SecretService::default())) }

    pub fn new(keys: Arc<dyn KeyStore>) -> Self { Self { keys, key: tokio::sync::Mutex::new(None) } }

    async fn key_for(&self, slot: &mut Option<Key>, path: &Path) -> Result<Key, VaultError> {
        if let Some(key) = slot.as_ref() { return Ok(key.clone()); }
        if !store::exists(path) { return Err(VaultError::Absent); }
        let key = key::open(&*self.keys, true).await.map_err(VaultError::Key)?;
        *slot = Some(key.clone());
        Ok(key)
    }

    pub async fn status(&self, hangar_dir: &Path) -> Status {
        let path = store::path(hangar_dir);
        if !store::exists(&path) { return Status { role: Role::None, state: State::Absent }; }
        let mut slot = self.key.lock().await;
        let state = match self.key_for(&mut slot, &path).await {
            Ok(_) => State::Open,
            Err(VaultError::Key(KeyError::Lost | KeyError::Absent)) => State::KeyLost,
            Err(VaultError::Key(KeyError::Unsupported)) => return Status { role: Role::None, state: State::Absent },
            Err(VaultError::Absent) => return Status { role: Role::None, state: State::Absent },
            // Sem daemon ou sem resposta: o cofre não abre agora, como trancado.
            Err(_) => State::Locked,
        };
        Status { role: Role::Owner, state }
    }

    /// `PUT /api/vault/config {owner: "self"}`: chave e cofre vazio; com o cofre de pé, nada muda.
    pub async fn create(&self, hangar_dir: &Path) -> Result<(), VaultError> {
        let path = store::path(hangar_dir);
        let mut slot = self.key.lock().await;
        if store::exists(&path) { return Ok(()); }
        let key = key::create(&*self.keys).await.map_err(VaultError::Key)?;
        let sealed = key.clone();
        tokio::task::spawn_blocking(move || store::save(&path, &sealed, &store::Doc::default())).await.map_err(|_| VaultError::Io)??;
        *slot = Some(key);
        Ok(())
    }

    async fn read<T: Send + 'static>(&self, hangar_dir: &Path, f: impl FnOnce(store::Doc) -> Result<T, VaultError> + Send + 'static)
        -> Result<T, VaultError> {
        let path = store::path(hangar_dir);
        let mut slot = self.key.lock().await;
        let key = self.key_for(&mut slot, &path).await?;
        tokio::task::spawn_blocking(move || f(store::load(&path, &key)?)).await.map_err(|_| VaultError::Io)?
    }

    async fn write<T: Send + 'static>(&self, hangar_dir: &Path, f: impl FnOnce(&mut store::Doc) -> Result<T, VaultError> + Send + 'static)
        -> Result<T, VaultError> {
        let path = store::path(hangar_dir);
        let mut slot = self.key.lock().await;
        let key = self.key_for(&mut slot, &path).await?;
        tokio::task::spawn_blocking(move || {
            let mut doc = store::load(&path, &key)?;
            let out = f(&mut doc)?;
            store::save(&path, &key, &doc)?;
            Ok(out)
        }).await.map_err(|_| VaultError::Io)?
    }

    pub async fn list(&self, hangar_dir: &Path) -> Result<Vec<Summary>, VaultError> {
        self.read(hangar_dir, |doc| Ok(doc.entries.iter().map(Summary::from).collect())).await
    }

    pub async fn add(&self, hangar_dir: &Path, new: NewEntry) -> Result<String, VaultError> {
        if !name_ok(&new.name) || !secret_ok(&new.password) || !field_ok(&new.username, MAX_FIELD)
            || !field_ok(&new.url, MAX_FIELD) || !field_ok(&new.notes, MAX_SECRET) {
            return Err(VaultError::Invalid);
        }
        let id = new_id()?;
        let at = now();
        let entry = Entry { id: id.clone(), name: new.name.trim().to_owned(), username: new.username, password: new.password,
            url: new.url, notes: new.notes, policy: new.policy, created_at: at, updated_at: at };
        self.write(hangar_dir, move |doc| {
            if doc.entries.iter().any(|e| same_name(&e.name, &entry.name)) { return Err(VaultError::DuplicateName); }
            doc.entries.push(entry);
            Ok(id)
        }).await
    }

    pub async fn update(&self, hangar_dir: &Path, id: &str, change: EntryChange) -> Result<(), VaultError> {
        let valid = change.name.as_deref().is_none_or(name_ok) && change.password.as_deref().is_none_or(|p| secret_ok(p))
            && change.username.as_ref().is_none_or(|v| field_ok(v, MAX_FIELD))
            && change.url.as_ref().is_none_or(|v| field_ok(v, MAX_FIELD))
            && change.notes.as_ref().is_none_or(|v| field_ok(v, MAX_SECRET));
        if !valid { return Err(VaultError::Invalid); }
        let id = id.to_owned();
        self.write(hangar_dir, move |doc| {
            if let Some(name) = &change.name
                && doc.entries.iter().any(|e| e.id != id && same_name(&e.name, name)) {
                return Err(VaultError::DuplicateName);
            }
            let entry = doc.entries.iter_mut().find(|e| e.id == id).ok_or(VaultError::UnknownEntry)?;
            if let Some(name) = change.name { entry.name = name.trim().to_owned(); }
            if let Some(username) = change.username { entry.username = username; }
            if let Some(password) = change.password { entry.password = password; }
            if let Some(url) = change.url { entry.url = url; }
            if let Some(notes) = change.notes { entry.notes = notes; }
            if let Some(policy) = change.policy { entry.policy = policy; }
            entry.updated_at = now();
            Ok(())
        }).await
    }

    pub async fn remove(&self, hangar_dir: &Path, id: &str) -> Result<(), VaultError> {
        let id = id.to_owned();
        self.write(hangar_dir, move |doc| {
            let before = doc.entries.len();
            doc.entries.retain(|e| e.id != id);
            if doc.entries.len() == before { Err(VaultError::UnknownEntry) } else { Ok(()) }
        }).await
    }

    pub async fn password(&self, hangar_dir: &Path, id: &str) -> Result<Zeroizing<String>, VaultError> {
        let id = id.to_owned();
        self.read(hangar_dir, move |doc| {
            doc.entries.into_iter().find(|e| e.id == id).map(|e| e.password).ok_or(VaultError::UnknownEntry)
        }).await
    }
}
