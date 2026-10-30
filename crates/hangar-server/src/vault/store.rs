//! `~/.hangar/vault/vault.bin` = `versão (1 byte) || nonce (12) || AES-256-GCM(json)`. Arquivo que
//! não abre é erro: tratá-lo como cofre vazio apagaria as senhas na próxima gravação.
use ring::aead;
use serde::{Deserialize, Serialize};
use std::{io, path::{Path, PathBuf}};
use zeroize::Zeroizing;

pub const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy { #[default] Ask, Free }

/// Sem `Debug`: a senha não pode sair num `{:?}`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub password: Zeroizing<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub policy: Policy,
    pub created_at: f64,
    pub updated_at: f64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Doc { pub entries: Vec<Entry> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// O arquivo sumiu.
    Missing,
    /// Versão desconhecida, adulterado ou cifrado com outra chave.
    Unreadable,
    Io,
}

pub fn path(hangar_dir: &Path) -> PathBuf { hangar_dir.join("vault").join("vault.bin") }

/// Na dúvida (sem permissão para olhar), existe: criar por cima apagaria o cofre.
pub fn exists(path: &Path) -> bool { !matches!(path.try_exists(), Ok(false)) }

fn cipher(key: &[u8; 32]) -> Result<aead::LessSafeKey, StoreError> {
    Ok(aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| StoreError::Io)?))
}

/// Só conta os bytes.
struct Count(usize);

impl io::Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { self.0 += bytes.len(); Ok(bytes.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

pub fn seal(key: &[u8; 32], doc: &Doc) -> Result<Vec<u8>, StoreError> {
    let mut nonce = [0u8; NONCE_LEN];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce).map_err(|_| StoreError::Io)?;
    // Tamanho medido antes: o buffer já nasce com o JSON e a etiqueta, e nada realoca deixando
    // cópia da senha sem zerar.
    let mut size = Count(0);
    serde_json::to_writer(&mut size, doc).map_err(|_| StoreError::Io)?;
    let mut body = Zeroizing::new(Vec::with_capacity(size.0 + aead::AES_256_GCM.tag_len()));
    serde_json::to_writer(&mut *body, doc).map_err(|_| StoreError::Io)?;
    cipher(key)?.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::from([VERSION]), &mut *body)
        .map_err(|_| StoreError::Io)?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + body.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn unseal(key: &[u8; 32], bytes: &[u8]) -> Result<Doc, StoreError> {
    if bytes.len() < 1 + NONCE_LEN + aead::AES_256_GCM.tag_len() || bytes[0] != VERSION { return Err(StoreError::Unreadable); }
    let nonce: [u8; NONCE_LEN] = bytes[1..1 + NONCE_LEN].try_into().map_err(|_| StoreError::Unreadable)?;
    let mut body = Zeroizing::new(bytes[1 + NONCE_LEN..].to_vec());
    let plain = cipher(key)?.open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::from([VERSION]), &mut body[..])
        .map_err(|_| StoreError::Unreadable)?;
    serde_json::from_slice(plain).map_err(|_| StoreError::Unreadable)
}

pub fn load(path: &Path, key: &[u8; 32]) -> Result<Doc, StoreError> {
    match std::fs::read(path) {
        Ok(bytes) => unseal(key, &bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(StoreError::Missing),
        Err(_) => Err(StoreError::Io),
    }
}

pub fn save(path: &Path, key: &[u8; 32], doc: &Doc) -> Result<(), StoreError> {
    let dir = path.parent().ok_or(StoreError::Io)?;
    std::fs::create_dir_all(dir).map_err(|_| StoreError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|_| StoreError::Io)?;
    }
    crate::runtime::queue::atomic_write(path, &seal(key, doc)?).map_err(|_| StoreError::Io)
}
