//! Motor de atualização do checkout: conserva trabalho local antes de alinhar com o origin.
mod auth_pair;
pub mod automatic;
pub mod channel;
pub mod http;
pub mod launch;
pub mod pinning;
pub mod pipeline;
pub mod prepare;
pub mod releases;
pub mod status;
pub mod system;
pub mod periodic;
pub mod periodic_tick;
use super::{
    StateFile,
    process::{ProcessOutput, run_with_env},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Service {
    pub home: PathBuf,
    pub root: PathBuf,
    pub state_dir: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub writing: bool,
    ordered_keys: Vec<String>,
    settings: Result<std::sync::Arc<crate::machine_peers::dotenv::SettingsSnapshot>,String>,
}
impl Service {
    pub fn new(
        home: PathBuf,
        root: PathBuf,
        claude: PathBuf,
        environment: BTreeMap<String, String>,
    ) -> Self {
        let ordered_keys=environment.keys().cloned().collect();
        let captured=crate::machine_peers::dotenv::SettingsEnvironment::from_pairs(environment.clone());
        let settings=crate::machine_peers::dotenv::SettingsSnapshot::capture(&home,Some(&root.join("backend")),captured).map(std::sync::Arc::new);
        Self {
            ordered_keys,
            settings,
            home,
            root,
            state_dir: super::state_dir(&claude),
            environment,
            writing: false,
        }
    }
    pub fn from_state(st: &crate::routes::AppState) -> Result<Self, String> {
        let home = st.accounts.env.home.clone();
        let root = crate::install_root::root(&home).ok_or("checkout da instalação indisponível")?;
        let mut service=Self::new(
            home,
            root,
            st.accounts.env.claude_base.clone(),
            st.accounts.env.base.clone(),
        );
        service.ordered_keys=st.accounts.env.ordered_pairs().into_iter().map(|(key,_)|key).collect();
        service.settings=st.settings_snapshot();
        Ok(service)
    }
    pub fn binary(&self, name: &str) -> Option<PathBuf> {
        let Some(path) = self.environment.get("PATH") else { return crate::machine_peers::doctor::find_binary(name); };
        std::env::split_paths(path).flat_map(|dir| {
            let names = if cfg!(windows) { vec![name.to_owned(), format!("{name}.exe"), format!("{name}.cmd")] } else { vec![name.to_owned()] };
            names.into_iter().map(move |name|dir.join(name))
        }).find(|path| {
            if !path.is_file() { return false; }
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; path.metadata().is_ok_and(|m|m.permissions().mode() & 0o111 != 0) }
            #[cfg(not(unix))] { true }
        })
    }
    pub fn state(&self) -> StateFile {
        StateFile::new(&self.state_dir)
    }
    fn settings_environment(&self)->crate::machine_peers::dotenv::SettingsEnvironment {
        let pairs=self.ordered_keys.iter().filter_map(|key|self.environment.get(key).map(|value|(key.clone(),value.clone())))
            .chain(self.environment.iter().filter(|(key,_)|!self.ordered_keys.contains(key)).map(|(key,value)|(key.clone(),value.clone())));
        crate::machine_peers::dotenv::SettingsEnvironment::from_pairs(pairs)
    }
    pub fn value(&self, key: &str) -> Result<String, String> {
        self.settings.as_ref().map_err(Clone::clone)?.value(key).map(|value|value.unwrap_or_default())
    }
    pub fn target(&self) -> Result<String, String> {
        let value = self.channel_value("CP_UPDATE_BRANCH")?;
        Ok(if value.is_empty() {
            "main".into()
        } else {
            value
        })
    }
    pub fn git(&self, args: &[&str], seconds: u64) -> Result<ProcessOutput, String> {
        let mut argv = vec![
            "git".to_owned(),
            "-C".into(),
            self.root.display().to_string(),
        ];
        argv.extend(args.iter().map(|s| (*s).into()));
        self.run(&argv, &self.root, seconds)
    }
    pub fn run(&self, args: &[String], cwd: &Path, seconds: u64) -> Result<ProcessOutput, String> {
        let state = self.state();
        run_with_env(
            args,
            cwd,
            Duration::from_secs(seconds),
            &self.environment,
            &|line| {
                if self.writing {
                    state.log(line);
                }
            },
        )
        .map_err(|error| error.to_string())
    }
    pub fn preflight(&self) -> Result<Value, String> {
        let missing = ["git", "node", "npm", "uv"]
            .into_iter()
            .filter(|name| self.binary(name).is_none())
            .map(str::to_owned)
            .collect();
        let topology = self.topology();
        self.preflight_with(missing, topology)
    }
    /// As dependências e a topologia são fatos da máquina, independentes do estado git.
    pub fn preflight_with(&self, missing: Vec<String>, topology: &str) -> Result<Value, String> {
        let status = self.git(&["status", "--porcelain=v1", "--branch"], 30)?;
        if status.code != 0 {
            return Ok(
                json!({"pode":false,"erro":"nao_e_repo","detalhe":tail(&status,12),"faltando":missing}),
            );
        }
        let lines: Vec<_> = status.stdout.lines().collect();
        let header = lines
            .first()
            .copied()
            .filter(|line| line.starts_with("## "))
            .unwrap_or("");
        let dirty = lines
            .iter()
            .skip(1)
            .filter(|line| !line.starts_with("??"))
            .count();
        let count_header = |kind: &str| -> u64 {
            if !header.contains("...") || header.contains("[gone]") {
                return 0;
            }
            header
                .split_once(&format!("{kind} "))
                .and_then(|(_, tail)| tail.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        let (mut ahead, mut behind) = (count_header("ahead"), count_header("behind"));
        let branch = self
            .git(&["rev-parse", "--abbrev-ref", "HEAD"], 30)?
            .stdout
            .trim()
            .to_owned();
        let target = self.target()?;
        let mut uncertain = false;
        if !switches(&branch, &target) {
            let counts = self.git(
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("origin/{target}...HEAD"),
                ],
                30,
            )?;
            let values: Vec<_> = counts.stdout.split_whitespace().collect();
            if counts.code == 0 && values.len() == 2 {
                behind = values[0]
                    .parse()
                    .map_err(|_| "contagem do origin inválida")?;
                ahead = values[1].parse().map_err(|_| "contagem local inválida")?;
            } else {
                uncertain = self.git(&["remote", "get-url", "origin"], 30)?.code == 0;
            }
        }
        let previous = std::fs::read_to_string(self.state_dir.join("branch")).unwrap_or_default();
        let work_branch = !matches!(branch.as_str(), "main" | "master")
            && branch != target
            && branch != previous.trim();
        let commit = self
            .git(&["rev-parse", "HEAD"], 30)?
            .stdout
            .trim()
            .to_owned();
        Ok(
            json!({"pode":missing.is_empty(),"faltando":missing,"branch":branch,"alvo":target,"branch_de_trabalho":work_branch,
            "sujo":dirty,"ahead":ahead,"ahead_incerto":uncertain,"behind":behind,"divergiu":ahead>0&&behind>0,"topologia":topology,"commit":commit}),
        )
    }
    pub fn rescue(&self, pre: &Value) -> Result<Option<String>, String> {
        if pre["sujo"].as_u64().unwrap_or(0) == 0
            && pre["ahead"].as_u64().unwrap_or(0) == 0
            && pre["ahead_incerto"] != true
            && pre["branch_de_trabalho"] != true
        {
            return Ok(None);
        }
        let name = format!("resgate/{}", chrono::Local::now().format("%Y-%m-%d-%H%M"));
        let branch = self.git(&["branch", "--force", &name, "HEAD"], 60)?;
        if branch.code != 0 {
            return Err(format!(
                "nao consegui criar a branch de resgate: {}",
                tail(&branch, 12)
            ));
        }
        if pre["sujo"].as_u64().unwrap_or(0) > 0 {
            let stash = self.git(
                &[
                    "stash",
                    "push",
                    "--include-untracked",
                    "-m",
                    &format!("hangar {name}"),
                ],
                120,
            )?;
            if stash.code != 0 {
                return Err(format!(
                    "nao consegui guardar as mudancas: {}",
                    tail(&stash, 12)
                ));
            }
        }
        if self
            .git(
                &["rev-parse", "--verify", &format!("refs/heads/{name}")],
                30,
            )?
            .code
            != 0
        {
            return Err(format!(
                "a branch de resgate {name} nao existe depois de criada"
            ));
        }
        Ok(Some(name))
    }
    pub fn switch_branch(&self, target: &str, reference: &str) -> Result<(), String> {
        let local = format!("refs/heads/{target}");
        let exists = self
            .git(&["rev-parse", "--verify", "--quiet", &local], 30)?
            .code
            == 0;
        if exists
            && self
                .git(
                    &[
                        "merge-base",
                        "--is-ancestor",
                        &local,
                        &format!("origin/{target}"),
                    ],
                    30,
                )?
                .code
                != 0
        {
            let name = format!(
                "resgate/{}-{target}",
                chrono::Local::now().format("%Y-%m-%d-%H%M")
            );
            let branch = self.git(&["branch", "--force", &name, &local], 60)?;
            if branch.code != 0
                || self
                    .git(
                        &["rev-parse", "--verify", &format!("refs/heads/{name}")],
                        30,
                    )?
                    .code
                    != 0
            {
                return Err(format!(
                    "nao consegui guardar a branch local {target}: {}",
                    tail(&branch, 12)
                ));
            }
            self.state().warn(&format!(
                "a branch local {target} tinha commits fora do origin: guardados em {name}"
            ));
        }
        let args = if exists {
            vec!["checkout", target]
        } else {
            vec!["checkout", "-b", target, reference]
        };
        let first = self.git(&args, 120)?;
        if first.code != 0 {
            let label = format!("hangar antes de trocar para {target}");
            let stash = self.git(&["stash", "push", "--include-untracked", "-m", &label], 120)?;
            if stash.code != 0 {
                return Err(format!(
                    "nao consegui guardar o que estava no disco: {}",
                    tail(&stash, 12)
                ));
            }
            self.state()
                .warn(&format!("arquivos soltos guardados no stash '{label}'"));
            let retry = self.git(&args, 120)?;
            if retry.code != 0 {
                return Err(format!(
                    "nao consegui trocar para a branch {target}: {}",
                    tail(&retry, 12)
                ));
            }
        }
        if !exists {
            let upstream = self.git(
                &["branch", "--set-upstream-to", &format!("origin/{target}")],
                30,
            )?;
            if upstream.code != 0 {
                self.state().warn(&format!(
                    "a branch {target} ficou sem upstream: {}",
                    tail(&upstream, 4)
                ));
            }
        }
        self.mark_channel(target);
        Ok(())
    }
    pub fn mark_channel(&self, target: &str) {
        let path = self.state_dir.join("branch");
        let result = if matches!(target, "main" | "master") {
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        } else {
            std::fs::create_dir_all(&self.state_dir).and_then(|_| {
                crate::runtime::queue::atomic_write(&path, target.as_bytes())
            })
        };
        if let Err(error) = result {
            self.state().warn(&format!("nao consegui gravar a branch da atualizacao ({target}): {error}: '{}'",path.display()));
        }
    }
}
pub fn switches(current: &str, target: &str) -> bool {
    !current.is_empty()
        && current != target
        && !(target == "main" && matches!(current, "main" | "master"))
}
fn without_ansi(value: &str) -> std::borrow::Cow<'_, str> {
    static ANSI: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());
    ANSI.replace_all(value, "")
}
pub fn tail(result: &ProcessOutput, limit: usize) -> String {
    let text = without_ansi(if result.stderr.is_empty() { &result.stdout } else { &result.stderr });
    let lines = crate::transcript::py::splitlines(crate::transcript::py::strip(&text));
    let start = if limit == 0 { 0 } else { lines.len().saturating_sub(limit) };
    lines[start..].join("\n")
}
pub fn failure_reason(result: &ProcessOutput) -> String {
    let joined = format!("{}{}", result.stdout, result.stderr);
    let text = without_ansi(&joined);
    let marked: Vec<_> = crate::transcript::py::splitlines(&text).into_iter()
        .filter_map(|line| line.split_once("##HANGAR-FALHA##").map(|(_, value)| crate::transcript::py::strip(value)))
        .collect();
    if marked.is_empty() { tail(result, 12) } else { marked.join("\n") }
}

pub fn valid_branch(value: &str) -> bool {
    value.is_empty()
        || (value.len() <= 100
            && !value.starts_with('-')
            && !value.contains("..")
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-')
            }))
}
impl Service {
    pub fn screen_state(&self) -> super::StateSnapshot {
        let state = self.state();
        let current = state.read();
        if current.get("fase").and_then(Value::as_str) != Some("rodando") {
            return current;
        }
        let _guard = match self.transition_guard() {
            Ok(Some(guard)) => guard,
            Ok(None) => return current,
            Err(error) => return super::StateSnapshot::failure(format!("update_lock_unavailable: {error}")),
        };
        // A leitura anterior pode pertencer à tentativa que acabou de transferir a posse.
        let current = state.read();
        if current.get("fase").and_then(Value::as_str) != Some("rodando") { return current; }
        match std::fs::read_to_string(self.state_dir.join("rodando.lock")) {
            Ok(raw) => if let Ok(pid) = raw.trim().parse::<u32>() {
                match crate::terminal_process::pid_alive(pid) {
                    Ok(true) => return current,
                    Ok(false) => {},
                    Err(error) => return super::StateSnapshot::failure(format!("update_lock_unavailable: {error}")),
                }
            },
            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData) => {},
            Err(error) => return super::StateSnapshot::failure(format!("update_lock_unavailable: {error}")),
        }
        let owner = current.get("pid").and_then(Value::as_u64).or_else(|| {
            std::fs::read_to_string(self.state_dir.join("rodando.lock"))
                .ok()?
                .trim()
                .parse()
                .ok()
        });
        if owner
            .and_then(|pid| u32::try_from(pid).ok())
            .is_some_and(super::pid_alive)
        {
            return current;
        }
        let recent = current
            .get("ts")
            .and_then(Value::as_str)
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .is_some_and(|ts| {
                let age = chrono::Local::now().fixed_offset() - ts;
                age >= chrono::TimeDelta::zero() && age < chrono::TimeDelta::seconds(120)
            });
        if owner.is_none() && recent {
            return current;
        }
        let backend = current.get("backend_pid").and_then(Value::as_u64);
        let target = current
            .get("commit_para")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let restarted = matches!(
            current.get("etapa").and_then(Value::as_str),
            Some("reiniciar" | "nativo")
        ) && backend.is_some_and(|pid| pid != u64::from(std::process::id()))
            && target.is_some_and(|target| {
                self.git(&["rev-parse", "HEAD"], 30)
                    .is_ok_and(|output| output.stdout.trim() == target)
            });
        if restarted {
            let marker = self.home.join(".hangar/native/release.json");
            let native = match std::fs::read(&marker) {
                Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .filter(Value::is_object)
                    .map(|v| !crate::machine_peers::store::truthy(&v["sem_build"]))
                    .unwrap_or(true),
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
            };
            if native {
                let why = if current.get("etapa").and_then(Value::as_str) == Some("reiniciar") {
                    "a atualizacao foi concluida pelo servidor novo"
                } else {
                    "o processo que o atualizava nao existe mais"
                };
                state.warn(&format!("app nativo nao atualizou: {why}"));
            }
            let _ = std::fs::remove_dir_all(self.root.join("frontend/.dist-velho"));
            state.write(json!({"fase":"pronto","ok":true,"texto":"Atualizado"}));
        } else {
            state.write(json!({"fase":"pronto","ok":false,"voltou":false,"no_ar":true,"pid":owner,"erro":"a atualizacao foi interrompida — o processo que a executava nao existe mais"}));
        }
        let _ = std::fs::remove_file(self.state_dir.join("rodando.lock"));
        state.read()
    }
}
