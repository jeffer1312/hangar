//! Leitor de identidade isolado; compartilha o cliente JSON-RPC com as sessões.
use super::{
    AccountKey, GuardMode, Provider,
    catalog::{Account, AccountService, codex_dto},
};
use hangar_codex::{
    client::{Client, Incoming},
    proto::{ClientInfo, ClientRequest, GetAccountParams, InitializeParams},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

type Signature = Vec<Option<Vec<u8>>>;
struct Cached {
    signature: Signature,
    generation: u64,
    at: Instant,
    value: Value,
}
#[derive(Default)]
struct CacheState {
    entries: HashMap<AccountKey, Cached>,
    generations: HashMap<AccountKey, u64>,
    refreshing: std::collections::HashSet<AccountKey>,
}
#[derive(Clone, Default)]
pub struct AuthCache(Arc<Mutex<CacheState>>);
impl AuthCache {
    pub fn invalidate(&self, key: &AccountKey) {
        let mut cache = self.0.lock().unwrap();
        cache.entries.remove(key);
        *cache.generations.entry(key.clone()).or_default() += 1;
    }
    pub(crate) fn signature(account: &Account) -> std::io::Result<Signature> {
        ["auth.json", "config.toml"]
            .iter()
            .map(|name| match std::fs::read(account.home.join(name)) {
                Ok(bytes) => Ok(Some(
                    ring::digest::digest(&ring::digest::SHA256, &bytes)
                        .as_ref()
                        .to_vec(),
                )),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            })
            .collect()
    }
    fn get(&self, key: &AccountKey, signature: &Signature) -> Option<Value> {
        let cache = self.0.lock().unwrap();
        let row = cache.entries.get(key)?;
        let ttl = if row.value["status"] == "unavailable" {
            20
        } else {
            60
        };
        (row.signature == *signature
            && row.generation == *cache.generations.get(key).unwrap_or(&0)
            && row.at.elapsed() < Duration::from_secs(ttl))
        .then(|| row.value.clone())
    }
    /// Último valor da mesma identidade, vencido ou não; troca de credencial nunca o reaproveita.
    fn known(&self, key: &AccountKey, signature: &Signature) -> Option<Value> {
        let cache = self.0.lock().unwrap();
        let row = cache.entries.get(key)?;
        (row.signature == *signature
            && row.generation == *cache.generations.get(key).unwrap_or(&0))
        .then(|| row.value.clone())
    }
    pub(crate) fn generation(&self, key: &AccountKey) -> u64 {
        *self.0.lock().unwrap().generations.get(key).unwrap_or(&0)
    }
    pub(crate) fn put(
        &self,
        key: &AccountKey,
        signature: Signature,
        generation: u64,
        value: Value,
    ) {
        let mut cache = self.0.lock().unwrap();
        if *cache.generations.get(key).unwrap_or(&0) == generation {
            cache.entries.insert(
                key.clone(),
                Cached {
                    signature,
                    generation,
                    at: Instant::now(),
                    value,
                },
            );
        }
    }
}

#[derive(Default)]
struct ReadersState {
    closing: bool,
    next_id: u64,
    active: HashMap<u64, Arc<AuthReader>>,
}
struct AuthReader {
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
}
#[derive(Clone, Default)]
pub struct NativeReaders(Arc<Mutex<ReadersState>>);
impl NativeReaders {
    fn register(&self) -> Option<(u64, Arc<AuthReader>)> {
        let mut state = self.0.lock().unwrap();
        if state.closing {
            return None;
        }
        let id = state.next_id;
        state.next_id += 1;
        let reader = Arc::new(AuthReader {
            cancel: watch::channel(false).0,
            done: watch::channel(false).0,
        });
        state.active.insert(id, reader.clone());
        Some((id, reader))
    }
    fn finish(&self, id: u64) {
        if let Some(reader) = self.0.lock().unwrap().active.remove(&id) {
            reader.done.send_replace(true);
        }
    }
    pub async fn close(&self) {
        let readers: Vec<_> = {
            let mut state = self.0.lock().unwrap();
            state.closing = true;
            state.active.values().cloned().collect()
        };
        for reader in &readers {
            reader.cancel.send_replace(true);
        }
        for reader in readers {
            let _ = reader.done.subscribe().wait_for(|done| *done).await;
        }
    }
}
async fn reader_cancelled(cancel: &mut watch::Receiver<bool>) {
    let _ = cancel.wait_for(|cancelled| *cancelled).await;
}

/// O auxiliar conserva árvore e transporte até a limpeza confirmar o término.
pub(crate) struct NativeProcess {
    pub client: Client,
    pub completed: tokio::sync::mpsc::UnboundedReceiver<Value>,
    child: tokio::process::Child,
    tree: crate::terminal_process::CommandTree,
    drain: tokio::task::JoinHandle<()>,
    closed: bool,
    _admin: super::storage::NewDirectory,
}
impl NativeProcess {
    pub async fn open(service: &AccountService, account: &Account) -> Result<Self, &'static str> {
        let mut command = service.codex_command().ok_or("codex_account_cli_missing")?;
        let admin_path = std::env::temp_dir().join(format!(
            "hangar-codex-admin-{}",
            super::claude_login::nonce().map_err(|_| "codex_account_login_failed")?
        ));
        let admin = super::storage::NewDirectory::create(&admin_path)
            .map_err(|_| "codex_account_login_failed")?;
        command.args(["-c", "project_root_markers=[]"]);
        if !account.is_default {
            command.args(["-c", r#"cli_auth_credentials_store="file""#]);
        }
        command
            .arg("app-server")
            .env_clear()
            .envs(service.env.codex(account))
            .current_dir(&admin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command)
            .map_err(|_| "codex_account_login_failed")?;
        let (client, mut incoming, mut child) =
            Client::spawn_stdio(command).map_err(|_| "codex_account_login_failed")?;
        if tree.attach(&child).is_err() {
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            return Err("codex_account_login_failed");
        }
        let (events, completed) = tokio::sync::mpsc::unbounded_channel();
        let reply = client.clone();
        let drain = tokio::spawn(async move {
            while let Some(event) = incoming.recv().await {
                match event {
                    Incoming::Request { id, .. } => {
                        let _ = reply
                            .respond(
                                id,
                                Err((-32601, "auxiliar de conta não executa pedidos".into())),
                            )
                            .await;
                    }
                    Incoming::Notification { method, params }
                        if method == "account/login/completed" =>
                    {
                        // O evento pode chegar antes da resposta que revela seu login_id.
                        let _ = events.send(params);
                    }
                    _ => {}
                }
            }
        });
        Ok(Self {
            client,
            completed,
            child,
            tree,
            drain,
            closed: false,
            _admin: admin,
        })
    }
    pub async fn initialize(&mut self) -> Result<(), hangar_codex::client::ClientError> {
        self.client
            .request::<Value>(
                ClientRequest::Initialize(InitializeParams {
                    client_info: ClientInfo {
                        name: "hangar_accounts".into(),
                        title: None,
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                    capabilities: None,
                }),
                Duration::from_secs(30),
            )
            .await?;
        self.client.notify("initialized", json!({})).await
    }
    pub async fn read(&mut self) -> Result<Value, hangar_codex::client::ClientError> {
        self.client
            .request::<Value>(
                ClientRequest::AccountRead(GetAccountParams {
                    refresh_token: false,
                }),
                Duration::from_secs(30),
            )
            .await
            .map(|v| auth_public(&v))
    }
    pub async fn close(&mut self) {
        if self.closed {
            return;
        }
        self.drain.abort();
        crate::terminal_process::finish(&mut self.tree).await;
        let _ = self.child.wait().await;
        self.closed = true;
    }
}

pub fn auth_public(result: &Value) -> Value {
    let Some(account) = result.get("account") else {
        return unavailable(false);
    };
    if account.is_null() {
        return super::catalog::disconnected_auth();
    }
    match account["type"].as_str() {
        Some("chatgpt") => {
            json!({"method":"oauth","status":"connected","email":account["email"],"plan":account["planType"]})
        }
        Some("apiKey") => json!({"method":"api_key","status":"connected","email":null,"plan":null}),
        _ => unavailable(false),
    }
}
pub(crate) fn unavailable(missing: bool) -> Value {
    let mut result = json!({"method":"unknown","status":"unavailable","email":null,"plan":null});
    if missing {
        result["reason"] = json!("cli_missing");
    }
    result
}

impl AccountService {
    pub fn visible_codex_accounts(&self) -> Result<Vec<Account>, super::catalog::AccountError> {
        let mut accounts = self.snapshot(Provider::Codex)?;
        if self.codex_command().is_none() && !self.env.codex_default.exists() {
            accounts.retain(|account| !account.is_default);
        }
        Ok(accounts)
    }
    /// O estado de preparo vem do coordenador; ausência desse serviço não vira idle.
    /// Para listar: o último login conhecido na hora e a leitura nova por trás, uma por conta.
    pub async fn read_codex_auth_fast(&self, account: &Account) -> Value {
        let known = AccountKey::new(Provider::Codex, &account.home)
            .ok()
            .zip(AuthCache::signature(account).ok())
            .and_then(|(key, signature)| {
                if self.codex_auth.get(&key, &signature).is_some() {
                    return None;
                }
                self.codex_auth
                    .known(&key, &signature)
                    .map(|value| (key, value))
            });
        let Some((key, value)) = known else {
            return self.read_codex_auth(account).await;
        };
        if self.codex_auth.0.lock().unwrap().refreshing.insert(key.clone()) {
            let (service, account) = (self.clone(), account.clone());
            tokio::spawn(async move {
                service.read_codex_auth(&account).await;
                service.codex_auth.0.lock().unwrap().refreshing.remove(&key);
            });
        }
        value
    }
    pub async fn codex_snapshot(&self, account: &Account, sync: Value) -> Value {
        codex_dto(account, self.read_codex_auth_fast(account).await, sync)
    }
    pub async fn read_codex_auth(&self, account: &Account) -> Value {
        // O worker conserva guarda e árvore até o fim, mesmo se a requisição HTTP sumir.
        let Some((id, reader)) = self.codex_readers.register() else {
            return unavailable(false);
        };
        let service = self.clone();
        let account = account.clone();
        tokio::spawn(async move {
            let mut cancel = reader.cancel.subscribe();
            let value = service.read_codex_auth_owned(&account, &mut cancel).await;
            // O retorno do leitor só ocorre após fechar a árvore e soltar a guarda.
            service.codex_readers.finish(id);
            value
        })
        .await
        .unwrap_or_else(|_| unavailable(false))
    }
    async fn read_codex_auth_owned(
        &self,
        account: &Account,
        cancel: &mut watch::Receiver<bool>,
    ) -> Value {
        if *cancel.borrow() {
            return unavailable(false);
        }
        let Ok(key) = AccountKey::new(Provider::Codex, &account.home) else {
            return unavailable(false);
        };
        let Ok(_guard) = self.locks.try_acquire(&key, GuardMode::Shared) else {
            return unavailable(false);
        };
        let Ok(current) = self.resolve(Provider::Codex, &account.id) else {
            return unavailable(false);
        };
        if AccountKey::new(Provider::Codex, &current.home)
            .ok()
            .as_ref()
            != Some(&key)
        {
            return unavailable(false);
        }
        let Ok(signature) = AuthCache::signature(account) else {
            return unavailable(false);
        };
        if let Some(value) = self.codex_auth.get(&key, &signature) {
            return value;
        }
        let generation = self.codex_auth.generation(&key);
        if *cancel.borrow() {
            return unavailable(false);
        }
        let value = match NativeProcess::open(self, account).await {
            Ok(mut native) => {
                let result = tokio::select! {
                    biased;
                    () = reader_cancelled(cancel) => None,
                    result = tokio::time::timeout(Duration::from_secs(6), async {
                        native.initialize().await?;
                        native.read().await
                    }) => Some(result),
                };
                native.close().await;
                match result {
                    Some(Ok(Ok(value))) => value,
                    _ => unavailable(false),
                }
            }
            Err(code) => unavailable(code == "codex_account_cli_missing"),
        };
        if *cancel.borrow()
            || AuthCache::signature(account).ok().as_ref() != Some(&signature)
            || self.codex_auth.generation(&key) != generation
        {
            return unavailable(false);
        }
        self.codex_auth
            .put(&key, signature, generation, value.clone());
        value
    }
    pub(crate) fn codex_command(&self) -> Option<tokio::process::Command> {
        let path = self
            .env
            .base
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))?
            .1;
        for dir in std::env::split_paths(path) {
            if cfg!(windows) {
                let exe = dir.join("codex.exe");
                if exe.is_file() {
                    return Some(tokio::process::Command::new(exe));
                }
                // A instalação npm publica .cmd; executar o JS evita cmd.exe e seu quoting.
                let script = dir.join("node_modules/@openai/codex/bin/codex.js");
                if script.is_file() {
                    let node = std::env::split_paths(path)
                        .map(|dir| dir.join("node.exe"))
                        .find(|p| p.is_file())?;
                    let mut command = tokio::process::Command::new(node);
                    command.arg(script);
                    return Some(command);
                }
            } else {
                let executable: PathBuf = dir.join("codex");
                if executable.is_file() {
                    return Some(tokio::process::Command::new(executable));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn expiry_and_invalidations_reject_old_identity() {
        let root = tempfile::tempdir().unwrap();
        let key = AccountKey::new(Provider::Codex, root.path()).unwrap();
        let signature = vec![None, None];
        let cache = AuthCache::default();
        let connected = json!({"status":"connected","email":"fixture@example.test"});
        cache.put(&key, signature.clone(), 0, connected.clone());
        tokio::time::advance(Duration::from_secs(59)).await;
        assert_eq!(cache.get(&key, &signature), Some(connected));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(cache.get(&key, &signature), None);
        cache.put(&key, signature.clone(), 0, unavailable(false));
        tokio::time::advance(Duration::from_secs(19)).await;
        assert!(cache.get(&key, &signature).is_some());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(cache.get(&key, &signature), None);
        cache.invalidate(&key);
        cache.put(&key, signature.clone(), 0, json!({"status":"connected"}));
        assert_eq!(
            cache.get(&key, &signature),
            None,
            "consulta anterior à invalidação repovoou o cache"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fast_read_returns_last_known_login_after_ttl() {
        let home = tempfile::tempdir().unwrap();
        let empty_path = home.path().join("bin");
        std::fs::create_dir_all(&empty_path).unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let text = |path: &std::path::Path| path.to_string_lossy().into_owned();
        let service = AccountService::new(super::super::environment::AccountEnvironment::from_map(
            [
                ("HOME".into(), text(home.path())),
                ("USERPROFILE".into(), text(home.path())),
                ("PATH".into(), text(&empty_path)),
            ]
            .into(),
        ));
        let account = service.resolve(Provider::Codex, "default").unwrap();
        let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
        let connected = json!({"status":"connected","email":"fixture@example.test"});
        service.codex_auth.put(
            &key,
            AuthCache::signature(&account).unwrap(),
            0,
            connected.clone(),
        );
        tokio::time::advance(Duration::from_secs(61)).await;
        assert_eq!(
            service.read_codex_auth_fast(&account).await,
            connected,
            "a listagem espera a CLI em vez de mostrar o último login conhecido"
        );
        // Credencial trocada: nada conhecido para esta identidade, a leitura é esperada.
        std::fs::write(account.home.join("auth.json"), b"{}").unwrap();
        assert_ne!(service.read_codex_auth_fast(&account).await, connected);
    }
}
