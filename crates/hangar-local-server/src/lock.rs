//! Uma atualização (ou troca de versão) por vez na máquina: trava de arquivo em
//! `~/.hangar/update.lock`, solta sozinha quando o processo morre.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const FILE: &str = "update.lock";

/// Quanto a trava pode seguir ocupada por um fork em andamento antes de valer como outra rodada.
const GRACE: Duration = Duration::from_millis(500);

pub fn lock_file(hangar: &Path) -> PathBuf { hangar.join(FILE) }

#[derive(Debug)]
pub enum LockError {
    /// Outra rodada segura a trava.
    Locked,
    Io(std::io::Error),
}

pub struct UpdateLock {
    _file: std::fs::File,
}

impl UpdateLock {
    pub fn try_acquire(hangar: &Path) -> Result<UpdateLock, LockError> {
        std::fs::create_dir_all(hangar).map_err(LockError::Io)?;
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(lock_file(hangar)).map_err(LockError::Io)?;
        // O `flock` é do arquivo aberto: um filho que este processo cria entre o fork e o exec leva a
        // trava junto por um instante. Ocupada por mais que isso é outra rodada.
        let deadline = Instant::now() + GRACE;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(UpdateLock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Err(std::fs::TryLockError::WouldBlock) => return Err(LockError::Locked),
                Err(std::fs::TryLockError::Error(e)) => return Err(LockError::Io(e)),
            }
        }
    }
}
