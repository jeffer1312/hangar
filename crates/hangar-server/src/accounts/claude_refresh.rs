use super::{
    AccountKey, GuardMode, Provider,
    catalog::{Account, AccountService},
    claude_login::WindowClient,
    quotas::now,
};
use serde_json::Value;
use std::{fs, path::Path, sync::Arc, time::Duration};

#[derive(Clone, Default)]
pub struct PendingCleanup(
    Arc<tokio::sync::Mutex<std::collections::HashMap<AccountKey, (String, Option<super::AccountGuard>)>>>,
);

/// Renovações em curso, por conta. A sessão que vai abrir numa delas manda parar
/// (`yield_to_session`): a sessão renova o token sozinha, e abrir sessão nunca espera a renovação.
static RENEWALS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<AccountKey, Arc<tokio::sync::Notify>>>> =
    std::sync::LazyLock::new(Default::default);

struct Renewal {
    key: AccountKey,
    stop: Arc<tokio::sync::Notify>,
}
impl Renewal {
    fn start(key: &AccountKey) -> Self {
        let stop = Arc::new(tokio::sync::Notify::new());
        RENEWALS.lock().unwrap_or_else(|p| p.into_inner()).insert(key.clone(), stop.clone());
        Self { key: key.clone(), stop }
    }
}
impl Drop for Renewal {
    fn drop(&mut self) {
        let mut map = RENEWALS.lock().unwrap_or_else(|p| p.into_inner());
        if map.get(&self.key).is_some_and(|stop| Arc::ptr_eq(stop, &self.stop)) {
            map.remove(&self.key);
        }
    }
}

/// Manda a renovação desta conta, se houver, soltar a trava agora.
pub fn yield_to_session(key: &AccountKey) {
    if let Some(stop) = RENEWALS.lock().unwrap_or_else(|p| p.into_inner()).get(key) {
        stop.notify_one();
    }
}

/// A guarda compartilhada de quem vai subir uma sessão na conta: a renovação cede a cada tentativa.
pub async fn acquire_for_session(
    locks: &super::AccountLocks,
    key: &AccountKey,
    deadline: std::time::Instant,
) -> Result<super::AccountGuard, super::LockError> {
    loop {
        yield_to_session(key);
        match locks.try_acquire(key, GuardMode::Shared) {
            Err(super::LockError::Busy) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            result => return result,
        }
    }
}

/// Renovação rápida cujo CLI rodou e não renovou espera antes de rodar de novo: ele sobe o
/// Claude com todos os MCPs (segundos no Windows) e a lista de credenciais esperava por ele.
/// Credencial trocada (login novo) ou renovação completa libera na hora.
#[derive(Clone, Default)]
pub struct QuickRenewalBackoff(
    Arc<std::sync::Mutex<std::collections::HashMap<AccountKey, (Value, f64, u32)>>>,
);

impl QuickRenewalBackoff {
    const WAITS: [f64; 4] = [1800.0, 3600.0, 7200.0, 21600.0];

    pub fn pending(&self, key: &AccountKey, credential: &Value, at: f64) -> bool {
        let map = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.get(key)
            .is_some_and(|(seen, until, _)| seen == credential && at < *until)
    }

    /// Devolve a espera aplicada, em segundos.
    pub fn failed(&self, key: &AccountKey, credential: Value, at: f64) -> f64 {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let attempts = map
            .get(key)
            .filter(|entry| entry.0 == credential)
            .map_or(0, |entry| entry.2);
        let wait = Self::WAITS[(attempts as usize).min(Self::WAITS.len() - 1)];
        map.insert(key.clone(), (credential, at + wait, attempts + 1));
        wait
    }

    pub fn clear(&self, key: &AccountKey) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }
}

fn oauth(path: &Path) -> Value {
    fs::read(path.join(".credentials.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|v| v["claudeAiOauth"].clone())
        .unwrap_or(Value::Null)
}
/// A conta tem com o que subir o Claude: token de acesso no prazo, ou de renovação ainda válido. No
/// macOS a credencial mora no Keychain, fora do arquivo: lá não dá para afirmar a falta.
pub fn has_login(home: &Path) -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    let auth = oauth(home);
    let live = |field: &str| auth[field].as_f64().is_none_or(|ms| ms / 1000.0 > now());
    let present = |field: &str| auth[field].as_str().is_some_and(|s| !s.is_empty());
    (present("accessToken") && live("expiresAt"))
        || (present("refreshToken") && live("refreshTokenExpiresAt"))
}

pub fn renewed(before: &Value, after: &Value) -> bool {
    let Some(expiry) = after["expiresAt"].as_f64() else {
        return false;
    };
    before != after && before["expiresAt"].as_f64().is_none_or(|old| expiry > old)
}

impl AccountService {
    pub async fn refresh_claude(
        &self,
        account: &Account,
        windows: WindowClient,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        quick: bool,
    ) -> Result<(), &'static str> {
        let service = self.clone();
        let account = account.clone();
        tokio::spawn(async move {
            service
                .refresh_claude_owned(account, windows, runtime, quick)
                .await
        })
        .await
        .unwrap_or(Err("renovacao-falhou"))
    }

    async fn refresh_claude_owned(
        &self,
        account: Account,
        client: WindowClient,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        quick: bool,
    ) -> Result<(), &'static str> {
        let before = oauth(&account.home);
        if !before["refreshToken"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
            || before["refreshTokenExpiresAt"]
                .as_f64()
                .is_some_and(|ms| ms / 1000.0 <= now())
        {
            return Err("login-necessario");
        }
        if quick && account.is_default {
            return Err("sessao-viva");
        }
        let key =
            AccountKey::new(Provider::Claude, &account.home).map_err(|_| "renovacao-falhou")?;
        if quick && self.quick_renewals.pending(&key, &before, now()) {
            return Err("renovacao-falhou");
        }
        // Limpeza incerta conserva a guarda; a próxima tentativa só prossegue após o ACK.
        {
            let mut pending = self.refresh_cleanup.0.lock().await;
            if let Some((operation, _)) = pending.get(&key) {
                client
                    .call(&key, operation, "close", None)
                    .await
                    .map_err(|_| "renovacao-falhou")?;
                pending.remove(&key);
                if let Ok(record) = self.login_record(&key) {
                    let _ = fs::remove_file(record);
                }
            }
        }
        let guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| "sessao-viva")?;
        // Só quem segura a conta se registra: a perdedora de uma disputa apagaria o registro da vencedora.
        let renewal = Renewal::start(&key);
        let window = Arc::new(std::sync::Mutex::new(None::<String>));
        let work = self.renew_locked(account, key.clone(), guard, client.clone(), quick, runtime, window.clone());
        tokio::select! { biased;
            result = work => result,
            _ = renewal.stop.notified() => {
                tracing::info!(code = "refresh_yielded", "renovação cedeu a conta para uma sessão; a sessão renova o token");
                let operation = window.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some(operation) = operation {
                    self.close_yielded(client, key, operation);
                }
                Err("cedeu-sessao")
            }
        }
    }

    /// Fecha a janela de uma renovação interrompida sem segurar a conta; falhando, segue tentando
    /// como a limpeza estacionada, ainda sem a trava.
    fn close_yielded(&self, client: WindowClient, key: AccountKey, operation: String) {
        let service = self.clone();
        tokio::spawn(async move {
            let Ok(record) = service.login_record(&key) else { return };
            if client.call(&key, &operation, "close", None).await.is_ok() {
                let _ = fs::remove_file(record);
            } else {
                service.park_cleanup(client, key, operation, None, record).await;
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn renew_locked(
        &self,
        account: Account,
        key: AccountKey,
        _guard: super::AccountGuard,
        client: WindowClient,
        quick: bool,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        window: Arc<std::sync::Mutex<Option<String>>>,
    ) -> Result<(), &'static str> {
        self.validate_claude(&account, &key)
            .map_err(|_| "renovacao-falhou")?;
        self.usage(runtime.as_deref(), &key)
            .await
            .ensure_unused()
            .map_err(|_| "sessao-viva")?;
        let before = oauth(&account.home);
        let cli = self.claude_cli(&account, &["mcp", "list"]).await;
        if renewed(&before, &oauth(&account.home)) {
            self.quick_renewals.clear(&key);
            self.claude_auth.invalidate(&key);
            return Ok(());
        }
        if quick {
            // CLI que nem subiu (ausente, processo recusado) não custou nada: a espera fica para o que rodou.
            if let Err(code @ ("cli_missing" | "cli_not_started")) = cli {
                tracing::warn!(code, "renovação rápida sem o CLI do Claude; nada de espera");
            } else {
                let wait = self.quick_renewals.failed(&key, before, now());
                tracing::warn!(
                    code = "quick_renewal_backoff",
                    wait_s = wait,
                    "renovação rápida não renovou; próxima tentativa automática após a espera"
                );
            }
            return Err("renovacao-falhou");
        }
        let operation = super::claude_login::nonce().map_err(|_| "renovacao-falhou")?;
        let record = self.login_record(&key).map_err(|_| "renovacao-falhou")?;
        crate::runtime::queue::atomic_write(
            &record,
            serde_json::json!({"key":key,"operation":operation})
                .to_string()
                .as_bytes(),
        )
        .map_err(|_| "renovacao-falhou")?;
        *window.lock().unwrap_or_else(|p| p.into_inner()) = Some(operation.clone());
        if client
            .call(&key, &operation, "refresh", None)
            .await
            .is_err()
        {
            if client.call(&key, &operation, "close", None).await.is_err() {
                self.park_cleanup(client, key, operation, Some(_guard), record)
                    .await;
            } else {
                let _ = fs::remove_file(record);
            }
            return Err("renovacao-falhou");
        }
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let success = loop {
            if renewed(&before, &oauth(&account.home)) {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            timer.tick().await;
        };
        if client.call(&key, &operation, "close", None).await.is_err() {
            self.park_cleanup(client, key, operation, Some(_guard), record)
                .await;
            return Err("renovacao-falhou");
        }
        let _ = fs::remove_file(record);
        self.claude_auth.invalidate(&key);
        if success {
            self.quick_renewals.clear(&key);
        }
        if success { Ok(()) } else { Err("timeout") }
    }

    /// A guarda segue presa até o ACK do fechamento, mas o fechamento é retentado em segundos:
    /// esperar a próxima rodada deixava a conta recusando sessão nova por horas.
    async fn park_cleanup(
        &self,
        client: WindowClient,
        key: AccountKey,
        operation: String,
        guard: Option<super::AccountGuard>,
        record: std::path::PathBuf,
    ) {
        tracing::warn!(
            code = "refresh_cleanup_pending",
            locked = guard.is_some(),
            "janela da renovação não fechou; o fechamento segue sendo tentado"
        );
        let pending = self.refresh_cleanup.0.clone();
        pending
            .lock()
            .await
            .insert(key.clone(), (operation.clone(), guard));
        let service = self.clone();
        tokio::spawn(async move {
            let renewal = Renewal::start(&key);
            for (attempt, wait) in [5, 15, 30, 60].into_iter().chain(std::iter::repeat(300)).enumerate() {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {},
                    _ = renewal.stop.notified() => {
                        if let Some(entry) = pending.lock().await.get_mut(&key).filter(|entry| entry.0 == operation) {
                            entry.1 = None;
                            tracing::info!(code = "refresh_cleanup_yielded", "conta liberada para uma sessão; a janela segue sendo fechada sem a trava");
                        }
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                    }
                }
                let still_parked = |map: &std::collections::HashMap<AccountKey, (String, Option<super::AccountGuard>)>| {
                    map.get(&key).is_some_and(|(op, _)| *op == operation)
                };
                if !still_parked(&*pending.lock().await) {
                    return;
                }
                if let Err(error) = client.call(&key, &operation, "close", None).await {
                    tracing::warn!(
                        code = "refresh_cleanup_retry_failed",
                        reason = error.code,
                        attempt = attempt + 1,
                        "janela da renovação ainda não fechou; conta segue travada"
                    );
                    continue;
                }
                let mut map = pending.lock().await;
                if still_parked(&map) {
                    map.remove(&key);
                    let _ = fs::remove_file(&record);
                    service.claude_auth.invalidate(&key);
                    tracing::info!(code = "refresh_cleanup_done", "janela da renovação fechada; conta liberada");
                }
                return;
            }
        });
    }
}

pub fn start(
    service: AccountService,
    runtime: Arc<crate::runtime::gateway::RuntimeRegistry>,
) -> (
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        // ponytail: espera fixa de 2 min depois da subida.
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(120),
            Duration::from_secs(6 * 3600),
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased;
                _=stopped.changed()=>break,
                _=interval.tick()=>{},
            }
            let Ok(accounts) = service.claude_accounts() else {
                continue;
            };
            for (_, account) in accounts {
                if *stopped.borrow() {
                    break;
                }
                if !account.is_default
                    && !super::storage::real_file(&account.home.join(super::storage::CLAUDE_MARKER))
                {
                    continue;
                }
                if oauth(&account.home)["expiresAt"]
                    .as_f64()
                    .is_none_or(|ms| ms / 1000.0 - now() > 5400.0)
                {
                    continue;
                }
                if let Err(reason) = service
                    .refresh_claude(&account, service.claude_windows.clone(), Some(runtime.clone()), false)
                    .await
                {
                    tracing::info!(code = reason, "renovação de conta não concluída");
                }
            }
        }
    });
    (stop, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn changing_file_without_extending_expiry_is_not_renewal() {
        let old = json!({"accessToken":"old","expiresAt":1000});
        assert!(!renewed(
            &old,
            &json!({"accessToken":"other","expiresAt":1000})
        ));
        assert!(!renewed(&old, &json!({"accessToken":"other"})));
        assert!(renewed(
            &old,
            &json!({"accessToken":"new","expiresAt":2000})
        ));
    }

    #[test]
    fn failed_quick_renewal_waits_until_the_credential_changes_or_the_wait_ends() {
        let temp = tempfile::tempdir().unwrap();
        let key = AccountKey::new(Provider::Claude, temp.path()).unwrap();
        let backoff = QuickRenewalBackoff::default();
        let old = json!({"refreshToken":"a"});
        assert!(!backoff.pending(&key, &old, 1000.0));
        assert_eq!(backoff.failed(&key, old.clone(), 1000.0), 1800.0);
        assert!(backoff.pending(&key, &old, 1300.0));
        assert!(!backoff.pending(&key, &json!({"refreshToken":"b"}), 1300.0));
        assert!(!backoff.pending(&key, &old, 2800.0));
        assert_eq!(backoff.failed(&key, old.clone(), 3000.0), 3600.0);
        assert!(backoff.pending(&key, &old, 6599.0));
        assert!(!backoff.pending(&key, &old, 6600.0));
        backoff.clear(&key);
        assert!(!backoff.pending(&key, &old, 3000.0));
        assert_eq!(backoff.failed(&key, old, 3000.0), 1800.0);
    }

    /// A sessão que pede a conta faz a renovação soltar a trava na hora; sem renovação, outra
    /// trava exclusiva (exclusão, login) continua valendo.
    #[tokio::test]
    async fn a_session_takes_the_account_from_a_renewal_at_once() {
        let temp = tempfile::tempdir().unwrap();
        let locks = super::super::AccountLocks::new(temp.path().join("locks"));
        let key = AccountKey::new(Provider::Claude, temp.path()).unwrap();
        let renewal = Renewal::start(&key);
        let guard = locks.try_acquire(&key, GuardMode::Exclusive).unwrap();
        let task = tokio::spawn(async move {
            renewal.stop.notified().await;
            drop(guard);
        });
        let started = std::time::Instant::now();
        acquire_for_session(&locks, &key, started + Duration::from_secs(5)).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        task.await.unwrap();
        assert!(!RENEWALS.lock().unwrap().contains_key(&key), "a renovação encerrada sai do registro");

        let other = locks.try_acquire(&key, GuardMode::Exclusive);
        let other = other.unwrap();
        assert!(matches!(
            acquire_for_session(&locks, &key, std::time::Instant::now() + Duration::from_millis(100)).await,
            Err(super::super::LockError::Busy)
        ));
        drop(other);
    }

    /// Renovação de verdade, presa no CLI: a sessão leva a conta e a renovação responde que cedeu.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_running_renewal_yields_to_a_session() {
        let temp = tempfile::tempdir().unwrap();
        let account_home = temp.path().join(".claude-test");
        let bin = temp.path().join("bin");
        fs::create_dir(&account_home).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(account_home.join(".credentials.json"),
            json!({"claudeAiOauth":{"accessToken":"before","refreshToken":"refresh","expiresAt":1000}}).to_string()).unwrap();
        // Caminho absoluto: o `PATH` da CLI é só esta pasta, e sem o `sleep` ela sairia antes de a sessão pedir a conta.
        crate::write_test_executable(&bin.join("claude"), b"#!/bin/sh\nexec /bin/sleep 30\n");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, axum::Router::new()).await.unwrap(); });
        let mut env = super::super::environment::AccountEnvironment::from_map(Default::default());
        env.home = temp.path().into();
        env.claude_base = temp.path().join(".claude");
        env.codex_default = temp.path().join(".codex");
        env.claude_fixed = format!("test:{}", account_home.display());
        env.base.insert("PATH".into(), bin.to_string_lossy().into_owned());
        // Sem o tmux da máquina: o resultado não pode depender do que roda nela.
        env.terminal_fixture = Some(Arc::new(|| Ok(String::new())));
        let service = AccountService::new(env);
        let account = service.claude_by_label("test").unwrap();
        let windows = WindowClient::with(Arc::new(super::super::claude_window::Refused));
        let runtime = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(address, "synthetic".into(), "synthetic".into()));
        let key = AccountKey::new(Provider::Claude, &account_home).unwrap();
        let locks = service.locks.clone();
        let renewing = {
            let service = service.clone();
            tokio::spawn(async move { service.refresh_claude(&account, windows, Some(runtime), true).await })
        };
        while !RENEWALS.lock().unwrap().contains_key(&key) {
            assert!(!renewing.is_finished(), "a renovação acabou sem segurar a conta");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let started = std::time::Instant::now();
        acquire_for_session(&locks, &key, started + Duration::from_secs(5)).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        assert_eq!(renewing.await.unwrap(), Err("cedeu-sessao"));
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_cli_renews_only_when_account_is_unused() {
        let temp = tempfile::tempdir().unwrap();
        let account_home = temp.path().join(".claude-test");
        let bin = temp.path().join("bin");
        fs::create_dir(&account_home).unwrap();
        fs::create_dir(&bin).unwrap();
        let old = json!({"claudeAiOauth":{"accessToken":"before","refreshToken":"refresh","expiresAt":1000}});
        fs::write(account_home.join(".credentials.json"), old.to_string()).unwrap();
        let cli = bin.join("claude");
        crate::write_test_executable(&cli, b"#!/bin/sh\nprintf '%s' '{\"claudeAiOauth\":{\"accessToken\":\"after\",\"refreshToken\":\"refresh\",\"expiresAt\":20000000000000}}' > \"$CLAUDE_CONFIG_DIR/.credentials.json\"\n");
        // Uso de verdade: sidecar de sessão sem terminal na conta. Os fatos nunca vêm do Python.
        let sidecars = temp.path().join(".hangar/claude-headless");
        fs::create_dir_all(&sidecars).unwrap();
        fs::write(sidecars.join("busy.json"), json!({"name":"busy","config_dir":account_home}).to_string()).unwrap();
        let python_facts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = python_facts.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app=axum::Router::new().route("/internal/accounts/facts",axum::routing::post(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut env = super::super::environment::AccountEnvironment::from_map(Default::default());
        env.home = temp.path().into();
        env.claude_base = temp.path().join(".claude");
        env.codex_default = temp.path().join(".codex");
        env.claude_fixed = format!("test:{}", account_home.display());
        env.base
            .insert("PATH".into(), bin.to_string_lossy().into_owned());
        // Sem o tmux da máquina: o resultado não pode depender do que roda nela.
        env.terminal_fixture = Some(Arc::new(|| Ok(String::new())));
        let service = AccountService::new(env);
        let account = service.claude_by_label("test").unwrap();
        let windows = WindowClient::with(Arc::new(super::super::claude_window::Refused));
        let runtime = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(
            address,
            "synthetic".into(),
            "synthetic".into(),
        ));
        assert_eq!(
            service
                .refresh_claude(&account, windows.clone(), Some(runtime.clone()), true)
                .await,
            Err("sessao-viva")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(account_home.join(".credentials.json")).unwrap()
            )
            .unwrap(),
            old
        );
        fs::remove_file(sidecars.join("busy.json")).unwrap();
        assert_eq!(
            service
                .refresh_claude(&account, windows, Some(runtime), true)
                .await,
            Ok(())
        );
        assert_eq!(oauth(&account_home)["accessToken"], "after");
        let key = AccountKey::new(Provider::Claude, &account_home).unwrap();
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_ok()
        );
        assert_eq!(python_facts.load(std::sync::atomic::Ordering::SeqCst), 0, "fatos de uso pedidos ao Python");
        server.abort();
    }
}
