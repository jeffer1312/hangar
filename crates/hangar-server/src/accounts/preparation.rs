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

/// As etapas nativas compartilham o serviço do servidor; a herança continua no callback.
pub trait CodexSetup: Send + Sync {
    fn update(self: Arc<Self>, force: bool) -> futures_util::future::BoxFuture<'static, Result<Value, String>>;
    fn repair(self: Arc<Self>, account: Account) -> futures_util::future::BoxFuture<'static, Result<Option<bool>, String>>;
    fn project(self: Arc<Self>, account: Account, cwd: String) -> futures_util::future::BoxFuture<'static, Result<(), String>>;
    fn status(&self) -> Result<Value, String>;
}
impl CodexSetup for crate::codex_setup::service::Service {
    fn update(self: Arc<Self>, force: bool) -> futures_util::future::BoxFuture<'static, Result<Value, String>> {
        Box::pin(async move {
            match self.update_and_wait("account_prepare", force).await {
                Ok(source) => Ok(source),
                Err(error) => match self.status() {
                    Ok(source) if matches!(source["estado"].as_str(), Some("erro" | "parcial" | "indisponivel")) => Ok(source),
                    _ => Err(error),
                },
            }
        })
    }
    fn repair(self: Arc<Self>, account: Account) -> futures_util::future::BoxFuture<'static, Result<Option<bool>, String>> {
        Box::pin(async move { self.repair_account(account, false).await })
    }
    fn project(self: Arc<Self>, account: Account, cwd: String) -> futures_util::future::BoxFuture<'static, Result<(), String>> {
        Box::pin(async move { self.prepare_project(&account.home, Path::new(&cwd)).await })
    }
    fn status(&self) -> Result<Value, String> { crate::codex_setup::service::Service::status(self) }
}

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
pub const PREPARED_MARKER: &str = ".hangar-prepared-v1";

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
    fn codex_preparation_service(&self) -> Result<Arc<dyn CodexSetup>, AccountError> {
        self.codex_setup.lock().unwrap_or_else(|error| error.into_inner()).as_ref().and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| AccountError::new(503, "codex_setup_unavailable", "o serviço de integração Codex não está disponível", json!({})))
    }

    async fn finish_codex_preparation(&self, account: &Account, source: &Value, mut result: Value) -> Value {
        merge_source_status(source, &mut result);
        if result["status"] == "ready" {
            let pending = match self.codex_preparation_service() {
                Ok(service) => service.repair(account.clone()).await,
                Err(_) => return failed("codex_setup_unavailable"),
            };
            match pending {
                Ok(Some(pending)) => result["trust_pending"] = json!(pending),
                Ok(None) => return failed("codex_cli_missing"),
                Err(_) => return failed("codex_account_prepare_failed"),
            }
        }
        result
    }
    async fn finish_project_preparation(&self, account: &Account, cwd: &str, result: Value) -> Value {
        if matches!(result["status"].as_str(), Some("ready" | "partial")) {
            let setup = match self.codex_preparation_service() {
                Ok(setup) => setup,
                Err(_) => return failed("codex_setup_unavailable"),
            };
            if setup.project(account.clone(), cwd.to_owned()).await.is_err() {
                tracing::warn!(code="codex_project_instructions_failed", "instruções do projeto não foram preparadas");
            }
        }
        result
    }

    pub async fn prepare_for_session(
        &self, account: &Account, bridge: AccountsBridge, deadline: tokio::time::Instant,
    ) -> Result<Value, AccountError> {
        let mut status = self.prepare(account, bridge.clone(), false, None).await?;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(AccountError::new(503,"codex_account_preparing",
                    "a conta continua em preparo; aguarde a conclusão e tente abrir novamente",json!({"account_id":account.id})));
            }
            match status["status"].as_str() {
                Some("ready")=>return Ok(status),
                Some("running")=>{},
                _=>return Err(AccountError::new(503,"codex_account_prepare_incomplete",
                    "o preparo da conta não terminou; confira as pendências na sincronização",json!({"account_id":account.id,"issues":status["issues"]}))),
            }
            // O preparo publica estado, mas não oferece assinatura; a espera só lê o estado e tem prazo.
            tokio::time::sleep_until((tokio::time::Instant::now()+Duration::from_millis(200)).min(deadline)).await;
            status = self.preparation_status(account,&bridge).await;
        }
    }

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
    pub(crate) fn stored_status(&self, account: &Account, key: &AccountKey) -> Value {
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
        available.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
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
        if account.is_default && account.home.join(PREPARED_MARKER).is_file() {
            json!({"status":"ready","trust_pending":false,"issues":[],"etapa":null,"herdado":null})
        } else {
            idle_sync()
        }
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
                    let mut result = service.finish_callback(&previous, &bridge).await;
                    if previous.cwd.is_none() {
                        result = match service.codex_preparation_service().and_then(|setup| setup.status().map_err(|_| AccountError::io())) {
                            Ok(source) => service.finish_codex_preparation(&account, &source, result).await,
                            Err(_) => failed("codex_setup_unavailable"),
                        };
                    } else if let Some(cwd) = &previous.cwd {
                        result = service.finish_project_preparation(&account, cwd, result).await;
                    }
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
        let force_pending = self.force_path(&key)?.try_exists().map_err(|_| AccountError::io())?;
        let marker = account.home.join(PREPARED_MARKER);
        let prepared = if !force && !force_pending && marker.is_file() {
            self.preparation_status(account, &bridge).await
        } else {
            Value::Null
        };
        {
            let mut states = self.preparations.lock().unwrap();
            if let Some(state) = states.get_mut(&key).filter(|state| state.running) {
                state.force_pending |= force;
                if force {
                    Self::persist(&self.force_path(&key)?, &json!(true))?;
                }
                return Ok(state.result.clone());
            }
            if prepared["status"] == "ready" {
                return Ok(prepared);
            }
            match std::fs::remove_file(&marker) {
                Ok(())=>{},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound=>{},
                Err(_)=>return Err(AccountError::io()),
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
                    if result["status"] == "ready" && Self::persist(&account.home.join(PREPARED_MARKER),&json!({"version":1})).is_err() {
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
    /// O preparo da pasta pendente de uma conta Claude, aqui mesmo: os eventos vão ao diário do
    /// servidor, e a resposta e o log levam o motivo de uma falha, o mesmo que o comando mostra. Bloqueante.
    pub fn seed_claude(&self, path: &Path) -> Result<(), AccountError> {
        let outcome = super::claude_config::seed(&self.env.home, path);
        crate::diag::record_blocking(crate::diag::diary_dir_in(&self.env).as_deref(), &outcome.diary, "conta");
        outcome.result.map(drop).map_err(|error| {
            tracing::warn!(%error, "o preparo da conta Claude falhou");
            AccountError::new(500, "account_prepare_failed", format!("não foi possível preparar a conta: {}", error.cause()), json!({}))
        })
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
        let source = if cwd.is_none() {
            if let Some(state) = self.preparations.lock().unwrap().get_mut(&key) {
                state.result["etapa"] = json!("principal");
            }
            Some(self.codex_preparation_service()?.update(force).await
                .map_err(|_| AccountError::io())?)
        } else { None };
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
        let result = if let Some(source) = source {
            self.finish_codex_preparation(account, &source, result).await
        } else { result };
        let result = if let Some(cwd) = &request.cwd {
            self.finish_project_preparation(account, cwd, result).await
        } else { result };
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

fn merge_source_status(source: &Value, result: &mut Value) {
    let optional = source["estado"] == "indisponivel" && source["etapa"]["codigo"] == "etapa_sem_claude";
    if source.is_object() && !optional && !matches!(source["estado"].as_str(), None | Some("ok" | "ocioso")) {
        if result["status"] == "ready" { result["status"] = json!("partial"); }
        if result.get("issues").is_none() { result["issues"] = json!([]); }
        if let Some(issues) = result["issues"].as_array_mut() {
            issues.insert(0, json!({"code":"codex_account_source_sync_incomplete","params":{"status":source["estado"]}}));
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn incomplete_source_preserves_account_issues_and_blocks_ready() {
        let mut result = json!({"status":"ready","issues":[{"code":"existing"}]});
        merge_source_status(&json!({"estado":"erro"}), &mut result);
        assert_eq!(result["status"], "partial");
        assert_eq!(result["issues"][0], json!({"code":"codex_account_source_sync_incomplete","params":{"status":"erro"}}));
        assert_eq!(result["issues"][1]["code"], "existing");
    }
    #[test]
    fn absent_claude_is_optional_but_other_unavailability_is_not() {
        let ready = json!({"status":"ready","issues":[]});
        let mut result = ready.clone();
        merge_source_status(&json!({"estado":"indisponivel","etapa":{"codigo":"etapa_sem_claude"}}), &mut result);
        assert_eq!(result, ready);
        merge_source_status(&json!({"estado":"indisponivel","etapa":{"codigo":"other"}}), &mut result);
        assert_eq!(result["status"], "partial");
    }
}
