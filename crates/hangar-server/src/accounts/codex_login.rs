//! O evento de sucesso inicia confirmação; identidade legível é quem conclui o login.
use super::{
    AccountKey, GuardMode, Provider,
    catalog::{Account, AccountError, AccountService},
    native::{AuthCache, NativeProcess},
};
use hangar_codex::proto::{
    AccountLoginCompletedNotification, CancelLoginAccountParams, CancelLoginAccountResponse,
    ClientRequest, LoginAccountParams, LoginAccountResponse,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Mutex as AsyncMutex, watch},
    time::Instant,
};

#[derive(Clone)]
pub struct CodexInvalidator {
    client: reqwest::Client,
    upstream: std::net::SocketAddr,
    secret: String,
    instance: String,
}
impl CodexInvalidator {
    pub fn new(
        upstream: std::net::SocketAddr,
        secret: String,
        instance: String,
    ) -> Result<Self, AccountError> {
        if !upstream.ip().is_loopback() || instance.is_empty() {
            return Err(AccountError::io());
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| AccountError::io())?;
        Ok(Self {
            client,
            upstream,
            secret,
            instance,
        })
    }
    pub(crate) async fn invalidate(&self, key: &AccountKey) -> Result<(), &'static str> {
        let response = self
            .client
            .post(format!(
                "http://{}/internal/accounts/codex-invalidate",
                self.upstream
            ))
            .header("x-hangar-internal", &self.secret)
            .header("x-hangar-runtime-instance", &self.instance)
            .header("content-type", "application/json")
            .body(json!({"key":key}).to_string())
            .send()
            .await
            .map_err(|_| "codex_account_cache_invalidation_failed")?;
        if !response.status().is_success() {
            return Err("codex_account_cache_invalidation_failed");
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "codex_account_cache_invalidation_failed")?;
        if serde_json::from_slice::<Value>(&bytes)
            .ok()
            .is_none_or(|value| value["ok"] != true)
        {
            return Err("codex_account_cache_invalidation_failed");
        }
        Ok(())
    }
}
#[derive(Clone, Default)]
pub struct CodexLogins {
    attempts: Arc<Mutex<HashMap<AccountKey, Arc<Attempt>>>>,
    gates: super::locks::KeyedGates,
    closing: Arc<std::sync::atomic::AtomicBool>,
}
struct Attempt {
    id: String,
    status: watch::Sender<Value>,
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
}
impl CodexLogins {
    fn gate(&self, key: &AccountKey) -> Arc<AsyncMutex<()>> {
        self.gates.gate(key)
    }
    fn attempt(&self, key: &AccountKey) -> Option<Arc<Attempt>> {
        self.attempts.lock().unwrap().get(key).cloned()
    }
    pub async fn close(&self) {
        let attempts: Vec<_> = {
            let attempts = self.attempts.lock().unwrap();
            self.closing
                .store(true, std::sync::atomic::Ordering::Release);
            attempts.values().cloned().collect()
        };
        for attempt in attempts {
            attempt.cancel.send_replace(true);
            wait_done(&attempt).await;
        }
    }
}
async fn wait_done(attempt: &Attempt) {
    let _ = attempt.done.subscribe().wait_for(|done| *done).await;
}
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    let _ = cancel.wait_for(|cancelled| *cancelled).await;
}
fn issue(code: &str) -> Value {
    json!({"code":code,"params":{}})
}

pub fn validate_storage(account: &Account) -> Result<(), AccountError> {
    if account.is_default {
        return Ok(());
    }
    let config = std::fs::read_to_string(account.home.join("config.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .ok_or_else(|| {
            AccountError::codex(409, "codex_account_prepare_required", Some(&account.id))
        })?;
    if config
        .get("cli_auth_credentials_store")
        .and_then(toml::Value::as_str)
        != Some("file")
    {
        return Err(AccountError::codex(
            409,
            "codex_account_auth_storage_invalid",
            Some(&account.id),
        ));
    }
    Ok(())
}
impl AccountService {
    pub fn codex_login_status(&self, account: &Account) -> Value {
        let Ok(key) = AccountKey::new(Provider::Codex, &account.home) else {
            return Value::Null;
        };
        self.codex_logins
            .attempt(&key)
            .map(|a| public_attempt(&a))
            .unwrap_or(Value::Null)
    }
    pub async fn start_codex_login(
        &self,
        account: &Account,
        bridge: super::bridge::AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        invalidator: CodexInvalidator,
    ) -> Result<Value, AccountError> {
        // O proprietário continua vivo mesmo se o cliente HTTP cancelar a abertura.
        let (service, account) = (self.clone(), account.clone());
        tokio::spawn(async move {
            service
                .start_codex_login_owned(account, bridge, runtime, invalidator)
                .await
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    async fn start_codex_login_owned(
        &self,
        account: Account,
        bridge: super::bridge::AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        invalidator: CodexInvalidator,
    ) -> Result<Value, AccountError> {
        let key =
            AccountKey::new(Provider::Codex, &account.home).map_err(|_| AccountError::io())?;
        let gate = self.codex_logins.gate(&key);
        let _serial = gate.lock().await;
        if let Some(attempt) = self.codex_logins.attempt(&key)
            && !*attempt.done.borrow()
        {
            return Ok(public_attempt(&attempt));
        }
        let guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| AccountError::codex(409, "codex_account_in_use", Some(&account.id)))?;
        let current = self.resolve(Provider::Codex, &account.id)?;
        if AccountKey::new(Provider::Codex, &current.home)
            .ok()
            .as_ref()
            != Some(&key)
        {
            return Err(AccountError::io());
        }
        validate_storage(&current)?;
        let facts = self.usage(&bridge, runtime.as_deref(), &key).await;
        facts.ensure_unused().map_err(|code| {
            AccountError::codex(
                409,
                if code == "account_in_use" {
                    "codex_account_in_use"
                } else {
                    "account_usage_unknown"
                },
                Some(&account.id),
            )
        })?;
        self.codex_auth.invalidate(&key);
        let id = super::claude_login::nonce().map_err(|_| AccountError::io())?;
        let (status, _) =
            watch::channel(json!({"account_id":account.id,"attempt_id":id,"status":"starting"}));
        let (cancel, _) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let attempt = Arc::new(Attempt {
            id,
            status,
            cancel,
            done,
        });
        {
            let mut attempts = self.codex_logins.attempts.lock().unwrap();
            if self
                .codex_logins
                .closing
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(AccountError::io());
            }
            attempts.insert(key.clone(), attempt.clone());
        }
        let service = self.clone();
        let worker = attempt.clone();
        tokio::spawn(async move {
            let mut native = None;
            let mut cancel = worker.cancel.subscribe();
            let deadline = Instant::now() + Duration::from_secs(900);
            let mut result = tokio::select! {
                biased;
                ()=cancelled(&mut cancel)=>Err("cancelled"),
                result=tokio::time::timeout_at(deadline,service.run_codex_login(&account,&worker,&mut native))=>result.unwrap_or(Err("codex_account_login_timeout")),
            };
            if *worker.cancel.borrow()
                && let Some(helper) = native.as_mut()
            {
                let login_id = worker.status.borrow()["native_login_id"]
                    .as_str()
                    .map(str::to_owned);
                if let Some(login_id) = login_id {
                    let _ = helper
                        .client
                        .request::<CancelLoginAccountResponse>(
                            ClientRequest::AccountLoginCancel(CancelLoginAccountParams {
                                login_id,
                            }),
                            Duration::from_secs(30),
                        )
                        .await;
                }
            }
            if let Some(helper) = native.as_mut() {
                helper.close().await;
            }
            service.codex_auth.invalidate(&key);
            if let Err(code) = invalidator.invalidate(&key).await {
                result = Err(code);
            }
            let mut public = worker.status.borrow().clone();
            public.as_object_mut().unwrap().remove("native_login_id");
            public["status"] = json!(if *worker.cancel.borrow() {
                "cancelled"
            } else if result.is_ok() {
                "completed"
            } else {
                "failed"
            });
            if let Err(code) = result
                && !*worker.cancel.borrow()
            {
                public["error"] = issue(code);
            }
            drop(guard);
            worker.status.send_replace(public);
            worker.done.send_replace(true);
        });
        attempt
            .status
            .subscribe()
            .wait_for(|state| state["status"] != "starting")
            .await
            .map_err(|_| AccountError::io())?;
        Ok(public_attempt(&attempt))
    }
    async fn run_codex_login(
        &self,
        account: &Account,
        attempt: &Attempt,
        native: &mut Option<NativeProcess>,
    ) -> Result<(), &'static str> {
        *native = Some(NativeProcess::open(self, account).await?);
        let helper = native.as_mut().unwrap();
        helper
            .initialize()
            .await
            .map_err(|_| "codex_account_login_failed")?;
        let response = helper
            .client
            .request::<LoginAccountResponse>(
                ClientRequest::AccountLoginStart(LoginAccountParams::ChatgptDeviceCode),
                Duration::from_secs(30),
            )
            .await
            .map_err(|_| "codex_account_login_failed")?;
        let LoginAccountResponse::ChatgptDeviceCode {
            login_id,
            verification_url,
            user_code,
        } = response;
        if login_id.is_empty() || verification_url.is_empty() || user_code.is_empty() {
            return Err("codex_account_login_failed");
        }
        attempt.status.send_replace(json!({"account_id":account.id,"attempt_id":attempt.id,"status":"waiting","native_login_id":login_id,"verification_url":verification_url,"user_code":user_code}));
        loop {
            let params = helper
                .completed
                .recv()
                .await
                .ok_or("codex_account_login_failed")?;
            let Ok(event) = serde_json::from_value::<AccountLoginCompletedNotification>(params)
            else {
                continue;
            };
            if event.login_id.as_ref().is_some_and(|id| id != &login_id) {
                continue;
            }
            if !event.success {
                return Err("codex_account_login_failed");
            }
            break;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tick.tick().await;
            let result = tokio::time::timeout_at(deadline, stable_identity(helper, account)).await;
            if matches!(&result,Ok(Ok(auth)) if auth["status"]=="connected") {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        // A guarda exclusiva continua no mesmo descritor durante a releitura nova.
        native.as_mut().unwrap().close().await;
        *native = Some(NativeProcess::open(self, account).await?);
        let helper = native.as_mut().unwrap();
        helper
            .initialize()
            .await
            .map_err(|_| "codex_account_login_failed")?;
        let auth = stable_identity(helper, account).await?;
        if auth["status"] == "connected" {
            Ok(())
        } else {
            Err("codex_account_login_failed")
        }
    }
    pub async fn cancel_codex_login(
        &self,
        account: &Account,
        id: &str,
    ) -> Result<Value, AccountError> {
        let (service, account, id) = (self.clone(), account.clone(), id.to_owned());
        tokio::spawn(async move {
            let key =
                AccountKey::new(Provider::Codex, &account.home).map_err(|_| AccountError::io())?;
            let gate = service.codex_logins.gate(&key);
            let _serial = gate.lock().await;
            let attempt = service
                .codex_logins
                .attempt(&key)
                .filter(|a| a.id == id)
                .ok_or_else(|| {
                    AccountError::codex(409, "codex_login_attempt_mismatch", Some(&account.id))
                })?;
            if !*attempt.done.borrow() {
                attempt.cancel.send_replace(true);
                wait_done(&attempt).await;
            }
            Ok(public_attempt(&attempt))
        })
        .await
        .map_err(|_| AccountError::io())?
    }
}
fn public_attempt(attempt: &Attempt) -> Value {
    let mut value = attempt.status.borrow().clone();
    value.as_object_mut().unwrap().remove("native_login_id");
    if value["status"] == "starting" {
        value["status"] = json!("waiting")
    };
    value
}
async fn stable_identity(
    helper: &mut NativeProcess,
    account: &Account,
) -> Result<Value, &'static str> {
    let before = AuthCache::signature(account).map_err(|_| "codex_account_login_failed")?;
    let auth = helper
        .read()
        .await
        .map_err(|_| "codex_account_login_failed")?;
    if AuthCache::signature(account).ok().as_ref() != Some(&before) {
        return Ok(super::native::unavailable(false));
    }
    Ok(auth)
}
