//! Projetos e comandos de execução; os arquivos e o socket são os mesmos do launcher anterior.
#![forbid(unsafe_code)]
mod ports;
mod runner;

pub use runner::{RunInfo, Runner, slug};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub detail: String,
}
impl Error {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(500, e.to_string())
    }
}
#[derive(Clone)]
pub struct Service {
    pub config: PathBuf,
    pub preferences: PathBuf,
    pub program: Vec<String>,
    pub scope: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub windows: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStatus {
    pub name: String,
    pub cwd: String,
    pub command: String,
    pub port: Option<i64>,
    pub state: String,
    pub since: Option<i64>,
    pub exit_status: Option<i64>,
    pub tmux: String,
    pub has_stop_command: bool,
}
impl Service {
    fn load(&self) -> Result<Value, Error> {
        match hangar_core::fs::read_replaced_to_string(&self.config) {
            Ok(s) => serde_json::from_str::<Value>(&s)
                .map(|v| if v.is_object() { v } else { json!({}) })
                .map_err(|e| Error::new(500, format!("projects.json invalido: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
            Err(e) => Err(Error::new(500, format!("projects.json invalido: {e}"))),
        }
    }
    fn entry(&self, name: &str) -> Result<Value, Error> {
        let entry = self.load()?[name].clone();
        if !entry.is_object() {
            return Err(Error::new(
                404,
                format!("projeto '{name}' nao esta no projects.json"),
            ));
        }
        if !entry["cwd"].is_string() || !entry["command"].is_string() {
            return Err(Error::new(
                500,
                format!("projects.json: '{name}' precisa de cwd e command (string)"),
            ));
        }
        Ok(entry)
    }
    fn mutate(&self, change: impl FnOnce(&mut Value)) -> Result<(), Error> {
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.config.with_file_name("projects.json.lock"))?;
        lock.lock()?;
        let mut data = self.load()?;
        change(&mut data);
        let mut bytes =
            serde_json::to_vec_pretty(&data).map_err(|e| Error::new(500, e.to_string()))?;
        bytes.push(b'\n');
        hangar_core::fs::atomic_write(&self.config, &bytes)
            .map_err(|e| Error::new(500, format!("falha ao gravar projects.json: {e}")))
    }
    fn status(
        &self,
        name: &str,
        cfg: &Value,
        runs: &BTreeMap<String, RunInfo>,
        ports: &BTreeMap<i64, (bool, Option<String>)>,
    ) -> ProjectStatus {
        let cwd = cfg["cwd"].as_str().unwrap_or("");
        let port = cfg["port"].as_i64();
        let id = slug(cwd);
        let info = runs.get(&id);
        let (listening, owner) = port
            .and_then(|p| ports.get(&p))
            .cloned()
            .unwrap_or_default();
        let root = std::fs::canonicalize(cwd)
            .map(hangar_core::fs::without_verbatim)
            .unwrap_or_else(|_| PathBuf::from(cwd));
        let mine = owner
            .as_ref()
            .is_some_and(|o| o != "<indeterminado>" && PathBuf::from(o).starts_with(&root));
        let state = match info {
            None => {
                if mine {
                    "external"
                } else {
                    "stopped"
                }
            }
            Some(i) if i.exited => "failed",
            Some(_) if port.is_some_and(|p| p != 0) && !mine => {
                if listening && owner.as_deref() == Some("<indeterminado>") {
                    "running"
                } else {
                    "starting"
                }
            }
            Some(_) => "running",
        };
        ProjectStatus {
            name: name.into(),
            cwd: cwd.into(),
            command: cfg["command"].as_str().unwrap_or("").into(),
            port,
            state: state.into(),
            since: info.and_then(|i| i.since),
            exit_status: info.and_then(|i| i.exit_status),
            tmux: id,
            has_stop_command: cfg["stop_command"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()),
        }
    }
    pub async fn list(&self) -> Result<Vec<ProjectStatus>, Error> {
        let cfg = self.load()?;
        let runs = self.all_runs().await;
        let entries: Vec<_> = cfg
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, c)| c.is_object())
            .collect();
        let ports = ports::read(
            entries
                .iter()
                .filter_map(|(_, c)| c["port"].as_i64())
                .collect(),
        )
        .await;
        let mut result: Vec<_> = entries
            .into_iter()
            .map(|(n, c)| self.status(n, c, &runs, &ports))
            .collect();
        let rank = |s: &str| match s {
            "failed" => 0,
            "running" | "starting" | "external" => 1,
            _ => 2,
        };
        result.sort_by_key(|p| (rank(&p.state), p.name.to_lowercase()));
        Ok(result)
    }
    pub async fn upsert(
        &self,
        name: &str,
        cwd: &str,
        command: &str,
        port: Option<i64>,
        stop: Option<&str>,
    ) -> Result<ProjectStatus, Error> {
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(Error::new(400, "nome inválido (sem '/' nem '..')"));
        }
        if command.trim().is_empty() {
            return Err(Error::new(400, "command é obrigatório"));
        }
        let expanded = if cwd == "~" || cwd.starts_with("~/") {
            self.env
                .get("HOME")
                .map(|h| PathBuf::from(h).join(cwd.strip_prefix("~/").unwrap_or("")))
                .unwrap_or_else(|| cwd.into())
        } else {
            cwd.into()
        };
        if !expanded.is_dir() {
            return Err(Error::new(400, format!("cwd não existe: {cwd}")));
        }
        self.mutate(|d| {
            if !d[name].is_object() {
                d[name] = json!({});
            }
            d[name]["cwd"] = json!(cwd);
            d[name]["command"] = json!(command);
            if let Some(p) = port {
                d[name]["port"] = json!(p);
            }
            if let Some(s) = stop {
                d[name]["stop_command"] = json!(s);
            }
        })?;
        self.one(name).await
    }
    async fn one(&self, name: &str) -> Result<ProjectStatus, Error> {
        let cfg = self.entry(name)?;
        let ports = ports::read(cfg["port"].as_i64().into_iter().collect()).await;
        Ok(self.status(name, &cfg, &self.all_runs().await, &ports))
    }
    pub async fn remove(&self, name: &str) -> Result<(), Error> {
        let cfg = self.entry(name)?;
        if self
            .all_runs()
            .await
            .get(&slug(cfg["cwd"].as_str().unwrap()))
            .is_some_and(|i| !i.exited)
        {
            return Err(Error::new(
                409,
                format!("projeto '{name}' está rodando — pare antes de remover"),
            ));
        }
        self.mutate(|d| {
            d.as_object_mut().unwrap().remove(name);
        })
    }
    pub async fn start(&self, name: &str) -> Result<ProjectStatus, Error> {
        let cfg = self.entry(name)?;
        let cwd = cfg["cwd"].as_str().unwrap();
        if !PathBuf::from(cwd).is_dir() {
            return Err(Error::new(400, format!("cwd nao existe: {cwd}")));
        }
        self.start_run(cwd, cfg["command"].as_str().unwrap())
            .await?;
        self.one(name).await
    }
    pub async fn stop(&self, name: &str) -> Result<(), Error> {
        let cfg = self.entry(name)?;
        let cwd = cfg["cwd"].as_str().unwrap();
        let mut error = None;
        if let Some(stop) = cfg["stop_command"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
        {
            let shell = if self.windows {
                self.env
                    .get("COMSPEC")
                    .map(String::as_str)
                    .unwrap_or("cmd.exe")
            } else {
                "/bin/sh"
            };
            let args = if self.windows {
                vec!["/c", stop]
            } else {
                vec!["-lc", stop]
            };
            let mut cmd = tokio::process::Command::new(shell);
            cmd.args(args)
                .current_dir(cwd)
                .envs(&self.env)
                .kill_on_drop(true);
            match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
                Err(_) => {
                    error = Some("stop_command estourou 30s — confira processos orfaos".to_owned())
                }
                Ok(Err(e)) => error = Some(format!("stop_command falhou: {e}")),
                Ok(Ok(out)) if self.windows && !out.status.success() => {
                    let name = runner::first_token(stop);
                    let builtins = "assoc break call cd chdir cls color copy date del dir echo endlocal erase exit for ftype goto if md mkdir mklink move path pause popd prompt pushd rd rem ren rename rmdir set setlocal shift start time title type ver verify vol";
                    let mut env = self.env.clone();
                    let path = std::env::join_paths(
                        std::env::split_paths(env.get("PATH").map(String::as_str).unwrap_or(""))
                            .chain(std::iter::once(PathBuf::from(cwd))),
                    )
                    .unwrap_or_default();
                    env.insert("PATH".into(), path.to_string_lossy().into_owned());
                    if !name.is_empty()
                        && !builtins
                            .split_whitespace()
                            .any(|b| b.eq_ignore_ascii_case(&name))
                        && hangar_core::process::resolve_program(&name, &env).is_none()
                    {
                        error = Some(format!(
                            "stop_command nao rodou: o Windows nao tem `{name}` (nem no PATH nem em {cwd}) — o processo que ele mataria pode ter ficado orfao"
                        ));
                    }
                }
                _ => {}
            }
        }
        if let Err(mut e) = self.stop_run(cwd).await {
            if let Some(error) = error {
                e.detail.push_str(&format!(". Alem disso: {error}"));
            }
            return Err(e);
        }
        if let Some(e) = error {
            return Err(Error::new(500, e));
        }
        Ok(())
    }
    pub async fn pane(&self, name: &str) -> Result<String, Error> {
        Ok(self
            .run_pane(self.entry(name)?["cwd"].as_str().unwrap())
            .await)
    }
}
