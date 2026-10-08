//! Cofre, Codex e destinos Pi/omp são gravados aqui, sem largar a guarda da conta antes do fim do I/O.
use super::{
    AccountGuard, AccountKey, GuardMode, Provider, UsageFacts,
    bridge::AccountsBridge,
    catalog::{Account, AccountError, AccountService},
    storage,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH: &str = "https://auth.openai.com";

#[derive(Clone)]
pub struct DeviceOAuth {
    client: reqwest::Client,
    base: String,
}
impl DeviceOAuth {
    pub fn new(base: &str) -> Result<Self, AccountError> {
        let url = reqwest::Url::parse(base).map_err(|_| AccountError::io())?;
        // HTTP só é aceito em loopback, para um transporte local controlado.
        if !(url.scheme() == "https"
            || (url.scheme() == "http"
                && url
                    .host_str()
                    .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                    .is_some_and(|ip| ip.is_loopback())))
        {
            return Err(AccountError::io());
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .user_agent("hangar/1.0")
                .build()
                .map_err(|_| AccountError::io())?,
            base: base.trim_end_matches('/').into(),
        })
    }
    async fn post(
        &self,
        path: &str,
        body: Value,
        form: bool,
    ) -> Result<(u16, Value), &'static str> {
        let mut request = self.client.post(format!("{}{path}", self.base));
        request = if form {
            request
                .header("content-type", "application/x-www-form-urlencoded")
                .body(
                    form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(
                            body.as_object()
                                .ok_or("device_response_invalid")?
                                .iter()
                                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or(""))),
                        )
                        .finish(),
                )
        } else {
            request
                .header("content-type", "application/json")
                .body(body.to_string())
        };
        let response = request
            .send()
            .await
            .map_err(|_| "device_network_unavailable")?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "device_network_unavailable")?
        {
            if bytes.len() + chunk.len() > 65536 {
                return Err("device_response_invalid");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((
            status,
            serde_json::from_slice(&bytes).map_err(|_| "device_response_invalid")?,
        ))
    }
}

#[derive(Default)]
struct Attempts {
    closing: bool,
    current: Option<Arc<Attempt>>,
    active: Vec<watch::Sender<bool>>,
}
struct Attempt {
    id: String,
    status: watch::Sender<Value>,
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
}
#[derive(Clone)]
pub struct DeviceLogins {
    state: Arc<Mutex<Attempts>>,
    gate: Arc<AsyncMutex<()>>,
    oauth: DeviceOAuth,
}
impl Default for DeviceLogins {
    fn default() -> Self {
        Self::new(DeviceOAuth::new(AUTH).expect("endereço OAuth estático válido"))
    }
}
impl DeviceLogins {
    pub fn new(oauth: DeviceOAuth) -> Self {
        Self {
            state: Default::default(),
            gate: Default::default(),
            oauth,
        }
    }
    pub fn status(&self) -> Value {
        self.state
            .lock()
            .unwrap()
            .current
            .as_ref()
            .map(|a| a.status.borrow().clone())
            .unwrap_or_else(|| json!({"etapa":"idle"}))
    }
    pub async fn cancel(&self, id: Option<&str>) -> Result<Value, AccountError> {
        let _serial = self.gate.lock().await;
        let attempt = self.state.lock().unwrap().current.clone();
        if let Some(attempt) = attempt {
            if id.is_some_and(|id| id != attempt.id) {
                return Err(AccountError::codex(
                    409,
                    "codex_login_attempt_mismatch",
                    None,
                ));
            }
            attempt.cancel.send_replace(true);
            wait_done(&attempt).await;
            self.state.lock().unwrap().current = None;
        }
        Ok(json!({"etapa":"idle"}))
    }
    pub async fn close(&self) {
        let (current, active) = {
            let mut state = self.state.lock().unwrap();
            state.closing = true;
            (state.current.clone(), state.active.clone())
        };
        if let Some(attempt) = current {
            attempt.cancel.send_replace(true);
            wait_done(&attempt).await;
        }
        for done in active {
            let mut done = done.subscribe();
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
    }
}
async fn wait_done(attempt: &Attempt) {
    let mut done = attempt.done.subscribe();
    while !*done.borrow_and_update() {
        if done.changed().await.is_err() {
            break;
        }
    }
}
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    while !*cancel.borrow_and_update() {
        if cancel.changed().await.is_err() {
            break;
        }
    }
}

/// Só o aviso de invalidação do Codex ainda vai ao Python; Pi/omp não passam por aqui.
#[derive(Clone)]
pub struct DeviceBridge {
    invalidator: super::codex_login::CodexInvalidator,
}
impl DeviceBridge {
    pub fn new(
        upstream: std::net::SocketAddr,
        secret: String,
        instance: String,
    ) -> Result<Self, AccountError> {
        if !upstream.ip().is_loopback() || instance.is_empty() {
            return Err(AccountError::io());
        }
        Ok(Self {
            invalidator: super::codex_login::CodexInvalidator::new(upstream, secret, instance)?,
        })
    }
}

fn vault_path(service: &AccountService) -> std::path::PathBuf {
    service.env.home.join(".hangar/auth/openai-codex.json")
}
fn read_json(path: &Path) -> Option<Value> {
    if !storage::real_file(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 65536 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn has_auth(path: &Path) -> bool {
    read_json(&path.join("auth.json")).is_some_and(|value| {
        value["tokens"]["refresh_token"]
            .as_str()
            .is_some_and(|t| !t.is_empty())
            || value["OPENAI_API_KEY"]
                .as_str()
                .is_some_and(|t| !t.is_empty())
    })
}
fn claims(access: &str) -> Value {
    access
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}
fn tokens(response: &Value) -> Result<Value, &'static str> {
    let access = response["access_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or("device_tokens_invalid")?;
    let refresh = response["refresh_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or("device_tokens_invalid")?;
    let claims = claims(access);
    let expiry = claims["exp"].as_i64().unwrap_or_else(|| {
        chrono::Utc::now().timestamp() + response["expires_in"].as_i64().unwrap_or(0)
    });
    Ok(
        json!({"access":access,"refresh":refresh,"id_token":response["id_token"].as_str().unwrap_or(""),
        "expires_ms":expiry.saturating_mul(1000),
        "account_id":claims["https://api.openai.com/auth"]["chatgpt_account_id"].as_str().unwrap_or(""),
        "plano":claims["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str().unwrap_or("")}),
    )
}
fn valid_tokens(value: &Value) -> bool {
    ["access", "refresh"]
        .iter()
        .all(|name| value[*name].as_str().is_some_and(|v| !v.is_empty()))
        && value["id_token"].is_string()
        && value["expires_ms"].is_i64()
        && value["account_id"].is_string()
        && value["plano"].is_string()
}
/// Cofre e Codex: destino e pasta reais, sem link.
pub(super) fn atomic_json(path: &Path, value: &Value) -> Result<(), &'static str> {
    write_atomic(path, value, true)
}
/// Pi aceita os destinos vinculados do writer anterior: o link de arquivo é trocado pelo arquivo
/// novo, sem escrever no alvo, e uma pasta vinculada continua vinculada.
pub(super) fn atomic_json_linked(path: &Path, value: &Value) -> Result<(), &'static str> {
    write_atomic(path, value, false)
}
fn write_atomic(path: &Path, value: &Value, strict: bool) -> Result<(), &'static str> {
    use std::io::Write;
    let parent = path.parent().ok_or("device_storage_failed")?;
    if strict && path.exists() && !storage::real_file(path) {
        return Err("device_storage_failed");
    }
    std::fs::create_dir_all(parent).map_err(|_| "device_storage_failed")?;
    if !(if strict { storage::real_dir(parent) } else { parent.is_dir() }) {
        return Err("device_storage_failed");
    }
    let tmp = parent.join(format!(
        ".device-{}.tmp",
        super::claude_login::nonce().map_err(|_| "device_storage_failed")?
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(|_| "device_storage_failed")?;
        file.write_all(value.to_string().as_bytes())
            .map_err(|_| "device_storage_failed")?;
        file.sync_all().map_err(|_| "device_storage_failed")?;
        drop(file);
        // A substituição também precisa aceitar um destino existente no Windows.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
            }
            let from: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
                return Err("device_storage_failed");
            }
        }
        #[cfg(not(windows))]
        std::fs::rename(&tmp, path).map_err(|_| "device_storage_failed")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

impl AccountService {
    async fn device_guard(
        &self,
        facts_bridge: &AccountsBridge,
        runtime: Option<&Arc<crate::runtime::gateway::RuntimeRegistry>>,
    ) -> Result<(Account, AccountKey, AccountGuard), AccountError> {
        let account = self.resolve(Provider::Codex, "default")?;
        let key =
            AccountKey::new(Provider::Codex, &account.home).map_err(|_| AccountError::io())?;
        let guard = self
            .locks
            .try_acquire(&key, GuardMode::Exclusive)
            .map_err(|_| AccountError::codex(409, "codex_account_in_use", Some("default")))?;
        if self.device_logins.state.lock().unwrap().closing {
            return Err(AccountError::io());
        }
        if account.home.join("config.toml").exists() {
            let config = std::fs::read_to_string(account.home.join("config.toml"))
                .ok()
                .and_then(|text| text.parse::<toml::Table>().ok())
                .ok_or_else(|| {
                    AccountError::codex(409, "codex_account_auth_storage_invalid", Some("default"))
                })?;
            if config
                .get("cli_auth_credentials_store")
                .is_some_and(|v| v.as_str() != Some("file"))
            {
                return Err(AccountError::codex(
                    409,
                    "codex_account_auth_storage_invalid",
                    Some("default"),
                ));
            }
        }
        let mut facts = facts_bridge
            .facts(std::slice::from_ref(&key))
            .await
            .map(|mut rows| rows.remove(0).facts)
            .unwrap_or_default();
        facts.merge(match runtime {
            Some(runtime) => runtime.account_usage(&key).await,
            None => UsageFacts::default(),
        });
        facts.ensure_unused().map_err(|code| {
            AccountError::codex(
                409,
                if code == "account_in_use" {
                    "codex_account_in_use"
                } else {
                    "account_usage_unknown"
                },
                Some("default"),
            )
        })?;
        Ok((account, key, guard))
    }
    pub async fn start_device_login(
        &self,
        facts: AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        bridge: DeviceBridge,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        // O trabalho registrado sobrevive à perda da resposta HTTP de início.
        tokio::spawn(async move { service.start_device_owned(facts, runtime, bridge).await })
            .await
            .map_err(|_| AccountError::io())?
    }
    async fn start_device_owned(
        &self,
        facts: AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        bridge: DeviceBridge,
    ) -> Result<Value, AccountError> {
        let _serial = self.device_logins.gate.lock().await;
        if self
            .device_logins
            .state
            .lock()
            .unwrap()
            .current
            .as_ref()
            .is_some_and(|a| !*a.done.borrow())
        {
            return Err(AccountError::codex(409, "device_login_in_progress", None));
        }
        let (account, key, guard) = self.device_guard(&facts, runtime.as_ref()).await?;
        let id = super::claude_login::nonce().map_err(|_| AccountError::io())?;
        let (status, _) =
            watch::channel(json!({"etapa":"iniciando","attempt_id":id,"user_code":"",
            "url":format!("{AUTH}/codex/device"),"erro":"","resultado":null}));
        let (cancel, _) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let attempt = Arc::new(Attempt {
            id,
            status,
            cancel,
            done,
        });
        {
            let mut state = self.device_logins.state.lock().unwrap();
            if state.closing {
                return Err(AccountError::io());
            }
            state.current = Some(attempt.clone());
        }
        let service = self.clone();
        let worker = attempt.clone();
        tokio::spawn(async move {
            let mut cancel = worker.cancel.subscribe();
            let authenticated = tokio::select! {
                biased;
                ()=cancelled(&mut cancel)=>Err("device_login_cancelled"),
                result=tokio::time::timeout(Duration::from_secs(900),service.run_device(&worker))=>
                    result.unwrap_or(Err("device_login_timeout")),
            };
            // Depois de publicar o cofre, a guarda segue com os destinos até o fim do I/O.
            let mut guard = Some(guard);
            let result = match authenticated {
                Ok(tokens) if !*cancel.borrow() => {
                    match atomic_json(&vault_path(&service), &tokens) {
                        Ok(()) => Ok(service
                            .propagate_device_guarded(
                                &account,
                                &key,
                                &bridge,
                                &tokens,
                                guard.take().expect("guarda presente"),
                            )
                            .await),
                        Err(code) => Err(code),
                    }
                }
                Ok(_) => Err("device_login_cancelled"),
                Err(code) => Err(code),
            };
            drop(guard);
            let mut status = worker.status.borrow().clone();
            match result {
                Ok(result) => {
                    status["etapa"] = json!("concluido");
                    status["resultado"] = result;
                }
                Err(code) => {
                    status["etapa"] = json!(if code == "device_login_cancelled" {
                        "cancelado"
                    } else {
                        "falhou"
                    });
                    status["erro"] = json!(code);
                }
            }
            worker.status.send_replace(status);
            worker.done.send_replace(true);
        });
        let mut status = attempt.status.subscribe();
        while status.borrow_and_update()["etapa"] == "iniciando" {
            if status.changed().await.is_err() {
                return Err(AccountError::io());
            }
        }
        let value = status.borrow().clone();
        Ok(value)
    }
    async fn run_device(&self, attempt: &Attempt) -> Result<Value, &'static str> {
        let oauth = &self.device_logins.oauth;
        let (status, reply) = oauth
            .post(
                "/api/accounts/deviceauth/usercode",
                json!({"client_id":CLIENT_ID}),
                false,
            )
            .await?;
        let device = reply["device_auth_id"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or("device_code_failed")?;
        let code = reply["user_code"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or("device_code_failed")?;
        if status != 200 {
            return Err("device_code_failed");
        }
        let mut interval = reply["interval"]
            .as_f64()
            .or_else(|| {
                reply["interval"]
                    .as_str()
                    .and_then(|v| v.trim().parse().ok())
            })
            .filter(|v| v.is_finite())
            .unwrap_or(5.0)
            .clamp(1.0, 900.0);
        let mut snapshot = attempt.status.borrow().clone();
        snapshot["etapa"] = json!("aguardando");
        snapshot["user_code"] = json!(code);
        attempt.status.send_replace(snapshot);
        loop {
            tokio::time::sleep(Duration::from_secs_f64(interval)).await;
            let result = oauth
                .post(
                    "/api/accounts/deviceauth/token",
                    json!({"device_auth_id":device,"user_code":code}),
                    false,
                )
                .await;
            let (status, reply) = match result {
                Ok(reply) => reply,
                Err("device_network_unavailable") => continue,
                Err(code) => return Err(code),
            };
            if status == 200 {
                let code = reply["authorization_code"]
                    .as_str()
                    .ok_or("device_response_invalid")?;
                let verifier = reply["code_verifier"]
                    .as_str()
                    .ok_or("device_response_invalid")?;
                let (status, response) = oauth
                    .post(
                        "/oauth/token",
                        json!({"grant_type":"authorization_code",
                    "client_id":CLIENT_ID,"code":code,"code_verifier":verifier,
                    "redirect_uri":format!("{AUTH}/deviceauth/callback")}),
                        true,
                    )
                    .await?;
                if status != 200 {
                    return Err("device_token_exchange_failed");
                }
                return tokens(&response);
            }
            let error = reply["error"]
                .as_str()
                .or_else(|| reply["error"]["code"].as_str())
                .unwrap_or("");
            if status == 403 || status == 404 || error == "deviceauth_authorization_pending" {
                continue;
            }
            if error == "slow_down" {
                interval = (interval + 5.0).min(900.0);
                continue;
            }
            return Err("device_authorization_failed");
        }
    }
    async fn propagate_device_guarded(
        &self,
        account: &Account,
        key: &AccountKey,
        bridge: &DeviceBridge,
        tokens: &Value,
        guard: AccountGuard,
    ) -> Value {
        let result = if has_auth(&account.home) {
            json!({"ok":true,"motivo":"ja-logado"})
        } else if !storage::real_dir(&account.home) {
            json!({"ok":false,"motivo":"nao-instalado"})
        } else {
            let auth = json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":tokens["id_token"],
                "access_token":tokens["access"],"refresh_token":tokens["refresh"],"account_id":tokens["account_id"]},
                "last_refresh":chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()});
            match atomic_json(&account.home.join("auth.json"), &auth) {
                Ok(()) => {
                    self.codex_auth.invalidate(key);
                    match bridge.invalidator.invalidate(key).await {
                        Ok(()) => {
                            json!({"ok":true,"motivo":account.home.join("auth.json").to_string_lossy()})
                        }
                        Err(code) => json!({"ok":false,"motivo":code}),
                    }
                }
                Err(code) => json!({"ok":false,"motivo":code}),
            }
        };
        let env = self.env.clone();
        let tokens = tokens.clone();
        // A guarda entra no worker por valor e só sai depois de fechado todo o I/O Pi/omp.
        let mut results = tokio::task::spawn_blocking(move || {
            let results = super::secondary_auth::write(&env, &tokens);
            drop(guard);
            results
        })
        .await
        .unwrap_or_else(|_| {
            let failed = json!({"ok":false,"motivo":"armazenamento-indisponivel"});
            json!({"pi":failed,"omp":failed})
        });
        results["codex"] = result;
        results
    }
    pub async fn propagate_device(
        &self,
        facts: AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        bridge: DeviceBridge,
        import: bool,
    ) -> Result<Value, AccountError> {
        let (done, _) = watch::channel(false);
        {
            let mut state = self.device_logins.state.lock().unwrap();
            if state.closing {
                return Err(AccountError::io());
            }
            state.active.retain(|done| !*done.borrow());
            state.active.push(done.clone());
        }
        let service = self.clone();
        tokio::spawn(async move {
            let result = service
                .propagate_device_owned(facts, runtime, bridge, import)
                .await;
            done.send_replace(true);
            result
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    async fn propagate_device_owned(
        &self,
        facts: AccountsBridge,
        runtime: Option<Arc<crate::runtime::gateway::RuntimeRegistry>>,
        bridge: DeviceBridge,
        import: bool,
    ) -> Result<Value, AccountError> {
        let (account, key, guard) = self.device_guard(&facts, runtime.as_ref()).await?;
        let mut saved = read_json(&vault_path(self)).filter(valid_tokens);
        if saved.is_none()
            && import
            && let Some(auth) = read_json(&account.home.join("auth.json"))
        {
            saved=tokens(&json!({"access_token":auth["tokens"]["access_token"],
                    "refresh_token":auth["tokens"]["refresh_token"],"id_token":auth["tokens"]["id_token"].as_str().unwrap_or("")})).ok();
            if let Some(tokens) = &saved {
                atomic_json(&vault_path(self), tokens).map_err(|_| AccountError::io())?;
            }
        }
        if import {
            return Ok(json!({"cofre":saved.is_some()}));
        }
        Ok(match saved {
            Some(tokens) => {
                self.propagate_device_guarded(&account, &key, &bridge, &tokens, guard)
                    .await
            }
            None => {
                json!({"codex":{"ok":false,"motivo":"sem-login"},"pi":{"ok":false,"motivo":"sem-login"},"omp":{"ok":false,"motivo":"sem-login"}})
            }
        })
    }
    pub async fn device_state(&self, _bridge: &DeviceBridge) -> Result<Value, AccountError> {
        let (done, _) = watch::channel(false);
        {
            let mut state = self.device_logins.state.lock().unwrap();
            if state.closing {
                return Err(AccountError::io());
            }
            state.active.retain(|done| !*done.borrow());
            state.active.push(done.clone());
        }
        let service = self.clone();
        // A leitura não toma a conta; o shutdown só espera arquivo e conexão fechados.
        tokio::spawn(async move {
            let env = service.env.clone();
            let secondary =
                tokio::task::spawn_blocking(move || super::secondary_auth::inspect(&env)).await;
            done.send_replace(true);
            let Ok(Ok(secondary)) = secondary else {
                return Err(AccountError::codex(503, "device_bridge_unavailable", None));
            };
            let saved = read_json(&vault_path(&service)).filter(valid_tokens);
            // A leitura de estado não dispara propagação nem importa credenciais.
            Ok(
                json!({"cofre":saved.is_some(),"plano":saved.as_ref().map(|v|v["plano"].clone()).unwrap_or(json!("")),
                "expira_em":saved.as_ref().map(|v|v["expires_ms"].clone()).unwrap_or(Value::Null),
                "codex":has_auth(&service.env.codex_default),"pi":secondary["pi"].as_bool().unwrap_or(false),
                "omp":secondary["omp"].as_bool().unwrap_or(false)}),
            )
        })
        .await
        .map_err(|_| AccountError::io())?
    }
}
