//! Adaptação da transferência ao ciclo de vida e aos processos já possuídos pelo servidor.
use super::*;
use crate::accounts::AccountService;
use hangar_codex::client::{Client, Incoming};
use hangar_codex_transfer::{ImportOptions, NativeImport};
use hangar_conversation_transfer::{
    ImportBoundary, Store, TransferError, TransferPhase, TransferRecord, convert_snapshot,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

const VERSION: &str = hangar_codex_transfer::SUPPORTED_VERSION;
fn error(code: &str) -> TransferError {
    TransferError::new(code)
}
fn transfer_error(e: RuntimeError) -> TransferError {
    error(match e.code.as_str() {
        "erro_sessao_iniciando" | "erro_sessao_esperando_resposta" | "erro_sessao_trabalhando" => {
            "session_transfer_source_busy"
        }
        "erro_fila_pendente" => "session_transfer_queue_pending",
        _ => "session_transfer_source_changed",
    })
}
pub(crate) fn store(home: &Path) -> Store {
    Store::new(home.join(".hangar/conversation-transfers"))
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub credential_id: String,
    pub source_life: String,
    pub source_jsonl: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}
impl Request {
    pub(crate) fn valid(&self) -> bool {
        self.credential_id.starts_with("codex:")
            && !self.source_life.is_empty()
            && !self.source_jsonl.is_empty()
            && self.model.as_ref().is_none_or(|m| !m.trim().is_empty())
            && self.effort.as_deref().is_none_or(|e| {
                ["none", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&e)
            })
    }
}

fn permission(current: &Current) -> Result<(&'static str, &'static str), TransferError> {
    let mode = current
        .spec
        .permission_mode
        .map(PermissionMode::name)
        .unwrap_or("");
    let base = if mode == "plan" {
        current.previous_non_plan.as_deref().unwrap_or("")
    } else {
        mode
    };
    let permission = match base {
        "bypassPermissions" => "Full Access",
        "manual" | "default" => "Ask for approval",
        "acceptEdits" => "Approve for me",
        _ => return Err(error("session_transfer_invalid_permission_mode")),
    };
    Ok((permission, if mode == "plan" { "plan" } else { "default" }))
}

// Uma segunda recuperação não pode disputar os processos da primeira.
fn operation(home: &Path, name: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<(PathBuf, String), Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry((home.to_owned(), name.into()))
        .or_default()
        .clone()
}

fn processes(root: i64) -> Result<Value, TransferError> {
    let lives = tree(root).map_err(|_| error("session_transfer_process_identity_unknown"))?;
    let mut result = serde_json::Map::new();
    for (pid, birth) in lives {
        result.insert(
            pid.to_string(),
            json!({"started":birth,"command":command_digest(pid)?}),
        );
    }
    if !result.contains_key(&root.to_string()) {
        return Err(error("session_transfer_process_identity_unknown"));
    }
    Ok(Value::Object(result))
}

fn command_digest(pid: i64) -> Result<String, TransferError> {
    #[cfg(target_os = "linux")]
    let command = {
        let bytes = std::fs::read(format!("/proc/{pid}/cmdline"))
            .map_err(|_| error("session_transfer_process_identity_unknown"))?;
        String::from_utf8_lossy(&bytes).replace('\0', " ")
    };
    #[cfg(not(target_os = "linux"))]
    let command = SystemProcs::default()
        .argv_checked(pid)
        .map_err(|_| error("session_transfer_process_identity_unknown"))?
        .join(" ");
    if command.is_empty() {
        return Err(error("session_transfer_process_identity_unknown"));
    }
    Ok(digest(command.as_bytes()))
}

/// O processo `pid` gravado em `value` ainda é o mesmo: mesmo nascimento e mesmo comando.
fn recorded_alive(value: &Value, pid: i64) -> Result<bool, TransferError> {
    let identity = &value[pid.to_string()];
    let birth = identity["started"]
        .as_f64()
        .ok_or_else(|| error("session_transfer_invalid_record"))?;
    if super::super::lifecycle::process_absent(&SystemProcs::default(), pid, birth)
        .map_err(|_| error("session_transfer_process_identity_unknown"))?
    {
        return Ok(false);
    }
    Ok(command_digest(pid).ok().as_deref() == identity["command"].as_str())
}

async fn stop_recorded(value: &Value) -> Result<(), TransferError> {
    let values = value
        .as_object()
        .ok_or_else(|| error("session_transfer_invalid_record"))?;
    let mut lives = Vec::new();
    for (pid, identity) in values {
        let pid = pid
            .parse::<i64>()
            .map_err(|_| error("session_transfer_invalid_record"))?;
        let birth = identity["started"]
            .as_f64()
            .ok_or_else(|| error("session_transfer_invalid_record"))?;
        if !super::super::lifecycle::process_absent(&SystemProcs::default(), pid, birth)
            .map_err(|_| error("session_transfer_process_identity_unknown"))?
        {
            if identity["command"].as_str() != Some(command_digest(pid)?.as_str()) {
                return Err(error("session_transfer_process_identity_changed"));
            }
            lives.push((pid, birth));
        }
    }
    if lives.is_empty() {
        return Ok(());
    }
    force_kill(&lives);
    wait_gone(lives, Duration::from_secs(5))
        .await
        .map_err(|_| error("session_transfer_import_cleanup_unconfirmed"))
}

/// O cliente não executa ferramenta nem inferência durante a preparação.
struct ImportClient {
    client: Client,
    child: tokio::process::Child,
    tree: crate::terminal_process::CommandTree,
    drain: tokio::task::JoinHandle<()>,
    store: Store,
    record: TransferRecord,
    service: AccountService,
    models: Arc<crate::dictation::catalog::Models>,
    account_home: PathBuf,
}
impl NativeImport for ImportClient {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, TransferError> {
        self.client
            .request_method(method, params, Duration::from_secs(60))
            .await
            .map_err(|_| error("session_transfer_native_import_failed"))
    }
    async fn raw_model(&mut self, config: &Value, model: &str) -> Result<Value, TransferError> {
        self.models
            .raw_model(&self.service, &self.account_home, config, model, VERSION)
            .await
            .map_err(error)
    }
    async fn save_destination(
        &mut self,
        destination: Value,
        boundary: Option<ImportBoundary>,
    ) -> Result<(), TransferError> {
        self.record.destination_meta = Some(destination);
        self.record.boundary = boundary;
        self.store.save(&self.record)
    }
}
impl ImportClient {
    async fn open(
        service: AccountService,
        models: Arc<crate::dictation::catalog::Models>,
        account: &crate::accounts::catalog::Account,
        record: &TransferRecord,
        limit: usize,
    ) -> Result<Self, TransferError> {
        let store = store(&service.env.home);
        let mut private = store.runtime(&record.id)?;
        let mut command = service
            .codex_command()
            .ok_or_else(|| error("session_transfer_native_unavailable"))?;
        command
            .args([
                "app-server",
                "--stdio",
                "-c",
                &format!("tool_output_token_limit={limit}"),
            ])
            .env_clear()
            .envs(service.env.codex(account))
            .env("CP_SESSION_NAME", &record.name)
            .env(
                "CP_SESSION_KEY",
                record.origin_meta["key"].as_str().unwrap_or(""),
            )
            .current_dir(record.origin_meta["cwd"].as_str().unwrap_or(""))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for key in crate::terminal_process::PRIVATE_ENV_KEYS {
            command.env_remove(key);
        }
        command.env_remove("TMUX").env_remove("TMUX_PANE");
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command)
            .map_err(|_| error("session_transfer_native_unavailable"))?;
        // Gravado só aqui: daqui em diante um processo pode existir sem identidade registrada.
        private["import_launching"] = json!(true);
        store.write_runtime(&record.id, &private)?;
        let (client, mut incoming, mut child) = match Client::spawn_stdio(command) {
            Ok(started) => started,
            Err(_) => {
                private["import_launching"] = json!(false);
                private["import_stopped"] = json!(true);
                store.write_runtime(&record.id, &private)?;
                return Err(error("session_transfer_native_unavailable"));
            }
        };
        if tree.attach(&child).is_err() {
            let _ = child.start_kill();
            if child.wait().await.is_ok() {
                private["import_launching"] = json!(false);
                private["import_stopped"] = json!(true);
                store.write_runtime(&record.id, &private)?;
            }
            return Err(error("session_transfer_native_unavailable"));
        }
        let saved = match child.id() {
            Some(pid) => (|| {
                private["import_processes"] = processes(i64::from(pid))?;
                private["import_pid"] = json!(pid);
                private["import_launching"] = json!(false);
                store.write_runtime(&record.id, &private)
            })(),
            None => Err(error("session_transfer_process_identity_unknown")),
        };
        if let Err(e) = saved {
            crate::terminal_process::finish(&mut tree).await;
            // Só a espera confirmada prova que nada ficou vivo; sem ela a limpeza segue em aberto.
            if child.wait().await.is_ok() {
                private["import_launching"] = json!(false);
                private["import_stopped"] = json!(true);
                if let Err(write) = store.write_runtime(&record.id, &private) {
                    tracing::warn!(transfer=%record.id,code=%write.code,"importador parado sem registro gravado");
                }
            }
            return Err(e);
        }
        let responder = client.clone();
        let drain = tokio::spawn(async move {
            while let Some(event) = incoming.recv().await {
                if let Incoming::Request { id, .. } = event {
                    let _ = responder
                        .respond(
                            id,
                            Err((-32601, "a importação não executa ferramentas".into())),
                        )
                        .await;
                }
            }
        });
        Ok(Self {
            client,
            child,
            tree,
            drain,
            store,
            record: record.clone(),
            service,
            models,
            account_home: account.home.clone(),
        })
    }
    async fn close(&mut self) -> Result<(), TransferError> {
        let saved = (|| {
            let mut private = self.store.runtime(&self.record.id)?;
            if let Some(pid) = self.child.id()
                && let Ok(lives) = processes(i64::from(pid))
            {
                private["import_processes"] = lives;
                private["import_tree_final"] = json!(true);
                self.store.write_runtime(&self.record.id, &private)?;
            }
            Ok::<_, TransferError>(private)
        })();
        self.drain.abort();
        crate::terminal_process::finish(&mut self.tree).await;
        self.child
            .wait()
            .await
            .map_err(|_| error("session_transfer_import_cleanup_unconfirmed"))?;
        let mut private = saved?;
        private["import_stopped"] = json!(true);
        self.store.write_runtime(&self.record.id, &private)?;
        Ok(())
    }
}

async fn check_target(
    service: &AccountService,
    id: &str,
) -> Result<crate::accounts::catalog::Account, TransferError> {
    let accounts = service
        .visible_codex_accounts()
        .map_err(|_| error("session_transfer_unknown_account").with_status(400))?;
    let account = accounts
        .into_iter()
        .find(|a| {
            format!(
                "codex:{}",
                crate::accounts::catalog::resolved(&a.home).display()
            ) == id
        })
        .ok_or_else(|| error("session_transfer_unknown_account").with_status(400))?;
    let auth = service.read_codex_auth(&account).await;
    if auth["status"] != "connected" {
        return Err(error("session_transfer_login_required"));
    }
    let quota = service
        .quotas(None, false, false)
        .await
        .map_err(|_| error("session_transfer_account_unavailable"))?;
    if quota
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["id"] == id && row["estado"] == "lida")
        .any(|row| {
            row["janelas"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|w| w["pct"].as_f64().is_some_and(|p| p >= 99.0))
        })
    {
        return Err(error("session_transfer_account_full"));
    }
    let mut command = service
        .codex_command()
        .ok_or_else(|| error("session_transfer_native_unavailable"))?;
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|o| o.status.success())
        .ok_or_else(|| error("session_transfer_native_unavailable"))?;
    if !String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .any(|s| s == VERSION)
    {
        return Err(error("session_transfer_codex_version_unsupported"));
    }
    Ok(account)
}

impl RuntimeRegistry {
    pub(crate) async fn transfer_key(&self, name: &str) -> Result<Option<String>, TransferError> {
        if let Some(spec) =
            crate::session_open::record::from_sidecars(&self.codex_environment().home, name)
                .map_err(|_| error("session_transfer_source_changed"))?
        {
            return Ok(Some(spec.key));
        }
        if let Some((key, _, _, _)) = self.terminal_life(name).await {
            return Ok(Some(key));
        }
        if let Some(watcher) = self.watcher_ref() {
            let target = crate::session_open::terminal::target(cfg!(windows), name);
            if let Some(shown) = watcher
                .mux_text(&["show-environment", "-t", &target, "CP_SESSION_KEY"])
                .await
            {
                return Ok(shown
                    .trim()
                    .strip_prefix("CP_SESSION_KEY=")
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned));
            }
        }
        crate::session_open::record::live(&self.codex_environment().home, name)
            .map(|spec| spec.map(|spec| spec.key))
            .map_err(|_| error("session_transfer_source_changed"))
    }
    pub(crate) async fn transfer_blocked(&self, name: &str) -> bool {
        let records = match store(&self.codex_environment().home).incomplete_for_session(name) {
            Ok(records) => records,
            Err(failure) => {
                tracing::warn!(session=%name,code=%failure.code,"registros desta transferência não puderam ser lidos");
                return true;
            }
        };
        if records.is_empty() {
            return false;
        }
        let key = match self.transfer_key(name).await {
            Ok(key) => key,
            Err(_) => return true,
        };
        records
            .iter()
            .any(|r| key.as_ref().is_none_or(|key| r.origin_meta["key"] == *key))
    }
    pub(crate) async fn pending_transfer_lives(
        &self,
    ) -> Result<HashMap<String, String>, TransferError> {
        let mut lives = HashMap::new();
        let scan = store(&self.codex_environment().home).scan_incomplete()?;
        for id in scan.invalid_ids {
            tracing::warn!(transfer=%id,code="session_transfer_invalid_record","registro inválido excluído dos fatos de outras sessões");
        }
        for record in scan.records {
            match self.transfer_key(&record.name).await {
                Ok(key)
                    if key
                        .as_ref()
                        .is_none_or(|key| record.origin_meta["key"] == *key) =>
                {
                    lives.insert(record.name, record.source_life);
                }
                Ok(_) => {}
                Err(failure) => {
                    tracing::warn!(transfer=%record.id,session=%record.name,code=%failure.code,"identidade da transferência não pôde ser conferida")
                }
            }
        }
        Ok(lives)
    }
    pub(crate) async fn pending_transfer_snapshot(
        &self,names:std::collections::HashSet<String>,
    )->Result<(HashMap<String,String>,std::collections::HashSet<String>),TransferError> {
        let lives=self.pending_transfer_lives().await?;
        let store=store(&self.codex_environment().home);
        let mut uncertain=std::collections::HashSet::new();
        for name in names {
            let unreadable=match store.incomplete_for_session(&name) {
                Ok(records)=>!records.is_empty() && self.transfer_key(&name).await.is_err(),
                Err(failure)=>{tracing::warn!(session=%name,code=%failure.code,"convite conservado até conferir a transferência desta sessão");true},
            };
            if unreadable {uncertain.insert(name);}
        }
        Ok((lives,uncertain))
    }
    /// Vida de origem da transferência pendente só desta sessão, e se o registro dela ficou sem conferir.
    /// Sem registro incompleto não consulta o multiplexador.
    pub(crate) async fn pending_transfer_life(&self,name:&str)->(Option<String>,bool) {
        let records=match store(&self.codex_environment().home).incomplete_for_session(name) {
            Ok(records)=>records,
            Err(failure)=>{tracing::warn!(session=%name,code=%failure.code,"convite conservado até conferir a transferência desta sessão");return (None,true)},
        };
        if records.is_empty() {return (None,false);}
        match self.transfer_key(name).await {
            Ok(key)=>(records.into_iter().filter(|r|key.as_ref().is_none_or(|key|r.origin_meta["key"]==*key)).last().map(|r|r.source_life),false),
            Err(failure)=>{tracing::warn!(session=%name,code=%failure.code,"identidade da transferência não pôde ser conferida");(None,true)},
        }
    }
    pub(crate) async fn transfer_to_codex(
        &self,
        name: &str,
        request: Request,
        service: AccountService,
        models: Arc<crate::dictation::catalog::Models>,
    ) -> Result<Value, TransferError> {
        let env = self.codex_environment();
        let store = store(&env.home);
        let op = operation(&env.home, name);
        let _op = op.try_lock().map_err(|_| error("session_transfer_busy"))?;
        if self.transfer_blocked(name).await {
            return Err(error("session_transfer_busy"));
        }
        self.ingress()
            .hold_transfer(name, Duration::from_secs(60))
            .await
            .map_err(|_| error("session_transfer_gate_unavailable").with_status(503))?;
        let result = self
            .transfer_locked(name, request, service, models, &store)
            .await;
        if !self.transfer_blocked(name).await {
            self.ingress().release_transfer(name);
        }
        result
    }

    async fn transfer_locked(
        &self,
        name: &str,
        request: Request,
        service: AccountService,
        models: Arc<crate::dictation::catalog::Models>,
        store: &Store,
    ) -> Result<Value, TransferError> {
        let current = self
            .claude_current(name, BusyCheck::Refuse, false)
            .await
            .map_err(transfer_error)?;
        if current.spec.read_only {
            return Err(error("session_transfer_read_only"));
        }
        if matches!(current.spec.account, Account::Engine { .. }) {
            return Err(error("session_transfer_source_changed"));
        }
        let env = self.codex_environment();
        let source = match &current.life {
            Life::Terminal { target } => PathBuf::from(&target.transcript),
            _ => super::super::claude_launch::transcript(&env, &current.spec, &current.sid),
        };
        let source_life_matches = request.source_life == format!("k:{}", current.spec.key)
            || matches!(&current.life,Life::Terminal{target} if request.source_life==format!("t:{}",target.binding.created));
        if !source_life_matches
            || crate::accounts::catalog::resolved(&source)
                != crate::accounts::catalog::resolved(Path::new(&request.source_jsonl))
        {
            return Err(error("session_transfer_source_changed"));
        }
        let (permission, collaboration) = permission(&current)?;
        let account = check_target(&service, &request.credential_id).await?;
        let (catalog, _) = models
            .get(
                &service,
                &tokio::sync::Semaphore::new(1),
                "codex",
                &account.home,
                false,
            )
            .await
            .map_err(|_| error("session_transfer_native_unavailable"))?;
        crate::dictation::catalog::validate(
            &catalog,
            request.model.as_deref(),
            request.effort.as_deref(),
            None,
        )
        .map_err(|_| error("session_transfer_invalid_model_choice"))?;
        self.transfer_queue_empty(name, &current.spec.key).await?;
        // Reconfere sob a mesma porta depois das consultas à conta.
        let fresh = self
            .claude_current(name, BusyCheck::Refuse, false)
            .await
            .map_err(transfer_error)?;
        if fresh.spec.key != current.spec.key || fresh.sid != current.sid {
            return Err(error("session_transfer_source_changed"));
        }
        let origin = json!({"name":name,"provider":"claude","cwd":current.spec.cwd,"jsonl":source,"session_id":current.sid,"key":current.spec.key,
            "config_dir":current.spec.account.config_dir().map_or_else(||env.claude_base.to_string_lossy().into_owned(),str::to_owned),
            "headless":current.spec.surface==Surface::Headless,"model":current.spec.model,"effort":current.spec.effort,
            "permission_mode":current.spec.permission_mode.map(PermissionMode::name),"previous_non_plan":current.previous_non_plan,
            "subagent_model":current.spec.subagent_model,"jev":current.spec.jev});
        let id = crate::session_open::open::uuid4(&crate::mods::state::random_hex(16));
        let mut record = TransferRecord {
            id,
            name: name.into(),
            source_life: request.source_life,
            phase: TransferPhase::Preparing,
            source: None,
            origin_meta: origin,
            destination_meta: None,
            boundary: None,
            error_code: None,
        };
        let root = match &current.life {
            Life::Terminal { target } => i64::from(target.metadata.agent_pid),
            Life::Headless { meta, .. } => meta["cano"]["pid"].as_i64().unwrap_or(0),
            Life::Bare { pane_pid } => *pane_pid,
        };
        let lives = if root > 0 {
            processes(root)?
        } else {
            json!({})
        };
        let original = match &current.life {
            Life::Headless { meta, .. } => meta.clone(),
            _ => record.origin_meta.clone(),
        };
        store.prepare_runtime(&record.id, original, lives)?;
        let mut private = store.runtime(&record.id)?;
        private["spec"] = serde_json::to_value(&current.spec)
            .map_err(|_| error("session_transfer_invalid_record"))?;
        private["settings"] = current.settings.clone().unwrap_or(Value::Null);
        if root > 0 {
            private["root_pid"] = json!(root);
        }
        store.write_runtime(&record.id, &private)?;
        store.save(&record)?;
        let outcome=async {
            self.stop_claude(name,&current,Surface::Terminal).await.map_err(|_|error("session_transfer_source_not_stopped"))?;
            private["source_stopped"]=json!(true);store.write_runtime(&record.id,&private)?;
            record.phase=TransferPhase::SourceStopped;store.save(&record)?;
            record=store.capture_snapshot(&record,&source,Vec::new())?;
            let context=convert_snapshot(Path::new(&record.source.as_ref().ok_or_else(||error("session_transfer_invalid_record"))?.path))?;
            if record.source.as_ref().is_none_or(|source|source.digest!=context.source_digest) {return Err(error("session_transfer_source_changed"));}
            hangar_conversation_transfer::verify_source(record.source.as_ref().unwrap())?;
            record.source.as_mut().unwrap().selected_uuids=context.selected_uuids.iter().cloned().collect();store.save(&record)?;
            let options=ImportOptions{cwd:current.spec.cwd.clone(),account_home:account.home.to_string_lossy().into_owned(),account_id:account.id.clone(),transfer_id:record.id.clone(),model:request.model,effort:request.effort,permission_mode:permission.into(),collaboration_mode:collaboration.into()};
            let mut client=ImportClient::open(service,models,&account,&record,hangar_codex_transfer::import_output_limit(&context)).await?;
            let imported=async {
                client.request("initialize",json!({"clientInfo":{"name":"hangar","title":null,"version":"0.1.0"},"capabilities":{"experimentalApi":true}})).await?;
                client.client.notify("initialized",json!({})).await.map_err(|_|error("session_transfer_native_unavailable"))?;
                hangar_codex_transfer::prepare_import(&mut client,&options,&context).await
            }.await;
            let cleanup=client.close().await;
            let prepared=imported?;cleanup?;
            record=store.load(&record.id)?.ok_or_else(||error("session_transfer_invalid_record"))?;
            let mut meta=record.destination_meta.clone().ok_or_else(||error("session_transfer_invalid_record"))?;
            for (key,value) in [("name",json!(name)),("provider",json!("codex")),("key",json!(current.spec.key)),("headless",json!(current.spec.surface==Surface::Headless)),("jev",json!(current.spec.jev)),("lifecycle_id",json!(format!("k:{}",current.spec.key))),("previous_non_plan",if collaboration=="plan" {json!(permission)} else {Value::Null})] {meta[key]=value;}
            record.destination_meta=Some(meta.clone());record.phase=TransferPhase::Imported;store.save(&record)?;
            meta["collaboration_mode"]=json!(collaboration);
            let watcher=self.watcher_ref().ok_or_else(||error("session_transfer_native_unavailable"))?;
            let _round=watcher.misses.lock().await;
            watcher.adopt_transfer(self,name,meta.as_object().ok_or_else(||error("session_transfer_invalid_record"))?).await.map_err(|_|error("session_transfer_native_import_failed"))?;
            record.phase=TransferPhase::Publishing;store.save(&record)?;
            if let Some(snapshot)=&record.source {hangar_conversation_transfer::verify_source(snapshot)?;if digest(&std::fs::read(&source).map_err(|_|error("session_transfer_source_changed"))?)!=snapshot.digest {return Err(error("session_transfer_source_changed"));}}
            let mut next=current.spec.clone();next.provider=crate::session_open::AgentProvider::Codex;next.account=Account::Codex{account_id:account.id.clone()};next.model=Some(prepared.model.clone());next.effort=prepared.effort.clone();next.conversation=Some(prepared.thread_id.clone());next.permission_mode=PermissionMode::codex(permission);next.subagent_model=None;next.customizations=None;
            crate::session_open::record::write(&env.home,&next).map_err(|_|error("session_transfer_publish_failed"))?;
            self.remove_claude_sidecar(name).await.map_err(|_|error("session_transfer_publish_failed"))?;
            let mut complete=record.clone();complete.phase=TransferPhase::Complete;
            crate::speech::history::transfer_history::verify_boundary(&serde_json::to_value(&complete).map_err(|_|error("session_transfer_invalid_record"))?,Path::new(&prepared.rollout_path),&account.home).map_err(|_|error("session_transfer_import_persistence_mismatch"))?;
            record.phase=TransferPhase::Complete;store.save(&record)?;
            Ok(json!({"ok":true,"provider":"codex","conta":request.credential_id,"model":prepared.model,"effort":prepared.effort,"transfer_id":record.id}))
        }.await;
        if let Err(cause) = outcome {
            record = match store.load(&record.id) {
                Ok(saved) => saved.unwrap_or(record),
                Err(failure) => {
                    tracing::warn!(transfer=%record.id,code=%failure.code,"registro não pôde ser relido; restauração usa a etapa confirmada em memória");
                    record
                }
            };
            record.error_code = Some(cause.code.clone());
            self.restore_transfer_record(store, &mut record).await?;
            return Err(cause);
        }
        outcome
    }

    async fn transfer_queue_empty(&self, name: &str, key: &str) -> Result<(), TransferError> {
        let queue = self.codex_environment().claude_base.join(".hangar-queue");
        let (state, _) = super::super::codex_terminal::queue_files(&queue, key);
        let rows = match super::super::queue::read_replaced(&state) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .map_err(|_| error("session_transfer_queue_pending"))?["rows"]
                .clone(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let path = queue.join(format!("{name}.jsonl"));
                match std::fs::read_to_string(path) {
                    Ok(raw) => Value::Array(
                        raw.lines()
                            .map(serde_json::from_str)
                            .collect::<Result<Vec<Value>, _>>()
                            .map_err(|_| error("session_transfer_queue_pending"))?,
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!([]),
                    Err(_) => return Err(error("session_transfer_queue_pending")),
                }
            }
            Err(_) => return Err(error("session_transfer_queue_pending")),
        };
        if rows
            .as_array()
            .ok_or_else(|| error("session_transfer_queue_pending"))?
            .iter()
            .any(|r| r["delivered"] != true || r["confirmed"] != true)
        {
            return Err(error("session_transfer_queue_pending"));
        }
        Ok(())
    }

    pub(crate) async fn recover_transfers(&self) {
        let store = store(&self.codex_environment().home);
        match store.scan_incomplete() {
            Ok(scan) => {
                for id in scan.invalid_ids {
                    tracing::warn!(transfer=%id,code="session_transfer_invalid_record","registro inválido não pôde ser recuperado");
                }
                for record in scan.records {
                    if let Err(e) = self.recover_transfer(record).await {
                        tracing::warn!(code=%e.code,"transferência interrompida não pôde ser recuperada");
                    }
                }
            }
            Err(e) => tracing::warn!(code=%e.code,"registros de transferência ilegíveis"),
        }
    }
    pub(crate) async fn recover_transfer(
        &self,
        mut record: TransferRecord,
    ) -> Result<Value, TransferError> {
        let env = self.codex_environment();
        let store = store(&env.home);
        let op = operation(&env.home, &record.name);
        let _op = op.try_lock().map_err(|_| error("session_transfer_busy"))?;
        if self
            .transfer_key(&record.name)
            .await?
            .as_ref()
            .is_some_and(|key| record.origin_meta["key"] != *key)
        {
            record.phase = TransferPhase::RestoreFailed;
            record.error_code = Some("session_transfer_source_changed".into());
            store.save(&record)?;
            return Err(error("session_transfer_source_changed"));
        }
        // Uma recuperação anterior pode ter conservado a retenção na mesma instância.
        if !self.ingress().held(&record.name) {
            self.ingress()
                .hold_transfer(&record.name, Duration::from_secs(60))
                .await
                .map_err(|_| error("session_transfer_gate_unavailable"))?;
        }
        self.restore_transfer_record(&store, &mut record).await?;
        self.ingress().release_transfer(&record.name);
        Ok(json!({"ok":true,"provider":"claude","transfer_id":record.id}))
    }

    async fn restore_transfer_record(
        &self,
        store: &Store,
        record: &mut TransferRecord,
    ) -> Result<(), TransferError> {
        if let Some(failure) = store.begin_restore(record)? {
            tracing::warn!(transfer=%record.id,code=%failure.code,cause=?record.error_code,"etapa de restauração não gravada; origem ainda será recuperada");
        }
        let restored = self.restore_transfer_inner(store, record).await;
        store.finish_restore(record, restored)
    }

    async fn restore_transfer_inner(
        &self,
        store: &Store,
        record: &TransferRecord,
    ) -> Result<(), TransferError> {
        let mut private = store.runtime(&record.id)?;
        if private["import_launching"] == true {
            return Err(error("session_transfer_import_cleanup_unconfirmed"));
        }
        if private["import_stopped"] != true
            && let Some(pid) = private["import_pid"].as_i64()
        {
            let recorded = private["import_processes"][pid.to_string()]["started"]
                .as_f64()
                .ok_or_else(|| error("session_transfer_process_identity_unknown"))?;
            let absent =
                super::super::lifecycle::process_absent(&SystemProcs::default(), pid, recorded)
                    .map_err(|_| error("session_transfer_process_identity_unknown"))?;
            if !absent {
                if private["import_processes"][pid.to_string()]["command"].as_str()
                    != Some(command_digest(pid)?.as_str())
                {
                    return Err(error("session_transfer_process_identity_changed"));
                }
                let live = processes(pid)?;
                let saved = private["import_processes"]
                    .as_object_mut()
                    .ok_or_else(|| error("session_transfer_invalid_record"))?;
                saved.extend(live.as_object().cloned().unwrap_or_default());
                private["import_tree_final"] = json!(true);
                store.write_runtime(&record.id, &private)?;
            } else if private["import_tree_final"] != true {
                return Err(error("session_transfer_import_cleanup_unconfirmed"));
            }
            stop_recorded(&private["import_processes"]).await?;
            private["import_stopped"] = json!(true);
            store.write_runtime(&record.id, &private)?;
        }
        let name = &record.name;
        let env = self.codex_environment();
        let spec: SessionSpec = if private.get("spec").is_some() {
            serde_json::from_value(private["spec"].clone())
                .map_err(|_| error("session_transfer_invalid_record"))?
        } else {
            let mut spec =
                crate::session_open::record::claude_from_meta(&record.origin_meta, name, 0.0)
                    .map_err(|_| error("session_transfer_invalid_record"))?;
            if record.origin_meta["headless"] == false {
                spec.surface = Surface::Terminal;
            }
            spec
        };
        // A vida nova com o mesmo nome pertence a outra sessão e não é encerrada.
        if let Some(current) = crate::session_open::record::live(&env.home, name)
            .map_err(|_| error("session_transfer_source_changed"))?
            && current.key != spec.key
        {
            return Err(error("session_transfer_source_changed"));
        }
        if let Some(meta) = super::super::codex_sidecar::read(&super::super::codex_sidecar::path(
            &super::super::codex_sidecar::dir(&env.home),
            name,
        )).map_err(|_| error("session_transfer_import_cleanup_unconfirmed"))? {
            if meta.get("transfer_id").and_then(Value::as_str) != Some(&record.id) {
                return Err(error("session_transfer_source_changed"));
            }
            self.watcher_ref()
                .ok_or_else(|| error("session_transfer_native_unavailable"))?
                .abort_transfer(self, name, &record.id)
                .await
                .map_err(|_| error("session_transfer_import_cleanup_unconfirmed"))?;
        }
        if let Some(source) = &record.source {
            hangar_conversation_transfer::verify_source(source)?;
            let raw = std::fs::read(
                record.origin_meta["jsonl"]
                    .as_str()
                    .ok_or_else(|| error("session_transfer_invalid_record"))?,
            )
            .map_err(|_| error("session_transfer_source_changed"))?;
            if digest(&raw) != source.digest {
                return Err(error("session_transfer_source_changed"));
            }
        }
        let sid = record.origin_meta["session_id"]
            .as_str()
            .ok_or_else(|| error("session_transfer_invalid_record"))?
            .to_owned();
        // Uma parada recusada conserva a origem viva: não abre outra por cima dela.
        if let Ok(current) = self.claude_current(name, BusyCheck::Skip, false).await {
            if current.spec.key != spec.key || current.sid != sid {
                return Err(error("session_transfer_source_changed"));
            }
            let alive = match &current.life {
                Life::Terminal { .. } => true,
                Life::Headless { .. } => self
                    .claude_headless_view(name)
                    .await
                    .is_some_and(|(_, view)| view["alive"] == true),
                _ => false,
            };
            if alive {
                return Ok(());
            }
        }
        // Na subida a origem ainda não foi adotada; sem parada confirmada, a raiz viva prova que ela segue.
        if private["source_stopped"] != true
            && let Some(root) = private["root_pid"].as_i64()
            && recorded_alive(&private["processes"], root)?
        {
            return Ok(());
        }
        stop_recorded(&private["processes"]).await?;
        let generation = self
            .queue_generation(&spec.key)
            .await
            .map_err(transfer_error)?;
        let original = private["original"].clone();
        let current = Current {
            spec: spec.clone(),
            sid: sid.clone(),
            generation,
            life: Life::Headless {
                meta: original,
                open: false,
            },
            previous_non_plan: record.origin_meta["previous_non_plan"]
                .as_str()
                .map(str::to_owned),
            settings: private.get("settings").filter(|v| !v.is_null()).cloned(),
        };
        let start = self
            .prepare_start(name, &spec, &current, &sid)
            .await
            .map_err(|_| error("session_transfer_restore_failed"))?;
        self.start_claude(name, &spec, start, &sid)
            .await
            .map_err(|_| error("session_transfer_restore_failed"))?;
        crate::session_open::record::write(&env.home, &spec)
            .map_err(|_| error("session_transfer_restore_failed"))?;
        Ok(())
    }
}

fn digest(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_never_reopens_or_blocks_an_old_life_over_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), home.to_string_lossy().into_owned())].into(),
        );
        let runtime = RuntimeRegistry::new(
            "127.0.0.1:9".parse().unwrap(),
            "fixture".into(),
            "fixture".into(),
        )
        .with_codex_environment(env);
        let origin = json!({"name":"session","provider":"claude","key":"11111111111111111111111111111111","cwd":home,"config_dir":home.join(".claude"),"session_id":"original","jsonl":home.join("original.jsonl"),"headless":true,"permission_mode":"default"});
        let original =
            crate::session_open::record::claude_from_meta(&origin, "session", 1.0).unwrap();
        let mut newer = original.clone();
        newer.key = "22222222222222222222222222222222".into();
        crate::session_open::record::write(home, &newer).unwrap();
        let mut newer_meta = origin.clone();
        newer_meta["key"] = json!(newer.key);
        super::super::super::codex_sidecar::put(
            &home.join(".hangar/claude-headless"),
            "session",
            newer_meta.as_object().unwrap(),
        )
        .unwrap();
        let record = TransferRecord {
            id: "01234567-89ab-4cde-8fab-0123456789ab".into(),
            name: "session".into(),
            source_life: format!("k:{}", original.key),
            phase: TransferPhase::SourceStopped,
            source: None,
            origin_meta: origin.clone(),
            destination_meta: None,
            boundary: None,
            error_code: None,
        };
        let store = store(home);
        store.save(&record).unwrap();
        store
            .prepare_runtime(&record.id, origin, json!({}))
            .unwrap();
        let mut private = store.runtime(&record.id).unwrap();
        private["spec"] = serde_json::to_value(original).unwrap();
        store.write_runtime(&record.id, &private).unwrap();
        let error = runtime.recover_transfer(record.clone()).await.unwrap_err();
        assert_eq!(error.code, "session_transfer_source_changed");
        assert_eq!(
            store.load(&record.id).unwrap().unwrap().phase,
            TransferPhase::RestoreFailed
        );
        assert!(
            runtime
                .ingress()
                .enter("session", Duration::ZERO)
                .await
                .is_ok()
        );
        assert!(!runtime.transfer_blocked("session").await);
        assert_eq!(
            crate::session_open::record::live(home, "session")
                .unwrap()
                .unwrap()
                .key,
            newer.key
        );
    }

    #[tokio::test]
    async fn recovery_does_not_restore_while_an_untracked_import_may_still_be_alive() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), home.to_string_lossy().into_owned())].into(),
        );
        let runtime = RuntimeRegistry::new(
            "127.0.0.1:9".parse().unwrap(),
            "fixture".into(),
            "fixture".into(),
        )
        .with_codex_environment(env);
        let record = TransferRecord {
            id: "11234567-89ab-4cde-8fab-0123456789ab".into(),
            name: "session".into(),
            source_life: "k:key".into(),
            phase: TransferPhase::SourceStopped,
            source: None,
            origin_meta: json!({"key":"key"}),
            destination_meta: None,
            boundary: None,
            error_code: None,
        };
        let store = store(home);
        store.save(&record).unwrap();
        store
            .write_runtime(&record.id, &json!({"import_launching":true}))
            .unwrap();
        let error = runtime.recover_transfer(record.clone()).await.unwrap_err();
        assert_eq!(
            error.params["phase"],
            "session_transfer_import_cleanup_unconfirmed"
        );
        assert!(runtime.ingress().held("session"));
        assert_eq!(
            store.load(&record.id).unwrap().unwrap().phase,
            TransferPhase::RestoreFailed
        );
        assert!(runtime.list_snapshots_readonly().await.is_empty());
    }

    #[tokio::test]
    async fn import_that_never_spawned_does_not_block_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // Sem PATH no ambiente não há `codex`: a importação desiste antes de lançar processo.
        let service = AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), home.to_string_lossy().into_owned())].into(),
        ));
        let record = TransferRecord {
            id: "41234567-89ab-4cde-8fab-0123456789ab".into(),
            name: "session".into(),
            source_life: "k:key".into(),
            phase: TransferPhase::SourceStopped,
            source: None,
            origin_meta: json!({"key":"key"}),
            destination_meta: None,
            boundary: None,
            error_code: None,
        };
        let store = store(home);
        store.save(&record).unwrap();
        store.prepare_runtime(&record.id, json!({}), json!({})).unwrap();
        let account = crate::accounts::catalog::Account {
            id: "default".into(),
            home: home.join(".codex"),
            is_default: true,
        };
        let failure = ImportClient::open(service, Default::default(), &account, &record, 1)
            .await
            .err()
            .unwrap();
        assert_eq!(failure.code, "session_transfer_native_unavailable");
        assert!(store.runtime(&record.id).unwrap().get("import_launching").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_recovery_keeps_a_source_that_was_never_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), home.to_string_lossy().into_owned())].into(),
        );
        let runtime = RuntimeRegistry::new(
            "127.0.0.1:9".parse().unwrap(),
            "fixture".into(),
            "fixture".into(),
        )
        .with_codex_environment(env);
        let mut source = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let root = i64::from(source.id());
        let origin = json!({"name":"session","provider":"claude","key":"11111111111111111111111111111111","cwd":home,"config_dir":home.join(".claude"),"session_id":"original","jsonl":home.join("original.jsonl"),"headless":true,"permission_mode":"default"});
        let spec = crate::session_open::record::claude_from_meta(&origin, "session", 1.0).unwrap();
        let record = TransferRecord {
            id: "51234567-89ab-4cde-8fab-0123456789ab".into(),
            name: "session".into(),
            source_life: format!("k:{}", spec.key),
            phase: TransferPhase::Preparing,
            source: None,
            origin_meta: origin.clone(),
            destination_meta: None,
            boundary: None,
            error_code: None,
        };
        let store = store(home);
        store.save(&record).unwrap();
        store
            .prepare_runtime(&record.id, origin, processes(root).unwrap())
            .unwrap();
        let mut private = store.runtime(&record.id).unwrap();
        private["spec"] = serde_json::to_value(spec).unwrap();
        private["root_pid"] = json!(root);
        store.write_runtime(&record.id, &private).unwrap();
        // A sessão ainda não foi adotada: só a identidade gravada da raiz diz que ela segue viva.
        runtime.recover_transfer(record).await.unwrap();
        assert!(source.try_wait().unwrap().is_none(), "a origem viva não é encerrada");
        let _ = source.kill();
        let _ = source.wait();
    }

    #[tokio::test]
    async fn corrupt_transfer_is_local_and_does_not_prevent_other_recoveries() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), home.to_string_lossy().into_owned())].into(),
        );
        let runtime = RuntimeRegistry::new(
            "127.0.0.1:9".parse().unwrap(),
            "fixture".into(),
            "fixture".into(),
        )
        .with_codex_environment(env);
        let store = store(home);
        let record = |id: &str, name: &str| TransferRecord {
            id: id.into(),
            name: name.into(),
            source_life: "k:key".into(),
            phase: TransferPhase::SourceStopped,
            source: None,
            origin_meta: json!({"key":"key"}),
            destination_meta: None,
            boundary: None,
            error_code: None,
        };
        let broken = record("21234567-89ab-4cde-8fab-0123456789ab", "broken");
        let healthy = record("31234567-89ab-4cde-8fab-0123456789ab", "recoverable");
        store.save(&broken).unwrap();
        store.save(&healthy).unwrap();
        store
            .write_runtime(&healthy.id, &json!({"import_launching":true}))
            .unwrap();
        std::fs::write(
            home.join(format!(".hangar/conversation-transfers/{}.json", broken.id)),
            b"{",
        )
        .unwrap();
        assert!(runtime.transfer_blocked("broken").await);
        assert!(!runtime.transfer_blocked("unrelated").await);
        let lives = runtime.pending_transfer_lives().await.unwrap();
        assert_eq!(lives.get("recoverable").map(String::as_str), Some("k:key"));
        assert!(!lives.contains_key("broken"));
        let (_,uncertain)=runtime.pending_transfer_snapshot(["broken".into(),"recoverable".into(),"unrelated".into()].into()).await.unwrap();
        assert_eq!(uncertain,["broken".into()].into());
        runtime.recover_transfers().await;
        assert_eq!(
            store.load(&healthy.id).unwrap().unwrap().phase,
            TransferPhase::RestoreFailed
        );
        assert!(runtime.ingress().held("recoverable"));
        assert!(!runtime.transfer_blocked("unrelated").await);
    }

    #[test]
    fn transfer_body_requires_both_source_identifiers_and_never_accepts_another_target() {
        let base = json!({"credential_id":"codex:/account","source_life":"k:key","source_jsonl":"/source.jsonl"});
        assert!(
            serde_json::from_value::<Request>(base.clone())
                .unwrap()
                .valid()
        );
        for key in ["source_life", "source_jsonl"] {
            let mut wrong = base.clone();
            wrong.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<Request>(wrong).is_err());
        }
        let mut wrong = base.clone();
        wrong["config_dir"] = json!("/claude");
        assert!(serde_json::from_value::<Request>(wrong).is_err());
        let mut wrong = base;
        wrong["credential_id"] = json!("claude:/account");
        assert!(!serde_json::from_value::<Request>(wrong).unwrap().valid());
    }
}
