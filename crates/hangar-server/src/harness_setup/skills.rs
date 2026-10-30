//! Materializa as fontes conhecidas sem substituir arquivos ou links pessoais.
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
static LOCK: Mutex<()> = Mutex::new(());

pub struct RebuildReport {
    pub stats: Value,
    pub errors: BTreeMap<String, String>,
}

#[cfg(all(test, unix))]
#[path = "skills_report_tests.rs"]
mod report_tests;

fn children(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match fs::metadata(path) {
        Ok(meta) if !meta.is_dir() => return Ok(vec![]),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
        _ => {}
    }
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}
fn skills(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    Ok(children(path)?
        .into_iter()
        .filter(|p| p.join("SKILL.md").is_file())
        .collect())
}
fn version(path: &Path) -> (bool, Vec<u64>, String) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match name
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(parts) => (true, parts, String::new()),
        Err(_) => (false, vec![], name.into_owned()),
    }
}
fn roots(env: &AccountEnvironment, repo: &Path) -> Vec<PathBuf> {
    vec![
        env.home.join(".claude/skills"),
        repo.join("skills"),
        env.home.join(".claude/plugins/cache"),
        env.home.join(".claude/plugins/marketplaces"),
        env.home.join(".agents/skills"),
    ]
}
pub fn sources(
    env: &AccountEnvironment,
    repo: &Path,
) -> std::io::Result<BTreeMap<String, PathBuf>> {
    let mut result = BTreeMap::new();
    for (i, root) in roots(env, repo).iter().enumerate() {
        let mut found = Vec::new();
        match i {
            2 => {
                for plugin in children(root)? {
                    let mut versions = Vec::new();
                    for sub in children(&plugin)? {
                        versions.extend(
                            children(&sub)?
                                .into_iter()
                                .filter(|v| v.join("skills").is_dir()),
                        );
                    }
                    if let Some(newest) =
                        versions.into_iter().max_by_key(|p| (version(p), p.clone()))
                    {
                        found.extend(skills(&newest.join("skills"))?);
                    }
                }
            }
            3 => {
                for market in children(root)? {
                    found.extend(skills(&market.join("skills"))?);
                }
            }
            _ => found = skills(root)?,
        }
        for source in found {
            if let Some(name) = source.file_name() {
                result
                    .entry(name.to_string_lossy().into_owned())
                    .or_insert(source);
            }
        }
    }
    Ok(result)
}
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}
fn managed(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(link) = fs::read_link(path) else {
        return false;
    };
    let link = normalized(&if link.is_absolute() {
        link
    } else {
        path.parent().unwrap().join(link)
    });
    roots.iter().any(|root| link.starts_with(normalized(root)))
}
fn link(path: &Path, source: &Path, roots: &[PathBuf]) -> std::io::Result<()> {
    let directory = tempfile::Builder::new()
        .prefix(".hangar-skill-")
        .tempdir_in(path.parent().unwrap())?;
    let temporary = directory.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(source, &temporary)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(source, &temporary)?;
    if fs::symlink_metadata(path).is_ok() && !managed(path, roots) {
        let _ = fs::remove_file(&temporary);
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "entrada pessoal preservada",
        ));
    }
    let result = fs::rename(&temporary, path);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn rebuild_one(
    bridge: &Path,
    sources: &BTreeMap<String, PathBuf>,
    roots: &[PathBuf],
    dry: bool,
) -> std::io::Result<Value> {
    let mut result = json!({"criados":0,"trocados":0,"removidos":0,"mantidos":0,"pulados_reais":0});
    let mut existing: BTreeMap<String, PathBuf> = children(bridge)?
        .into_iter()
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    for (name, path) in existing.clone() {
        if !managed(&path, roots) {
            if sources.contains_key(&name) {
                result["pulados_reais"] = json!(result["pulados_reais"].as_u64().unwrap() + 1);
            }
            continue;
        }
        if !sources.contains_key(&name) || !path.exists() {
            result["removidos"] = json!(result["removidos"].as_u64().unwrap() + 1);
            if !dry {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
            existing.remove(&name);
        }
    }
    if !dry {
        fs::create_dir_all(bridge)?;
    }
    for (name, source) in sources {
        let path = bridge.join(name);
        if existing.get(name).is_some_and(|p| !managed(p, roots)) {
            continue;
        }
        let same = path.is_symlink()
            && fs::canonicalize(&path)
                .ok()
                .zip(fs::canonicalize(source).ok())
                .is_some_and(|(a, b)| a == b);
        let field = if same {
            "mantidos"
        } else if path.is_symlink() {
            "trocados"
        } else if fs::symlink_metadata(&path).is_err() {
            "criados"
        } else {
            "pulados_reais"
        };
        result[field] = json!(result[field].as_u64().unwrap() + 1);
        if !dry && ["criados", "trocados"].contains(&field) {
            link(&path, source, roots)?;
        }
    }
    Ok(result)
}
pub fn rebuild(env: &AccountEnvironment, repo: &Path, dry: bool) -> Value {
    rebuild_report(env, repo, dry).stats
}

pub fn rebuild_report(env: &AccountEnvironment, repo: &Path, dry: bool) -> RebuildReport {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sources = match sources(env, repo) {
        Ok(sources) => sources,
        Err(error) => {
            tracing::warn!(%error,"Varredura de fontes incompleta; espelhos preservados");
            return RebuildReport {
                stats: json!({"fontes":{"erro":1}}),
                errors: BTreeMap::from([("fontes".into(),format!("{}: {error}",env.home.display()))]),
            };
        }
    };
    if sources.is_empty() {
        tracing::warn!("Nenhuma fonte de skills encontrada; espelhos preservados");
        return RebuildReport { stats: json!({}), errors: BTreeMap::new() };
    }
    let roots = roots(env, repo);
    let mut result = json!({});
    let mut errors = BTreeMap::new();
    for (target, relative) in [("pi", ".pi/agent"), ("kimi", ".kimi-code")] {
        let parent = env.home.join(relative);
        if !parent.is_dir() {
            continue;
        }
        result[target] = rebuild_one(&parent.join("skills-bridge"), &sources, &roots, dry)
            .unwrap_or_else(|error| {
                tracing::warn!(target,%error,"Falha ao atualizar skills");
                errors.insert(target.into(),format!("{}: {error}",parent.join("skills-bridge").display()));
                json!({"erro":1})
            });
        if result[target].get("erro").is_none() {
            match bridge_configured(env, target) {
                Ok(Some(false)) => {
                    let verb = if dry { "seria montada" } else { "foi montada" };
                    tracing::warn!(
                        "{} não está na config do {target} — a ponte {verb} mas o {target} não vai ler. Adicione o caminho à lista de skills dele.",
                        parent.join("skills-bridge").display()
                    );
                }
                Err(error) => {
                    tracing::warn!(target,%error,"Configuração ilegível; não deu para conferir a lista de skills")
                }
                _ => {}
            }
        }
        tracing::info!(target,stats=%result[target],"Skills conferidas");
    }
    RebuildReport { stats: result, errors }
}

pub fn bridge_configured(env: &AccountEnvironment, target: &str) -> Result<Option<bool>, String> {
    let (relative, file, key) = match target {
        "pi" => (".pi/agent", "settings.json", "skills"),
        "kimi" => (".kimi-code", "config.toml", "extra_skill_dirs"),
        _ => return Err("harness desconhecido".into()),
    };
    let root = env.home.join(relative);
    let raw = match fs::read_to_string(root.join(file)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let data: Value = if target == "pi" {
        serde_json::from_str(&raw).map_err(|e| e.to_string())?
    } else {
        toml::from_str(&raw).map_err(|e| e.to_string())?
    };
    let bridge = normalized(&root.join("skills-bridge"));
    Ok(Some(
        data[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|value| {
                let value = if value == "~" {
                    env.home.to_string_lossy().into_owned()
                } else if let Some(tail) = value
                    .strip_prefix("~/")
                    .or_else(|| value.strip_prefix("~\\"))
                {
                    env.home.join(tail).to_string_lossy().into_owned()
                } else {
                    value.to_owned()
                };
                let re = regex::Regex::new(r"\$\{([^}]+)\}|\$([A-Za-z0-9_]+)").unwrap();
                let expanded = re.replace_all(&value, |captures: &regex::Captures| {
                    let key = captures
                        .get(1)
                        .or_else(|| captures.get(2))
                        .unwrap()
                        .as_str();
                    env.base
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| captures[0].to_owned())
                });
                let expanded = if cfg!(windows) {
                    let percent = regex::Regex::new(r"%%|%([^%]+)%").unwrap();
                    percent
                        .replace_all(&expanded, |captures: &regex::Captures| {
                            captures.get(1).map_or_else(
                                || "%".to_owned(),
                                |key| {
                                    env.base
                                        .get(key.as_str())
                                        .cloned()
                                        .unwrap_or_else(|| captures[0].to_owned())
                                },
                            )
                        })
                        .into_owned()
                } else {
                    expanded.into_owned()
                };
                normalized(Path::new(&expanded)) == bridge
            }),
    ))
}
