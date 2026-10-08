//! O callback conserva sua própria guarda; resposta perdida nunca encerra trabalho vivo.
use super::{
    AccountKey, AccountService, GuardMode, Provider,
    bridge::AccountsBridge,
    catalog::{Account, AccountError, idle_sync},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRequest {
    pub operation: String,
    pub instance: String,
    pub key: AccountKey,
    pub account_id: String,
    pub seed: bool,
    pub force: bool,
    pub cwd: Option<String>,
}

#[derive(Clone)]
pub struct PreparationState {
    pub running: bool,
    pub force_pending: bool,
    pub result: Value,
}
pub type Preparations = Arc<Mutex<HashMap<AccountKey, PreparationState>>>;
pub type PreparationGates = super::locks::KeyedGates;

pub fn running() -> Value {
    json!({"status":"running","trust_pending":false,"issues":[],"etapa":null})
}
pub fn failed(code: &str) -> Value {
    json!({"status":"error","trust_pending":false,"issues":[{"code":code,"params":{}}]})
}
fn complete(value: &Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("ready" | "partial" | "error")
    )
}

impl AccountService {
    fn preparation_gate(&self, key: &AccountKey) -> Arc<tokio::sync::Mutex<()>> {
        self.preparation_gates.gate(key)
    }
    pub(crate) fn sidecar(
        &self,
        key: &AccountKey,
        extension: &str,
    ) -> Result<PathBuf, AccountError> {
        self.locks
            .sidecar(key, extension)
            .map_err(|_| AccountError::io())
    }
    pub fn preparation_path(&self, key: &AccountKey) -> Result<PathBuf, AccountError> {
        self.sidecar(key, "prepare.json")
    }
    pub(crate) fn result_path(&self, key: &AccountKey) -> Result<PathBuf, AccountError> {
        self.sidecar(key, "prepare-result.json")
    }
    pub(crate) fn force_path(&self, key: &AccountKey) -> Result<PathBuf, AccountError> {
        self.sidecar(key, "prepare-force.json")
    }
    pub(crate) fn persist(path: &Path, value: &Value) -> Result<(), AccountError> {
        // A publicação atômica usa o mesmo volume e não remove o lock de existência.
        crate::runtime::queue::atomic_write(path, value.to_string().as_bytes())
            .map_err(|_| AccountError::io())
    }
    fn clear_force(&self, key: &AccountKey) -> Result<(), AccountError> {
        match std::fs::remove_file(self.force_path(key)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AccountError::io()),
        }
    }
    fn stored_status(&self, account: &Account, key: &AccountKey) -> Value {
        if account.is_default {
            return json!({"status":"ready","trust_pending":false,"issues":[],"etapa":null,"herdado":null});
        }
        let home = account.home.to_string_lossy();
        let digest: String = ring::digest::digest(&ring::digest::SHA256, home.as_bytes())
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let paths = [
            self.result_path(key).ok(),
            Some(
                self.env
                    .home
                    .join(".hangar/codex-contas")
                    .join(digest)
                    .join("estado.json"),
            ),
        ];
        let mut available = Vec::new();
        for path in paths.into_iter().flatten() {
            match std::fs::metadata(&path).and_then(|metadata| metadata.modified()) {
                Ok(modified) => available.push((modified, path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return failed("codex_account_state_invalid"),
            }
        }
        // Uma passagem pelo fallback pode publicar estado posterior ao último filho Rust.
        available.sort_by(|left, right| right.0.cmp(&left.0));
        for (_, path) in available {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    return serde_json::from_slice::<Value>(&bytes)
                        .ok()
                        .map(|value| value.get("public").cloned().unwrap_or(value))
                        .filter(|value| {
                            matches!(
                                value["status"].as_str(),
                                Some("idle" | "running" | "ready" | "partial" | "error")
                            )
                        })
                        .unwrap_or_else(|| failed("codex_account_state_invalid"));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return failed("codex_account_state_invalid"),
            }
        }
        idle_sync()
    }
    pub async fn preparation_status(&self, account: &Account, bridge: &AccountsBridge) -> Value {
        let Ok(key) = AccountKey::new(Provider::Codex, &account.home) else {
            return failed("account_prepare_invalid");
        };
        if let Some(state) = self.preparations.lock().unwrap().get(&key) {
            return state.result.clone();
        }
        let Ok(path) = self.preparation_path(&key) else {
            return failed("account_prepare_invalid");
        };
        match std::fs::read(path) {
            Ok(bytes) => {
                let Ok(previous) = serde_json::from_slice::<PrepareRequest>(&bytes) else {
                    return failed("account_prepare_invalid");
                };
                if previous.key != key || previous.account_id != account.id {
                    return failed("account_prepare_invalid");
                }
                if self.preparation_gate(&key).try_lock().is_err() {
                    return running();
                }
                let Ok(guard) = self.locks.try_acquire(&key, GuardMode::Shared) else {
                    return failed("account_prepare_busy");
                };
                let mut states = self.preparations.lock().unwrap();
                if let Some(state) = states.get(&key) {
                    return state.result.clone();
                }
                states.insert(
                    key.clone(),
                    PreparationState {
                        running: true,
                        force_pending: self.force_path(&key).is_ok_and(|path| path.exists()),
                        result: running(),
                    },
                );
                let (service, bridge, account) = (self.clone(), bridge.clone(), account.clone());
                tokio::spawn(async move {
                    let _guard = guard;
                    let result = service.finish_callback(&previous, &bridge).await;
                    let result =
                        if std::fs::remove_file(service.preparation_path(&key).unwrap()).is_err() {
                            failed("account_prepare_state_failed")
                        } else {
                            result
                        };
                    service
                        .complete_preparation(&key, &account, &bridge, result, false)
                        .await;
                });
                running()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.stored_status(account, &key)
            }
            Err(_) => failed("account_prepare_invalid"),
        }
    }
    pub async fn prepare(
        &self,
        account: &Account,
        bridge: AccountsBridge,
        force: bool,
        cwd: Option<String>,
    ) -> Result<Value, AccountError> {
        if account.is_default {
            return Ok(json!({"status":"ready","trust_pending":false,"issues":[]}));
        }
        if cwd
            .as_ref()
            .is_some_and(|value| !Path::new(value).is_absolute() || !Path::new(value).is_dir())
        {
            return Err(AccountError::new(
                400,
                "account_prepare_cwd_invalid",
                "pasta de trabalho inválida",
                json!({}),
            ));
        }
        let key =
            AccountKey::new(Provider::Codex, &account.home).map_err(|_| AccountError::io())?;
        let guard = self
            .locks
            .try_acquire(&key, GuardMode::Shared)
            .map_err(|_| AccountError::io())?;
        self.resolve(Provider::Codex, &account.id)?;
        {
            let mut states = self.preparations.lock().unwrap();
            if let Some(state) = states.get_mut(&key).filter(|state| state.running) {
                state.force_pending |= force;
                if force {
                    Self::persist(&self.force_path(&key)?, &json!(true))?;
                }
                return Ok(state.result.clone());
            }
            states.insert(
                key.clone(),
                PreparationState {
                    running: true,
                    force_pending: self.force_path(&key).is_ok_and(|path| path.exists()),
                    result: running(),
                },
            );
        }
        let service = self.clone();
        let account = account.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let result = service
                .run_preparation(Provider::Codex, &account, &bridge, false, force, cwd)
                .await
                .unwrap_or_else(|_| failed("account_prepare_failed"));
            service
                .complete_preparation(&key, &account, &bridge, result, force)
                .await;
        });
        Ok(running())
    }
    async fn complete_preparation(
        &self,
        key: &AccountKey,
        account: &Account,
        bridge: &AccountsBridge,
        mut result: Value,
        mut forced: bool,
    ) {
        loop {
            {
                let mut states = self.preparations.lock().unwrap();
                let state = states.get_mut(key).unwrap();
                if state.force_pending && !forced {
                    state.force_pending = false;
                } else {
                    if forced && self.clear_force(key).is_err() {
                        result = failed("account_prepare_state_failed");
                    }
                    if self
                        .result_path(key)
                        .and_then(|path| Self::persist(&path, &result))
                        .is_err()
                    {
                        result = failed("account_prepare_state_failed");
                    }
                    state.running = false;
                    state.force_pending = false;
                    state.result = result;
                    return;
                }
            }
            forced = true;
            result = self
                .run_preparation(Provider::Codex, account, bridge, false, true, None)
                .await
                .unwrap_or_else(|_| failed("account_prepare_failed"));
        }
    }
    pub async fn pretrust(
        &self,
        account: &Account,
        bridge: AccountsBridge,
        cwd: String,
    ) -> Result<Value, AccountError> {
        if !Path::new(&cwd).is_absolute() || !Path::new(&cwd).is_dir() {
            return Err(AccountError::new(
                400,
                "account_prepare_cwd_invalid",
                "pasta de trabalho inválida",
                json!({}),
            ));
        }
        let (service, account) = (self.clone(), account.clone());
        tokio::spawn(async move {
            let key =
                AccountKey::new(Provider::Codex, &account.home).map_err(|_| AccountError::io())?;
            let _guard = service
                .locks
                .try_acquire(&key, GuardMode::Shared)
                .map_err(|_| AccountError::io())?;
            service
                .run_preparation(Provider::Codex, &account, &bridge, false, false, Some(cwd))
                .await
        })
        .await
        .map_err(|_| AccountError::io())?
    }
    pub async fn seed_claude(
        &self,
        path: &Path,
        name: &str,
        bridge: &AccountsBridge,
    ) -> Result<(), AccountError> {
        let account = Account {
            id: name.into(),
            home: path.into(),
            is_default: false,
        };
        let result = self
            .run_preparation(Provider::Claude, &account, bridge, true, false, None)
            .await?;
        if matches!(result["status"].as_str(), Some("ready" | "partial")) {
            Ok(())
        } else {
            Err(AccountError::new(
                500,
                "account_prepare_failed",
                "não foi possível preparar a conta",
                json!({}),
            ))
        }
    }
    async fn run_preparation(
        &self,
        provider: Provider,
        account: &Account,
        bridge: &AccountsBridge,
        seed: bool,
        force: bool,
        cwd: Option<String>,
    ) -> Result<Value, AccountError> {
        let key = AccountKey::new(provider, &account.home).map_err(|_| AccountError::io())?;
        let gate = self.preparation_gate(&key);
        let _serial = gate.lock().await;
        let path = self.preparation_path(&key)?;
        // Após reinício, a instância anterior só pode ser consultada, nunca reiniciada.
        match std::fs::read(&path) {
            Ok(bytes) => {
                let previous: PrepareRequest =
                    serde_json::from_slice(&bytes).map_err(|_| AccountError::io())?;
                if previous.key != key || previous.account_id != account.id {
                    return Err(AccountError::io());
                }
                self.finish_callback(&previous, bridge).await;
                std::fs::remove_file(&path).map_err(|_| AccountError::io())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AccountError::io()),
        }
        let request = PrepareRequest {
            operation: super::claude_login::nonce().map_err(|_| AccountError::io())?,
            instance: bridge.instance().into(),
            key,
            account_id: account.id.clone(),
            seed,
            force,
            cwd,
        };
        Self::persist(
            &path,
            &serde_json::to_value(&request).map_err(|_| AccountError::io())?,
        )?;
        if force {
            self.clear_force(&request.key)?;
        }
        let initial = bridge.preparation_start(&request).await;
        let result = match initial {
            Ok(result) if complete(&result) => result,
            _ => self.finish_callback(&request, bridge).await,
        };
        // Revogar antes de liberar a guarda impede uma entrega HTTP atrasada de começar.
        std::fs::remove_file(&path).map_err(|_| AccountError::io())?;
        Ok(result)
    }
    async fn finish_callback(&self, request: &PrepareRequest, bridge: &AccountsBridge) -> Value {
        loop {
            match bridge.preparation_wait(&request.operation).await {
                Ok(result) if complete(&result) => return result,
                Ok(result) if result["status"] == "unknown" => {
                    return failed("account_prepare_interrupted");
                }
                Ok(result) => {
                    if let Some(state) = self.preparations.lock().unwrap().get_mut(&request.key) {
                        state.result = result;
                    }
                }
                Err(_) => {
                    if let Some(state) = self.preparations.lock().unwrap().get_mut(&request.key) {
                        state.result = failed("account_prepare_bridge_unavailable");
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
}
