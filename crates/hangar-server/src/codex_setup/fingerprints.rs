//! Assinaturas por fase separam fontes dos resultados gerenciados.
use super::{files, instructions, instructions_toml, phase::Error};
use crate::{
    accounts::environment::AccountEnvironment, config_sync::runtime_values::ValueGuard,
    orchestration::codec,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
fn error(kind: &'static str, message: impl Into<String>) -> Error {
    Error {
        kind,
        message: message.into(),
    }
}
fn io(error: std::io::Error) -> Error {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::NotADirectory => "NotADirectoryError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    };
    Error {
        kind,
        message: error.to_string(),
    }
}
fn take(value: &Value, key: &str, default: Value) -> Value {
    value.get(key).map(codec::clone_value).unwrap_or(default)
}
fn object<'a>(value: &'a Value) -> Result<&'a serde_json::Map<String, Value>, Error> {
    value.as_object().ok_or_else(|| {
        error(
            "AttributeError",
            format!("'{}' object has no attribute 'get'", type_name(value)),
        )
    })
}
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
fn keys(value: &Value) -> Result<Vec<String>, Error> {
    match value {
        Value::Object(map) => Ok(map.keys().cloned().collect()),
        Value::String(text) => Ok(text.chars().map(|c| c.to_string()).collect()),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error("TypeError", format!("argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{}'",type_name(v))))
            })
            .collect(),
        _ => Err(error(
            "TypeError",
            format!("'{}' object is not iterable", type_name(value)),
        )),
    }
}
fn records(path: &Path, windows: bool) -> Result<Vec<PathBuf>, Error> {
    let entries = match std::fs::read_dir(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(io(e)),
    };
    let mut result = Vec::new();
    for entry in entries {
        let path = entry.map_err(io)?.path();
        if super::files::matches_json_record(&path, windows) {
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
}
fn resolved(path: &Path) -> Result<PathBuf, Error> {
    super::plugins::resolved(path).map_err(|e| error("OSError", e))
}
fn walk(root: PathBuf, paths: &mut BTreeSet<PathBuf>) -> Result<(), Error> {
    paths.insert(root.clone());
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(v) => v,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => {
                continue;
            }
            Err(e) => return Err(io(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let is_dir = path.is_dir();
            if is_dir && ["__pycache__", ".git", ".hangar-uploads"].contains(&name.as_ref()) {
                continue;
            }
            if !name.ends_with(".pyc") {
                paths.insert(path.clone());
            }
            if is_dir && !entry.file_type().map_err(io)?.is_symlink() {
                pending.push(path);
            }
        }
    }
    Ok(())
}
pub(super) fn plugin_agents(
    home: &Path,
    settings: &Value,
) -> Result<Vec<(String, PathBuf)>, Error> {
    let installed = ValueGuard(files::object(
        &home.join(".claude/plugins/installed_plugins.json"),
    )?);
    let desired =
        super::plugin_selection::desired(settings).map_err(|e| error(e.kind, e.message))?;
    let mut result = Vec::new();
    if let Some(plugins) = installed.get("plugins").and_then(Value::as_object) {
        for id in desired {
            if let Some(items) = plugins.get(&id) {
                let items = match items {
                    Value::Array(items) => items,
                    Value::Object(_) | Value::String(_) => continue,
                    _ => {
                        return Err(error(
                            "TypeError",
                            format!("'{}' object is not iterable", type_name(items)),
                        ));
                    }
                };
                for item in items {
                    if item.get("scope") == Some(&json!("user")) {
                        if let Some(path) = item.get("installPath").and_then(Value::as_str) {
                            result.push((
                                id.rsplit_once('@').map(|v| v.0).unwrap_or(&id).into(),
                                Path::new(path).join("agents"),
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(result)
}
pub fn compute(
    env: &AccountEnvironment,
    account: &Path,
    repo: &Path,
    registry: &Value,
    memory: bool,
    phase: &str,
    sources: bool,
) -> Result<String, Error> {
    let settings = ValueGuard(files::object(&env.home.join(".claude/settings.json"))?);
    let config = match instructions_toml::read_typed(&account.join("config.toml")) {
        Ok(v) => v,
        Err(files::Error::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound
                && !account.join("config.toml").is_symlink() =>
        {
            instructions_toml::Typed {
                value: json!({}),
                dates: Default::default(),
            }
        }
        Err(files::Error::Io(e)) => return Err(io(e)),
        Err(e) => return Err(e.into()),
    };
    if let Some(kind) = config.dates.get(&vec!["projects".into()]) {
        return Err(error(
            "TypeError",
            format!("'datetime.{kind}' object is not iterable"),
        ));
    }
    let mut projects = keys(config.value.get("projects").unwrap_or(&json!({})))?;
    projects.sort();
    let desired =
        super::plugin_selection::desired(&settings).map_err(|e| error(e.kind, e.message))?;
    let claude = ValueGuard(files::object(&env.home.join(".claude.json"))?);
    let mut values = ValueGuard(match phase {
        "instructions" => json!({"projects":projects}),
        "plugins" => json!({"enabled":desired}),
        "fragments" => {
            let mut map = serde_json::Map::new();
            map.insert("hooks".into(), take(&settings, "hooks", json!({})));
            map.insert("env".into(), take(&settings, "env", json!({})));
            map.insert("mcp".into(), take(&claude, "mcpServers", json!({})));
            map.insert("memory".into(), json!(memory));
            map.insert("plugins".into(), json!(desired));
            Value::Object(map)
        }
        "skills" => json!({}),
        _ => return Err(error("ValueError", "Fase de integração desconhecida")),
    });
    let mut paths = BTreeSet::new();
    let mut instruction_sources = BTreeSet::from([
        env.home.join(".claude/CLAUDE.md"),
        env.home.join(".claude/CLAUDE.MD"),
    ]);
    for path in records(&account.join(".hangar-instrucoes"), cfg!(windows))? {
        let record = ValueGuard(files::object(&path)?);
        let source = record
            .get("fonte")
            .and_then(Value::as_str)
            .ok_or_else(|| error("ValueError", "Fonte de instruções inválida"))?;
        instruction_sources.insert(PathBuf::from(source));
        if !sources {
            let target = record
                .get("alvo")
                .and_then(Value::as_str)
                .ok_or_else(|| error("ValueError", "Alvo de instruções inválido"))?;
            if phase == "instructions" {
                paths.insert(path);
                paths.insert(PathBuf::from(target));
            }
        }
    }
    if phase == "instructions" {
        let mut pending: Vec<_> = instruction_sources.into_iter().collect();
        let mut visited = BTreeSet::new();
        while let Some(path) = pending.pop() {
            paths.insert(path.clone());
            if !path.is_file() || !visited.insert(resolved(&path)?) {
                continue;
            }
            let raw = std::fs::read(&path).map_err(io)?;
            let text = String::from_utf8(raw).map_err(|_| error("UnicodeDecodeError", "Arquivo de instruções não está em UTF-8"))?;
            for line in crate::transcript::py::splitlines(&text) {
                if let Some(imported) =
                    instructions::imported(line, path.parent().unwrap_or(Path::new(".")), env)
                {
                    if !visited.contains(&resolved(&imported)?) {
                        pending.push(imported);
                    }
                }
            }
        }
    }
    if phase == "plugins" {
        paths.extend([
            env.home.join(".claude/plugins/installed_plugins.json"),
            env.home.join(".claude/plugins/known_marketplaces.json"),
        ]);
    }
    let agents = plugin_agents(&env.home, &settings)?;
    let mut trees = match phase {
        "fragments" => BTreeSet::from([
            env.home.join(".claude/commands"),
            env.home.join(".claude/agents"),
            env.home.join(".claude/hooks"),
        ]),
        "skills" => BTreeSet::from([
            env.home.join(".claude/skills"),
            env.home.join(".agents/skills"),
            repo.join("skills"),
        ]),
        _ => BTreeSet::new(),
    };
    if phase == "fragments" {
        trees.extend(agents.into_iter().map(|(_, path)| path));
    }
    if phase == "skills" {
        for path in crate::harness_setup::skills::sources(env, repo)
            .map_err(io)?
            .into_values()
        {
            trees.insert(resolved(&path)?);
        }
        paths.insert(env.home.join(".claude/ecc-slim-keep.txt"));
    }
    if !sources {
        let registry = object(registry)?;
        let mut plugin_paths = Vec::new();
        if let Some(items) = registry.get("plugins") {
            let items = items.as_object().ok_or_else(|| {
                error(
                    "AttributeError",
                    format!("'{}' object has no attribute 'values'", type_name(items)),
                )
            })?;
            for item in items.values() {
                if let Some(path) = item.get("path").and_then(Value::as_str) {
                    plugin_paths.push(PathBuf::from(path));
                }
            }
        }
        if let Some(kind) = config.dates.get(&vec!["shell_environment_policy".into()]) {
            return Err(error(
                "AttributeError",
                format!("'datetime.{kind}' object has no attribute 'get'"),
            ));
        }
        let policy = ValueGuard(take(&config.value, "shell_environment_policy", json!({})));
        let policy = object(&policy)?;
        let limit = instructions::limit(account)?;
        let artifact_paths = registry
            .get("artefatos")
            .map(keys)
            .transpose()?
            .unwrap_or_default();
        let skill_paths = registry
            .get("skills")
            .map(keys)
            .transpose()?
            .unwrap_or_default();
        let map = values.0.as_object_mut().unwrap();
        match phase {
            "instructions" => {
                paths.insert(account.join("AGENTS.md"));
                map.insert(
                    "limit".into(),
                    take(&config.value, "project_doc_max_bytes", Value::Null),
                );
                map.insert(
                    "fallbacks".into(),
                    take(&config.value, "project_doc_fallback_filenames", Value::Null),
                );
            }
            "plugins" => {
                map.insert("plugins".into(), take(&config.value, "plugins", json!({})));
                map.insert(
                    "markets".into(),
                    take(&config.value, "marketplaces", json!({})),
                );
                paths.insert(account.join("plugins/installed_plugins.json"));
                paths.extend(plugin_paths);
            }
            "fragments" => {
                map.insert("agents".into(), take(&config.value, "agents", json!({})));
                map.insert(
                    "mcp_target".into(),
                    take(&config.value, "mcp_servers", json!({})),
                );
                map.insert(
                    "env_target".into(),
                    policy
                        .get("set")
                        .map(codec::clone_value)
                        .unwrap_or_else(|| json!({})),
                );
                map.insert(
                    "features".into(),
                    take(&config.value, "features", json!({})),
                );
                map.insert("limit".into(), limit);
                map.insert(
                    "fallbacks".into(),
                    take(&config.value, "project_doc_fallback_filenames", Value::Null),
                );
                paths.insert(account.join("hooks.json"));
                paths.insert(std::env::current_exe().map_err(io)?);
                paths.extend(artifact_paths.into_iter().map(PathBuf::from));
            }
            "skills" => {
                paths.extend(
                    skill_paths
                        .into_iter()
                        .map(PathBuf::from)
                        .filter(|path| path.is_absolute()),
                );
            }
            _ => unreachable!(),
        }
    }
    for root in trees {
        walk(root, &mut paths)?;
    }
    if !sources {
        let selected: Vec<&str> = match phase {
            "instructions" => vec!["project_doc_max_bytes", "project_doc_fallback_filenames"],
            "plugins" => vec!["plugins", "marketplaces"],
            "fragments" => vec![
                "agents",
                "mcp_servers",
                "features",
                "project_doc_fallback_filenames",
            ],
            _ => vec![],
        };
        for (path, kind) in &config.dates {
            if selected
                .iter()
                .any(|field| path.first().is_some_and(|p| p == field))
                || (phase == "fragments"
                    && path.starts_with(&["shell_environment_policy".into(), "set".into()]))
            {
                return Err(error(
                    "TypeError",
                    format!("Object of type {kind} is not JSON serializable"),
                ));
            }
        }
    }
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    digest.update(b"codex-phases-v1claude-only-hooks-v1");
    digest.update(&files::bytes(&values)?);
    for path in paths {
        let text = match std::fs::metadata(&path) {
            Ok(meta) => {
                let time = meta.modified().map_err(io)?;
                let nanos = match time.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_nanos().to_string(),
                    Err(e) => format!("-{}", e.duration().as_nanos()),
                };
                format!("{}:{}:{nanos}", path.display(), meta.len())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                format!("{}:ausente", path.display())
            }
            Err(e) => return Err(io(e)),
        };
        digest.update(text.as_bytes());
        // O executável usa metadata; fontes e artefatos precisam detectar edições do mesmo tamanho.
        if path.is_file() && path != std::env::current_exe().map_err(io)? {
            use std::io::Read;
            let mut file=std::fs::File::open(&path).map_err(io)?;let mut content=ring::digest::Context::new(&ring::digest::SHA256);let mut buffer=[0u8;8192];
            loop {let count=file.read(&mut buffer).map_err(io)?;if count==0{break;}content.update(&buffer[..count]);}
            let hash=content.finish().as_ref().iter().map(|byte|format!("{byte:02x}")).collect::<String>();
            digest.update(b":sha256:");digest.update(hash.as_bytes());
        }
    }
    if phase=="skills" {
        let enabled=super::plugin_selection::desired(&settings).map_err(|e|error(e.kind,e.message))?;
        let mut selection=json!({"enabled":enabled});
        if !sources {selection["plugins"]=take(&config.value,"plugins",json!({}));selection["marketplaces"]=take(&config.value,"marketplaces",json!({}));}
        digest.update(&files::bytes(&selection)?);
    }
    Ok(digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
#[path="fingerprints_review_tests.rs"]
mod review_tests;
