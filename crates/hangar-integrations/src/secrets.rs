//! Armazenamento cifrado isolado de segredos em `<hangar_dir>/integrations/secrets.bin`.
//!
//! Formato binário: `VERSION (1 byte = 1) || nonce (12 bytes) || AES-256-GCM(json)`, autenticado
//! com o cabeçalho de versão em `Aad`, totalmente isolado do cofre manual (`vault.bin`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use ring::aead;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use hangar_core::fs::{atomic_write, chmod, lock_exclusive, read_replaced};

pub const SECRET_KEY_LEN: usize = 32;
pub const SECRETS_FORMAT_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const MAX_SECRET_ID_BYTES: usize = 128;

pub type SecretKey = Zeroizing<[u8; SECRET_KEY_LEN]>;

/// Erros devolvidos pelo provedor de chave (chaveiro do SO ou mock de teste).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKeyError {
    Locked,
    Absent,
    Lost,
    Bus,
    Unsupported,
}

/// Erros de leitura, decifra ou gravação de `secrets.bin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretStoreError {
    KeyLost,
    Key(SecretKeyError),
    Corrupted,
    Io(String),
    InvalidId,
}

/// Tokens OAuth de uma integração. Não implementa `Debug` para impedir vazamento em logs.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: Zeroizing<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_client_secret: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_fingerprint: Option<[u8; 32]>,
    #[serde(default)]
    pub auth_expired: bool,
}

/// Segredos de uma integração. Não implementa `Debug` para impedir vazamento em logs.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntegrationSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthTokens>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, Zeroizing<String>>,
}

/// Chaveiro em memória para testes isolados.
#[derive(Default)]
pub struct MemoryKeyStore {
    key: Mutex<Option<[u8; SECRET_KEY_LEN]>>,
    writes: Mutex<u32>,
}

/// Repositório cifrado de segredos das integrações MCP em `secrets.bin`.
pub struct SecretStore {
    path: PathBuf,
    key_store: Arc<dyn SecretKeyStore>,
    cached_key: Arc<tokio::sync::Mutex<Option<SecretKey>>>,
}

/// Documento cifrado inteiro em `secrets.bin`. Sem `Debug` para nunca expor segredos.
#[derive(Clone, Default, Serialize, Deserialize)]
struct SecretsDoc {
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    entries: BTreeMap<String, IntegrationSecrets>,
}

struct ByteCounter(usize);

/// Abstração de leitura e gravação da chave de 32 bytes no chaveiro.
pub trait SecretKeyStore: Send + Sync {
    fn read(&self) -> BoxFuture<'_, Result<Option<SecretKey>, SecretKeyError>>;
    fn write(&self, key: SecretKey) -> BoxFuture<'_, Result<(), SecretKeyError>>;
}

impl IntegrationSecrets {
    /// Retorna `true` quando nenhum campo secreto está preenchido.
    pub fn is_empty(&self) -> bool {
        self.bearer_token.is_none()
            && self.client_secret.is_none()
            && self.oauth.is_none()
            && self.headers.is_empty()
            && self.env.is_empty()
    }
}

impl MemoryKeyStore {
    /// Cria um chaveiro em memória inicialmente vazio.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cria um chaveiro em memória já populado com uma chave de 32 bytes.
    pub fn with_key(key: [u8; SECRET_KEY_LEN]) -> Self {
        Self {
            key: Mutex::new(Some(key)),
            writes: Mutex::new(0),
        }
    }

    /// Quantas vezes uma nova chave foi gravada neste chaveiro.
    pub fn writes(&self) -> u32 {
        *self
            .writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Remove a chave armazenada em memória (simula perda de chave do chaveiro).
    pub fn clear(&self) {
        *self
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

impl SecretStore {
    /// Cria o repositório cifrado apontando para `<hangar_dir>/integrations/secrets.bin`.
    pub fn new(hangar_dir: impl AsRef<Path>, key_store: Arc<dyn SecretKeyStore>) -> Self {
        Self {
            path: secrets_path(hangar_dir.as_ref()),
            key_store,
            cached_key: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Retorna o caminho do arquivo `secrets.bin`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lê os segredos de `integration_id`, retornando `None` se o arquivo ou entrada não existir.
    ///
    /// # Errors
    ///
    /// Retorna [`SecretStoreError::InvalidId`] para `integration_id` inválido,
    /// [`SecretStoreError::KeyLost`] se `secrets.bin` existir sem chave no chaveiro,
    /// [`SecretStoreError::Corrupted`] se `secrets.bin` estiver adulterado ou cifrado com outra chave,
    /// [`SecretStoreError::Key`] para falhas do chaveiro, ou [`SecretStoreError::Io`] em falhas de E/S.
    pub async fn get(
        &self,
        integration_id: &str,
    ) -> Result<Option<IntegrationSecrets>, SecretStoreError> {
        validate_secret_id(integration_id)?;
        let mut slot = self.cached_key.lock().await;
        let path = self.path.clone();
        let Some(bytes) = tokio::task::spawn_blocking(move || read_optional_bytes(&path))
            .await
            .map_err(|error| SecretStoreError::Io(format!("tarefa interrompida: {error}")))??
        else {
            return Ok(None);
        };
        let key = self.ensure_key_for_existing(&mut slot).await?;
        let mut doc = unseal_doc(&key, &bytes)?;
        Ok(doc.entries.remove(integration_id))
    }

    /// Grava ou substitui os segredos de `integration_id`, inicializando a chave na primeira escrita.
    ///
    /// # Errors
    ///
    /// Retorna os mesmos erros de [`Self::update_with`].
    pub async fn set(
        &self,
        integration_id: &str,
        secrets: IntegrationSecrets,
    ) -> Result<(), SecretStoreError> {
        self.update_with(integration_id, move |slot| {
            *slot = Some(secrets);
            Ok(())
        })
        .await
    }

    /// Executa leitura, modificação e gravação atômica de `secrets.bin` sob trava em memória e em disco.
    ///
    /// Inicializa automaticamente a chave de 32 bytes quando `secrets.bin` ainda não existe e há
    /// segredo para gravar; caso `secrets.bin` já exista sem chave no chaveiro ou corrompido, falha
    /// explicitamente sem nunca gerar chave nova ou sobrescrever o arquivo.
    ///
    /// # Errors
    ///
    /// Retorna [`SecretStoreError::InvalidId`], [`SecretStoreError::KeyLost`],
    /// [`SecretStoreError::Corrupted`], [`SecretStoreError::Key`] ou [`SecretStoreError::Io`].
    pub async fn update_with<T, F>(
        &self,
        integration_id: &str,
        mutate: F,
    ) -> Result<T, SecretStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Option<IntegrationSecrets>) -> Result<T, SecretStoreError> + Send + 'static,
    {
        validate_secret_id(integration_id)?;
        let mut slot = self.cached_key.clone().lock_owned().await;
        let file_lock = lock_secrets_dir(&self.path).await?;
        let path_for_read = self.path.clone();
        let existing = tokio::task::spawn_blocking(move || read_optional_bytes(&path_for_read))
            .await
            .map_err(|error| SecretStoreError::Io(format!("tarefa interrompida: {error}")))??;

        match existing {
            Some(bytes) => {
                let key = self.ensure_key_for_existing(&mut slot).await?;
                let path = self.path.clone();
                let id = integration_id.to_owned();
                tokio::task::spawn_blocking(move || {
                    let _held_slot = slot;
                    let _held_file = file_lock;
                    let mut doc = unseal_doc(&key, &bytes)?;
                    let mut entry = doc.entries.get(&id).cloned();
                    let output = mutate(&mut entry)?;
                    match entry {
                        Some(updated) => {
                            doc.entries.insert(id, updated);
                        }
                        None => {
                            doc.entries.remove(&id);
                        }
                    }
                    doc.revision += 1;
                    save_doc(&path, &key, &doc)?;
                    Ok(output)
                })
                .await
                .map_err(|error| SecretStoreError::Io(format!("tarefa interrompida: {error}")))?
            }
            None => {
                let mut entry = None;
                let output = mutate(&mut entry)?;
                let Some(created) = entry else {
                    return Ok(output);
                };
                let key = self.ensure_key_for_create(&mut slot).await?;
                let path = self.path.clone();
                let id = integration_id.to_owned();
                tokio::task::spawn_blocking(move || {
                    let _held_slot = slot;
                    let _held_file = file_lock;
                    let mut doc = SecretsDoc {
                        revision: 1,
                        entries: BTreeMap::new(),
                    };
                    doc.entries.insert(id, created);
                    save_doc(&path, &key, &doc)?;
                    Ok(output)
                })
                .await
                .map_err(|error| SecretStoreError::Io(format!("tarefa interrompida: {error}")))?
            }
        }
    }

    /// Remove os segredos de `integration_id` se `secrets.bin` existir.
    ///
    /// # Errors
    ///
    /// Retorna [`SecretStoreError::InvalidId`], [`SecretStoreError::KeyLost`] se `secrets.bin`
    /// existir sem chave, [`SecretStoreError::Corrupted`] se estiver adulterado, ou erros de chaveiro/E/S.
    pub async fn delete(&self, integration_id: &str) -> Result<(), SecretStoreError> {
        validate_secret_id(integration_id)?;
        let mut slot = self.cached_key.clone().lock_owned().await;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| SecretStoreError::Io("secrets.bin sem pasta pai".into()))?;
        // Se a pasta pai ainda não existe, não há secrets.bin nem motivo para criá-la.
        match std::fs::metadata(parent) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(SecretStoreError::Io(format!(
                    "inspecionar pasta de segredos: {:?}",
                    error.kind()
                )));
            }
        }
        chmod(parent, 0o700).map_err(|error| {
            SecretStoreError::Io(format!("permissão da pasta: {:?}", error.kind()))
        })?;
        let file_lock = lock_exclusive(&self.path.with_extension("lock"))
            .await
            .map_err(|error| {
                SecretStoreError::Io(format!("trava de secrets.bin: {:?}", error.kind()))
            })?;
        let path_for_read = self.path.clone();
        let Some(bytes) = tokio::task::spawn_blocking(move || read_optional_bytes(&path_for_read))
            .await
            .map_err(|error| SecretStoreError::Io(format!("tarefa interrompida: {error}")))??
        else {
            return Ok(());
        };
        let key = self.ensure_key_for_existing(&mut slot).await?;
        let path = self.path.clone();
        let id = integration_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _held_slot = slot;
            let _held_file = file_lock;
            let mut doc = unseal_doc(&key, &bytes)?;
            if doc.entries.remove(&id).is_some() {
                doc.revision += 1;
                save_doc(&path, &key, &doc)?;
            }
            Ok(())
        })
        .await
        .map_err(|error| SecretStoreError::Io(format!("tarefa interrompida: {error}")))?
    }

    async fn ensure_key_for_existing(
        &self,
        slot: &mut Option<SecretKey>,
    ) -> Result<SecretKey, SecretStoreError> {
        if let Some(key) = slot.as_ref() {
            return Ok(key.clone());
        }
        match self.key_store.read().await {
            Ok(Some(key)) => {
                *slot = Some(key.clone());
                Ok(key)
            }
            Ok(None) | Err(SecretKeyError::Absent | SecretKeyError::Lost) => {
                tracing::warn!("integrações: secrets.bin existe mas a chave não foi encontrada");
                Err(SecretStoreError::KeyLost)
            }
            Err(error) => Err(SecretStoreError::Key(error)),
        }
    }

    async fn ensure_key_for_create(
        &self,
        slot: &mut Option<SecretKey>,
    ) -> Result<SecretKey, SecretStoreError> {
        if let Some(key) = slot.as_ref() {
            return Ok(key.clone());
        }
        match self.key_store.read().await {
            Ok(Some(key)) => {
                *slot = Some(key.clone());
                return Ok(key);
            }
            Ok(None) | Err(SecretKeyError::Absent | SecretKeyError::Lost) => {}
            Err(error) => return Err(SecretStoreError::Key(error)),
        }
        let mut key = Zeroizing::new([0u8; SECRET_KEY_LEN]);
        SystemRandom::new()
            .fill(&mut key[..])
            .map_err(|_| SecretStoreError::Key(SecretKeyError::Bus))?;
        self.key_store
            .write(key.clone())
            .await
            .map_err(SecretStoreError::Key)?;
        *slot = Some(key.clone());
        Ok(key)
    }
}

impl SecretKeyStore for MemoryKeyStore {
    fn read(&self) -> BoxFuture<'_, Result<Option<SecretKey>, SecretKeyError>> {
        let key = self
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map(Zeroizing::new);
        Box::pin(async move { Ok(key) })
    }

    fn write(&self, key: SecretKey) -> BoxFuture<'_, Result<(), SecretKeyError>> {
        *self
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(*key);
        *self
            .writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        Box::pin(async { Ok(()) })
    }
}

impl io::Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Display for SecretKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => write!(f, "chaveiro trancado"),
            Self::Absent => write!(f, "chave ausente"),
            Self::Lost => write!(f, "chave perdida"),
            Self::Bus => write!(f, "falha de comunicação com o chaveiro"),
            Self::Unsupported => write!(f, "chaveiro não suportado"),
        }
    }
}

impl std::error::Error for SecretKeyError {}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyLost => write!(f, "secrets.bin existe mas a chave não está disponível"),
            Self::Key(error) => write!(f, "erro no chaveiro: {error}"),
            Self::Corrupted => write!(f, "secrets.bin corrompido ou cifrado com outra chave"),
            Self::Io(reason) => write!(f, "erro de E/S em segredos: {reason}"),
            Self::InvalidId => write!(f, "identificador de integração inválido"),
        }
    }
}

impl std::error::Error for SecretStoreError {}

/// Retorna `<hangar_dir>/integrations/secrets.bin`.
pub fn secrets_path(hangar_dir: &Path) -> PathBuf {
    hangar_dir.join("integrations").join("secrets.bin")
}

fn validate_secret_id(id: &str) -> Result<(), SecretStoreError> {
    if id.trim().is_empty() || id.len() > MAX_SECRET_ID_BYTES || id.contains('\0') {
        return Err(SecretStoreError::InvalidId);
    }
    Ok(())
}

async fn lock_secrets_dir(path: &Path) -> Result<File, SecretStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| SecretStoreError::Io("secrets.bin sem pasta pai".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| SecretStoreError::Io(format!("criar pasta: {:?}", error.kind())))?;
    chmod(parent, 0o700)
        .map_err(|error| SecretStoreError::Io(format!("permissão da pasta: {:?}", error.kind())))?;
    lock_exclusive(&path.with_extension("lock"))
        .await
        .map_err(|error| SecretStoreError::Io(format!("trava de secrets.bin: {:?}", error.kind())))
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, SecretStoreError> {
    match read_replaced(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), "integrações: falha ao ler secrets.bin");
            Err(SecretStoreError::Io(format!(
                "leitura de secrets.bin: {:?}",
                error.kind()
            )))
        }
    }
}

fn save_doc(
    path: &Path,
    key: &[u8; SECRET_KEY_LEN],
    doc: &SecretsDoc,
) -> Result<(), SecretStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| SecretStoreError::Io("secrets.bin sem pasta pai".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| SecretStoreError::Io(format!("criar pasta: {:?}", error.kind())))?;
    chmod(parent, 0o700)
        .map_err(|error| SecretStoreError::Io(format!("permissão da pasta: {:?}", error.kind())))?;
    let sealed = seal_doc(key, doc)?;
    atomic_write(path, &sealed).map_err(|error| {
        SecretStoreError::Io(format!("gravação de secrets.bin: {:?}", error.kind()))
    })?;
    chmod(path, 0o600).map_err(|error| {
        SecretStoreError::Io(format!("permissão de secrets.bin: {:?}", error.kind()))
    })?;
    Ok(())
}

fn seal_doc(key: &[u8; SECRET_KEY_LEN], doc: &SecretsDoc) -> Result<Vec<u8>, SecretStoreError> {
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| SecretStoreError::Io("falha ao gerar nonce aleatório".into()))?;
    // Mede o tamanho antes para alocar o buffer exato em Zeroizing sem realocar segredos na heap.
    let mut size = ByteCounter(0);
    serde_json::to_writer(&mut size, doc).map_err(|_| {
        SecretStoreError::Io("falha ao medir tamanho do documento de segredos".into())
    })?;
    let mut body = Zeroizing::new(Vec::with_capacity(size.0 + aead::AES_256_GCM.tag_len()));
    serde_json::to_writer(&mut *body, doc)
        .map_err(|_| SecretStoreError::Io("falha ao serializar documento de segredos".into()))?;
    let head = [SECRETS_FORMAT_VERSION];
    cipher(key)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(&head),
            &mut *body,
        )
        .map_err(|_| SecretStoreError::Io("falha ao cifrar documento de segredos".into()))?;
    let mut out = Vec::with_capacity(head.len() + NONCE_LEN + body.len());
    out.extend_from_slice(&head);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    Ok(out)
}

fn unseal_doc(key: &[u8; SECRET_KEY_LEN], bytes: &[u8]) -> Result<SecretsDoc, SecretStoreError> {
    if bytes.first() != Some(&SECRETS_FORMAT_VERSION)
        || bytes.len() < 1 + NONCE_LEN + aead::AES_256_GCM.tag_len()
    {
        tracing::warn!("integrações: cabeçalho ou tamanho inválido em secrets.bin");
        return Err(SecretStoreError::Corrupted);
    }
    let (head, rest) = bytes.split_at(1);
    let nonce: [u8; NONCE_LEN] = rest[..NONCE_LEN]
        .try_into()
        .map_err(|_| SecretStoreError::Corrupted)?;
    let mut body = Zeroizing::new(rest[NONCE_LEN..].to_vec());
    let plain = cipher(key)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(head),
            &mut body[..],
        )
        .map_err(|_| {
            tracing::warn!("integrações: falha de autenticação ao abrir secrets.bin");
            SecretStoreError::Corrupted
        })?;
    serde_json::from_slice(plain).map_err(|_| SecretStoreError::Corrupted)
}

fn cipher(key: &[u8; SECRET_KEY_LEN]) -> Result<aead::LessSafeKey, SecretStoreError> {
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| SecretStoreError::Io("chave AES-256-GCM inválida".into()))?;
    Ok(aead::LessSafeKey::new(unbound))
}
