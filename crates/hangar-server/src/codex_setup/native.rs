//! App-server administrativo isolado, com contenção da árvore até no cancelamento.
use crate::accounts::{AccountService, environment::AccountEnvironment};
use hangar_codex::client::{Client, Incoming};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

pub struct Native {
    client: Option<Client>,
    cli: super::native_cli::AdminCli,
    close: Option<tokio::sync::oneshot::Sender<()>>,
    pending_directories: Arc<Mutex<Vec<PathBuf>>>,
    cleanup: Option<tokio::task::JoinHandle<Result<(), String>>>,
    pub events: tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
}
impl Native {
    pub async fn open_with_memory(env:&AccountEnvironment,home:&Path,codex:&Path,memory:bool)->Result<Self,String> {
        Self::open_with_directory(env,home,codex,None,memory).await
    }
    pub fn cli(&self)->super::native_cli::AdminCli { self.cli.clone() }
    pub async fn open(env: &AccountEnvironment, home: &Path, codex: &Path) -> Result<Self, String> {
        Self::open_with_directory(env, home, codex, None,false).await
    }
    pub async fn open_isolated(
        env: &AccountEnvironment,
        directory: tempfile::TempDir,
    ) -> Result<Self, String> {
        let home = directory.path().to_owned();
        Self::open_with_directory(env, &home, &home.join(".codex"), Some(directory),false).await
    }
    async fn open_with_directory(
        env: &AccountEnvironment,
        home: &Path,
        codex: &Path,
        directory: Option<tempfile::TempDir>,
        memory: bool,
    ) -> Result<Self, String> {
        let working_directory = tempfile::Builder::new()
            .prefix("hangar-codex-admin-")
            .tempdir()
            .map_err(|_| "não foi possível criar o diretório administrativo do Codex")?;
        let service = AccountService::new(env.clone());
        let mut command = service.codex_command().ok_or("Codex não encontrado")?;
        command.args(["-c", "project_root_markers=[]"]);
        if memory {command.args(["-c","features.external_agent_memory_import=true"]);}
        command
            .args(["app-server", "--stdio"])
            .env_clear()
            .envs(&env.base)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("CODEX_HOME", codex)
            .current_dir(working_directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in crate::terminal_process::PRIVATE_ENV_KEYS {command.env_remove(key);}
        if home != env.home {
            for (key, relative) in [
                ("XDG_CONFIG_HOME", ".config"),
                ("XDG_DATA_HOME", ".local/share"),
                ("XDG_STATE_HOME", ".local/state"),
                ("XDG_CACHE_HOME", ".cache"),
            ] {
                command.env(key, home.join(relative));
            }
        }
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command)
            .map_err(|_| "contenção do Codex indisponível")?;
        let (client, mut incoming, mut child) =
            Client::spawn_stdio(command).map_err(|_| "não foi possível iniciar o Codex")?;
        if tree.attach(&child).is_err() {
            let _ = child.start_kill();
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            return Err("não foi possível conter o Codex".into());
        }
        let (events, receive) = tokio::sync::mpsc::unbounded_channel();
        let responder = client.clone();
        let drain = tokio::spawn(async move {
            while let Some(event) = incoming.recv().await {
                match event {
                    Incoming::Request { id, .. } => {
                        let _ = responder
                            .respond(
                                id,
                                Err((-32601, "integração não executa ferramentas".into())),
                            )
                            .await;
                    }
                    Incoming::Notification { method, params } => {
                        let _ = events.send((method, params));
                    }
                }
            }
        });
        let mut directories = vec![working_directory.keep()];
        if let Some(directory) = directory {
            directories.push(directory.keep());
        }
        let pending_directories = Arc::new(Mutex::new(directories));
        let pending = pending_directories.clone();
        let cli=super::native_cli::AdminCli::new(env.clone(),home,codex);
        let cleanup_cli=cli.clone();
        let (close, done) = tokio::sync::oneshot::channel();
        let cleanup = tokio::spawn(async move {
            let _ = done.await;
            drain.abort();
            let _ = drain.await;
            let stopped=super::native_shutdown::finish(&mut child,&mut tree).await;
            let commands=cleanup_cli.close().await;
            stopped?;
            commands?;
            remove_directories(pending).await
        });
        let mut native = Self {
            client:Some(client),
            cli,
            close: Some(close),
            pending_directories,
            cleanup: Some(cleanup),
            events: receive,
        };
        let result=native.request("initialize",json!({"clientInfo":{"name":"hangar","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),Duration::from_secs(60)).await;
        if let Err(error) = result {
            native.close().await?;
            return Err(error);
        }
        if native
            .client
            .as_ref().unwrap()
            .notify("initialized", json!({}))
            .await
            .is_err()
        {
            native.close().await?;
            return Err("Codex encerrou na inicialização".into());
        }
        Ok(native)
    }
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.client.as_ref().ok_or_else(||format!("Codex indisponível em {method}"))?
            .request_method(method, params, timeout)
            .await
            .map_err(|error| {
                // A mensagem RPC pode carregar chave ou URL; só a classe e método chegam ao chamador.
                let kind = match error {
                    hangar_codex::client::ClientError::Rpc { .. } => "recusado",
                    _ => "indisponível",
                };
                format!("Codex {kind} em {method}")
            })
    }
    pub async fn close(&mut self) -> Result<(), String> {
        self.cli.stop_admission();
        drop(self.client.take());
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
        if self.cleanup.is_none()
            && !self
                .pending_directories
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        {
            let cli=self.cli.clone();let pending=self.pending_directories.clone();
            self.cleanup = Some(tokio::spawn(async move {cli.close().await?;remove_directories(pending).await}));
        }
        if let Some(task) = self.cleanup.as_mut() {
            // Cancelar o aguardante não perde a tarefa nem os diretórios que ainda precisam sair.
            let result = task.await.map_err(|_| "limpeza do Codex interrompida");
            self.cleanup = None;
            result??;
        }
        Ok(())
    }
}
async fn remove_directories(pending: Arc<Mutex<Vec<PathBuf>>>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || remove_pending(&pending))
        .await
        .map_err(|_| "limpeza do Codex interrompida")?
}
fn remove_pending(pending: &Mutex<Vec<PathBuf>>) -> Result<(), String> {
    let mut paths = pending.lock().unwrap_or_else(|e| e.into_inner());
    while let Some(path) = paths.first() {
        if let Err(error) = std::fs::remove_dir_all(path) {
            let error = format!(
                "não foi possível remover o diretório temporário do Codex: {:?}",
                error.kind()
            );
            tracing::warn!(%error, "limpeza do Codex incompleta");
            return Err(error);
        }
        paths.remove(0);
    }
    Ok(())
}
impl Drop for Native {
    fn drop(&mut self) {
        self.cli.stop_admission();
        drop(self.client.take());
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        } else if self.cleanup.is_none()
            && !self
                .pending_directories
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        {
            let pending = self.pending_directories.clone();
            let cli=self.cli.clone();
            if let Ok(runtime)=tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if cli.close().await.is_ok() {let _=remove_directories(pending).await;}
                });
                return;
            }
            let _ = std::thread::Builder::new()
                .name("codex-cleanup".into())
                .spawn(move || {
                    if cli.active_operations()==0 {let _ = remove_pending(&pending);}
                    else {tracing::warn!(code="codex_cleanup_pending","Diretórios administrativos conservados sem runtime para concluir a limpeza");}
                });
        }
    }
}

#[cfg(all(test,unix))]
#[path="native_owner_tests.rs"]
mod owner_tests;
