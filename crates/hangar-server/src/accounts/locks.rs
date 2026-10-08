use super::{AccountKey, GuardMode};
use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum LockError {
    Busy,
    Io(io::Error),
    Worker,
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
    pub async fn acquire(
        &self,
        key: &AccountKey,
        mode: GuardMode,
        deadline: Instant,
    ) -> Result<AccountGuard, LockError> {
        loop {
            let (locks, key) = (self.clone(), key.clone());
            match tokio::task::spawn_blocking(move || locks.try_acquire(&key, mode))
                .await
                .map_err(|_| LockError::Worker)?
            {
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
