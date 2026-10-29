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

fn children(path: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    entries
}
fn skills(path: &Path) -> Vec<PathBuf> {
    children(path)
        .into_iter()
        .filter(|p| p.join("SKILL.md").is_file())
        .collect()
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
pub fn sources(env: &AccountEnvironment, repo: &Path) -> BTreeMap<String, PathBuf> {
    let mut result = BTreeMap::new();
    for (i, root) in roots(env, repo).iter().enumerate() {
        let found = match i {
            2 => children(root)
                .into_iter()
                .flat_map(|plugin| {
                    let newest = children(&plugin)
                        .into_iter()
                        .flat_map(|sub| children(&sub))
                        .filter(|v| v.join("skills").is_dir())
                        .max_by_key(|p| (version(p), p.clone()));
                    newest
                        .map(|p| skills(&p.join("skills")))
                        .unwrap_or_default()
                })
                .collect(),
            3 => children(root)
                .into_iter()
                .flat_map(|p| skills(&p.join("skills")))
                .collect(),
            _ => skills(root),
        };
        for source in found {
            if let Some(name) = source.file_name() {
                result
                    .entry(name.to_string_lossy().into_owned())
                    .or_insert(source);
            }
        }
    }
    result
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
    let mut existing: BTreeMap<String, PathBuf> = children(bridge)
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
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sources = sources(env, repo);
    if sources.is_empty() {
        tracing::warn!("Nenhuma fonte de skills encontrada; espelhos preservados");
        return json!({});
    }
    let roots = roots(env, repo);
    let mut result = json!({});
    for (target, relative) in [("pi", ".pi/agent"), ("kimi", ".kimi-code")] {
        let parent = env.home.join(relative);
        if !parent.is_dir() {
            continue;
        }
        result[target] = rebuild_one(&parent.join("skills-bridge"), &sources, &roots, dry)
            .unwrap_or_else(|error| {
                tracing::warn!(target,%error,"Falha ao atualizar skills");
                json!({"erro":1})
            });
        tracing::info!(target,stats=%result[target],"Skills conferidas");
    }
    result
}
