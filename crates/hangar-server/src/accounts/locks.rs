use super::{AccountKey, GuardMode};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions, TryLockError},
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Fila por conta dentro do processo; entre processos quem protege é a trava de arquivo.
#[derive(Clone, Default)]
pub struct KeyedGates(Arc<Mutex<HashMap<AccountKey, Arc<tokio::sync::Mutex<()>>>>>);
impl KeyedGates {
    pub fn gate(&self, key: &AccountKey) -> Arc<tokio::sync::Mutex<()>> {
        self.0
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_default()
            .clone()
    }
}

#[derive(Debug)]
pub enum LockError {
    Busy,
    Io(io::Error),
}
impl From<io::Error> for LockError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub struct AccountGuard {
    pub key: AccountKey,
    pub mode: GuardMode,
    _file: File,
}

impl Drop for AccountGuard {
    fn drop(&mut self) {
        // Fechar só o descritor mantém a trava quando um filho herdou a referência.
        if let Err(error) = self._file.unlock() {
            tracing::warn!(
                code = "account_unlock_failed",
                io_kind = ?error.kind(),
                "não foi possível liberar a trava da conta; o descritor será fechado"
            );
        }
    }
}

#[derive(Clone)]
pub struct AccountLocks {
    root: PathBuf,
}

impl AccountLocks {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn system() -> io::Result<Self> {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or_else(|| io::Error::other("pasta pessoal indisponível"))?;
        Ok(Self::new(PathBuf::from(home).join(".hangar/account-locks")))
    }

    pub fn path(&self, key: &AccountKey) -> io::Result<PathBuf> {
        Ok(self.root.join(format!("{}.lock", key.digest()?)))
    }

    /// Registros da conta ao lado da trava: mesmo volume, publicação atômica possível.
    pub fn sidecar(&self, key: &AccountKey, extension: &str) -> io::Result<PathBuf> {
        Ok(self.path(key)?.with_extension(extension))
    }

    pub fn try_acquire(
        &self,
        key: &AccountKey,
        mode: GuardMode,
    ) -> Result<AccountGuard, LockError> {
        std::fs::create_dir_all(&self.root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path(key)?)?;
        let result = match mode {
            GuardMode::Shared => file.try_lock_shared(),
            GuardMode::Exclusive => file.try_lock(),
        };
        match result {
            Ok(()) => Ok(AccountGuard {
                key: key.clone(),
                mode,
                _file: file,
            }),
            Err(TryLockError::WouldBlock) => Err(LockError::Busy),
            Err(TryLockError::Error(error)) => Err(LockError::Io(error)),
        }
    }

    /// Cancelar a future abandona a espera; uma guarda entregue nunca vence por prazo.
    /// Cada tentativa roda aqui mesmo: `try_lock` não bloqueia, e uma tentativa no pool de bloqueio
    /// sobreviveria ao cancelamento e seguraria a trava depois dele.
    pub async fn acquire(
        &self,
        key: &AccountKey,
        mode: GuardMode,
        deadline: Instant,
    ) -> Result<AccountGuard, LockError> {
        loop {
            match self.try_acquire(key, mode) {
                Err(LockError::Busy) if Instant::now() < deadline => {
                    tokio::time::sleep(
                        Duration::from_millis(25)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    )
                    .await;
                }
                result => return result,
            }
        }
    }
}
