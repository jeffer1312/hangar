//! O que a tela do Computer Use mostra. Chave nunca sai inteira: só se existe e os 4 últimos caracteres.
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{Map, Value, json};

use hangar_core::pycompat::{py, truthy};

use crate::files::{known_from, read_entry, read_object};
use crate::{Error, Paths, ssh_hosts, targets};

const PRESET_URL: &str = "http://127.0.0.1:8317/v1/chat/completions";

/// O que vem do servidor: o env do Jev já resolvido (`TYPESAFE_API_KEY`, `LLM_*`…) e o CLIProxyAPI.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub jev: Map<String, Value>,
    pub cliproxy_keys: Vec<String>,
    pub cliproxy_installed: bool,
}

/// Lê na ordem do Python, para o mesmo arquivo ruim aparecer primeiro.
pub fn state(paths: &Paths, facts: &Facts) -> Result<Value, Error> {
    let main = read_entry(paths.main_file(), false)?;
    let enabled = main.entry.is_some();
    let known = known_from(paths, main.entry.clone())?;
    let install = read_object(&paths.install_json(), false)?;
    let env = env_of(known.as_ref(), paths, enabled)?;
    let targets = targets(paths);
    let agents: Vec<Value> = targets.iter().map(|t| t["path"].clone()).collect();
    let exe = paths.exe();
    let exe_size = std::fs::metadata(&exe).ok().filter(|m| m.is_file()).map(|m| m.len());
    let binary = paths.binary().is_file();
    // Binário ausente depois de uma instalação automática que falhou é bloqueio real: a tela mostra.
    let install_error = (crate::supported() && !binary).then(|| crate::install::install_error(paths)).flatten();
    let unavailable = match (crate::supported(), &install_error) {
        (false, _) => json!("platform"),
        (true, Some(_)) => json!("install_failed"),
        (true, None) => Value::Null,
    };
    let jev =|key: &str| facts.jev.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    let (llm_key, jev_key) = (jev("LLM_PROXY_KEY"), jev("TYPESAFE_API_KEY"));
    let mut files = vec![json!({"path": main.path.to_string_lossy(), "enabled": enabled})];
    for path in paths.config_files().into_iter().skip(1) {
        let account = read_entry(path, true)?;
        files.push(json!({"path": account.path.to_string_lossy(), "enabled": account.entry.is_some()}));
    }
    let default_agent = agents.first().cloned().unwrap_or_else(|| json!(""));
    let mut out = json!({
        // O exe sai DAQUI até para o alvo remoto (copiado na 1ª conexão): sem ele, nenhum alvo funciona.
        "agent_exe": {"path": exe.to_string_lossy(), "exists": exe_size.is_some(), "size": exe_size.unwrap_or(0)},
        "mode": "package",
        "installed_tag": install.get("tag").cloned().unwrap_or_else(|| json!("")),
        "package_exists": truthy(install.get("tag").unwrap_or(&Value::Null)) && exe_size.is_some() && binary,
        "unavailable": unavailable,
        "targets": targets,
        "ssh_hosts": ssh_hosts(paths),
        "local_available": cfg!(windows) || (crate::supported() && binary),
        "platform": if cfg!(windows) { "windows" } else { "linux" },
        "enabled": enabled,
        "project_dir": "",
        "agent_config": env.get("HCC_AGENT_CONFIG").cloned().unwrap_or(default_agent),
        "agent_configs": agents,
        // Só leitura: o modelo auxiliar e o Jev são os da página Jev.
        "llm_from_jev_aux": true,
        "llm_url": jev("LLM_PROXY_URL"),
        "llm_model": jev("LLM_MODEL"),
        "llm_effort": jev("LLM_EFFORT"),
        "llm_key_set": !llm_key.is_empty(),
        "llm_key_tail": tail(&llm_key),
        "jev_key_set": !jev_key.is_empty(),
        "jev_key_tail": tail(&jev_key),
        "jev_endpoint": jev("JEV_ENDPOINT"),
        "jev_model": jev("JEV_MODEL"),
        "cliproxy": {
            "preset_url": PRESET_URL,
            "has_keys": !facts.cliproxy_keys.is_empty(),
            "installed": facts.cliproxy_installed,
            "running": cliproxy_running(),
            "key_is_cliproxy": !llm_key.is_empty() && facts.cliproxy_keys.contains(&llm_key),
        },
        "files": files,
    });
    if let Some(code) = install_error {
        out["install_error"] = json!(code);
    }
    // Surrogate solto lido dos arquivos sai como U+FFFD, como no Python.
    if let Some(map) = out.as_object_mut() {
        py::scrub_map(map);
    }
    Ok(out)
}

/// `env` falso vale `{}`; verdadeiro e não objeto é erro com código (o Python caía em 500 cru).
fn env_of(entry: Option<&Map<String, Value>>, paths: &Paths, active: bool) -> Result<Map<String, Value>, Error> {
    match entry.and_then(|e| e.get("env")).filter(|v| truthy(v)) {
        None => Ok(Map::new()),
        Some(Value::Object(env)) => Ok(env.clone()),
        Some(_) => {
            let file = if active { paths.main_file() } else { paths.parked_file() };
            Err(Error { status: 500, code: "erro_computer_control_io",
                msg: format!("env em {} não é um objeto JSON", file.display()),
                params: json!({"file": file.to_string_lossy(), "error": "env inválido"}) })
        }
    }
}

fn tail(key: &str) -> String {
    let count = key.chars().count();
    if count < 8 { String::new() } else { key.chars().skip(count - 4).collect() }
}

fn cliproxy_running() -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], 8317)), Duration::from_millis(500)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_counts_code_points() {
        assert_eq!(tail("1234567"), "");
        assert_eq!(tail("abcd5678"), "5678");
        assert_eq!(tail("ççççççç-çé"), "ç-çé");
    }
}
