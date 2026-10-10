//! Subprocesso isolado: conserva a árvore até terminar e limita a saída sem registrá-la.
use crate::terminal_process::{CommandTree, OwnedTree};
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

pub struct Process {
    child: Option<Child>,
    tree: Option<CommandTree>,
    output: BufReader<tokio::process::ChildStdout>,
    stderr: tokio::task::JoinHandle<()>,
    directory: Option<tempfile::TempDir>,
    account_guard: Option<std::sync::Arc<crate::accounts::AccountGuard>>,
}
impl Process {
    pub fn open(mut command: Command, directory: tempfile::TempDir) -> Result<Self, &'static str> {
        command
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut tree =
            CommandTree::configure(&mut command).map_err(|_| "dictation_organization_failed")?;
        let mut child = command.spawn().map_err(|_| "dictation_cli_missing")?;
        if tree.attach(&child).is_err() {
            let _ = child.start_kill();
            return Err("dictation_organization_failed");
        }
        let output = BufReader::new(child.stdout.take().ok_or("dictation_organization_failed")?);
        let mut error = child.stderr.take().ok_or("dictation_organization_failed")?;
        let stderr = tokio::spawn(async move {
            // Só drena: erro de CLI pode conter autenticação ou a transcrição.
            let mut buffer = [0u8; 4096];
            while error.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
        });
        Ok(Self {
            child: Some(child),
            tree: Some(tree),
            output,
            stderr,
            directory: Some(directory),
            account_guard: None,
        })
    }
    pub async fn write(&mut self, text: &str) -> Result<(), &'static str> {
        let stdin = self
            .child
            .as_mut()
            .and_then(|c| c.stdin.as_mut())
            .ok_or("dictation_organization_failed")?;
        stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|_| "dictation_organization_failed")?;
        stdin
            .flush()
            .await
            .map_err(|_| "dictation_organization_failed")
    }
    pub fn protect_account(&mut self, guard: std::sync::Arc<crate::accounts::AccountGuard>) {
        self.account_guard = Some(guard);
    }
    pub async fn response(&mut self, id: &Value, claude: bool) -> Result<Value, &'static str> {
        let mut total = 0;
        loop {
            let mut line = Vec::new();
            let n = (&mut self.output)
                .take(1024 * 1024 + 1)
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| "dictation_catalog_unavailable")?;
            total += n;
            if n == 0 || total > 2 * 1024 * 1024 {
                return Err("dictation_catalog_unavailable");
            }
            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if claude {
                if value.pointer("/response/request_id") == Some(id) {
                    if value.pointer("/response/subtype").and_then(Value::as_str) == Some("error") {
                        return Err("dictation_catalog_unavailable");
                    }
                    return Ok(value["response"]["response"].clone());
                }
            } else if value.get("id") == Some(id) {
                if value.get("error").is_some_and(|v| !v.is_null()) {
                    return Err("dictation_catalog_unavailable");
                }
                return Ok(value["result"].clone());
            }
        }
    }
    pub async fn output(&mut self, input: &str) -> Result<Vec<u8>, &'static str> {
        self.write(input).await?;
        self.child.as_mut().and_then(|c| c.stdin.take());
        let mut output = Vec::new();
        (&mut self.output)
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| "dictation_organization_failed")?;
        if output.len() > 2 * 1024 * 1024 {
            return Err("dictation_organization_failed");
        }
        crate::terminal_process::leader_exited(self.child.as_mut().unwrap())
            .await
            .map_err(|_| "dictation_organization_failed")?;
        Ok(output)
    }
    pub async fn stop(&mut self) -> Option<std::process::ExitStatus> {
        if let Some(tree) = self.tree.as_mut() {
            crate::terminal_process::finish(tree).await;
        }
        let status = if let Some(child) = self.child.as_mut() {
            child.wait().await.ok()
        } else {
            None
        };
        self.child.take();
        self.tree.take();
        self.stderr.abort();
        self.directory.take();
        self.account_guard.take();
        status
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stderr.abort();
        if let (Some(mut child), Some(mut tree)) = (self.child.take(), self.tree.take()) {
            let _ = tree.terminate();
            let _ = child.start_kill();
            let directory = self.directory.take();
            let account_guard = self.account_guard.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    crate::terminal_process::finish(&mut tree).await;
                    let _ = child.wait().await;
                    drop(directory);
                    drop(account_guard);
                });
            }
        }
    }
}
pub async fn limited_output(
    command: Command,
    directory: tempfile::TempDir,
    input: &str,
    budget: Duration,
) -> Result<Vec<u8>, &'static str> {
    limited_output_guarded(command, directory, input, budget, None).await
}
pub async fn limited_output_guarded(
    command: Command,
    directory: tempfile::TempDir,
    input: &str,
    budget: Duration,
    guard: Option<std::sync::Arc<crate::accounts::AccountGuard>>,
) -> Result<Vec<u8>, &'static str> {
    let mut process = Process::open(command, directory)?;
    process.account_guard = guard;
    let outcome = tokio::time::timeout(budget, process.output(input))
        .await
        .unwrap_or(Err("dictation_organization_timeout"));
    let status = process.stop().await;
    if outcome.is_ok() && !status.is_some_and(|s| s.success()) {
        Err("dictation_organization_failed")
    } else {
        outcome
    }
}
