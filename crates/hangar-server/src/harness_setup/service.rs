//! Saúde e conserto compartilham os mesmos leitores e instaladores, sem fatos Python.
use super::{health, health_details as details};
use crate::{
    accounts::{AccountService, codex_device_login::DeviceBridge},
    cli_probe::Probe,
    runtime::gateway::RuntimeRegistry,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
#[derive(Clone)]
pub struct Service {
    pub accounts: AccountService,
    pub repo: PathBuf,
    pub probe: Probe,
    pub runtime: Option<Arc<RuntimeRegistry>>,
    pub device: Option<DeviceBridge>,
}
impl Service {
    pub async fn diagnose(&self) -> Result<Value, String> {
        let names = ["claude", "codex", "pi", "omp", "kimi", "tmux"];
        let versions = futures_util::future::join_all(
            names
                .iter()
                .map(|cli| self.probe.version_text(&self.accounts.env, cli)),
        )
        .await;
        let service = self.clone();
        let mut cards = tokio::task::spawn_blocking(move || service.cards(&versions))
            .await
            .map_err(|e| e.to_string())??;
        cards
            .as_array_mut()
            .expect("Lista de cards")
            .push(self.tmux().await?);
        Ok(cards)
    }
    fn cards(&self, versions: &[Option<String>]) -> Result<Value, String> {
        let env = &self.accounts.env;
        let hooks = self
            .accounts
            .claude_listed()
            .map_err(|e| e.message)?
            .into_iter()
            .filter(|(_, a)| a.home.join("settings.json").is_file())
            .map(|(_, a)| health::claude_hooks(&a.home))
            .collect::<Vec<_>>();
        let hooks = hooks
            .iter()
            .find(|v| v["ok"] == false)
            .or_else(|| hooks.iter().find(|v| v["ok"].is_null()))
            .or(hooks.first())
            .cloned()
            .unwrap_or_else(|| health::item("hooks", None, "nenhuma_conta", None, false, &[]));
        let mut cards = vec![
            json!({"id":"claude","nome":"Claude Code","instalado":versions[0].is_some(),"versao":versions[0],"itens":[hooks,details::plugins(env)?,health::wrapper(env,"claude",cfg!(windows)),health::fullscreen(env,"claude"),details::accounts(&self.accounts)?,health::mcp(env,"claude")?,health::default_model(env,"claude")?]}),
        ];
        for (index, (cli, name)) in [
            ("codex", "Codex"),
            ("pi", "Pi"),
            ("omp", "oh-my-pi"),
            ("kimi", "Kimi Code"),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = if cli == "codex" {
                env.home.join(".codex")
            } else {
                health::agent_dir(env, cli)?
            };
            let mut items = vec![];
            if dir.is_dir() {
                items.extend([
                    details::credentials(env, cli)?,
                    health::wrapper(env, cli, cfg!(windows)),
                ]);
                match cli {
                    "codex" => items.push(health::codex_hooks(env)),
                    "pi" => items.extend([
                        health::extensions(env, &self.repo, cli)?,
                        health::fullscreen(env, cli),
                        details::skills(env, &self.repo, cli)?,
                    ]),
                    "omp" => items.push(health::extensions(env, &self.repo, cli)?),
                    "kimi" => {
                        let status = dir.join("statusline.js").is_file();
                        items.extend([
                            details::kimi_hooks(env, &self.repo)?,
                            details::skills(env, &self.repo, cli)?,
                            health::item(
                                "statusline",
                                Some(status),
                                if status {
                                    "statusline_ok"
                                } else {
                                    "sem_statusline"
                                },
                                None,
                                false,
                                &[],
                            ),
                        ]);
                    }
                    _ => {}
                }
                items.extend([health::mcp(env, cli)?, health::default_model(env, cli)?]);
            }
            cards.push(json!({"id":cli,"nome":name,"instalado":versions[index+1].is_some()||dir.is_dir(),"versao":versions[index+1],"itens":items}));
        }
        Ok(Value::Array(cards))
    }
    async fn tmux_command(&self, args: &[&str]) -> Option<String> {
        let mut argv = vec!["tmux".to_owned()];
        argv.extend(args.iter().map(|s| s.to_string()));
        let output = crate::command_capture::capture_with_account_env(
            argv,
            self.accounts.env.clone(),
            Duration::from_secs(5),
        )
        .await
        .ok()?;
        output
            .success
            .then(|| crate::accounts::quotas::other::py_strip(&output.stdout).to_owned())
    }
    async fn tmux(&self) -> Result<Value, String> {
        let env = &self.accounts.env;
        let version = self.probe.version_text(env, "tmux").await;
        if version.is_none() {
            return Ok(
                json!({"id":"tmux","nome":"tmux","instalado":false,"versao":null,"itens":[]}),
            );
        }
        let path = if cfg!(windows) {
            health::psmux_config(env)
        } else {
            env.home.join(".tmux.conf")
        };
        let marker = if cfg!(windows) {
            "# >>> hangar windows-tmux >>>"
        } else {
            "# >>> hangar >>>"
        };
        let block = match std::fs::read(&path) {
            Ok(bytes) => std::str::from_utf8(&bytes)
                .map_err(|_| "configuração tmux não está em UTF-8")?
                .trim_start_matches('\u{feff}')
                .contains(marker),
            Err(_) => false,
        };
        let mut items = vec![health::item(
            "bloco",
            Some(block),
            if block {
                "tmux_bloco_ok"
            } else {
                "tmux_bloco_ausente"
            },
            if block { None } else { Some("tmux") },
            false,
            &[],
        )];
        let term = self
            .tmux_command(&["show", "-gv", "default-terminal"])
            .await
            .unwrap_or_default();
        let good = !term.is_empty() && !term.starts_with("tmux") && !term.starts_with("screen");
        items.push(health::item(
            "default_terminal",
            Some(good),
            if good {
                "tmux_term_ok"
            } else {
                "tmux_term_ruim"
            },
            if good { None } else { Some("tmux") },
            false,
            &[("valor", if term.is_empty() { "?".into() } else { term })],
        ));
        let environment = self
            .tmux_command(&["show-environment", "-g"])
            .await
            .unwrap_or_default();
        let good = environment.contains("COLORTERM=truecolor")
            && environment.contains("CLAUDE_CODE_TMUX_TRUECOLOR=1");
        items.push(health::item(
            "truecolor",
            Some(good),
            if good {
                "tmux_truecolor_ok"
            } else {
                "tmux_truecolor_ruim"
            },
            if good { None } else { Some("tmux") },
            false,
            &[],
        ));
        if !cfg!(windows) {
            let title = self
                .tmux_command(&["show", "-gv", "set-titles-string"])
                .await
                .unwrap_or_default();
            let good = title == "#S";
            items.push(health::item(
                "titulo",
                Some(good),
                if good {
                    "tmux_titulo_ok"
                } else {
                    "tmux_titulo_ruim"
                },
                if good { None } else { Some("tmux") },
                false,
                &[("valor", if title.is_empty() { "?".into() } else { title })],
            ));
        }
        let mouse = self
            .tmux_command(&["show", "-gv", "mouse"])
            .await
            .as_deref()
            == Some("on");
        items.push(health::item(
            "mouse",
            Some(true),
            if mouse {
                "tmux_mouse_on"
            } else {
                "tmux_mouse_off"
            },
            None,
            true,
            &[],
        ));
        let persistent = env.home.join(".tmux/plugins/tmux-resurrect").is_dir();
        items.push(health::item(
            "persistencia",
            Some(true),
            if persistent {
                "tmux_persist_on"
            } else {
                "tmux_persist_off"
            },
            None,
            true,
            &[],
        ));
        Ok(json!({"id":"tmux","nome":"tmux","instalado":true,"versao":version,"itens":items}))
    }
    pub fn wrapper_command(&self) -> Result<Vec<String>, String> {
        let bash = crate::cli_probe::resolve_program("bash", &self.accounts.env).ok_or(
            "sem bash nesta máquina — os wrappers e o bloco do tmux são do instalador POSIX",
        )?;
        Ok(vec![
            bash.to_string_lossy().into_owned(),
            self.repo
                .join("scripts/install-claude-wrapper.sh")
                .to_string_lossy()
                .into_owned(),
            "--no-statusline".into(),
        ])
    }
}
