use super::{
    AccountKey, GuardMode, Provider,
    bridge::AccountsBridge,
    catalog::{Account, AccountService},
    claude_login::WindowClient,
    quotas::now,
};
use serde_json::Value;
use std::{fs, path::Path, sync::Arc, time::Duration};

#[derive(Clone, Default)]
pub struct PendingCleanup(
    Arc<tokio::sync::Mutex<std::collections::HashMap<AccountKey, (String, super::AccountGuard)>>>,
);

fn oauth(path: &Path) -> Value {
    fs::read(path.join(".credentials.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|v| v["claudeAiOauth"].clone())
        .unwrap_or(Value::Null)
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
        bridge: AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        quick: bool,
    ) -> Result<(), &'static str> {
        let service = self.clone();
        let account = account.clone();
        tokio::spawn(async move {
            service
                .refresh_claude_owned(account, bridge, runtime, quick)
                .await
        })
        .await
        .unwrap_or(Err("renovacao-falhou"))
    }

    async fn refresh_claude_owned(
        &self,
        account: Account,
        bridge: AccountsBridge,
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
        let client = WindowClient::new(
            bridge.upstream(),
            bridge.secret().to_owned(),
            bridge.instance().to_owned(),
        )
        .map_err(|_| "renovacao-falhou")?;
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
        let _guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| "sessao-viva")?;
        self.validate_claude(&account, &key)
            .map_err(|_| "renovacao-falhou")?;
        self.usage(&bridge, runtime.as_deref(), &key)
            .await
            .ensure_unused()
            .map_err(|_| "sessao-viva")?;
        let before = oauth(&account.home);
        let _ = self.claude_cli(&account, &["mcp", "list"]).await;
        if renewed(&before, &oauth(&account.home)) {
            self.claude_auth.invalidate(&key);
            return Ok(());
        }
        if quick {
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
        if client
            .call(&key, &operation, "refresh", None)
            .await
            .is_err()
        {
            if client.call(&key, &operation, "close", None).await.is_err() {
                self.park_cleanup(client, key, operation, _guard, record)
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
            self.park_cleanup(client, key, operation, _guard, record)
                .await;
            return Err("renovacao-falhou");
        }
        let _ = fs::remove_file(record);
        self.claude_auth.invalidate(&key);
        if success { Ok(()) } else { Err("timeout") }
    }

    /// A guarda segue presa até o ACK do fechamento, mas o fechamento é retentado em segundos:
    /// esperar a próxima rodada deixava a conta recusando sessão nova por horas.
    async fn park_cleanup(
        &self,
        client: WindowClient,
        key: AccountKey,
        operation: String,
        guard: super::AccountGuard,
        record: std::path::PathBuf,
    ) {
        tracing::warn!(
            code = "refresh_cleanup_pending",
            "janela da renovação não fechou; conta segue travada até o fechamento"
        );
        let pending = self.refresh_cleanup.0.clone();
        pending
            .lock()
            .await
            .insert(key.clone(), (operation.clone(), guard));
        let service = self.clone();
        tokio::spawn(async move {
            for (attempt, wait) in [5, 15, 30, 60].into_iter().chain(std::iter::repeat(300)).enumerate() {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                let still_parked = |map: &std::collections::HashMap<AccountKey, (String, super::AccountGuard)>| {
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
    bridge: AccountsBridge,
    runtime: Arc<crate::runtime::gateway::RuntimeRegistry>,
) -> (
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        // ponytail: espera fixa; na subida o Python fica ~30 s sem atender e a renovação falhava.
        // Trocar por um sinal de prontidão do Python se a subida passar disso.
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
                    .refresh_claude(&account, bridge.clone(), Some(runtime.clone()), false)
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

    #[cfg(unix)]
    #[tokio::test]
    async fn native_cli_renews_only_when_account_is_unused() {
        use super::super::http::Json;
        
        let temp = tempfile::tempdir().unwrap();
        let account_home = temp.path().join(".claude-test");
        let bin = temp.path().join("bin");
        fs::create_dir(&account_home).unwrap();
        fs::create_dir(&bin).unwrap();
        let old = json!({"claudeAiOauth":{"accessToken":"before","refreshToken":"refresh","expiresAt":1000}});
        fs::write(account_home.join(".credentials.json"), old.to_string()).unwrap();
        let cli = bin.join("claude");
        crate::write_test_executable(&cli, b"#!/bin/sh\nprintf '%s' '{\"claudeAiOauth\":{\"accessToken\":\"after\",\"refreshToken\":\"refresh\",\"expiresAt\":20000000000000}}' > \"$CLAUDE_CONFIG_DIR/.credentials.json\"\n");
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = busy.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app=axum::Router::new().route("/internal/accounts/facts",axum::routing::post(move |body:bytes::Bytes| {
            let flag=flag.clone();
            async move {
                let body:Value=serde_json::from_slice(&body).unwrap();
                Json(json!([{"key":body["keys"][0],"facts":{"complete":true,
                    "sessions":if flag.load(std::sync::atomic::Ordering::SeqCst) {vec!["busy"]} else {vec![]},"pids":[]}}]))
            }
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
        let service = AccountService::new(env);
        let account = service.claude_by_label("test").unwrap();
        let bridge = AccountsBridge::new(address, "synthetic".into(), "synthetic".into()).unwrap();
        let runtime = Arc::new(crate::runtime::gateway::RuntimeRegistry::new(
            address,
            "synthetic".into(),
            "synthetic".into(),
        ));
        assert_eq!(
            service
                .refresh_claude(&account, bridge.clone(), Some(runtime.clone()), true)
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
        busy.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            service
                .refresh_claude(&account, bridge, Some(runtime), true)
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
        server.abort();
    }
}
