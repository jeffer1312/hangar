//! App-server administrativo isolado, com contenção da árvore até no cancelamento.
use crate::accounts::{AccountService, environment::AccountEnvironment};
use hangar_codex::client::{Client, Incoming};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};

pub struct Native {
    client: Client,
    close: Option<tokio::sync::oneshot::Sender<Option<tempfile::TempDir>>>,
    directory: Option<tempfile::TempDir>,
    cleanup: Option<tokio::task::JoinHandle<()>>,
    pub events: tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
}
impl Native {
    pub async fn open(env: &AccountEnvironment, home: &Path, codex: &Path) -> Result<Self, String> {
        Self::open_with_directory(env, home, codex, None).await
    }
    pub async fn open_isolated(
        env: &AccountEnvironment,
        directory: tempfile::TempDir,
    ) -> Result<Self, String> {
        let home = directory.path().to_owned();
        Self::open_with_directory(env, &home, &home.join(".codex"), Some(directory)).await
    }
    async fn open_with_directory(
        env: &AccountEnvironment,
        home: &Path,
        codex: &Path,
        directory: Option<tempfile::TempDir>,
    ) -> Result<Self, String> {
        let service = AccountService::new(env.clone());
        let mut command = service.codex_command().ok_or("Codex não encontrado")?;
        command
            .args(["-c", "project_root_markers=[]", "app-server", "--stdio"])
            .env_clear()
            .envs(&env.base)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("CODEX_HOME", codex)
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command)
            .map_err(|_| "contenção do Codex indisponível")?;
        let (client, mut incoming, mut child) =
            Client::spawn_stdio(command).map_err(|_| "não foi possível iniciar o Codex")?;
        if tree.attach(&child).is_err() {
            let _ = child.start_kill();
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
        let (close, done) = tokio::sync::oneshot::channel();
        let cleanup = tokio::spawn(async move {
            let directory = done.await.ok().flatten();
            drain.abort();
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            drop(directory);
        });
        let mut native = Self {
            client,
            close: Some(close),
            directory,
            cleanup: Some(cleanup),
            events: receive,
        };
        let result=native.request("initialize",json!({"clientInfo":{"name":"hangar","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),Duration::from_secs(60)).await;
        if let Err(error) = result {
            native.close().await;
            return Err(error);
        }
        if native
            .client
            .notify("initialized", json!({}))
            .await
            .is_err()
        {
            native.close().await;
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
        self.client
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
    pub async fn close(&mut self) {
        if let Some(close) = self.close.take() {
            let _ = close.send(self.directory.take());
        }
        if let Some(task) = self.cleanup.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        if let Some(close) = self.close.take() {
            let _ = close.send(self.directory.take());
        }
    }
}
