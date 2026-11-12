use futures_util::future::BoxFuture;
use hangar_server::accounts::{
    AccountKey, AccountService, GuardMode, Provider,
    bridge::AccountsBridge,
    catalog::Account,
    environment::AccountEnvironment,
    preparation::{CodexSetup, PrepareRequest},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, UNIX_EPOCH},
};

struct NativeSetup {
    calls: Mutex<Vec<String>>,
    result: Value,
    started: tokio::sync::mpsc::UnboundedSender<bool>,
    release: Option<Arc<tokio::sync::Semaphore>>,
}
impl CodexSetup for NativeSetup {
    fn update(self: Arc<Self>, force: bool) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("source:{force}"));
            Ok(json!({"estado":"ok"}))
        })
    }
    fn repair(
        self: Arc<Self>,
        account: Account,
    ) -> BoxFuture<'static, Result<Option<bool>, String>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("repair:{}", account.id));
            Ok(Some(true))
        })
    }
    fn project(
        self: Arc<Self>,
        _account: Account,
        _cwd: String,
    ) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("project".into());
            Ok(())
        })
    }
    fn status(&self) -> Result<Value, String> {
        Ok(json!({"estado":"ok"}))
    }
    fn inherit(
        self: Arc<Self>,
        account: Account,
        force: bool,
    ) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("inherit:{force}"));
            let _ = self.started.send(force);
            if let Some(release) = &self.release {
                release.acquire().await.map_err(|e| e.to_string())?.forget();
            }
            std::fs::create_dir_all(&account.home).map_err(|e| e.to_string())?;
            Ok(self.result.clone())
        })
    }
    fn trust_project(
        self: Arc<Self>,
        _account: Account,
        cwd: String,
    ) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("trust:{cwd}"));
            Ok(())
        })
    }
}
fn bind_native(
    service: &AccountService,
    status: &str,
    release: Option<Arc<tokio::sync::Semaphore>>,
) -> (Arc<NativeSetup>, tokio::sync::mpsc::UnboundedReceiver<bool>) {
    let (started, receive) = tokio::sync::mpsc::unbounded_channel();
    let issues = if status == "ready" {
        json!([])
    } else {
        json!([{"code":"codex_account_source_sync_incomplete","params":{"status":"parcial"}}])
    };
    let setup = Arc::new(NativeSetup {
        calls: Mutex::new(vec![]),
        result: json!({"status":status,"trust_pending":false,"issues":issues}),
        started,
        release,
    });
    let native: Arc<dyn CodexSetup> = setup.clone();
    *service.codex_setup.lock().unwrap() = Some(Arc::downgrade(&native));
    (setup, receive)
}
fn environment(root: &Path) -> AccountEnvironment {
    let home = root.to_string_lossy().into_owned();
    AccountEnvironment::from_map(
        [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
    )
}
fn bridge() -> AccountsBridge {
    // Sem servidor nesta porta: o preparo não pode depender de callback HTTP.
    AccountsBridge::new(
        "127.0.0.1:1".parse().unwrap(),
        "synthetic".into(),
        "instance".into(),
    )
    .unwrap()
}
async fn settled(service: &AccountService, account: &Account, bridge: &AccountsBridge) -> Value {
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = service.preparation_status(account, bridge).await;
            if result["status"] != "running"
                && service
                    .locks
                    .try_acquire(&key, GuardMode::Exclusive)
                    .is_ok()
            {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("O preparo não encerrou a operação nativa")
}
async fn started(receive: &mut tokio::sync::mpsc::UnboundedReceiver<bool>) -> bool {
    tokio::time::timeout(Duration::from_secs(15), receive.recv())
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn preparation_routes_belong_to_rust() {
    use axum::http::Method;
    use hangar_server::migration_status::rust_route;
    assert!(rust_route(&Method::POST, "/api/claude-configs"));
    assert!(rust_route(&Method::GET, "/api/codex-contas"));
}
#[tokio::test]
async fn detached_preparation_keeps_guard_and_force_waits_for_effective_worker() {
    let root = tempfile::tempdir().unwrap();
    let service = AccountService::new(environment(root.path()));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (native, mut receive) = bind_native(&service, "ready", Some(release.clone()));
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let bridge = bridge();
    assert_eq!(
        service.preparation_status(&account, &bridge).await["status"],
        "idle"
    );
    assert!(receive.try_recv().is_err());
    assert_eq!(
        service
            .prepare(&account, bridge.clone(), false, None)
            .await
            .unwrap()["status"],
        "running"
    );
    assert!(!started(&mut receive).await);
    assert_eq!(
        service
            .prepare(&account, bridge.clone(), true, None)
            .await
            .unwrap()["status"],
        "running"
    );
    assert!(receive.try_recv().is_err());
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    release.add_permits(1);
    assert!(started(&mut receive).await);
    assert!(
        service
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .is_err()
    );
    release.add_permits(1);
    let result = settled(&service, &account, &bridge).await;
    assert_eq!(result["status"], "ready");
    assert_eq!(result["trust_pending"], true);
    assert!(receive.try_recv().is_err());
    let calls = native.calls.lock().unwrap();
    assert!(calls.iter().any(|c| c == "inherit:false"));
    assert!(calls.iter().any(|c| c == "inherit:true"));
}
#[tokio::test]
async fn newer_legacy_state_is_not_hidden_by_coordinator_result() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let service = AccountService::new(env.clone());
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let result = service
        .locks
        .path(&key)
        .unwrap()
        .with_extension("prepare-result.json");
    std::fs::write(&result, json!({"status":"error","issues":[]}).to_string()).unwrap();
    let digest: String = ring::digest::digest(
        &ring::digest::SHA256,
        account.home.to_string_lossy().as_bytes(),
    )
    .as_ref()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
    let legacy = root
        .path()
        .join(".hangar/codex-contas")
        .join(digest)
        .join("estado.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let partial = json!({"status":"partial","trust_pending":false,"issues":[{"code":"configuration_collision"}]});
    std::fs::write(&legacy, json!({"public":partial}).to_string()).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&result)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&legacy)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(2))
        .unwrap();
    let restarted = AccountService::new(env);
    let bridge = bridge();
    assert_eq!(
        restarted.preparation_status(&account, &bridge).await,
        partial
    );
    std::fs::File::options()
        .write(true)
        .open(&result)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(3))
        .unwrap();
    assert_eq!(
        restarted.preparation_status(&account, &bridge).await["status"],
        "error"
    );
}
#[tokio::test]
async fn default_account_with_only_a_partial_marker_stays_partial() {
    let root = tempfile::tempdir().unwrap();
    let service = AccountService::new(environment(root.path()));
    let account = service.resolve(Provider::Codex, "default").unwrap();
    std::fs::create_dir_all(&account.home).unwrap();
    let bridge = bridge();
    for (marker, status) in [
        (r#"{"version":1}"#, "ready"),
        (r#"{"version":1,"status":"partial"}"#, "partial"),
    ] {
        std::fs::write(account.home.join(".hangar-prepared-v1"), marker).unwrap();
        assert_eq!(
            service.preparation_status(&account, &bridge).await["status"],
            status
        );
    }
}
#[tokio::test]
async fn ready_preparation_is_reused_after_restart_until_explicit_force() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let first = AccountService::new(env.clone());
    let (_native, mut receive) = bind_native(&first, "ready", None);
    let account = first.create(Provider::Codex, "extra", |_| Ok(())).unwrap();
    let bridge = bridge();
    assert_eq!(
        first
            .prepare(&account, bridge.clone(), false, None)
            .await
            .unwrap()["status"],
        "running"
    );
    assert_eq!(settled(&first, &account, &bridge).await["status"], "ready");
    assert!(!started(&mut receive).await);
    assert!(receive.try_recv().is_err());
    let restarted = AccountService::new(env);
    let (_native, mut receive) = bind_native(&restarted, "ready", None);
    assert_eq!(
        restarted
            .prepare(&account, bridge.clone(), false, None)
            .await
            .unwrap()["status"],
        "ready"
    );
    assert!(receive.try_recv().is_err());
    assert_eq!(
        restarted
            .prepare(&account, bridge.clone(), true, None)
            .await
            .unwrap()["status"],
        "running"
    );
    assert_eq!(
        settled(&restarted, &account, &bridge).await["status"],
        "ready"
    );
    assert!(started(&mut receive).await);
    assert!(receive.try_recv().is_err());
}
#[tokio::test]
async fn partial_preparation_opens_is_reused_and_retries_after_expiry() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let service = AccountService::new(env.clone());
    let (_native, mut receive) = bind_native(&service, "partial", None);
    let account = service.resolve(Provider::Codex, "default").unwrap();
    std::fs::create_dir_all(&account.home).unwrap();
    let bridge = bridge();
    let result = service
        .prepare_for_session(
            &account,
            bridge.clone(),
            tokio::time::Instant::now() + Duration::from_secs(15),
        )
        .await
        .unwrap();
    assert_eq!(result["status"], "partial");
    assert_eq!(
        result["issues"][0]["code"],
        "codex_account_source_sync_incomplete"
    );
    settled(&service, &account, &bridge).await;
    assert!(!started(&mut receive).await);
    let restarted = AccountService::new(env.clone());
    let (_native, mut receive) = bind_native(&restarted, "partial", None);
    assert_eq!(
        restarted
            .prepare_for_session(
                &account,
                bridge.clone(),
                tokio::time::Instant::now() + Duration::from_secs(15)
            )
            .await
            .unwrap()["status"],
        "partial"
    );
    assert!(receive.try_recv().is_err());
    let marker = account.home.join(".hangar-prepared-v1");
    std::fs::File::options()
        .write(true)
        .open(&marker)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    let restarted = AccountService::new(env);
    let (_native, mut receive) = bind_native(&restarted, "partial", None);
    assert_eq!(
        restarted
            .prepare_for_session(
                &account,
                bridge.clone(),
                tokio::time::Instant::now() + Duration::from_secs(15)
            )
            .await
            .unwrap()["status"],
        "partial"
    );
    settled(&restarted, &account, &bridge).await;
    assert!(!started(&mut receive).await);
    let marker: Value = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    assert_eq!(marker["status"], "partial");
}
#[tokio::test]
async fn preparation_error_refuses_session_open() {
    let root = tempfile::tempdir().unwrap();
    let service = AccountService::new(environment(root.path()));
    let (_native, _receive) = bind_native(&service, "error", None);
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let bridge = bridge();
    assert_eq!(
        service
            .prepare_for_session(
                &account,
                bridge.clone(),
                tokio::time::Instant::now() + Duration::from_secs(15)
            )
            .await
            .unwrap_err()
            .code,
        "codex_account_prepare_incomplete"
    );
    assert_eq!(
        settled(&service, &account, &bridge).await["status"],
        "error"
    );
}
#[tokio::test]
async fn first_open_waits_for_preparation_and_expired_birth_budget_refuses_open() {
    for id in ["default", "extra"] {
        let root = tempfile::tempdir().unwrap();
        let service = AccountService::new(environment(root.path()));
        let (_native, mut receive) = bind_native(&service, "ready", None);
        let account = if id == "default" {
            service.resolve(Provider::Codex, id).unwrap()
        } else {
            service.create(Provider::Codex, id, |_| Ok(())).unwrap()
        };
        std::fs::create_dir_all(&account.home).unwrap();
        let bridge = bridge();
        assert_eq!(
            service
                .prepare_for_session(
                    &account,
                    bridge.clone(),
                    tokio::time::Instant::now() + Duration::from_secs(15)
                )
                .await
                .unwrap()["status"],
            "ready"
        );
        settled(&service, &account, &bridge).await;
        assert!(!started(&mut receive).await);
        assert!(receive.try_recv().is_err());
        assert_eq!(
            service
                .prepare_for_session(&account, bridge.clone(), tokio::time::Instant::now())
                .await
                .unwrap_err()
                .code,
            "codex_account_preparing"
        );
    }
}
fn interrupted(service: &AccountService, account: &Account, force: bool) -> std::path::PathBuf {
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let request = PrepareRequest {
        operation: "0123456789abcdef0123456789abcdef".into(),
        instance: "previous".into(),
        key: key.clone(),
        account_id: account.id.clone(),
        seed: false,
        force: false,
        cwd: None,
    };
    let path = service.preparation_path(&key).unwrap();
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    if force {
        std::fs::write(
            service
                .locks
                .path(&key)
                .unwrap()
                .with_extension("prepare-force.json"),
            "true",
        )
        .unwrap();
    }
    path
}
#[tokio::test]
async fn stale_operation_is_interrupted_without_restarting_from_status_read() {
    let root = tempfile::tempdir().unwrap();
    let service = AccountService::new(environment(root.path()));
    let (_native, mut receive) = bind_native(&service, "ready", None);
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let path = interrupted(&service, &account, false);
    let bridge = bridge();
    let result = settled(&service, &account, &bridge).await;
    assert_eq!(result["status"], "error");
    assert_eq!(result["issues"][0]["code"], "account_prepare_interrupted");
    assert!(!path.exists());
    assert!(receive.try_recv().is_err());
    assert_eq!(
        service.preparation_status(&account, &bridge).await["issues"][0]["code"],
        "account_prepare_interrupted"
    );
    assert!(receive.try_recv().is_err());
}
#[tokio::test]
async fn pending_force_survives_interrupted_operation() {
    let root = tempfile::tempdir().unwrap();
    let service = AccountService::new(environment(root.path()));
    let (_native, mut receive) = bind_native(&service, "ready", None);
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let path = interrupted(&service, &account, true);
    let bridge = bridge();
    assert_eq!(
        settled(&service, &account, &bridge).await["status"],
        "ready"
    );
    assert!(started(&mut receive).await);
    assert!(receive.try_recv().is_err());
    assert!(!path.exists());
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    assert!(
        !service
            .locks
            .path(&key)
            .unwrap()
            .with_extension("prepare-force.json")
            .exists()
    );
}
#[tokio::test]
async fn pretrust_requires_prepared_account_and_valid_directory() {
    for status in ["ready", "partial"] {
        let root = tempfile::tempdir().unwrap();
        let service = AccountService::new(environment(root.path()));
        let (native, _receive) = bind_native(&service, status, None);
        let account = service
            .create(Provider::Codex, "extra", |_| Ok(()))
            .unwrap();
        let bridge = bridge();
        let cwd = root.path().display().to_string();
        assert_eq!(
            service
                .pretrust(&account, bridge.clone(), cwd.clone())
                .await
                .unwrap()["status"],
            "error"
        );
        assert!(
            !native
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("trust:"))
        );
        service
            .prepare(&account, bridge.clone(), false, None)
            .await
            .unwrap();
        assert_eq!(settled(&service, &account, &bridge).await["status"], status);
        assert_eq!(
            service
                .pretrust(&account, bridge.clone(), cwd.clone())
                .await
                .unwrap()["status"],
            status
        );
        assert!(
            native
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == &format!("trust:{cwd}"))
        );
        for invalid in [
            "relative".to_owned(),
            root.path().join("missing").display().to_string(),
        ] {
            assert_eq!(
                service
                    .pretrust(&account, bridge.clone(), invalid.clone())
                    .await
                    .unwrap_err()
                    .code,
                "account_prepare_cwd_invalid"
            );
            assert_eq!(
                service
                    .prepare(&account, bridge.clone(), false, Some(invalid))
                    .await
                    .unwrap_err()
                    .code,
                "account_prepare_cwd_invalid"
            );
        }
        settled(&service, &account, &bridge).await;
    }
}
