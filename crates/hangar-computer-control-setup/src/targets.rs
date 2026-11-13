//! Alvos: cada `<nome>-agent.json` de `targets/` é uma máquina que o MCP alcança.
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use hangar_core::pycompat::{py, pyjson};

use crate::{Error, Paths, files};

const SUFFIX: &str = "-agent.json";

static NAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[a-z0-9][a-z0-9._-]{0,40}\z").expect("regex fixa"));
static NOT_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._-]+").expect("regex fixa"));
// Nunca começa com '-': o valor vai ao argv do ssh.
static HOST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[A-Za-z0-9_][A-Za-z0-9._@-]*\z").expect("regex fixa"));

/// Corpo de `POST /api/computer-control/targets`. `project_dir` segue obrigatório e ignorado: o app
/// nativo atual ainda o manda.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRequest {
    pub project_dir: String,
    pub name: String,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub proxy_command: String,
    #[serde(default)]
    pub request_timeout: Option<u16>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Ssh,
    Local,
}

impl TargetRequest {
    /// O `Field(ge=1, le=600)` do Python; fora disso é corpo inválido (422).
    pub fn valid(&self) -> bool {
        self.request_timeout.is_none_or(|t| (1..=600).contains(&t))
    }
}

/// Os alvos em ordem de nome de arquivo. Ilegível ou não objeto aparece mesmo assim, como local.
pub fn targets(paths: &Paths) -> Vec<Value> {
    target_files(paths).into_iter().map(|file| {
        let path = paths.targets_dir().join(&file);
        let mut cfg = match std::fs::read(&path).ok().and_then(|bytes| files::parse(&bytes)) {
            Some(Value::Object(cfg)) => cfg,
            _ => Map::new(),
        };
        py::scrub_map(&mut cfg);
        let transport = cfg.get("transport").cloned().unwrap_or_else(|| json!("local"));
        // Por SSH só há agente de Windows; o local é o sistema desta máquina.
        let os = if transport == "ssh" || cfg!(windows) { "windows" } else { "linux" };
        json!({
            "name": file.strip_suffix(SUFFIX).unwrap_or(&file),
            "path": path.to_string_lossy(),
            "transport": transport,
            "host": cfg.get("host").cloned().unwrap_or_else(|| json!("")),
            "os": os,
        })
    }).collect()
}

/// Grava `<nome>-agent.json`; a pasta `targets/` nasce aqui se faltar.
pub fn create_target(paths: &Paths, request: &TargetRequest) -> Result<(), Error> {
    // Mesma ordem do Python: `.claude.json` ilegível recusa antes de qualquer conferência.
    files::known_entry(paths)?;
    let local = request.transport == Transport::Local;
    let mut name = py::strip(&request.name).to_owned();
    if name.is_empty() && !local {
        name = name_from_host(&request.host);
    }
    if !NAME_RE.is_match(&name) {
        return Err(Error::plain(400, "erro_computer_control_target_name",
            "nome do alvo: letras minúsculas, números, ponto, hífen ou sublinhado"));
    }
    let dir = paths.targets_dir();
    let path = dir.join(format!("{name}{SUFFIX}"));
    if path.exists() {
        return Err(exists(&name));
    }
    let mut cfg = Map::new();
    if local {
        cfg.insert("transport".into(), json!("local"));
        if cfg!(windows) {
            cfg.insert("command".into(), json!([paths.exe().to_string_lossy()]));
        } else if !crate::supported() {
            return Err(Error::plain(400, "erro_computer_control_unavailable", "o Computer Use não está disponível nesta máquina"));
        } else if paths.binary().is_file() {
            cfg.insert("command".into(), json!([paths.binary().to_string_lossy(), "agent"]));
            cfg.insert("request_timeout".into(), json!(15));
        } else {
            return Err(Error::plain(400, "erro_computer_control_local_only_windows",
                "instale o programa antes de cadastrar este computador"));
        }
    } else {
        cfg.insert("transport".into(), json!("ssh"));
        cfg.insert("host".into(), json!(check_host(&request.host)?));
        cfg.insert("agent_path".into(), json!(paths.exe().to_string_lossy()));
        let proxy = py::strip(&request.proxy_command);
        if !proxy.is_empty() {
            cfg.insert("proxy_command".into(), json!(proxy));
        }
    }
    if let Some(timeout) = request.request_timeout {
        cfg.insert("request_timeout".into(), json!(timeout));
    }
    std::fs::create_dir_all(&dir).map_err(|e| Error::at(&dir, &e))?;
    let text = pyjson::dumps_indent(&Value::Object(cfg), 2, true) + "\n";
    // Temporário com sufixo aleatório e `hard_link`, que falha se o alvo já existe: de dois pedidos
    // iguais ao mesmo tempo, um grava inteiro e o outro recebe 409.
    let suffix = BuildHasher::hash_one(&RandomState::new(), std::process::id());
    let temp = dir.join(format!(".{name}{SUFFIX}.{suffix:016x}.tmp"));
    let linked = std::fs::write(&temp, text).and_then(|()| std::fs::hard_link(&temp, &path));
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(exists(&name)),
        Err(e) => Err(Error::at(&path, &e)),
    }
}

/// Os `<nome>-agent.json` de `targets/`, em ordem de nome.
pub(crate) fn target_files(paths: &Paths) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(paths.targets_dir()) else { return Vec::new() };
    let mut names: Vec<String> = listing
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        // Mesmo recorte do glob do Python: oculto fica fora.
        .filter(|name| name.ends_with(SUFFIX) && !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

fn exists(name: &str) -> Error {
    Error { status: 409, code: "erro_computer_control_target_exists", msg: format!("o alvo {name} já existe"),
        params: json!({"name": name}) }
}

/// Host aparado e conferido inteiro.
pub(crate) fn check_host(host: &str) -> Result<String, Error> {
    let host = py::strip(host);
    if !HOST_RE.is_match(host) {
        return Err(Error::plain(400, "erro_computer_control_target_host", "informe o host SSH (sem espaços)"));
    }
    Ok(host.to_owned())
}

/// "administrator@delphi-03" vira "delphi-03": depois do último `@`, minúsculo, corte em 41.
fn name_from_host(host: &str) -> String {
    let last = host.rsplit('@').next().unwrap_or("").to_lowercase();
    let replaced = NOT_NAME.replace_all(&last, "-");
    replaced.trim_matches(['-', '.', '_']).chars().take(41).collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn agent_reader_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let home = tempfile::tempdir().unwrap();
            let paths = Paths::new(home.path().to_owned(), vec![]);
            std::fs::create_dir_all(paths.targets_dir()).unwrap();
            std::fs::write(paths.targets_dir().join("a-agent.json"), &bytes).unwrap();
            prop_assert_eq!(targets(&paths).len(), 1);
        }

        #[test]
        fn name_from_any_host_never_panics(host in "\\PC{0,80}") {
            let name = name_from_host(&host);
            prop_assert!(name.chars().count() <= 41);
        }
    }
}
