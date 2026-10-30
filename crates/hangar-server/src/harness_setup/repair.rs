//! O conserto usa as mesmas escritas da instalação e devolve a falha da ação.
use super::{health, health_details as details, service::Service};
use crate::{
    accounts::{Provider, claude_config},
    engines::{mirrors, probe},
    orchestration::codec,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
#[derive(Debug)]
pub struct Failure {
    pub code: Option<&'static str>,
    pub status: u16,
    pub message: String,
}
fn invalid(message: impl Into<String>) -> Failure {
    Failure {
        code: None,
        status: 400,
        message: message.into(),
    }
}
fn failed(message: impl Into<String>) -> Failure {
    Failure {
        code: None,
        status: 500,
        message: message.into(),
    }
}
impl Service {
    pub async fn repair(&self, id: &str) -> Result<String, Failure> {
        match id {
            "oauth" => {
                let result = self
                    .oauth(!details::oauth_saved(&self.accounts.env))
                    .await?;
                let line = result
                    .as_object()
                    .ok_or_else(|| failed("resposta de propagação inválida"))?
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v["motivo"].as_str().unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join("; ");
                if result
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|v| v["ok"] != true && v["motivo"] != "nao-instalado")
                {
                    return Err(invalid(line));
                }
                Ok(line)
            }
            "sync:pi" | "sync:omp" | "sync:kimi" | "sync:codex" => {
                self.sync_credentials(&id[5..]).await
            }
            "tmux" | "wrapper" => self.repair_wrapper(id).await,
            _ => {
                let service = self.clone();
                let id = id.to_owned();
                tokio::task::spawn_blocking(move || service.repair_files(&id))
                    .await
                    .map_err(|e| failed(e.to_string()))?
            }
        }
    }
    async fn oauth(&self, import: bool) -> Result<Value, Failure> {
        let bridge = self
            .device
            .clone()
            .ok_or_else(|| failed("serviço de autenticação indisponível"))?;
        self.accounts
            .propagate_device(self.runtime.clone(), bridge, import)
            .await
            .map_err(|e| Failure {
                code: Some(e.code),
                status: e.status,
                message: e.message,
            })
    }
    async fn sync_credentials(&self, cli: &str) -> Result<String, Failure> {
        let env = &self.accounts.env;
        let present = details::present(env, cli).map_err(failed)?;
        let mut lines = Vec::new();
        for (target, credential) in details::available(env, cli) {
            if present.contains(&target) {
                continue;
            }
            let Some(data) = credential else {
                match self.oauth(false).await {
                    Ok(value) => lines.push(format!(
                        "openai-codex: {}",
                        value[cli]["motivo"].as_str().unwrap_or("?")
                    )),
                    Err(error) => lines.push(format!(
                        "openai-codex: {}",
                        error
                            .code
                            .map(str::to_owned)
                            .unwrap_or_else(|| error.status.to_string())
                    )),
                }
                continue;
            };
            let key = data["api_key"].as_str().unwrap_or("");
            let (ok, reason) = if cli == "omp" {
                let env = env.clone();
                let target = target.clone();
                let key = key.to_owned();
                tokio::task::spawn_blocking(move || omp_key(&env, &target, &key))
                    .await
                    .map_err(|e| failed(e.to_string()))?
            } else {
                let base = data["base_url"].as_str().unwrap_or("");
                let models = probe::fetch(base, key).await.unwrap_or_default();
                let value = mirrors::sync(
                    env,
                    data["nome"].as_str().unwrap_or(""),
                    base,
                    key,
                    &models,
                    &[cli.into()],
                );
                (
                    value[cli]["ok"] == true,
                    value[cli]["motivo"]
                        .as_str()
                        .unwrap_or("alvo-desconhecido")
                        .to_owned(),
                )
            };
            lines.push(format!(
                "{target}: {}",
                if ok && !reason.contains("exporte") {
                    "ok"
                } else {
                    &reason
                }
            ));
        }
        Ok(if lines.is_empty() {
            "nada faltava".into()
        } else {
            lines.join("; ")
        })
    }
    fn repair_files(&self, id: &str) -> Result<String, Failure> {
        let env = &self.accounts.env;
        match id {
            "skills" => {
                let report = super::skills::rebuild_report(env, &self.repo, false);
                if !report.errors.is_empty() {
                    return Err(invalid(
                        report
                            .errors
                            .values()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" | "),
                    ));
                }
                let lines = report
                    .stats
                    .as_object()
                    .ok_or_else(|| failed("relatório de skills inválido"))?
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{k}: {} criados, {} removidos",
                            v["criados"].as_u64().unwrap_or(0),
                            v["removidos"].as_u64().unwrap_or(0)
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(if lines.is_empty() {
                    "nada a fazer".into()
                } else {
                    lines.join("; ")
                })
            }
            "hooks-claude" => {
                let (changes, errors) = super::hooks::sync_report(&self.accounts, &self.repo);
                if !errors.is_empty() {
                    return Err(failed(errors.join(" | ")));
                }
                let mut paths = BTreeSet::new();
                for values in changes.as_object().into_iter().flat_map(|v| v.values()) {
                    for value in values
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        paths.insert(value.to_owned());
                    }
                }
                Ok(format!("{} config dirs gravados", paths.len()))
            }
            "hooks-kimi" => {
                let changed = super::kimi_hooks::sync(env, &self.repo).unwrap_or_default();
                if details::kimi_hooks(env, &self.repo).map_err(failed)?["ok"] != true {
                    return Err(invalid(
                        "hooks do Kimi não ficaram instalados — ver o log do backend",
                    ));
                }
                Ok(if changed.is_empty() {
                    "já estava"
                } else {
                    "config.toml gravado"
                }
                .into())
            }
            "contas" => {
                let mut warnings = vec![];
                for account in self
                    .accounts
                    .snapshot(Provider::Claude)
                    .map_err(|e| failed(e.message))?
                {
                    let result = claude_config::reconcile_account(
                        &env.home,
                        &account.id,
                        None,
                        claude_config::Wait::within(Duration::from_secs(30)),
                    );
                    warnings.extend(
                        result
                            .result
                            .map_err(|e| failed(e.to_string()))?
                            .into_iter()
                            .map(|s| format!("{}: {s}", account.id)),
                    );
                }
                Ok(if warnings.is_empty() {
                    "contas reconciliadas, nada fora do lugar".into()
                } else {
                    warnings.join("; ")
                })
            }
            "fullscreen:claude" => {
                let path = env.home.join(".claude/settings.json");
                let data = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| codec::loads(&s))
                    .filter(Value::is_object)
                    .ok_or_else(|| invalid("settings.json ilegível"))?;
                let mut data = crate::config_sync::runtime_values::ValueGuard(data);
                if let Some(old) = data
                    .as_object_mut()
                    .unwrap()
                    .insert("tui".into(), json!("fullscreen"))
                {
                    codec::discard(old);
                }
                super::files::write_preserving(
                    &path,
                    &format!(
                        "{}\n",
                        crate::transcript::pyjson::dumps_indent(&data, 2, false)
                    ),
                )
                .map_err(failed)?;
                Ok("tui = fullscreen no settings.json; vale nas sessões novas".into())
            }
            "fullscreen:omp" => Err(invalid(
                "o omp não tem tela cheia: a rolagem dele é a do terminal (ver CLAUDE.md)",
            )),
            "fullscreen:pi" => {
                let path = env.home.join(".pi/agent/fullscreen-tui.json");
                if path.exists() {
                    return Err(invalid("já configurado — /fullscreen-on na TUI"));
                }
                crate::runtime::queue::atomic_write(&path, b"{\n  \"enabled\": true\n}\n")
                    .map_err(|e| failed(e.to_string()))?;
                Ok("fullscreen ligado; vale nas sessões novas".into())
            }
            "extensoes:pi" | "extensoes:omp" => self.link_extensions(&id[10..]),
            _ => Err(invalid(format!("conserto desconhecido: {id}"))),
        }
    }
    fn link_extensions(&self, cli: &str) -> Result<String, Failure> {
        let target = health::agent_dir(&self.accounts.env, cli)
            .map_err(failed)?
            .join("extensions");
        std::fs::create_dir_all(&target).map_err(|e| failed(e.to_string()))?;
        let mut count = 0;
        for name in [
            "hangar-state",
            "rich-status-line",
            "claude-bridge",
            "claude-todo",
            "claude-hooks-adapter",
            "git-checkpoint",
            "fullscreen-tui",
            "lib",
        ] {
            let file = if name == "lib" {
                name.to_owned()
            } else {
                format!("{name}.ts")
            };
            let path = target.join(&file);
            let source = self.repo.join("scripts/pi").join(file);
            let wanted = cli != "omp" || !matches!(name, "claude-todo" | "fullscreen-tui");
            if !wanted {
                if path.is_symlink()
                    && std::fs::canonicalize(&path)
                        .ok()
                        .is_some_and(|p| std::fs::canonicalize(&source).ok().as_ref() == Some(&p))
                {
                    std::fs::remove_file(path).map_err(|e| failed(e.to_string()))?;
                }
                continue;
            }
            if !source.exists() || (path.exists() && !path.is_symlink()) {
                continue;
            }
            if path.is_symlink() {
                std::fs::remove_file(&path).map_err(|e| failed(e.to_string()))?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&source, &path).map_err(|e| failed(e.to_string()))?;
            #[cfg(windows)]
            {
                if name == "lib" {
                    std::os::windows::fs::symlink_dir(&source, &path)
                } else {
                    std::os::windows::fs::symlink_file(&source, &path)
                }
                .map_err(|e| failed(e.to_string()))?;
            }
            count += 1;
        }
        Ok(format!("{count} extensões ligadas"))
    }
    async fn repair_wrapper(&self, id: &str) -> Result<String, Failure> {
        let (argv, label, success) = if cfg!(windows) {
            let shell = crate::cli_probe::resolve_program("powershell.exe", &self.accounts.env)
                .ok_or_else(|| invalid("powershell.exe não encontrado"))?;
            let script = if id == "tmux" {
                "setup-windows-tmux.ps1"
            } else {
                "setup-windows-wrappers.ps1"
            };
            let mut args = vec![
                shell.to_string_lossy().into_owned(),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                self.repo
                    .join("scripts")
                    .join(script)
                    .to_string_lossy()
                    .into_owned(),
                "-Apply".into(),
            ];
            if id == "tmux" {
                args.push("-SkipInstall".into());
            }
            (
                args,
                script.to_owned(),
                if id == "tmux" {
                    "config do psmux reaplicada — vale nas sessões novas"
                } else {
                    "bloco dos wrappers no perfil do PowerShell — vale em terminal novo"
                },
            )
        } else {
            (
                self.wrapper_command().map_err(invalid)?,
                "instalador".into(),
                if id == "tmux" {
                    "bloco do ~/.tmux.conf refeito e recarregado"
                } else {
                    "wrappers reescritos no rc — vale em terminal novo"
                },
            )
        };
        let env = self.accounts.env.base.clone();
        let cwd = self.repo.clone();
        let output = tokio::task::spawn_blocking(move || {
            crate::update::process::run_capture_with_env(
                &argv,
                &cwd,
                Duration::from_secs(900),
                &env,
            )
        })
        .await
        .map_err(|e| failed(e.to_string()))?
        .map_err(|e| failed(e.to_string()))?;
        if output.code != 0 {
            let text = if output.stderr.is_empty() {
                output.stdout
            } else {
                output.stderr
            };
            let detail = text
                .chars()
                .rev()
                .take(300)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            return Err(invalid(format!(
                "{label} saiu com {}: {detail}",
                output.code
            )));
        }
        Ok(success.into())
    }
}
fn omp_key(
    env: &crate::accounts::environment::AccountEnvironment,
    provider: &str,
    key: &str,
) -> (bool, String) {
    let path = match env.omp_directories() {
        Ok(d) => d.agent_dir.join("agent.db"),
        Err(e) => return (false, e.into()),
    };
    if !path.is_file() {
        return (false, "nao-instalado".into());
    }
    let result = (|| -> rusqlite::Result<bool> {
        let db = rusqlite::Connection::open(&path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        let exists = db.query_row(
            "select 1 from auth_credentials where provider=? limit 1",
            [provider],
            |_| Ok(true),
        );
        match exists {
            Ok(_) => return Ok(false),
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(e),
        }
        db.execute("insert into auth_credentials (provider, credential_type, data) values (?, 'api_key', ?)",rusqlite::params![provider,codec::dumps(&json!({"key":key,"source":"manual"}),false)])?;
        Ok(true)
    })();
    match result {
        Ok(false) => (true, "ja-existe".into()),
        Ok(true) => (true, path.to_string_lossy().into_owned()),
        Err(e) => (false, format!("sqlite: {e}")),
    }
}
