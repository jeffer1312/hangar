//! Tentativas por operação; a limpeza conserva exclusividade até o auxiliar parar.
use super::{
    AccountGuard, AccountKey, GuardMode, Provider,
    catalog::{Account, AccountError, AccountService},
    claude_auth,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs, io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Clone, Default)]
pub struct ClaudeLogins {
    attempts: Arc<AsyncMutex<HashMap<AccountKey, Arc<Attempt>>>>,
    gates: super::locks::KeyedGates,
}
struct Attempt {
    account: Account,
    key: AccountKey,
    operation: String,
    old_token: Option<Vec<u8>>,
    guard: Mutex<Option<AccountGuard>>,
}
pub fn nonce() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| io::Error::other("operação indisponível"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
#[derive(Clone)]
pub struct WindowClient {
    client: reqwest::Client,
    upstream: SocketAddr,
    secret: String,
    instance: String,
}
impl WindowClient {
    pub fn new(
        upstream: SocketAddr,
        secret: String,
        instance: String,
    ) -> Result<Self, AccountError> {
        if !upstream.ip().is_loopback() || instance.is_empty() {
            return Err(AccountError::io());
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| AccountError::io())?,
            upstream,
            secret,
            instance,
        })
    }
    pub(super) async fn call(
        &self,
        key: &AccountKey,
        operation: &str,
        action: &str,
        code: Option<&str>,
    ) -> Result<Value, AccountError> {
        let response = self.client.post(format!("http://{}/internal/accounts/claude-window", self.upstream))
            .header("x-hangar-internal", &self.secret).header("x-hangar-runtime-instance", &self.instance)
            .header("content-type", "application/json")
            .body(json!({"instance":self.instance,"key":key,"operation":operation,"action":action,"code":code}).to_string())
            .send().await.map_err(|_| bridge_error())?;
        if !response.status().is_success() {
            return Err(bridge_error());
        }
        let bytes = response.bytes().await.map_err(|_| bridge_error())?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| bridge_error())?;
        if value["ok"] != true {
            return Err(bridge_error());
        }
        Ok(value)
    }
}
fn bridge_error() -> AccountError {
    AccountError::new(
        409,
        "erro_login_ja_em_curso",
        "não foi possível controlar a janela de login",
        json!({}),
    )
}
fn cancelled(label: &str) -> AccountError {
    AccountError::new(
        409,
        "erro_login_sem_tentativa",
        format!("login da conta {label} cancelado"),
        json!({}),
    )
}
impl AccountService {
    pub async fn recover_claude_logins(&self, client: WindowClient) -> Result<(), AccountError> {
        for (_, account) in self.claude_accounts()? {
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let record = self.login_record(&key)?;
            if !record.exists() {
                continue;
            }
            let _guard = self
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .map_err(|_| AccountError::io())?;
            self.validate_claude(&account, &key)?;
            let raw: Value =
                serde_json::from_slice(&fs::read(&record).map_err(|_| AccountError::io())?)
                    .map_err(|_| AccountError::io())?;
            if serde_json::from_value::<AccountKey>(raw["key"].clone())
                .ok()
                .as_ref()
                != Some(&key)
            {
                return Err(AccountError::io());
            }
            let operation = raw["operation"]
                .as_str()
                .filter(|s| valid_operation(s))
                .ok_or_else(AccountError::io)?;
            client.call(&key, operation, "close", None).await?;
            fs::remove_file(record).map_err(|_| AccountError::io())?;
        }
        Ok(())
    }
    pub(super) fn login_record(
        &self,
        key: &AccountKey,
    ) -> Result<std::path::PathBuf, AccountError> {
        self.sidecar(key, "claude-login.json")
    }
    pub async fn start_claude_login(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move { service.start_claude_owned(&label, client).await })
            .await
            .map_err(|_| AccountError::io())?
    }
    async fn start_claude_owned(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let account = self.claude_by_label(label)?;
        let key =
            AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
        let gate = self.claude_logins.gates.gate(&key);
        let _gate = gate.lock().await;
        let previous = self.claude_logins.attempts.lock().await.get(&key).cloned();
        if let Some(previous) = previous {
            // A tela que abriu a anterior pode ter sumido sem cancelar: pedir de novo é a saída.
            tracing::warn!(
                code = "claude_login_replaced",
                "tentativa de login anterior substituída"
            );
            self.close_attempt(&previous, &client).await?;
        }
        let guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| bridge_error())?;
        self.validate_claude(&account, &key)?;
        let old_token = claude_auth::token_signature(&account.home, true)?;
        let record = self.login_record(&key)?;
        if let Ok(bytes) = fs::read(&record) {
            let raw: Value = serde_json::from_slice(&bytes).map_err(|_| AccountError::io())?;
            if serde_json::from_value::<AccountKey>(raw["key"].clone())
                .ok()
                .as_ref()
                != Some(&key)
            {
                return Err(AccountError::io());
            }
            let operation = raw["operation"]
                .as_str()
                .filter(|s| valid_operation(s))
                .ok_or_else(AccountError::io)?;
            client.call(&key, operation, "close", None).await?;
            fs::remove_file(&record).map_err(|_| AccountError::io())?;
        }
        let operation = nonce().map_err(|_| AccountError::io())?;
        crate::runtime::queue::atomic_write(
            &record,
            json!({"key":key,"operation":operation})
                .to_string()
                .as_bytes(),
        )
        .map_err(|_| AccountError::io())?;
        let attempt = Arc::new(Attempt {
            account,
            key: key.clone(),
            operation,
            old_token,
            guard: Mutex::new(Some(guard)),
        });
        self.claude_logins
            .attempts
            .lock()
            .await
            .insert(key.clone(), attempt.clone());
        if let Err(error) = client
            .call(&attempt.key, &attempt.operation, "open", None)
            .await
        {
            // A resposta perdida pode ter criado a janela: não liberar antes da limpeza.
            self.close_attempt(&attempt, &client).await?;
            return Err(error);
        }
        Ok(json!({"ok":true}))
    }
    async fn current_attempt(&self, attempt: &Arc<Attempt>) -> bool {
        self.claude_logins
            .attempts
            .lock()
            .await
            .get(&attempt.key)
            .is_some_and(|current| Arc::ptr_eq(current, attempt))
    }
    async fn close_attempt(
        &self,
        attempt: &Arc<Attempt>,
        client: &WindowClient,
    ) -> Result<(), AccountError> {
        if !self.current_attempt(attempt).await {
            return Ok(());
        }
        client
            .call(&attempt.key, &attempt.operation, "close", None)
            .await?;
        fs::remove_file(self.login_record(&attempt.key)?).map_err(|_| AccountError::io())?;
        self.claude_logins
            .attempts
            .lock()
            .await
            .remove(&attempt.key);
        attempt.guard.lock().unwrap().take();
        Ok(())
    }
    async fn observe_attempt(
        &self,
        attempt: &Arc<Attempt>,
        client: &WindowClient,
        strict: bool,
    ) -> Result<Option<Value>, AccountError> {
        if !self.current_attempt(attempt).await {
            return Err(cancelled(&attempt.account.id));
        }
        self.validate_claude(&attempt.account, &attempt.key)?;
        let signature_before = claude_auth::token_signature(&attempt.account.home, strict)?;
        if !strict && (signature_before.is_none() || signature_before == attempt.old_token) {
            return Ok(None);
        }
        let state = self.claude_auth_guarded(&attempt.account).await;
        if state["estado"] != "ok" {
            return Err(AccountError::new(
                409,
                if strict {
                    "erro_login_sem_tentativa"
                } else {
                    "erro_login_nao_confirmado"
                },
                format!(
                    "não consegui reler o estado da conta {}: {}",
                    attempt.account.id,
                    state["motivo"].as_str().unwrap_or("indisponivel")
                ),
                json!({}),
            ));
        }
        if state["loggedIn"] != true {
            return Ok(None);
        }
        let signature_after = claude_auth::token_signature(&attempt.account.home, strict)?;
        if signature_after != signature_before
            || signature_after.is_none()
            || signature_after == attempt.old_token
        {
            return Ok(None);
        }
        // A guarda continua viva enquanto a CLI termina e o onboarding é publicado.
        client
            .call(&attempt.key, &attempt.operation, "close", None)
            .await?;
        self.complete_claude_onboarding(&attempt.account);
        self.claude_auth.invalidate(&attempt.key);
        client
            .call(&attempt.key, &attempt.operation, "invalidate", None)
            .await?;
        self.close_attempt(attempt, client).await?;
        Ok(Some(
            json!({"ok":true,"email":state["email"],"plano":state["plano"]}),
        ))
    }
    pub async fn claude_step(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let account = match service.claude_by_label(&label) {
                Ok(account) => account,
                Err(_) => return Ok(json!({"etapa":"idle","url":null,"email":null,"plano":null})),
            };
            let key = AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key); let _gate = gate.lock().await;
            let Some(attempt) = service.claude_logins.attempts.lock().await.get(&key).cloned() else {
                return Ok(json!({"etapa":"idle","url":null,"email":null,"plano":null}));
            };
            let result = async {
                let pane = client.call(&key, &attempt.operation, "read", None).await?;
                if let Some(done) = service.observe_attempt(&attempt, &client, false).await? {
                    return Ok(json!({"etapa":"concluido","url":null,"email":done["email"],"plano":done["plano"]}));
                }
                Ok(json!({"etapa":"aguardando","url":pane["url"],"email":null,"plano":null}))
            }.await;
            if result.is_err() { service.close_attempt(&attempt, &client).await?; }
            result
        }).await.map_err(|_| AccountError::io())?
    }
    pub async fn confirm_claude(
        &self,
        label: &str,
        code: String,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let account = service.claude_by_label(&label)?;
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key);
            let attempt;
            {
                let _gate = gate.lock().await;
                attempt = service
                    .claude_logins
                    .attempts
                    .lock()
                    .await
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| cancelled(&label))?;
                if let Err(error) = client
                    .call(&key, &attempt.operation, "code", Some(&code))
                    .await
                {
                    service.close_attempt(&attempt, &client).await?;
                    return Err(error);
                }
            }
            drop(code);
            let started = tokio::time::Instant::now();
            let mut poll = tokio::time::interval(Duration::from_millis(500));
            loop {
                poll.tick().await;
                let _gate = gate.lock().await;
                let result = if started.elapsed() >= Duration::from_secs(300) {
                    Err(timeout(&label))
                } else {
                    service.observe_attempt(&attempt, &client, true).await
                };
                match result {
                    Ok(Some(done)) => return Ok(done),
                    Ok(None) => (),
                    Err(error) => {
                        service.close_attempt(&attempt, &client).await?;
                        return Err(error);
                    }
                }
            }
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    pub async fn cancel_claude(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let Ok(account) = service.claude_by_label(&label) else {
                return Ok(json!({"ok":true}));
            };
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key);
            let _gate = gate.lock().await;
            let attempt = service
                .claude_logins
                .attempts
                .lock()
                .await
                .get(&key)
                .cloned();
            if let Some(attempt) = attempt {
                service.close_attempt(&attempt, &client).await?;
            }
            Ok(json!({"ok":true}))
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    pub async fn logout_claude(
        &self,
        label: &str,
        client: WindowClient,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        let label = label.to_owned();
        tokio::spawn(async move {
            let account = service.claude_by_label(&label)?;
            let key =
                AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.claude_logins.gates.gate(&key);
            let _gate = gate.lock().await;
            let attempt = service
                .claude_logins
                .attempts
                .lock()
                .await
                .get(&key)
                .cloned();
            if let Some(attempt) = attempt {
                service.close_attempt(&attempt, &client).await?;
            }
            let _guard = service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .map_err(|_| bridge_error())?;
            service.validate_claude(&account, &key)?;
            let result = service.claude_logout_guarded(&account).await;
            client
                .call(
                    &key,
                    &nonce().map_err(|_| AccountError::io())?,
                    "invalidate",
                    None,
                )
                .await?;
            result?;
            Ok(json!({"ok":true}))
        })
        .await
        .map_err(|_| AccountError::io())?
    }
}
fn valid_operation(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn timeout(label: &str) -> AccountError {
    AccountError::new(
        504,
        "erro_login_timeout",
        format!("a conta {label} não apareceu logada em 300s"),
        json!({}),
    )
}

#[cfg(test)]
mod tests {
    use super::super::environment::AccountEnvironment;
    use super::*;
    use axum::{Router, routing::post};

    #[tokio::test]
    async fn new_login_replaces_abandoned_attempt() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into(),
        ));
        let account = service
            .create(Provider::Claude, "work", |_| Ok(()))
            .unwrap();
        let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
        let (actions, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let router = Router::new().route(
            "/internal/accounts/claude-window",
            post(move |body: axum::body::Bytes| {
                let value: Value = serde_json::from_slice(&body).unwrap();
                actions
                    .send((
                        value["action"].as_str().unwrap().to_owned(),
                        value["operation"].as_str().unwrap().to_owned(),
                    ))
                    .unwrap();
                async { json!({"ok":true,"url":null}).to_string() }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = WindowClient::new(
            listener.local_addr().unwrap(),
            "synthetic".into(),
            "test".into(),
        )
        .unwrap();
        let server = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
        service
            .start_claude_login("work", client.clone())
            .await
            .unwrap();
        let (action, first) = observed.recv().await.unwrap();
        assert_eq!(action, "open");
        // A tela que abriu a primeira sumiu sem cancelar: pedir de novo é a saída dela.
        service
            .start_claude_login("work", client.clone())
            .await
            .expect("a nova tentativa substitui a abandonada");
        assert_eq!(observed.recv().await.unwrap(), ("close".to_owned(), first.clone()));
        let (action, second) = observed.recv().await.unwrap();
        assert_eq!(action, "open");
        assert_ne!(second, first);
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_err(),
            "a tentativa nova conserva a guarda da conta"
        );
        server.abort();
    }

    #[tokio::test]
    async fn confirmation_has_300_seconds_after_protected_code_not_after_open() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("native");
        let script = fixture.join("node_modules/@anthropic-ai/claude-code/cli.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        let source = "const fs=require('fs'),p=require('path');fs.appendFileSync(p.join(process.env.CLAUDE_CONFIG_DIR,'status-called'), 'status\\n');process.stdout.write(JSON.stringify({loggedIn:false}));process.exit(1);";
        fs::write(script, source).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(
                fixture.join("claude"),
                format!("#!/usr/bin/env node\n{source}"),
            )
            .unwrap();
            fs::set_permissions(fixture.join("claude"), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = format!(
            "{}{separator}{}",
            fixture.display(),
            std::env::var("PATH").unwrap()
        );
        let home = root.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.clone()),
                ("USERPROFILE".into(), home),
                ("PATH".into(), path),
            ]
            .into_iter()
            .chain(
                std::env::vars()
                    .filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case("SystemRoot")),
            )
            .collect(),
        ));
        let account = service
            .create(Provider::Claude, "work", |_| Ok(()))
            .unwrap();
        let key = AccountKey::new(Provider::Claude, &account.home).unwrap();
        let (actions, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let router = Router::new().route(
            "/internal/accounts/claude-window",
            post(move |body: axum::body::Bytes| {
                let value: Value = serde_json::from_slice(&body).unwrap();
                actions
                    .send(value["action"].as_str().unwrap().to_owned())
                    .unwrap();
                async { json!({"ok":true,"url":null}).to_string() }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = WindowClient::new(
            listener.local_addr().unwrap(),
            "synthetic".into(),
            "test".into(),
        )
        .unwrap();
        let server = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
        service
            .start_claude_login("work", client.clone())
            .await
            .unwrap();
        assert_eq!(observed.recv().await.as_deref(), Some("open"));
        // Só avançar o relógio sem CLI em voo: o processo nativo usa I/O real.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(200)).await;
        tokio::time::resume();
        let confirming_service = service.clone();
        let confirming_client = client.clone();
        let confirming = tokio::spawn(async move {
            confirming_service
                .confirm_claude("work", "synthetic-code".into(), confirming_client)
                .await
        });
        assert_eq!(observed.recv().await.as_deref(), Some("code"));
        let status_called = account.home.join("status-called");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !status_called.exists() {
            assert!(
                !confirming.is_finished(),
                "a confirmação terminou antes da primeira leitura nativa"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "a CLI não recebeu a primeira leitura de confirmação"
            );
            tokio::task::yield_now().await;
        }
        let gate = service.claude_logins.gates.gate(&key);
        let control = gate.lock().await;
        assert!(observed.is_empty());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(299)).await;
        tokio::time::resume();
        drop(control);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while observed.is_empty() && fs::read_to_string(&status_called).unwrap().lines().count() < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "a confirmação não avaliou a fronteira de 299 s"
            );
            tokio::task::yield_now().await;
        }
        let control = gate.lock().await;
        assert!(
            observed.try_recv().is_err(),
            "a confirmação não fecha a janela antes dos seus 300 s"
        );
        assert!(!confirming.is_finished());
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_err()
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::time::resume();
        drop(control);
        let error = confirming.await.unwrap().unwrap_err();
        assert_eq!(error.status, 504);
        assert_eq!(error.code, "erro_login_timeout");
        assert_eq!(observed.recv().await.as_deref(), Some("close"));
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_ok()
        );
        assert_eq!(
            service.claude_step("work", client).await.unwrap()["etapa"],
            "idle"
        );
        server.abort();
    }
}
