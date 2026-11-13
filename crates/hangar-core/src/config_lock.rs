//! A trava que o Claude Code usa no `.claude.json` (`proper-lockfile`: a pasta `<arquivo real>.lock`).
//! Sem ela, uma sessão viva da conta que já tinha lido o arquivo regrava a cópia dela por cima.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Quanto se espera a trava antes de desistir (como o próprio Claude Code faz).
const WAIT: Duration = Duration::from_secs(1);
/// Trava mais velha que isto foi largada por um processo morto (o `stale` padrão do `proper-lockfile`).
const STALE: Duration = Duration::from_secs(10);

pub struct ConfigLock(PathBuf, Option<SystemTime>);

impl Drop for ConfigLock {
    fn drop(&mut self) {
        // Outro processo que a julgou vencida e pegou a dele tem outra data: essa não é nossa para apagar.
        let current = std::fs::metadata(&self.0).and_then(|m| m.modified()).ok();
        if self.1.is_some() && current != self.1 { return; }
        // Trava que fica para trás segura o Claude Code da conta até ela vencer.
        if let Err(e) = std::fs::remove_dir(&self.0) && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(lock = %self.0.display(), error = %e, "trava do .claude.json não saiu; o Claude Code espera ela vencer");
        }
    }
}

pub fn lock_path(file: &Path) -> PathBuf {
    let mut lock = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned()).into_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

pub fn lock_config(file: &Path) -> Result<ConfigLock, String> {
    let lock = lock_path(file);
    let deadline = Instant::now() + WAIT;
    loop {
        match std::fs::create_dir(&lock) {
            Ok(()) => {
                let created = std::fs::metadata(&lock).and_then(|m| m.modified()).ok();
                return Ok(ConfigLock(lock, created));
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
