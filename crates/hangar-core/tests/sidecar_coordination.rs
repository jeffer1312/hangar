//! Sidecar gravado por um processo e lido ou regravado por outro: a trava `<nome>.lock` e a leitura
//! durante a troca. Os casos que o contrato em pytest cobria do lado Python; a leitura que espera a
//! troca do Windows e o leitor que compartilha a exclusão estão nos testes de `fs.rs`.
use std::time::{Duration, Instant};

#[test]
fn reading_a_missing_file_does_not_wait() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let error = hangar_core::fs::read_replaced(&dir.path().join("ausente.json")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    // No Windows a ausência ainda espera a troca curta (~50 ms), nunca mais que isso.
    assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
}

#[cfg(unix)]
#[test]
fn reading_never_retries_on_posix() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.json");
    std::fs::write(&target, b"{}").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Quem roda como root lê mesmo sem permissão: não há recusa para conferir.
    if std::fs::read(&target).is_ok() { return; }
    let started = Instant::now();
    let error = hangar_core::fs::read_replaced(&target).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(started.elapsed() < Duration::from_millis(100), "{:?}", started.elapsed());
}

#[test]
fn a_replaced_file_reads_whole() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("s.json");
    hangar_core::fs::atomic_write(&target, br#"{"name": "s", "key": "k"}"#).unwrap();
    hangar_core::fs::atomic_write(&target, br#"{"name": "s", "key": "novo"}"#).unwrap();
    assert_eq!(hangar_core::fs::read_replaced(&target).unwrap(), br#"{"name": "s", "key": "novo"}"#);
}

/// Quem espera a trava a pega logo que o outro solta (o `LK_LOCK` do Windows tentava a cada 1 s).
#[test]
fn the_lock_is_taken_soon_after_the_other_writer_releases() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("s.lock");
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = {
        let lock = lock.clone();
        std::thread::spawn(move || {
            let file = hangar_core::fs::lock_file(&lock).unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            drop(file);
        })
    };
    held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    let _mine = hangar_core::fs::lock_file(&lock).unwrap();
    let waited = started.elapsed();
    holder.join().unwrap();
    assert!(waited >= Duration::from_millis(100) && waited < Duration::from_millis(800), "{waited:?}");
}
