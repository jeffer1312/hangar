//! Publicação do Jev nos registros ativos e guardados do Computer Use.
use super::{
    legacy::{AUX, base_url, read_object},
    runtime::RuntimeConfig,
};
use crate::orchestration::truthy;
use serde_json::{Map, Value, json};
use std::path::Path;
const NAME: &str = "hangar-computer-control";
const LLM: [&str; 4] = ["LLM_PROXY_URL", "LLM_MODEL", "LLM_EFFORT", "LLM_PROXY_KEY"];
const REPLACED: [&str; 7] = [
    "TYPESAFE_API_KEY",
    "JEV_ENDPOINT",
    "JEV_MODEL",
    "LLM_PROXY_URL",
    "LLM_PROXY_KEY",
    "LLM_MODEL",
    "LLM_EFFORT",
];

fn env_of(entry: &Map<String, Value>, path: &Path) -> Result<Map<String, Value>, String> {
    match entry.get("env").filter(|v| truthy(v)) {
        None => Ok(Map::new()),
        Some(Value::Object(env)) => Ok(env.clone()),
        _ => Err(format!("env em {} não é um objeto JSON", path.display())),
    }
}
fn write(
    path: &Path,
    data: Map<String, Value>,
    private: bool,
    expected: Option<&[u8]>,
    link: Option<&Path>,
) -> Result<(), String> {
    super::files::write_file_checked(
        path,
        &super::bundle::Blob {
            data: crate::transcript::pyjson::dumps_indent(&json!(data), 2, true).into_bytes(),
            mode: if private {
                0o600
            } else {
                super::files::mode(path).unwrap_or(0o644)
            },
        },
        expected,
        link,
    )
    .map_err(|e| e.to_string())
}
impl RuntimeConfig {
    fn jev_env(&self) -> Map<String, Value> {
        let mut output = Map::new();
        let key = self
            .get("jev_api_key")
            .as_str()
            .unwrap_or("")
            .trim()
            .to_owned();
        if !key.is_empty() {
            let endpoint = self.get("jev_endpoint");
            let model = self.get("jev_model");
            let (endpoint, model) =
                crate::voice::jev::destination(&key, endpoint.as_str(), model.as_str());
            output.insert("TYPESAFE_API_KEY".into(), json!(key));
            output.insert(
                "JEV_ENDPOINT".into(),
                json!(endpoint.unwrap_or_else(|| "https://api.typesafe.ai/v1/systemone".into())),
            );
            output.insert(
                "JEV_MODEL".into(),
                json!(model.unwrap_or_else(|| "jev-latest".into())),
            );
        }
        for (key, target) in [AUX[0], AUX[2], AUX[3], AUX[1]].into_iter().zip(LLM) {
            let value = self.get(key).as_str().unwrap_or("").trim().to_owned();
            if value.is_empty() {
                continue;
            }
            let value = if key == AUX[0] {
                let base = base_url(&value);
                let split = base.find(['?', '#']).unwrap_or(base.len());
                format!("{}/chat/completions{}", &base[..split], &base[split..])
            } else {
                value
            };
            output.insert(target.into(), json!(value));
        }
        output
    }
    fn preserve_old(
        &self,
        path: &Path,
        old: &Map<String, Value>,
        new: &Map<String, Value>,
    ) -> Result<(), String> {
        let old: Map<String, Value> = LLM
            .into_iter()
            .filter_map(|key| {
                old.get(key).filter(|v| truthy(v)).map(|v| {
                    (
                        key.into(),
                        json!(crate::orchestration::codec::python_string(v)),
                    )
                })
            })
            .collect();
        if old.is_empty() || old.iter().all(|(k, v)| new.get(k) == Some(v)) {
            return Ok(());
        }
        let backup = self.env.home.join(".hangar/computer-use-llm-antigo.json");
        let link = std::fs::read_link(&backup).ok();
        let raw = crate::codex_setup::files::read(&backup).map_err(|error| error.to_string())?;
        let mut previous = read_object(&backup)?;
        let source = path.to_string_lossy().into_owned();
        if previous.contains_key(&source) {
            return Ok(());
        }
        previous.insert(source, json!(old));
        write(&backup, previous, true, raw.as_deref(), link.as_deref())
    }
    pub fn sync_jev(&self) -> Result<(), String> {
        let jev = self.jev_env();
        let service = crate::accounts::AccountService::new(self.env.clone());
        let mut paths = vec![self.env.home.join(".claude.json")];
        let default = crate::accounts::catalog::resolved(&self.env.home.join(".claude"));
        for (_, account) in service.claude_accounts().map_err(|e| e.message)? {
            let path = account.home.join(".claude.json");
            if crate::accounts::catalog::resolved(&account.home) != default && path.is_file() {
                paths.push(path);
            }
        }
        // Todos os documentos são lidos sob a mesma trava dos demais escritores participantes.
        // A ordem evita inversão de travas entre duas publicações; editores externos não participam.
        let mut locked_paths = paths.clone();
        locked_paths.push(self.env.home.join(".hangar/computer-control.json"));
        locked_paths.push(self.env.home.join(".hangar/computer-use-llm-antigo.json"));
        let mut locked_paths: Vec<_> = locked_paths
            .into_iter()
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect();
        locked_paths.sort();
        locked_paths.dedup();
        let mut guards = Vec::new();
        for path in &locked_paths {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            guards.push(crate::session_open::claude::lock_config(path)?);
        }
        let merged = |env: &Map<String, Value>| {
            let mut result: Map<_, _> = env
                .iter()
                .filter(|(k, _)| !REPLACED.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            result.extend(jev.clone());
            result
        };
        let mut pending = Vec::new();
        for path in paths {
            let data = read_object(&path)?;
            let servers = data.get("mcpServers").filter(|v| truthy(v));
            if servers.is_some_and(|v| !v.is_object()) {
                return Err(format!(
                    "mcpServers em {} não é um objeto JSON",
                    path.display()
                ));
            }
            let entry = servers
                .and_then(|v| v.get(NAME))
                .and_then(Value::as_object)
                .cloned();
            if let Some(entry) = entry {
                let env = env_of(&entry, &path)?;
                let next = merged(&env);
                if next != env {
                    self.preserve_old(&path, &env, &jev)?;
                    pending.push(path);
                }
            }
        }
        let publish = |path: &Path, active: bool| -> Result<(), String> {
            let read_raw = || match crate::runtime::queue::read_replaced(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.to_string()),
            };
            for _ in 0..3 {
                let link = std::fs::read_link(path).ok();
                let raw = read_raw()?;
                let mut data = read_object(path)?;
                let entry = if active {
                    let servers = data.get("mcpServers").filter(|v| truthy(v));
                    if servers.is_some_and(|v| !v.is_object()) {
                        return Err(format!(
                            "mcpServers em {} não é um objeto JSON",
                            path.display()
                        ));
                    }
                    servers
                        .and_then(|v| v.get(NAME))
                        .and_then(Value::as_object)
                        .cloned()
                } else if data.is_empty() {
                    None
                } else {
                    Some(data.clone())
                };
                let Some(mut entry) = entry else {
                    return Ok(());
                };
                let env = env_of(&entry, path)?;
                let next = merged(&env);
                if next == env {
                    return Ok(());
                }
                self.preserve_old(path, &env, &jev)?;
                entry.insert("env".into(), json!(next));
                if active {
                    data.get_mut("mcpServers").unwrap()[NAME] = json!(entry);
                } else {
                    data = entry;
                }
                // Detecta alterações observadas durante a mescla; não é CAS com escritor externo.
                if read_raw()? != raw {
                    continue;
                }
                return write(path, data, !active, raw.as_deref(), link.as_deref());
            }
            Err(format!(
                "Arquivo continua mudando: {}; tentar novamente",
                path.display()
            ))
        };
        publish(&self.env.home.join(".hangar/computer-control.json"), false)?;
        for path in pending {
            publish(&path, true)?;
        }
        Ok(())
    }
}
