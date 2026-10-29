//! Captura separada dos dois canais; timeout e cancelamento encerram só a árvore criada.
use crate::{
    accounts::environment::AccountEnvironment,
    terminal_process::{self, CommandTree, OwnedTree},
};
use std::{io, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

fn error_name(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        _ => "OSError",
    }
    .into()
}

#[derive(Clone, Debug)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub async fn capture(
    args: Vec<String>,
    env: AccountEnvironment,
    budget: Duration,
) -> Result<Output, String> {
    let Some(program) = args.first() else {
        return Err("FileNotFoundError".into());
    };
    let mut command = Command::new(program);
    command
        .args(&args[1..])
        .env_clear()
        .envs(env.base.iter().filter(|(key, _)| {
            key.as_str() != "CLAUDE_CONFIG_DIR"
                && !terminal_process::PRIVATE_ENV_KEYS.contains(&key.as_str())
        }))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut tree = match CommandTree::configure_detached(&mut command) {
        Ok(tree) => tree,
        Err(error) => return Err(error_name(&error)),
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Err(error_name(&error)),
    };
    if let Err(error) = tree.attach(&child) {
        let _ = tree.terminate();
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(error_name(&error));
    }
    let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
    // O supervisor da captura conserva processo e pipes mesmo se o pedido for cancelado.
    let task = tokio::spawn(async move {
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let outcome = {
            let collect = async {
                let (a, b, c) = tokio::join!(
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err),
                    terminal_process::leader_exited(&mut child)
                );
                a?;
                b?;
                c?;
                Ok::<(), io::Error>(())
            };
            tokio::select! {
                result = tokio::time::timeout(budget,collect) => match result {
                    Ok(Ok(())) => Ok(()), Ok(Err(error)) => Err(error_name(&error)), Err(_) => Err("timeout".into()),
                },
                _ = cancelled => Err("interrompido".into()),
            }
        };
        if outcome.is_err() {
            terminal_process::finish(&mut tree).await;
        }
        let status = child.wait().await;
        outcome?;
        let status = status.map_err(|error| error_name(&error))?;
        let decode = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes)
                .replace("\r\n", "\n")
                .replace('\r', "\n")
        };
        Ok(Output {
            success: status.success(),
            stdout: decode(&out),
            stderr: decode(&err),
        })
    });
    let result = task
        .await
        .unwrap_or_else(|_| Err("falha ao acompanhar o comando".into()));
    drop(cancel);
    result
}

pub async fn run(args: Vec<String>, env: AccountEnvironment, budget: Duration) -> (bool, String) {
    match capture(args, env, budget).await {
        Err(error) => (false, error),
        Ok(out) => {
            let text = if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            };
            let text = text.trim();
            let skip = text.chars().count().saturating_sub(300);
            (out.success, text.chars().skip(skip).collect())
        }
    }
}
