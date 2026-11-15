//! A trava que o Claude Code usa no `.claude.json` (`proper-lockfile`: a pasta `<arquivo real>.lock`).
//! Sem ela, uma sessão viva da conta que já tinha lido o arquivo regrava a cópia dela por cima.
//! Arquivo que só o Hangar trava usa `lock_owned`, que o sistema solta quando o processo morre.
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Quanto se espera a trava antes de desistir (como o próprio Claude Code faz).
const WAIT: Duration = Duration::from_secs(1);
/// Trava mais velha que isto foi largada por um processo morto (o `stale` padrão do `proper-lockfile`).
const STALE: Duration = Duration::from_secs(10);

pub struct ConfigLock(Held);

enum Held {
    Dir(PathBuf, Option<SystemTime>),
    // Fechar o arquivo solta a trava.
    File { _handle: File },
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let Held::Dir(path, created) = &self.0 else { return };
        // Outro processo que a julgou vencida e pegou a dele tem outra data: essa não é nossa para apagar.
        let current = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if created.is_some() && current != *created { return; }
        // Trava que fica para trás segura o Claude Code da conta até ela vencer.
        if let Err(e) = std::fs::remove_dir(path) && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(lock = %path.display(), error = %e, "trava do .claude.json não saiu; o Claude Code espera ela vencer");
        }
    }
}

pub fn lock_path(file: &Path) -> PathBuf {
    suffixed(file, ".lock")
}

fn suffixed(file: &Path, suffix: &str) -> PathBuf {
    let mut lock = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned()).into_os_string();
    lock.push(suffix);
    PathBuf::from(lock)
}

pub fn lock_config(file: &Path) -> Result<ConfigLock, String> {
    let lock = lock_path(file);
    let deadline = Instant::now() + WAIT;
    loop {
        match std::fs::create_dir(&lock) {
            Ok(()) => {
                let created = std::fs::metadata(&lock).and_then(|m| m.modified()).ok();
                return Ok(ConfigLock(Held::Dir(lock, created)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Vencida não tem dono vivo para disputar. Apagá-la pelo caminho é que arrisca: entre ler a
                // data e apagar, outro processo pode trocá-la pela dele. O Claude Code a recupera sozinho.
                let stale = std::fs::metadata(&lock).and_then(|m| m.modified()).ok()
                    .and_then(|t| t.elapsed().ok()).is_some_and(|age| age > STALE);
                if stale { return Err("trava largada".into()); }
                if Instant::now() >= deadline { return Err("trava ocupada".into()); }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Arquivo do Hangar que o Claude Code nunca trava: flock em `<arquivo real>.flock`. Um `kill -9` no
/// meio não deixa trava para trás, ao contrário da pasta de `lock_config`.
pub fn lock_owned(file: &Path) -> Result<ConfigLock, String> {
    let lock = suffixed(file, ".flock");
    let handle = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + WAIT;
    loop {
        match handle.try_lock() {
            Ok(()) => return Ok(ConfigLock(Held::File { _handle: handle })),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::WouldBlock) => return Err("trava ocupada".into()),
            Err(TryLockError::Error(e)) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pasta largada por um processo morto não segura a trava do Hangar, e a do flock sai com o dono.
    #[test]
    fn owned_lock_ignores_a_leftover_dir_and_frees_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("computer-control.json");
        std::fs::create_dir(lock_path(&file)).unwrap();
        let held = lock_owned(&file).unwrap();
        assert_eq!(lock_owned(&file).err().as_deref(), Some("trava ocupada"));
        drop(held);
        lock_owned(&file).unwrap();
    }
}
