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
    #[cfg(windows)]
    let mut attempts = 0;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                #[cfg(windows)]
                {
                    attempts += 1;
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

#[cfg(all(test, windows))]
#[tokio::test]
async fn windows_contention_matches_original_deadline_and_reacquisition() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/instructions-lock-windows.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let held = acquire(home.path(), tokio::time::sleep).await.unwrap();
    let started = std::time::Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(14),
        acquire(home.path(), tokio::time::sleep),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(started.elapsed() >= Duration::from_secs(8));
    assert!(error.to_string().starts_with("codex_instructions_busy:"));
    assert!(fixture["contender"].get("error").is_some());
    drop(held);
    let raw = std::fs::read(home.path().join(".hangar-instrucoes.lock")).unwrap();
    assert_eq!(
        raw.iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        fixture["locked_bytes"].as_str().unwrap()
    );
    let again = acquire(home.path(), tokio::time::sleep).await;
    assert_eq!(again.is_ok(), fixture["reacquired"].as_bool().unwrap());
}
