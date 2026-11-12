//! Consulta do catálogo ao CLI sem consumir um turno de conversa.
use super::cache::{Probe, ProbeResult, valid_commands};
use crate::terminal_process::{CommandTree, OwnedTree};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct NativeProbe {
    pub accounts: crate::accounts::AccountService,
    pub cwd: Option<PathBuf>,
    pub settings: Value,
}

impl NativeProbe {
    pub fn new(accounts: crate::accounts::AccountService) -> Self {
        Self {
            accounts,
            cwd: None,
            settings: Value::Null,
        }
    }

    fn plugin_roots(&self) -> Vec<String> {
        crate::session_open::claude::plugin_roots(&self.accounts.env.home)
    }
}

impl Probe for NativeProbe {
    fn cache_key(&self, config_dir: Option<&Path>) -> String {
        let context = json!([
            config_dir.unwrap_or(&self.accounts.env.claude_base),
            self.cwd,
            crate::session_open::claude::settings_json(&self.settings),
            self.plugin_roots(),
        ]);
        format!(
            "session-v1:{}",
            sha1_smol::Sha1::from(context.to_string()).digest()
        )
    }

    fn signature(&self, config_dir: Option<&Path>) -> Result<String, &'static str> {
        let command = self
            .accounts
            .claude_command()
            .ok_or("claude_commands_cli_unavailable")?;
        let executable =
            crate::list::links::resolve_lenient(Path::new(command.as_std().get_program()));
        let metadata = executable
            .metadata()
            .map_err(|_| "claude_commands_cli_unavailable")?;
        let mut signature = vec![
            self.cache_key(config_dir),
            executable.to_string_lossy().into_owned(),
            modified(&metadata)?,
            metadata.len().to_string(),
        ];
        // Na instalação npm, atualizar o script altera o catálogo sem atualizar o Node.
        for arg in command
            .as_std()
            .get_args()
            .filter(|arg| Path::new(arg).extension().is_some_and(|e| e == "js"))
        {
            let path = Path::new(arg);
            let meta = path
                .metadata()
                .map_err(|_| "claude_commands_cli_unavailable")?;
            signature.extend([
                path.to_string_lossy().into_owned(),
                modified(&meta)?,
                meta.len().to_string(),
            ]);
        }
        let default = self.accounts.env.home.join(".claude");
        let config_dir = config_dir.unwrap_or(&default);
        for path in [
            "settings.json",
            "plugins/installed_plugins.json",
            "skills",
            "commands",
        ] {
            signature.push(
                config_dir
                    .join(path)
                    .metadata()
                    .ok()
                    .and_then(|m| modified(&m).ok())
                    .unwrap_or_else(|| "-".into()),
            );
        }
        if let Some(cwd) = &self.cwd {
            for parent in cwd.ancestors() {
                for path in ["settings.json", "settings.local.json", "skills", "commands"] {
                    watch(&parent.join(".claude").join(path), &mut signature);
                }
            }
        }
        // Restaurar a pasta de um plugin também invalida uma consulta que o encontrou ausente.
        let registry = std::fs::read(config_dir.join("plugins/installed_plugins.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
        for entries in registry
            .as_ref()
            .and_then(|v| v["plugins"].as_object())
            .into_iter()
            .flat_map(|plugins| plugins.values())
        {
            for entry in entries.as_array().into_iter().flatten() {
                if let Some(root) = entry["installPath"].as_str() {
                    watch_plugin(Path::new(root), &mut signature);
                }
            }
        }
        for root in self.plugin_roots() {
            watch_plugin(Path::new(&root), &mut signature);
        }
        Ok(signature.join("|"))
    }

    fn run<'a>(
        &'a self,
        config_dir: Option<&'a Path>,
    ) -> Pin<Box<dyn Future<Output = ProbeResult> + Send + 'a>> {
        Box::pin(async move {
            let mut command = self
                .accounts
                .claude_command()
                .ok_or("claude_commands_cli_unavailable")?;
            let mut environment = self.accounts.env.base.clone();
            environment.retain(|key, _| {
                let upper = key.to_ascii_uppercase();
                !crate::terminal_process::PRIVATE_ENV_KEYS.contains(&upper.as_str())
                    && !matches!(
                        upper.as_str(),
                        "TMUX"
                            | "TMUX_PANE"
                            | "CP_SESSION_NAME"
                            | "CP_SESSION_KEY"
                            | "CLAUDE_CONFIG_DIR"
                    )
            });
            if let Some(dir) = config_dir {
                environment.insert(
                    "CLAUDE_CONFIG_DIR".into(),
                    dir.to_string_lossy().into_owned(),
                );
            }
            command
                .args([
                    "-p",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--setting-sources",
                    if self.cwd.is_some() {
                        "user,project,local"
                    } else {
                        "user"
                    },
                ])
                .env_clear()
                .envs(environment);
            if !self.settings.is_null() {
                command.args([
                    "--settings",
                    &crate::session_open::claude::settings_json(&self.settings),
                ]);
            }
            for root in self.plugin_roots() {
                command.args(["--plugin-dir", &root]);
            }
            let account_dir = config_dir
                .map(Path::to_owned)
                .unwrap_or_else(|| self.accounts.env.home.join(".claude"));
            let accounts = self.accounts.clone();
            let guard = tokio::task::spawn_blocking(move || {
                let key = crate::accounts::AccountKey::new(
                    crate::accounts::Provider::Claude,
                    &account_dir,
                )
                .map_err(|_| "claude_commands_account_unavailable")?;
                accounts
                    .locks
                    .try_acquire(&key, crate::accounts::GuardMode::Shared)
                    .map(Arc::new)
                    .map_err(|_| "claude_commands_account_busy")
            })
            .await
            .map_err(|_| "claude_commands_probe_interrupted")??;
            let mut process = Process::open(command, guard, self.cwd.as_deref()).await?;
            let result = process.initialize().await;
            process.stop().await;
            result
        })
    }
}

fn watch(path: &Path, signature: &mut Vec<String>) {
    signature.push(path.to_string_lossy().into_owned());
    signature.push(
        path.metadata()
            .ok()
            .and_then(|m| {
                modified(&m)
                    .ok()
                    .map(|stamp| format!("{stamp}:{}", m.len()))
            })
            .unwrap_or_else(|| "-".into()),
    );
}

fn watch_plugin(root: &Path, signature: &mut Vec<String>) {
    signature.push(format!("plugin:{}:{}", root.display(), root.is_dir()));
    for path in [
        root.join(".claude-plugin/plugin.json"),
        root.join("skills"),
        root.join("commands"),
    ] {
        watch(&path, signature);
    }
}

fn modified(metadata: &std::fs::Metadata) -> Result<String, &'static str> {
    let stamp = metadata
        .modified()
        .map_err(|_| "claude_commands_cli_unavailable")?;
    Ok(match stamp.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().to_string(),
        Err(error) => format!("-{}", error.duration().as_nanos()),
    })
}

struct Process {
    child: Option<tokio::process::Child>,
    tree: Option<CommandTree>,
    output: BufReader<tokio::process::ChildStdout>,
    directory: Option<tempfile::TempDir>,
    guard: Option<Arc<crate::accounts::AccountGuard>>,
}

impl Process {
    async fn open(
        mut command: tokio::process::Command,
        guard: Arc<crate::accounts::AccountGuard>,
        cwd: Option<&Path>,
    ) -> Result<Self, &'static str> {
        let directory = cwd
            .is_none()
            .then(|| tempfile::Builder::new().prefix("hangar-slash-").tempdir())
            .transpose()
            .map_err(|_| "claude_commands_probe_directory")?;
        command
            .current_dir(cwd.unwrap_or_else(|| directory.as_ref().unwrap().path()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut tree = CommandTree::configure(&mut command)
            .map_err(|_| "claude_commands_probe_start_failed")?;
        let mut child = command
            .spawn()
            .map_err(|_| "claude_commands_probe_start_failed")?;
        if tree.attach(&child).is_err() {
            let _ = tree.terminate();
            let _ = child.kill().await;
            return Err("claude_commands_probe_start_failed");
        }
        let output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or("claude_commands_probe_start_failed")?,
        );
        Ok(Self {
            child: Some(child),
            tree: Some(tree),
            output,
            directory,
            guard: Some(guard),
        })
    }

    async fn initialize(&mut self) -> ProbeResult {
        let child = self.child.as_mut().ok_or("claude_commands_probe_exited")?;
        let input = json!({"type":"control_request","request_id":"hangar_slash","request":{"subtype":"initialize"}}).to_string() + "\n";
        // A espera do processo no Windows fecha child.stdin; a solicitação conserva o cano.
        let mut stdin = child.stdin.take().ok_or("claude_commands_probe_exited")?;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|_| "claude_commands_probe_exited")?;
        stdin
            .flush()
            .await
            .map_err(|_| "claude_commands_probe_exited")?;
        tokio::select! {
            answer = response(&mut self.output) => answer,
            _ = crate::terminal_process::leader_exited(child) => Err("claude_commands_probe_exited"),
            _ = tokio::time::sleep(Duration::from_secs(120)) => Err("claude_commands_probe_timeout"),
        }
    }

    async fn stop(&mut self) {
        if let Some(tree) = self.tree.as_mut() {
            crate::terminal_process::finish(tree).await;
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait().await;
        }
        self.child.take();
        self.tree.take();
        self.directory.take();
        self.guard.take();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let (Some(mut child), Some(mut tree)) = (self.child.take(), self.tree.take()) {
            let _ = tree.terminate();
            let _ = child.start_kill();
            let (directory, guard) = (self.directory.take(), self.guard.take());
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    crate::terminal_process::finish(&mut tree).await;
                    let _ = child.wait().await;
                    drop(directory);
                    drop(guard);
                });
            }
        }
    }
}

async fn response(output: &mut BufReader<tokio::process::ChildStdout>) -> ProbeResult {
    loop {
        let mut raw = Vec::new();
        let n = output
            .read_until(b'\n', &mut raw)
            .await
            .map_err(|_| "claude_commands_probe_read_failed")?;
        if n == 0 {
            return Err("claude_commands_probe_exited");
        }
        let Ok(event) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        if event["type"] != "control_response" || event["response"]["request_id"] != "hangar_slash"
        {
            continue;
        }
        if event["response"]["subtype"] == "error" {
            return Err("claude_commands_probe_rejected");
        }
        let commands = valid_commands(&event["response"]["response"]["commands"]);
        if commands.is_empty() {
            return Err("claude_commands_probe_empty");
        }
        return Ok(commands);
    }
}
