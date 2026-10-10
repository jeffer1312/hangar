//! Prova de descritores entre processos, incluindo o chamador Python.
use hangar_server::accounts::{
    AccountKey, AccountLocks, GuardMode, LockError, Provider, UsageFacts,
};
use std::time::{Duration, Instant};

#[test]
fn shared_key_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/accounts_contract/account-keys.json"
    ))
    .unwrap();
    for row in fixture.as_array().unwrap() {
        let provider: Provider = serde_json::from_value(row["provider"].clone()).unwrap();
        for value in row["inputs"].as_array().unwrap() {
            let canonical = if row["windows"] == true {
                hangar_server::accounts::types::normalize_windows_path(value.as_str().unwrap())
            } else {
                value.as_str().unwrap().to_owned()
            };
            assert_eq!(canonical, row["canonical"]);
            assert_eq!(
                hangar_server::accounts::types::key_digest(provider, &canonical),
                row["sha256"]
            );
        }
    }
}

#[test]
fn missing_account_keeps_key_after_birth_and_removal() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("revisão").join("nova");
    let before = AccountKey::new(Provider::Codex, &home).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert_eq!(before, AccountKey::new(Provider::Codex, &home).unwrap());
    let locks = AccountLocks::new(root.path().join("locks"));
    let guard = locks.try_acquire(&before, GuardMode::Shared).unwrap();
    std::fs::remove_dir_all(home).unwrap();
    assert!(matches!(
        locks.try_acquire(&before, GuardMode::Exclusive),
        Err(LockError::Busy)
    ));
    drop(guard);
    assert!(locks.path(&before).unwrap().is_file());
    assert!(locks.try_acquire(&before, GuardMode::Exclusive).is_ok());
}

#[test]
fn shared_birth_allows_preparation_but_excludes_identity_changes() {
    let root = tempfile::tempdir().unwrap();
    let key = AccountKey::new(Provider::Claude, &root.path().join("account")).unwrap();
    let locks = AccountLocks::new(root.path().join("locks"));
    let birth = locks.try_acquire(&key, GuardMode::Shared).unwrap();
    let preparation = locks.try_acquire(&key, GuardMode::Shared).unwrap();
    assert!(matches!(
        locks.try_acquire(&key, GuardMode::Exclusive),
        Err(LockError::Busy)
    ));
    drop(birth);
    assert!(matches!(
        locks.try_acquire(&key, GuardMode::Exclusive),
        Err(LockError::Busy)
    ));
    drop(preparation);
    let identity = locks.try_acquire(&key, GuardMode::Exclusive).unwrap();
    assert!(matches!(
        locks.try_acquire(&key, GuardMode::Shared),
        Err(LockError::Busy)
    ));
    assert!(matches!(
        locks.try_acquire(&key, GuardMode::Exclusive),
        Err(LockError::Busy)
    ));
    drop(identity);
}

/// A tentativa roda na própria espera, não no pool de bloqueio: com o pool ocupado a trava livre sai na
/// hora, e uma espera cancelada não deixa tentativa solta que pegue a trava depois.
#[test]
fn acquire_does_not_queue_behind_the_blocking_pool() {
    let runtime = tokio::runtime::Builder::new_multi_thread().max_blocking_threads(1).enable_all().build().unwrap();
    let (release, hold) = std::sync::mpsc::channel::<()>();
    let busy = runtime.spawn_blocking(move || { let _ = hold.recv(); });
    let root = tempfile::tempdir().unwrap();
    let key = AccountKey::new(Provider::Codex, root.path()).unwrap();
    let locks = AccountLocks::new(root.path().join("locks"));
    let acquired = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), locks.acquire(&key, GuardMode::Exclusive, Instant::now())).await
    });
    release.send(()).unwrap();
    runtime.block_on(busy).unwrap();
    assert!(matches!(acquired, Ok(Ok(_))), "a trava livre esperou o pool de bloqueio");
}

#[tokio::test]
async fn deadline_is_for_waiting_not_for_guard_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let key = AccountKey::new(Provider::Codex, root.path()).unwrap();
    let locks = AccountLocks::new(root.path().join("locks"));
    let guard = locks
        .acquire(&key, GuardMode::Shared, Instant::now())
        .await
        .unwrap();
    assert!(matches!(
        locks
            .acquire(&key, GuardMode::Exclusive, Instant::now())
            .await,
        Err(LockError::Busy)
    ));
    let mut pending = Box::pin(locks.acquire(
        &key,
        GuardMode::Exclusive,
        Instant::now() + Duration::from_secs(60),
    ));
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    drop(pending);
    assert!(matches!(
        locks.try_acquire(&key, GuardMode::Exclusive),
        Err(LockError::Busy)
    ));
    drop(guard);
    assert!(locks.try_acquire(&key, GuardMode::Exclusive).is_ok());
}

#[test]
fn incomplete_facts_cannot_authorize_exclusion() {
    assert_eq!(
        UsageFacts::default().ensure_unused(),
        Err("account_usage_unknown")
    );
    let mut facts = UsageFacts {
        complete: true,
        sessions: Vec::new(),
        pids: Vec::new(),
        holders: Vec::new(),
    };
    assert_eq!(facts.ensure_unused(), Ok(()));
    facts.merge(UsageFacts {
        complete: false,
        sessions: vec!["nascendo".into()],
        pids: vec![17],
        holders: Vec::new(),
    });
    assert_eq!(facts.ensure_unused(), Err("account_usage_unknown"));
    facts.complete = true;
    assert_eq!(facts.ensure_unused(), Err("account_in_use"));
}

use std::{
    fs::OpenOptions,
    io::{self, BufRead, Write},
    path::PathBuf,
};

#[test]
fn probe_process() {
    let Some(path) = std::env::var_os("ACCOUNT_PROBE_PATH") else {
        return;
    };
    let path = PathBuf::from(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    let shared = std::env::var("ACCOUNT_PROBE_MODE").as_deref() == Ok("shared");
    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    let state = match result {
        Ok(()) => "acquired",
        Err(std::fs::TryLockError::WouldBlock) => "busy",
        Err(error) => panic!("falha ao adquirir descritor: {error}"),
    };
    println!("ACCOUNT_PROBE:{state}");
    io::stdout().flush().unwrap();
    let _ = io::stdin().lock().lines().next();
}
