//! Contrato HTTP do catálogo; o backend de reserva não pode produzir a resposta.
use axum::{Router, http::StatusCode};
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn owner_catalog_is_not_a_python_proxy() {
    let hits = Arc::new(AtomicUsize::new(0));
    let recorded = hits.clone();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let python = tokio::spawn(async move {
        axum::serve(
            upstream,
            Router::new().fallback(move || {
                recorded.fetch_add(1, Ordering::SeqCst);
                async { (StatusCode::SERVICE_UNAVAILABLE, "handler Python bloqueado") }
            }),
        )
        .await
        .unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: upstream_addr,
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = isolated_service(root.path());
    let server = tokio::spawn(hangar_server::routes::serve_with_state(listener, state));
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/api/claude-configs"))
        .bearer_auth("contract-only")
        .send()
        .await
        .unwrap();
    let status = response.status();
    server.abort();
    python.abort();
    assert_eq!(
        status, 200,
        "o catálogo do dono ainda depende do handler Python"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "o catálogo chegou ao Python"
    );
}

#[test]
fn catalog_and_native_login_routes_belong_to_rust() {
    use axum::http::Method;
    use hangar_server::migration_status::rust_route;
    assert!(rust_route(&Method::GET, "/api/claude-configs"));
    assert!(rust_route(&Method::POST, "/api/codex-contas"));
    assert!(rust_route(&Method::DELETE, "/api/codex-contas/extra"));
    assert!(rust_route(&Method::DELETE, "/api/claude-configs/extra"));
    assert!(rust_route(&Method::GET, "/api/codex-contas"));
    assert!(rust_route(&Method::POST, "/api/claude-configs"));
    assert!(rust_route(
        &Method::DELETE,
        "/api/codex-contas/extra/login"
    ));
}

fn isolated_service(home: &std::path::Path) -> hangar_server::accounts::AccountService {
    use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
    AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ]
        .into(),
    ))
}

#[test]
fn marker_is_published_only_after_seed_and_rollback_owns_its_directory() {
    use hangar_server::accounts::{Provider, catalog::AccountError};
    let root = tempfile::tempdir().unwrap();
    let service = isolated_service(root.path());
    let result = service.create(Provider::Claude, "fresh", |path| {
        assert!(service.snapshot(Provider::Claude).unwrap().is_empty());
        assert!(!path.join(".hangar-conta").exists());
        std::fs::write(path.join("config.json"), "{}").unwrap();
        Err(AccountError::io())
    });
    assert!(result.is_err());
    assert!(!root.path().join(".claude-fresh").exists());
    let existing = root.path().join(".claude-existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("preserve"), "não remover").unwrap();
    assert!(
        service
            .create(Provider::Claude, "existing", |_| panic!(
                "não semear pasta alheia"
            ))
            .is_err()
    );
    assert!(existing.join("preserve").exists());
    let account = service
        .create(Provider::Claude, "fresh", |_| Ok(()))
        .unwrap();
    assert!(account.home.join(".hangar-conta").is_file());
    assert!(!account.home.join(".hangar-account-pending").exists());
    let displaced = root.path().join("displaced");
    assert!(
        service
            .create(Provider::Claude, "replaced", |path| {
                std::fs::rename(path, &displaced).unwrap();
                std::fs::create_dir(path).unwrap();
                std::fs::write(path.join("keep"), "pasta de outra operação").unwrap();
                Err(AccountError::io())
            })
            .is_err()
    );
    assert!(
        root.path().join(".claude-replaced/keep").is_file(),
        "rollback removeu pasta que não criou"
    );
}

#[test]
fn invalid_names_markers_and_default_are_protected() {
    use hangar_server::accounts::{AccountKey, GuardMode, Provider, UsageFacts};
    let root = tempfile::tempdir().unwrap();
    let service = isolated_service(root.path());
    for name in ["name\n", "..", "a/b", "Upper", "default"] {
        assert!(
            service.create(hangar_server::accounts::Provider::Codex, name, |_| Ok(())).is_err(),
            "nome indevido: {name:?}"
        );
    }
    let account = service
        .create(Provider::Codex, "fresh", |_| Ok(()))
        .unwrap();
    std::fs::write(
        account.home.join(".hangar-codex-conta"),
        r#"{"version":true,"id":"fresh"}"#,
    )
    .unwrap();
    assert_eq!(service.snapshot(Provider::Codex).unwrap().len(), 1);
    assert!(service.resolve(Provider::Codex, "fresh").is_err());
    let default = service.resolve(Provider::Codex, "default").unwrap();
    assert!(service.protect_delete(Provider::Codex, &default).is_err());
    let valid = service
        .create(Provider::Codex, "valid", |_| Ok(()))
        .unwrap();
    let key = AccountKey::new(Provider::Codex, &valid.home).unwrap();
    let guard = service
        .locks
        .try_acquire(&key, GuardMode::Exclusive)
        .unwrap();
    assert!(
        service
            .delete(Provider::Codex, &valid, &guard, &UsageFacts::default())
            .is_err()
    );
    assert!(valid.home.exists());
}

#[test]
fn active_fixed_and_broken_links_cannot_be_removed_or_recreated() {
    use hangar_server::accounts::Provider;
    let root = tempfile::tempdir().unwrap();
    let mut service = isolated_service(root.path());
    let account = service
        .create(Provider::Claude, "active", |_| Ok(()))
        .unwrap();
    service.env.claude_base = account.home.clone();
    assert_eq!(
        service
            .protect_delete(Provider::Claude, &account)
            .unwrap_err()
            .code,
        "erro_conta_ativa_backend"
    );
    service.env.claude_base = root.path().join(".claude");
    service.env.claude_fixed = format!("Rótulo:{}", account.home.display());
    assert_eq!(
        service
            .protect_delete(Provider::Claude, &account)
            .unwrap_err()
            .code,
        "erro_conta_lista_fixa"
    );
    assert_eq!(service.claude_catalog().unwrap()[0]["label"], "Rótulo");
    assert_eq!(
        service
            .create(Provider::Claude, "hidden", |_| Ok(()))
            .unwrap_err()
            .code,
        "erro_config_dirs_fixo"
    );
    service.env.claude_fixed.clear();
    for provider in [Provider::Claude, Provider::Codex] {
        let path = service.target(provider, "broken");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(root.path().join("absent"), &path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path().join("absent"), &path).unwrap();
        assert!(
            service
                .create(provider, "broken", |_| panic!("link não é pasta nova"))
                .is_err()
        );
        assert!(service.resolve(provider, "broken").is_err());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn additional_environment_drops_identity_but_default_preserves_it() {
    use hangar_server::accounts::{Provider, environment::AccountEnvironment};
    let root = tempfile::tempdir().unwrap();
    let mut service = isolated_service(root.path());
    let mut base = service.env.base.clone();
    for name in [
        "OPENAI_API_KEY",
        "CODEX_AUTH_TOKEN",
        "CHATGPT_ACCESS_SECRET",
        "OPENAI_BASE_URL",
        "CODEX_SQLITE_HOME",
        "XDG_CACHE_HOME",
    ] {
        base.insert(name.into(), "synthetic".into());
    }
    base.insert("PATH".into(), "programs".into());
    base.insert(
        "CODEX_HOME".into(),
        root.path().join("captured").to_string_lossy().into(),
    );
    service.env = AccountEnvironment::from_map(base);
    let default = service.resolve(Provider::Codex, "default").unwrap();
    assert_eq!(default.home, root.path().join("captured"));
    assert!(service.env.codex(&default).contains_key("OPENAI_API_KEY"));
    let extra = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let env = service.env.codex(&extra);
    for name in [
        "OPENAI_API_KEY",
        "CODEX_AUTH_TOKEN",
        "CHATGPT_ACCESS_SECRET",
        "OPENAI_BASE_URL",
        "CODEX_SQLITE_HOME",
        "XDG_CACHE_HOME",
    ] {
        assert!(!env.contains_key(name), "variável herdada: {name}");
    }
    assert_eq!(env["PATH"], "programs");
    assert_eq!(env["CODEX_HOME"], extra.home.to_string_lossy());
}

#[test]
fn environment_matches_shared_python_golden() {
    use hangar_server::accounts::{catalog::Account, environment::AccountEnvironment};
    use std::collections::BTreeMap;
    let root = tempfile::tempdir().unwrap();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../backend/tests/fixtures/accounts_contract/environment.json"
    ))
    .unwrap();
    let materialize = |name: &str| -> BTreeMap<String, String> {
        fixture[name]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| {
                let value = value.as_str().unwrap();
                let value = if let Some(tail) = value.strip_prefix("<HOME>") {
                    root.path()
                        .join(tail.trim_start_matches('/'))
                        .to_string_lossy()
                        .into_owned()
                } else {
                    value.into()
                };
                (key.clone(), value)
            })
            .collect()
    };
    let environment = AccountEnvironment::from_map(materialize("base"));
    for (id, is_default, golden) in [("default", true, "default"), ("extra", false, "additional")] {
        let home = if is_default {
            environment.codex_default.clone()
        } else {
            root.path().join(".codex-extra")
        };
        assert_eq!(
            environment.codex(&Account {
                id: id.into(),
                home,
                is_default
            }),
            materialize(golden)
        );
    }
}

#[test]
fn deletion_removes_readonly_pack_but_keeps_external_link_target() {
    use hangar_server::accounts::{AccountKey, GuardMode, Provider, UsageFacts};
    let root = tempfile::tempdir().unwrap();
    let service = isolated_service(root.path());
    let account = service
        .create(Provider::Codex, "extra", |_| Ok(()))
        .unwrap();
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), "dados").unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside, account.home.join("linked")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, account.home.join("linked")).unwrap();
    let pack = account.home.join("pack");
    std::fs::write(&pack, "pack").unwrap();
    let mut permissions = std::fs::metadata(&pack).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&pack, permissions).unwrap();
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let guard = service
        .locks
        .try_acquire(&key, GuardMode::Exclusive)
        .unwrap();
    service
        .delete(
            Provider::Codex,
            &account,
            &guard,
            &UsageFacts {
                complete: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!account.home.exists());
    assert!(outside.join("keep").is_file());
    assert!(service.locks.path(&key).unwrap().exists());
}

#[cfg(windows)]
#[test]
fn deletion_unlinks_windows_junction_without_removing_its_target() {
    use hangar_server::accounts::{AccountKey, GuardMode, Provider, UsageFacts};
    let root = tempfile::tempdir().unwrap();
    let service = isolated_service(root.path());
    let account = service
        .create(Provider::Codex, "junction", |_| Ok(()))
        .unwrap();
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), "dados externos").unwrap();
    let junction = account.home.join("junction");
    let creation = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(creation.status.success(), "{creation:?}");
    let kind = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-Item -LiteralPath $env:HANGAR_TEST_JUNCTION).LinkType",
        ])
        .env("HANGAR_TEST_JUNCTION", &junction)
        .output()
        .unwrap();
    assert!(kind.status.success());
    assert_eq!(String::from_utf8_lossy(&kind.stdout).trim(), "Junction");
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let guard = service
        .locks
        .try_acquire(&key, GuardMode::Exclusive)
        .unwrap();
    service
        .delete(
            Provider::Codex,
            &account,
            &guard,
            &UsageFacts {
                complete: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!account.home.exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("keep")).unwrap(),
        "dados externos"
    );
}

/// O `codex` de um runner Windows frio passa às vezes do prazo de 6 s da leitura, várias vezes seguidas:
/// `unavailable` aqui é a demora do processo, não o resultado. Relê, descartando a entrada do cache,
/// por até um minuto; o que o teste mede é o isolamento da conta.
async fn read_codex_auth_settled(
    service: &hangar_server::accounts::AccountService,
    account: &hangar_server::accounts::catalog::Account,
) -> serde_json::Value {
    use hangar_server::accounts::{AccountKey, Provider};
    let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let value = service.read_codex_auth(account).await;
        if value["status"] != "unavailable" || std::time::Instant::now() >= deadline {
            return value;
        }
        service.codex_auth.invalidate(&key);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn installed_codex_reads_disconnected_account_in_isolated_home() {
    use hangar_server::accounts::Provider;
    let root = tempfile::tempdir().unwrap();
    let mut service = isolated_service(root.path());
    for (key, value) in std::env::vars() {
        if matches!(
            key.to_ascii_uppercase().as_str(),
            "PATH" | "SYSTEMROOT" | "WINDIR" | "TEMP" | "TMP"
        ) {
            service.env.base.insert(key, value);
        }
    }
    let account = service
        .create(Provider::Codex, "native", |_| Ok(()))
        .unwrap();
    service
        .env
        .base
        .insert("OPENAI_API_KEY".into(), "synthetic-parent-key".into());
    // Prepara a instalação fria sem consumir o prazo da leitura que o teste verifica.
    #[cfg(windows)]
    let mut installed = {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/d", "/c", "codex", "--version"]);
        command
    };
    #[cfg(not(windows))]
    let mut installed = {
        let mut command = tokio::process::Command::new("codex");
        command.arg("--version");
        command
    };
    let prepared = installed.env_clear().envs(service.env.codex(&account))
        .current_dir(root.path()).kill_on_drop(true).output().await.unwrap();
    assert!(prepared.status.success(), "O Codex instalado precisa estar pronto para a fixture");
    assert_eq!(
        read_codex_auth_settled(&service, &account).await,
        serde_json::json!({"method":"none","status":"disconnected","email":null,"plan":null})
    );
    std::fs::write(
        account.home.join("auth.json"),
        r#"{"OPENAI_API_KEY":"synthetic-key-only-for-account-read"}"#,
    )
    .unwrap();
    assert_eq!(
        read_codex_auth_settled(&service, &account).await,
        serde_json::json!({"method":"api_key","status":"connected","email":null,"plan":null})
    );
}

#[tokio::test]
async fn account_read_uses_existing_client_and_exposes_only_public_identity() {
    use hangar_codex::{
        client::Client,
        proto::{ClientRequest, GetAccountParams},
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (ours, theirs) = tokio::io::duplex(4096);
    let (r, w) = tokio::io::split(ours);
    let (client, _incoming) = Client::over_lines(r, w);
    let server = tokio::spawn(async move {
        let (r, mut w) = tokio::io::split(theirs);
        let mut r = tokio::io::BufReader::new(r);
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "account/read");
        assert_eq!(request["params"], json!({"refreshToken":false}));
        let response = json!({"id":request["id"],"result":{"account":{"type":"chatgpt","email":"fixture@example.invalid","planType":"plus","accessToken":"synthetic-secret"}}});
        w.write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    });
    let raw = client
        .request::<Value>(
            ClientRequest::AccountRead(GetAccountParams {
                refresh_token: false,
            }),
            std::time::Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(
        hangar_server::accounts::native::auth_public(&raw),
        json!({"method":"oauth","status":"connected","email":"fixture@example.invalid","plan":"plus"})
    );
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn http_probe_process() {
    let Ok(upstream) = std::env::var("ACCOUNT_HTTP_UPSTREAM") else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(Config {
        listen: addr,
        upstream: upstream.parse().unwrap(),
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    println!("ACCOUNT_HTTP:{addr}");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    hangar_server::serve_until_with_state(listener, state, std::future::pending::<()>())
        .await
        .unwrap();
}

#[test]
fn account_guard_remains_held_until_parent_ownership_ends() {
    use hangar_server::accounts::{AccountKey, AccountLocks, GuardMode, LockError, Provider};
    let root = tempfile::tempdir().unwrap();
    let key = AccountKey::new(Provider::Codex, root.path()).unwrap();
    let locks = AccountLocks::new(root.path().join("locks"));
    for mode in [GuardMode::Shared, GuardMode::Exclusive] {
        let guard = locks.try_acquire(&key, mode).unwrap();
        assert!(
            matches!(
                locks.try_acquire(&key, GuardMode::Exclusive),
                Err(LockError::Busy)
            ),
            "guarda viva deixou outra operação adquirir Exclusive"
        );
        drop(guard);
        assert!(
            locks.try_acquire(&key, GuardMode::Exclusive).is_ok(),
            "término da guarda não liberou Exclusive"
        );
    }
}

#[cfg(target_os = "linux")]
struct PreExecGuardChild {
    pid: libc::pid_t,
    release: std::fs::File,
}

#[cfg(target_os = "linux")]
impl PreExecGuardChild {
    fn spawn(lock_path: &std::path::Path, close_in_child: bool) -> Self {
        use std::{
            io::Read,
            os::fd::{AsRawFd, FromRawFd},
        };
        let lock_fd = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| std::fs::read_link(entry.path()).ok().as_deref() == Some(lock_path))
            .unwrap()
            .file_name()
            .to_str()
            .unwrap()
            .parse::<libc::c_int>()
            .unwrap();
        assert_ne!(
            unsafe { libc::fcntl(lock_fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let channel = || {
            let mut fds = [-1; 2];
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            unsafe {
                (
                    std::fs::File::from_raw_fd(fds[0]),
                    std::fs::File::from_raw_fd(fds[1]),
                )
            }
        };
        let (mut ready_read, ready_write) = channel();
        let (release_read, release_write) = channel();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // O filho só fecha descritores; não executa Drop de guarda herdada.
            unsafe {
                libc::close(ready_read.as_raw_fd());
                libc::close(release_write.as_raw_fd());
                if close_in_child && libc::close(lock_fd) != 0 {
                    libc::_exit(2);
                }
                if libc::write(ready_write.as_raw_fd(), b"R".as_ptr().cast(), 1) != 1 {
                    libc::_exit(3);
                }
                let mut signal = 0_u8;
                let received =
                    libc::read(release_read.as_raw_fd(), (&mut signal as *mut u8).cast(), 1);
                libc::_exit(if received == 1 && signal == b'X' {
                    0
                } else {
                    4
                });
            }
        }
        drop(ready_write);
        drop(release_read);
        let child = Self {
            pid,
            release: release_write,
        };
        let mut ready = [0];
        ready_read.read_exact(&mut ready).unwrap();
        assert_eq!(ready, *b"R");
        child
    }
}

#[cfg(target_os = "linux")]
impl Drop for PreExecGuardChild {
    fn drop(&mut self) {
        use std::io::Write;
        let released = self.release.write_all(b"X");
        let mut status = -1;
        let waited = unsafe { libc::waitpid(self.pid, &mut status, 0) };
        if !std::thread::panicking() {
            assert!(released.is_ok(), "não foi possível liberar o filho próprio");
            assert_eq!(waited, self.pid, "filho próprio não foi recolhido");
            assert_eq!(status, 0, "filho próprio não encerrou normalmente");
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn account_guard_drop_releases_lock_while_pre_exec_child_is_alive() {
    use hangar_server::accounts::{AccountKey, AccountLocks, GuardMode, Provider};
    let root = tempfile::tempdir().unwrap();
    let key = AccountKey::new(Provider::Codex, root.path()).unwrap();
    let locks = AccountLocks::new(root.path().join("locks"));
    let guard = locks.try_acquire(&key, GuardMode::Shared).unwrap();
    let child = PreExecGuardChild::spawn(&locks.path(&key).unwrap(), false);
    drop(guard);
    let exclusive = locks.try_acquire(&key, GuardMode::Exclusive);
    assert!(
        exclusive.is_ok(),
        "término da guarda no pai deixou Exclusive ocupada pelo descritor herdado pré-exec: {exclusive:?}"
    );
    drop(exclusive);
    drop(child);
}

#[cfg(target_os = "linux")]
#[test]
fn account_guard_child_close_preserves_live_parent_ownership() {
    use hangar_server::accounts::{AccountKey, AccountLocks, GuardMode, LockError, Provider};
    let root = tempfile::tempdir().unwrap();
    let key = AccountKey::new(Provider::Codex, root.path()).unwrap();
    let locks = AccountLocks::new(root.path().join("locks"));
    let guard = locks.try_acquire(&key, GuardMode::Shared).unwrap();
    let child = PreExecGuardChild::spawn(&locks.path(&key).unwrap(), true);
    assert!(
        matches!(
            locks.try_acquire(&key, GuardMode::Exclusive),
            Err(LockError::Busy)
        ),
        "fechar descritor no filho liberou a guarda ainda viva do pai"
    );
    drop(guard);
    assert!(locks.try_acquire(&key, GuardMode::Exclusive).is_ok());
    drop(child);
}

#[tokio::test]
async fn accounts_named_like_subroutes_reach_deletion() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let python = tokio::spawn(async move {
        axum::serve(
            upstream,
            Router::new().fallback(|| async { (StatusCode::SERVICE_UNAVAILABLE, "sem fatos") }),
        )
        .await
        .unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: upstream_addr,
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = isolated_service(root.path());
    for name in ["login", "prepare", "rate-limit-reset"] {
        state
            .accounts
            .create(hangar_server::accounts::Provider::Codex, name, |_| Ok(()))
            .unwrap();
    }
    let server = tokio::spawn(hangar_server::routes::serve_with_state(listener, state));
    for name in ["login", "prepare", "rate-limit-reset"] {
        let response = reqwest::Client::new()
            .delete(format!("http://{addr}/api/codex-contas/{name}"))
            .bearer_auth("contract-only")
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: serde_json::Value =
            serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or_default();
        // Sem fatos de uso a exclusão recusa; o que importa é chegar nela e não numa sub-rota.
        assert_eq!(status, 409, "DELETE da conta {name} caiu em outra rota: {body}");
        assert!(
            matches!(
                body["detail"]["code"].as_str(),
                Some("account_usage_unknown" | "codex_account_in_use")
            ),
            "DELETE da conta {name} não chegou à exclusão: {body}"
        );
        assert!(root.path().join(format!(".codex-{name}")).exists());
    }
    server.abort();
    python.abort();
}

#[tokio::test]
async fn account_failure_reaches_the_exportable_journal() {
    let (journal, mut entries) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let python = tokio::spawn(async move {
        axum::serve(
            upstream,
            Router::new()
                .route(
                    "/internal/diag",
                    axum::routing::post(move |body: axum::body::Bytes| {
                        journal.send(serde_json::from_slice(&body).unwrap()).unwrap();
                        async { StatusCode::OK }
                    }),
                )
                .fallback(|| async { (StatusCode::SERVICE_UNAVAILABLE, "sem fatos") }),
        )
        .await
        .unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: upstream_addr,
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = isolated_service(root.path());
    state
        .accounts
        .create(hangar_server::accounts::Provider::Codex, "journal", |_| Ok(()))
        .unwrap();
    let server = tokio::spawn(hangar_server::routes::serve_with_state(listener, state));
    let response = reqwest::Client::new()
        .delete(format!("http://{addr}/api/codex-contas/journal"))
        .bearer_auth("contract-only")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let entry = tokio::time::timeout(std::time::Duration::from_secs(5), entries.recv())
        .await
        .expect("a falha da conta não chegou ao diário")
        .unwrap();
    assert_eq!(entry["evento"], "rust.accounts_failed");
    assert_eq!(entry["sessao"], "codex:journal");
    assert_eq!(entry["codigo"], "account_usage_unknown");
    server.abort();
    python.abort();
}

/// Connect e convidado chegam ao Python; contas ainda têm um escritor só, o Rust, pela ponte.
#[tokio::test]
async fn python_ports_reach_the_account_owner_through_the_private_bridge() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let python = tokio::spawn(async move {
        axum::serve(upstream, Router::new().fallback(|| async { StatusCode::SERVICE_UNAVAILABLE }))
            .await
            .unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let private = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = private.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: addr,
        upstream: upstream_addr,
        internal_secret: "contract-internal".into(),
        auth_token: "contract-only".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = isolated_service(root.path());
    let app = hangar_server::routes::terminal_router(Arc::new(state));
    let server = tokio::spawn(async move {
        axum::serve(private, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await
            .unwrap()
    });
    let send = |path: &'static str, secret: Option<&'static str>| {
        let mut request = reqwest::Client::new()
            .post(format!("http://{addr}/__hangar_server/accounts/public"))
            .header("x-hangar-path", path)
            .header("content-type", "application/json")
            .body(r#"{"name":"via-connect"}"#);
        if let Some(secret) = secret {
            request = request.header("x-hangar-internal", secret);
        }
        async move {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            let body: serde_json::Value =
                serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or_default();
            (status, body)
        }
    };
    let (status, body) = send("/api/codex-contas", Some("contract-internal")).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["id"], "via-connect");
    assert!(root.path().join(".codex-via-connect").is_dir());
    let (status, body) = send("/api/credenciais", Some("contract-internal")).await;
    assert_eq!((status, body["code"].as_str()), (404, Some("account_route_not_owned")));
    let (status, _) = send("/api/codex-contas", None).await;
    assert_eq!(status, 404);
    server.abort();
    python.abort();
}
