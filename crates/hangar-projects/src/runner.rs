use crate::{Error, Service};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{LazyLock, Mutex},
    time::Duration,
};
static MAKE_TARGET: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^([a-zA-Z0-9_-]+):").unwrap());
static PREFS_WRITE: Mutex<()> = Mutex::new(());
const MAX_CUSTOM: usize = 500;
const MAX_LABEL: usize = 200;
const MAX_COMMAND: usize = 200_000;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub command: String,
    pub since: Option<i64>,
    pub exited: bool,
    pub exit_status: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runner {
    pub label: String,
    pub command: String,
    pub source: String,
    pub is_dev_guess: bool,
}
pub fn slug(cwd: &str) -> String {
    let base = Path::new(cwd)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!(
        "{}-{}",
        if base.is_empty() { "proj" } else { &base },
        &sha1_smol::Sha1::from(cwd).digest().to_string()[..6]
    )
}
impl Service {
    async fn mux(&self, args: Vec<String>) -> (bool, String, String) {
        let Some(program) = self.program.first() else {
            return (false, String::new(), "multiplexador indisponível".into());
        };
        let scoped = args.first().is_some_and(|a| a == "new-session") && !self.scope.is_empty();
        let mut cmd = tokio::process::Command::new(if scoped { &self.scope[0] } else { program });
        if scoped {
            cmd.args(&self.scope[1..]).arg(program);
        }
        cmd.args(&self.program[1..])
            .args(["-L", "cppkt-run"])
            .args(args)
            .env_clear()
            .envs(&self.env)
            .kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(5), cmd.output()).await {
            Ok(Ok(o)) => (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).into(),
                String::from_utf8_lossy(&o.stderr).into(),
            ),
            Ok(Err(e)) => (false, String::new(), e.to_string()),
            Err(_) => (false, String::new(), "prazo de 5s esgotado".into()),
        }
    }
    /// Ausente é vazio; ilegível é erro, para uma gravação nunca apagar as outras pastas.
    fn read_prefs(&self) -> Result<Value, Error> {
        let path = hangar_core::fs::legacy_sidecar_path(self.preferences.clone());
        match hangar_core::fs::read_replaced(&path) {
            Ok(b) => serde_json::from_slice::<Value>(&b)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(|| Error::coded(500, "runner_prefs_unreadable", "preferências dos comandos ilegíveis")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
            Err(e) => Err(Error::coded(500, "runner_prefs_unreadable", e.to_string())),
        }
    }
    fn preference(&self, cwd: &str) -> Value {
        Self::entry(&self.read_prefs().unwrap_or_else(|_| json!({})), cwd)
    }
    fn entry(p: &Value, cwd: &str) -> Value {
        if let Some(s) = p[cwd].as_str() {
            json!({"remembered":s,"custom":[]})
        } else if p[cwd].is_object() {
            p[cwd].clone()
        } else {
            json!({"remembered":null,"custom":[]})
        }
    }
    fn save_pref(&self, cwd: &str, key: &str, value: Value) -> Result<(), Error> {
        // O arquivo é de todas as sessões: duas gravações juntas perderiam uma das mudanças.
        let _serial = PREFS_WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = self.read_prefs()?;
        let mut entry = Self::entry(&p, cwd);
        entry[key] = value;
        p[cwd] = entry;
        if let Some(parent) = self.preferences.parent() {
            std::fs::create_dir_all(parent)?;
        }
        hangar_core::fs::atomic_write(
            &self.preferences,
            &serde_json::to_vec(&p).map_err(|e| Error::new(500, e.to_string()))?,
        )?;
        Ok(())
    }
    pub fn remembered(&self, cwd: &str) -> Option<String> {
        self.preference(cwd)["remembered"]
            .as_str()
            .map(str::to_owned)
    }
    pub fn custom_commands(&self, cwd: &str) -> Vec<Runner> {
        self.preference(cwd)["custom"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let label = v["label"].as_str()?;
                let command = v["command"].as_str()?;
                if label.trim().is_empty() || command.trim().is_empty() {
                    None
                } else {
                    Some(Runner {
                        label: label.into(),
                        command: command.into(),
                        source: "custom".into(),
                        is_dev_guess: false,
                    })
                }
            })
            .collect()
    }
    pub fn set_custom_commands(&self, cwd: &str, commands: Value) -> Result<Vec<Runner>, Error> {
        let commands = commands
            .as_array()
            .ok_or_else(|| Error::coded(422, "erro_corpo_invalido", "commands deve ser uma lista"))?;
        let over = |v: &Value, key: &str, max: usize| v[key].as_str().is_some_and(|s| s.chars().count() > max);
        if commands.len() > MAX_CUSTOM
            || commands.iter().any(|c| over(c, "label", MAX_LABEL) || over(c, "command", MAX_COMMAND))
        {
            return Err(Error::coded(422, "erro_corpo_invalido", "comandos personalizados acima do limite"));
        }
        for (i, c) in commands.iter().enumerate() {
            if c["label"].as_str().is_none_or(|s| s.trim().is_empty())
                || c["command"].as_str().is_none_or(|s| s.trim().is_empty())
            {
                return Err(Error::coded(
                    400,
                    "erro_runner_custom",
                    format!(
                        "comando personalizado {}: rotulo e comando sao obrigatorios",
                        i + 1
                    ),
                ));
            }
        }
        let commands: Vec<_> = commands
            .iter()
            .map(|v| json!({"label":v["label"],"command":v["command"]}))
            .collect();
        self.save_pref(cwd, "custom", json!(commands))?;
        Ok(self.custom_commands(cwd))
    }
    /// `None` = o multiplexador não respondeu: nem viva nem parada.
    async fn exists(&self, name: &str) -> Option<bool> {
        let (ok, _, error) = self
            .mux(vec!["has-session".into(), "-t".into(), format!("={name}")])
            .await;
        if ok {
            Some(true)
        } else {
            hangar_core::terminal_process::mux_absence(error.trim()).then_some(false)
        }
    }
    async fn kill_run(&self, name: &str) -> bool {
        self.mux(vec![
            "kill-session".into(),
            "-t".into(),
            if self.windows {
                name.into()
            } else {
                format!("={name}")
            },
        ])
        .await;
        if self.exists(name).await == Some(false) {
            return true;
        }
        let (_, pids, _) = self
            .mux(vec![
                "list-panes".into(),
                "-t".into(),
                format!("={name}:"),
                "-F".into(),
                "#{pane_pid}".into(),
            ])
            .await;
        for pid in pids
            .split_whitespace()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            let mut cmd = if self.windows {
                let mut c = tokio::process::Command::new("taskkill");
                c.args(["/F", "/PID", pid]);
                c
            } else {
                let mut c = tokio::process::Command::new("kill");
                c.args(["-9", pid]);
                c
            };
            let _ =
                tokio::time::timeout(Duration::from_secs(5), cmd.kill_on_drop(true).output()).await;
        }
        for _ in 0..20 {
            if self.exists(name).await == Some(false) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }
    pub async fn all_runs(&self) -> BTreeMap<String, RunInfo> {
        let (ok, out, _) = self
            .mux(vec![
                "list-panes".into(),
                "-a".into(),
                "-F".into(),
                "#{session_name}\t#{session_created}\t#{pane_dead}\t#{pane_dead_status}".into(),
            ])
            .await;
        let mut result = BTreeMap::new();
        if ok {
            for line in out.lines() {
                let p: Vec<_> = line.split('\t').collect();
                if p.len() == 4 {
                    result.insert(
                        p[0].into(),
                        RunInfo {
                            command: String::new(),
                            since: p[1].parse().ok(),
                            exited: p[2] == "1",
                            exit_status: p[3].parse().ok(),
                        },
                    );
                }
            }
        }
        result
    }
    pub async fn run_status(&self, cwd: &str) -> Option<RunInfo> {
        let mut info = self.all_runs().await.remove(&slug(cwd))?;
        info.command = self.remembered(cwd).unwrap_or_default();
        Some(info)
    }
    pub async fn start_run(&self, cwd: &str, command: &str) -> Result<RunInfo, Error> {
        self.save_pref(cwd, "remembered", json!(command))?;
        let name = slug(cwd);
        if !self.kill_run(&name).await {
            return Err(Error::new(
                409,
                format!(
                    "o run anterior nao morreu (sessao {name}); o novo NAO subiu — o processo antigo segue rodando"
                ),
            ));
        }
        self.mux(vec!["start-server".into()]).await;
        self.mux(vec![
            "set-option".into(),
            "-g".into(),
            "remain-on-exit".into(),
            "on".into(),
        ])
        .await;
        let shell = if self.windows {
            format!(
                "{} /c {command}",
                self.env
                    .get("COMSPEC")
                    .map(String::as_str)
                    .unwrap_or("cmd.exe")
            )
        } else {
            format!(
                "exec {} -lc '{}'",
                self.env
                    .get("SHELL")
                    .map(String::as_str)
                    .unwrap_or("/bin/sh"),
                command.replace('\'', "'\"'\"'")
            )
        };
        let (ok, _, error) = self
            .mux(vec![
                "new-session".into(),
                "-d".into(),
                "-s".into(),
                name,
                "-c".into(),
                cwd.into(),
                "-x".into(),
                "200".into(),
                "-y".into(),
                "50".into(),
                shell,
            ])
            .await;
        if !ok {
            return Err(Error::new(
                502,
                format!(
                    "o run nao subiu: {}",
                    if error.trim().is_empty() {
                        "rc=1"
                    } else {
                        error.trim()
                    }
                ),
            ));
        }
        Ok(self.run_status(cwd).await.unwrap_or(RunInfo {
            command: command.into(),
            since: None,
            exited: false,
            exit_status: None,
        }))
    }
    pub async fn stop_run(&self, cwd: &str) -> Result<(), Error> {
        let name = slug(cwd);
        if self.kill_run(&name).await {
            Ok(())
        } else {
            Err(Error::new(
                409,
                format!("o run (sessao {name}) nao morreu — o processo segue rodando"),
            ))
        }
    }
    pub async fn run_pane(&self, cwd: &str) -> String {
        self.mux(vec![
            "capture-pane".into(),
            "-p".into(),
            "-t".into(),
            format!("={}:", slug(cwd)),
            "-S".into(),
            "-200".into(),
        ])
        .await
        .1
    }
    pub fn detect_runners(&self, cwd: &str) -> Vec<Runner> {
        let cwd = Path::new(cwd);
        let mut out = Vec::new();
        let mut add = |label: String, command: String, source: &str| {
            out.push(Runner {
                label,
                command,
                source: source.into(),
                is_dev_guess: false,
            })
        };
        let pm = if cwd.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if cwd.join("bun.lockb").is_file() {
            "bun"
        } else if cwd.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm"
        };
        if let Some(data) = std::fs::read(cwd.join("package.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            if let Some(scripts) = data["scripts"].as_object() {
                for name in scripts.keys().filter(|s| !s.is_empty()) {
                    add(name.clone(), format!("{pm} run {name}"), "npm");
                }
            }
        }
        if let Ok(bytes) = std::fs::read(cwd.join("Makefile")) {
            let text = String::from_utf8_lossy(&bytes);
            let mut seen = std::collections::HashSet::new();
            for c in MAKE_TARGET.captures_iter(&text) {
                if seen.insert(c[1].to_owned()) {
                    add(c[1].into(), format!("make {}", &c[1]), "make");
                }
            }
        }
        if cwd.join("Cargo.toml").is_file() {
            add("cargo run".into(), "cargo run".into(), "stack");
        }
        if let Some(data) = std::fs::read_to_string(cwd.join("pyproject.toml"))
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        {
            if let Some(scripts) = data
                .get("project")
                .and_then(|p| p.get("scripts"))
                .and_then(toml::Value::as_table)
            {
                for name in scripts.keys().filter(|s| !s.is_empty()) {
                    add(name.clone(), format!("uv run {name}"), "stack");
                }
            }
        }
        let mut best = None;
        let mut score = 0;
        for (i, r) in out.iter().enumerate() {
            let rank = match r.label.to_lowercase().as_str() {
                "dev" => 5,
                "start" => 4,
                "serve" => 3,
                "watch" => 2,
                "run" => 1,
                _ => 0,
            };
            if rank > score {
                score = rank;
                best = Some(i);
            }
        }
        if let Some(i) = best {
            out[i].is_dev_guess = true;
        }
        out
    }
}
