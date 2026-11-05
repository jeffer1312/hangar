//! Exclusão entre preparações da mesma conta, sem bloquear o executor assíncrono.
use std::{
    fs::{File, OpenOptions, TryLockError},
    future::Future,
    io,
    path::Path,
    time::Duration,
};

pub async fn acquire<F: Future<Output = ()>>(
    home: &Path,
    sleep: impl Fn(Duration) -> F,
) -> io::Result<File> {
    std::fs::create_dir_all(home)?;
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(home.join(".hangar-instrucoes.lock"))?;
    #[cfg(windows)]
    {
        use std::io::Write;
        if file.metadata()?.len() == 0 {
            let mut writer = &file;
            writer.write_all(b"0")?;
            writer.flush()?;
        }
    }
    let mut attempts = 0;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                attempts += 1;
                #[cfg(not(windows))]
                if attempts == 100 { return Err(io::Error::new(io::ErrorKind::WouldBlock,"codex_instructions_busy: outra preparação mantém a trava da conta")); }
                #[cfg(windows)]
                {
                    if attempts == 10 {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "codex_instructions_busy: outra preparação mantém a trava da conta",
                        ));
                    }
                    sleep(Duration::from_secs(1)).await;
                }
                #[cfg(not(windows))]
                sleep(Duration::from_millis(100)).await;
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

#[cfg(all(test,unix))]
#[tokio::test]
async fn unix_contention_is_bounded_and_releases_the_handle() {
    let home=tempfile::tempdir().unwrap();let held=acquire(home.path(), |_|async{}).await.unwrap();
    let error=acquire(home.path(), |_|async{}).await.unwrap_err();assert_eq!(error.kind(),io::ErrorKind::WouldBlock);
    drop(held);assert!(acquire(home.path(), |_|async{}).await.is_ok());
}
