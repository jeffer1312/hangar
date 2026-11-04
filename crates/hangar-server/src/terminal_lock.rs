//! Uma trava de teclado por sessão, compartilhada com os chamadores que ainda entram pelo Python.
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Guard {
    _file: File,
}

pub fn path(home: &Path, name: &str) -> PathBuf {
    let digest = sha1_smol::Sha1::from(name).digest().to_string();
    home.join(".hangar/terminal-locks").join(format!("{digest}.lock"))
}

/// Esgotado o prazo, `WouldBlock`: quem pediu já desistiu e não pode digitar depois.
pub fn acquire(home: &Path, name: &str, wait: Duration) -> io::Result<Guard> {
    let path = path(home, name);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Guard { _file: file }),
            // shortcut: flock não tem espera com prazo; sondar a cada 25 ms basta para teclado.
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Err(TryLockError::WouldBlock) => return Err(io::ErrorKind::WouldBlock.into()),
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_lock_serializes_independent_writers_until_the_guard_is_dropped() {
        let home = tempfile::tempdir().unwrap();
        let guard = acquire(home.path(), "sessão", Duration::ZERO).unwrap();
        let contender = OpenOptions::new().read(true).write(true).open(path(home.path(), "sessão")).unwrap();
        assert!(matches!(contender.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(guard);
        contender.try_lock().unwrap();
    }

    #[test]
    fn keyboard_lock_gives_up_after_the_deadline_instead_of_waiting_forever() {
        let home = tempfile::tempdir().unwrap();
        let _held = acquire(home.path(), "ocupada", Duration::ZERO).unwrap();
        let error = acquire(home.path(), "ocupada", Duration::from_millis(50)).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn keyboard_lock_has_the_same_identity_for_every_account_and_no_path_components_from_the_name() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(path(home.path(), "abc"), home.path().join(".hangar/terminal-locks/a9993e364706816aba3e25717850c26c9cd0d89d.lock"));
        let guard = acquire(home.path(), "../sessão", Duration::ZERO).unwrap();
        assert!(path(home.path(), "../sessão").is_file());
        assert_eq!(path(home.path(), "../sessão").parent(), Some(home.path().join(".hangar/terminal-locks").as_path()));
        drop(guard);
    }
}
