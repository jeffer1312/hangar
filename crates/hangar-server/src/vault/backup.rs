//! Cópia diária do cofre noutra máquina (`backup_peer` do `vault.json`, a VPS) e a restauração.
//! A cópia abre só com a senha de recuperação, que nunca é gravada: o chaveiro guarda o sal e a
//! chave derivada dela (PBKDF2-HMAC-SHA256), num item próprio (`kind: vault-backup-key`).
//!
//! Formato da cópia: `versão (1) || sal (16) || nonce (12) || AES-256-GCM(json do cofre)`, com versão e
//! sal autenticados. Quem guarda não decifra: o `PUT` diz quantas senhas a cópia tem (`?entries=N`), a
//! cópia vazia vira `<data>.empty.bin`, e a mais nova não vazia nunca é apagada, para um cofre
//! esvaziado por engano não levar junto a última cópia boa.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io, num::NonZeroU32, path::{Path, PathBuf}, sync::Arc, time::Duration};
use zeroize::Zeroizing;

use super::{Key, KeyError, Vault, VaultError, config, key::{self, KEY_LEN}, store::{self, Doc}};
use crate::{groups::peers::{PeerClient, PeerError}, routes::AppState};

pub const FORMAT: u8 = 1;
pub const SALT_LEN: usize = 16;
/// O item do chaveiro: sal || chave derivada.
pub const RECOVERY_LEN: usize = SALT_LEN + KEY_LEN;
const ITERATIONS: NonZeroU32 = match NonZeroU32::new(600_000) { Some(n) => n, None => panic!("voltas do PBKDF2") };
pub const MAX_COPY: usize = 4 * 1024 * 1024;
pub const KEEP: usize = 7;
const MAX_PASSWORD: usize = 1024;
const FIRST: Duration = Duration::from_secs(5 * 60);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Cópia que falhou (máquina da cópia fora, chaveiro trancado) tenta de novo antes do dia seguinte.
const RETRY: Duration = Duration::from_secs(60 * 60);
const EMPTY_SUFFIX: &str = ".empty.bin";
const COPY_SUFFIX: &str = ".bin";

/// Sem `Debug`: a chave não pode sair num `{:?}`.
pub struct Recovery { pub salt: [u8; SALT_LEN], pub key: Key }

impl Recovery {
    pub fn to_item(&self) -> Zeroizing<Vec<u8>> {
        let mut item = Zeroizing::new(Vec::with_capacity(RECOVERY_LEN));
        item.extend_from_slice(&self.salt);
        item.extend_from_slice(&self.key[..]);
        item
    }

    pub fn from_item(item: &[u8]) -> Option<Self> {
        if item.len() != RECOVERY_LEN { return None; }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&item[..SALT_LEN]);
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        key.copy_from_slice(&item[SALT_LEN..]);
        Some(Self { salt, key })
    }

    /// Sal novo e a chave derivada da senha. PBKDF2 de 600 000 voltas: chamar fora do executor.
    pub fn new(password: &str) -> Result<Self, VaultError> {
        let mut salt = [0u8; SALT_LEN];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut salt).map_err(|_| VaultError::Io)?;
        Ok(Self { key: derive(password, &salt), salt })
    }
}

pub fn password_ok(password: &str) -> bool { !password.is_empty() && password.len() <= MAX_PASSWORD }

pub fn derive(password: &str, salt: &[u8; SALT_LEN]) -> Key {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, ITERATIONS, salt, password.as_bytes(), &mut key[..]);
    key
}

fn head(salt: &[u8; SALT_LEN]) -> [u8; 1 + SALT_LEN] {
    let mut head = [FORMAT; 1 + SALT_LEN];
    head[1..].copy_from_slice(salt);
    head
}

pub fn seal_copy(recovery: &Recovery, doc: &Doc) -> Result<Vec<u8>, store::StoreError> {
    store::seal_with(&recovery.key, &head(&recovery.salt), doc)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyError { Unreadable, WrongPassword }

/// Senha errada e cópia adulterada são o mesmo erro: o GCM não separa os dois.
pub fn open_copy(password: &str, bytes: &[u8]) -> Result<(Doc, Recovery), CopyError> {
    if !looks_like_copy(bytes) { return Err(CopyError::Unreadable); }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[1..1 + SALT_LEN]);
    let key = derive(password, &salt);
    let doc = store::unseal_with(&key, 1 + SALT_LEN, bytes).map_err(|_| CopyError::WrongPassword)?;
    Ok((doc, Recovery { salt, key }))
}

/// O que quem guarda confere sem a chave: versão e tamanho mínimo.
pub fn looks_like_copy(bytes: &[u8]) -> bool { bytes.len() >= 1 + SALT_LEN + 12 + 16 && bytes[0] == FORMAT }

// --- Quem guarda as cópias -------------------------------------------------------------------

fn copies_root(hangar_dir: &Path) -> PathBuf { hangar_dir.join("vault-backups") }

fn copies_dir(hangar_dir: &Path, server_id: &str) -> PathBuf { copies_root(hangar_dir).join(server_id) }

/// As cópias da pasta, da mais velha para a mais nova (o nome começa pela data UTC).
fn copy_names(dir: &Path) -> io::Result<Vec<String>> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut names = Vec::new();
    for item in read {
        let name = item?.file_name().to_string_lossy().into_owned();
        // `.queue-*.tmp` é a gravação atômica em curso.
        if !name.starts_with('.') && name.ends_with(COPY_SUFFIX) { names.push(name); }
    }
    names.sort();
    Ok(names)
}

/// O que sai, das cópias em ordem: tudo além das `KEEP` mais novas, menos a mais nova não vazia.
pub fn prune(names: &[String]) -> Vec<String> {
    let cut = names.len().saturating_sub(KEEP);
    let newest_full = names.iter().rposition(|n| !n.ends_with(EMPTY_SUFFIX));
    names[..cut].iter().enumerate().filter(|(i, _)| Some(*i) != newest_full).map(|(_, n)| n.clone()).collect()
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private_dir(_dir: &Path) -> io::Result<()> { Ok(()) }

/// `PUT /api/vault/backup/{server_id}`: grava a cópia (0600, pasta 0700) e poda as velhas.
pub fn keep_copy(hangar_dir: &Path, server_id: &str, bytes: &[u8], entries: u64) -> io::Result<String> {
    let dir = copies_dir(hangar_dir, server_id);
    std::fs::create_dir_all(&dir)?;
    private_dir(&copies_root(hangar_dir))?;
    private_dir(&dir)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
    let name = format!("{stamp}{}", if entries == 0 { EMPTY_SUFFIX } else { COPY_SUFFIX });
    crate::runtime::queue::atomic_write(&dir.join(&name), bytes)?;
    for old in prune(&copy_names(&dir)?) {
        if let Err(error) = std::fs::remove_file(dir.join(&old)) && error.kind() != io::ErrorKind::NotFound {
            tracing::warn!(code = "vault_backup_prune", server_id, "uma cópia velha do cofre não saiu: {error}");
        }
    }
    Ok(name)
}

/// `GET /api/vault/backup/{server_id}`: a cópia mais nova, vazia ou não.
pub fn newest_copy(hangar_dir: &Path, server_id: &str) -> io::Result<Option<Vec<u8>>> {
    let dir = copies_dir(hangar_dir, server_id);
    match copy_names(&dir)?.last() {
        Some(name) => std::fs::read(dir.join(name)).map(Some),
        None => Ok(None),
    }
}

// --- Quem envia (a dona) ---------------------------------------------------------------------

/// `~/.hangar/vault/backup-state.json`: a última revisão aceita pelo `peer` e o resultado da última tentativa.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SendState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

fn state_path(hangar_dir: &Path) -> PathBuf { hangar_dir.join("vault").join("backup-state.json") }

/// Arquivo torto conta como nenhuma cópia aceita: o pior caso é mandar de novo.
pub fn load_state(hangar_dir: &Path) -> SendState {
    std::fs::read(state_path(hangar_dir)).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()
}

fn save_state(hangar_dir: &Path, state: &SendState) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    crate::runtime::queue::atomic_write(&state_path(hangar_dir), &bytes)
}

/// O `backup` de `GET /api/vault/status`.
pub fn status(hangar_dir: &Path, peer: &str) -> Value {
    let state = load_state(hangar_dir);
    let mut body = json!({"peer": peer});
    if state.peer.as_deref() == Some(peer) {
        if let Some(at) = state.last_ok_at { body["last_ok_at"] = json!(at); }
        if let Some(error) = state.last_error { body["last_error"] = json!(error); }
    }
    body
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent { Skipped(&'static str), Sent, Failed(String) }

fn key_text(error: KeyError) -> &'static str {
    match error {
        KeyError::Locked => "o chaveiro está trancado",
        KeyError::Absent | KeyError::Lost => "a chave do cofre sumiu do chaveiro",
        KeyError::Bus => "o chaveiro não respondeu",
        KeyError::Unsupported => "esta máquina não tem chaveiro do sistema",
    }
}

fn vault_text(error: VaultError) -> &'static str {
    match error {
        VaultError::Key(error) => key_text(error),
        VaultError::Absent => "esta máquina não tem cofre",
        VaultError::Unreadable => "o arquivo do cofre não abriu com a chave do chaveiro",
        _ => "não foi possível ler o cofre",
    }
}

fn record(hangar_dir: &Path, peer: &str, result: Result<u64, String>) -> Sent {
    let mut state = load_state(hangar_dir);
    if state.peer.as_deref() != Some(peer) { state = SendState { peer: Some(peer.to_owned()), ..SendState::default() }; }
    let sent = match result {
        Ok(revision) => {
            state.revision = Some(revision);
            state.last_ok_at = Some(super::now());
            state.last_error = None;
            Sent::Sent
        }
        Err(why) => {
            state.last_error = Some(why.clone());
            Sent::Failed(why)
        }
    };
    match save_state(hangar_dir, &state) {
        Err(error) if sent == Sent::Sent => Sent::Failed(format!("a cópia foi aceita, mas o backup-state.json não gravou: {error}")),
        _ => sent,
    }
}

/// Uma rodada: só na dona com `backup_peer` e senha de recuperação, e só quando a revisão mudou
/// desde a última cópia aceita por aquele `peer`.
pub async fn send_once(vault: &Vault, peers: &PeerClient, hangar_dir: &Path, server_id: &str) -> Sent {
    if !store::exists(&store::path(hangar_dir)) { return Sent::Skipped("sem cofre nesta máquina"); }
    let peer = match config::load(hangar_dir) {
        Ok(cfg) => match cfg.backup_peer { Some(peer) => peer, None => return Sent::Skipped("sem backup_peer") },
        Err(_) => return Sent::Failed("o vault.json desta máquina não lê".into()),
    };
    if !config::valid_server_id(server_id) {
        return record(hangar_dir, &peer, Err("esta máquina não tem CP_SERVER_ID válido para nomear a cópia".into()));
    }
    let recovery = match vault.keys.read_recovery().await {
        Ok(Some(item)) => match Recovery::from_item(&item) {
            Some(recovery) => recovery,
            None => return record(hangar_dir, &peer, Err("o item da senha de recuperação no chaveiro está torto: defina de novo".into())),
        },
        Ok(None) => return Sent::Skipped("sem senha de recuperação"),
        Err(error) => return record(hangar_dir, &peer, Err(key_text(error).into())),
    };
    let doc = match vault.read(hangar_dir, Ok).await {
        Ok(doc) => doc,
        Err(error) => return record(hangar_dir, &peer, Err(vault_text(error).into())),
    };
    let state = load_state(hangar_dir);
    if state.peer.as_deref() == Some(peer.as_str()) && state.revision == Some(doc.revision) { return Sent::Skipped("sem mudança"); }
    let (revision, entries) = (doc.revision, doc.entries.len());
    let sealed = match tokio::task::spawn_blocking(move || seal_copy(&recovery, &doc)).await {
        Ok(Ok(sealed)) => sealed,
        _ => return record(hangar_dir, &peer, Err("a cópia não cifrou".into())),
    };
    if sealed.len() > MAX_COPY { return record(hangar_dir, &peer, Err("o cofre cifrado passa de 4 MiB".into())); }
    let path = format!("/api/vault/backup/{server_id}?entries={entries}");
    let result = match peers.call_bytes(&peer, reqwest::Method::PUT, &path, Some(sealed), 64 * 1024).await {
        Ok((_, raw)) => {
            let reported = serde_json::from_slice::<Value>(&raw).ok()
                .and_then(|v| v.get("server_id").and_then(Value::as_str).map(str::to_owned));
            match reported {
                Some(reported) if reported != peer => Err(format!(
                    "a máquina '{peer}' do peers.json responde como '{reported}': confira o CP_SERVER_ID dela")),
                _ => Ok(revision),
            }
        }
        Err(error) => Err(error.text(&peer)),
    };
    record(hangar_dir, &peer, result)
}

/// Rodada do laço e da senha nova: uma por vez.
pub async fn run(st: &AppState) -> Sent {
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = ONE.lock().await;
    let sent = send_once(&st.vault, &st.peers, &st.hangar_dir, &st.server_id).await;
    if let Sent::Failed(why) = &sent {
        tracing::warn!(code = "vault_backup_failed", "a cópia do cofre não foi: {why}");
        st.diag.report("rust.vault_backup_failed", "", "erro_cofre_backup", "a cópia diária do cofre não foi aceita");
    }
    sent
}

/// O laço diário: primeira rodada 5 min depois da subida. Só o `serve` liga.
pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST).await;
        loop {
            let failed = matches!(run(&st).await, Sent::Failed(_));
            tokio::time::sleep(if failed { RETRY } else { INTERVAL }).await;
        }
    })
}

/// `POST /api/vault/recovery`: só na dona. A cópia seguinte sai com a senha nova.
pub async fn set_recovery(vault: &Vault, hangar_dir: &Path, password: Zeroizing<String>) -> Result<(), VaultError> {
    if !store::exists(&store::path(hangar_dir)) { return Err(VaultError::Absent); }
    let recovery = tokio::task::spawn_blocking(move || Recovery::new(&password)).await.map_err(|_| VaultError::Io)??;
    vault.keys.write_recovery(recovery.to_item()).await.map_err(VaultError::Key)?;
    // A cópia que está lá abre com a senha velha: esquecer a revisão aceita força a próxima.
    let mut state = load_state(hangar_dir);
    if state.revision.take().is_some() { save_state(hangar_dir, &state).map_err(|_| VaultError::Io)?; }
    Ok(())
}

// --- Restauração -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    NotEmpty,
    /// `erro_cofre_dona_remota`: o `vault.json` daqui aponta outra dona.
    RemoteOwner,
    Vault(VaultError),
    NoCopy,
    PeerDown(String),
    Unreadable,
    WrongPassword,
    /// O cofre voltou, mas o item da senha de recuperação não gravou.
    RecoveryNotSaved(KeyError),
}

impl Vault {
    /// Vazio, ausente, ou com a chave sumida do chaveiro: esse arquivo não abre mais, e a cópia é o que resta.
    async fn restorable(&self, hangar_dir: &Path) -> Result<bool, VaultError> {
        if !store::exists(&store::path(hangar_dir)) { return Ok(true); }
        match self.read(hangar_dir, |doc| Ok(doc.entries.is_empty())).await {
            Err(VaultError::Key(KeyError::Lost | KeyError::Absent)) => Ok(true),
            other => other,
        }
    }

    /// Grava `doc` só num cofre vazio ou ausente, conferido de novo sob a trava de gravação.
    /// Ausente: a chave sai do `key::create` (nova, ou a que sobrou sem arquivo). Arquivo cuja chave
    /// sumiu vai para o lado (`vault.bin.perdido-<data>`), nunca apagado, e o cofre nasce como ausente.
    async fn restore_doc(&self, hangar_dir: &Path, mut doc: Doc) -> Result<(), RestoreError> {
        let path = store::path(hangar_dir);
        let mut slot = self.key.clone().lock_owned().await;
        let mut existed = store::exists(&path);
        let opened = if existed { Some(self.key_for(&mut slot, &path).await) } else { None };
        let key = match opened {
            Some(Ok(key)) => key,
            Some(Err(VaultError::Key(KeyError::Lost | KeyError::Absent))) => {
                set_aside(&path).map_err(|_| RestoreError::Vault(VaultError::Io))?;
                existed = false;
                key::create(&*self.keys).await.map_err(|e| RestoreError::Vault(VaultError::Key(e)))?
            }
            Some(Err(error)) => return Err(RestoreError::Vault(error)),
            None => key::create(&*self.keys).await.map_err(|e| RestoreError::Vault(VaultError::Key(e)))?,
        };
        tokio::task::spawn_blocking(move || {
            if existed {
                let current = store::load(&path, &key).map_err(|e| RestoreError::Vault(e.into()))?;
                if !current.entries.is_empty() { return Err(RestoreError::NotEmpty); }
                doc.revision = doc.revision.max(current.revision);
            }
            doc.revision += 1;
            store::save(&path, &key, &doc).map_err(|e| RestoreError::Vault(e.into()))?;
            *slot = Some(key);
            Ok(())
        }).await.map_err(|_| RestoreError::Vault(VaultError::Io))?
    }
}

/// O arquivo sem chave sai do caminho com a hora UTC no nome; um que já esteja lá não é sobrescrito.
fn set_aside(path: &Path) -> io::Result<()> {
    let aside = path.with_file_name(format!("vault.bin.perdido-{}", chrono::Utc::now().format("%Y%m%dT%H%M%SZ")));
    if aside.exists() { return Err(io::ErrorKind::AlreadyExists.into()); }
    std::fs::rename(path, aside)
}

/// `POST /api/vault/restore`: busca a cópia de `server_id` em `from_peer`, abre com a senha e grava
/// num cofre vazio. Senha errada não toca em nada. Máquina que aponta para outra dona não ganha um
/// cofre próprio ao lado.
pub async fn restore(vault: &Vault, peers: &PeerClient, hangar_dir: &Path, from_peer: &str, server_id: &str,
                     password: Zeroizing<String>) -> Result<(), RestoreError> {
    match config::placement(hangar_dir) {
        Ok(config::Placement::Remote(_)) => return Err(RestoreError::RemoteOwner),
        Err(_) => return Err(RestoreError::Vault(VaultError::Io)),
        Ok(_) => {}
    }
    if !vault.restorable(hangar_dir).await.map_err(RestoreError::Vault)? { return Err(RestoreError::NotEmpty); }
    let raw = match peers.call_bytes(from_peer, reqwest::Method::GET, &format!("/api/vault/backup/{server_id}"), None, MAX_COPY).await {
        Ok((_, raw)) => raw,
        Err(PeerError::Refused { status: 404, detail }) if detail.get("code").and_then(Value::as_str) == Some("erro_cofre_backup_ausente") =>
            return Err(RestoreError::NoCopy),
        Err(error) => return Err(RestoreError::PeerDown(error.text(from_peer))),
    };
    let opened = tokio::task::spawn_blocking(move || open_copy(&password, &raw)).await.map_err(|_| RestoreError::Vault(VaultError::Io))?;
    let (doc, recovery) = opened.map_err(|e| match e {
        CopyError::Unreadable => RestoreError::Unreadable,
        CopyError::WrongPassword => RestoreError::WrongPassword,
    })?;
    vault.restore_doc(hangar_dir, doc).await?;
    // A mesma senha continua abrindo as próximas cópias.
    vault.keys.write_recovery(recovery.to_item()).await.map_err(RestoreError::RecoveryNotSaved)
}
