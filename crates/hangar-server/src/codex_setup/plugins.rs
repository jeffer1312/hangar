//! Adaptação dos arquivos de plugins mantidos no cache da conta.
use super::{ecc, files, normalize};
use crate::{
    accounts::{catalog, environment::AccountEnvironment},
    engines::probe::truthy,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    future::Future,
    path::{Path, PathBuf},
};
pub struct Context<'a> {
    pub account: &'a Path,
    pub backups: &'a Path,
    pub repo: &'a Path,
    pub python: &'a str,
    pub windows: bool,
}
pub trait Runtime {
    fn prepare(
        &self,
        root: &Path,
        backups: &Path,
    ) -> impl Future<Output = Result<bool, String>> + Send;
}
pub struct NativeRuntime(pub AccountEnvironment);
impl Runtime for NativeRuntime {
    async fn prepare(&self, root: &Path, backups: &Path) -> Result<bool, String> {
        ecc::prepare(root, backups, &ecc::NativeRunner(self.0.clone())).await
    }
}
fn resolved(path: &Path) -> Result<PathBuf, String> {
    resolved_depth(path, 0)
}
fn resolved_depth(path: &Path, depth: usize) -> Result<PathBuf, String> {
    if depth >= 40 {
        return Err("Muitos links simbólicos no caminho do plugin".into());
    }
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(catalog::resolved(&path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
            let mut result = PathBuf::new();
            for part in absolute.components() {
                match part {
                    std::path::Component::ParentDir => {
                        result.pop();
                    }
                    std::path::Component::CurDir => {}
                    part => {
                        result.push(part);
                        if result.is_symlink() {
                            let target =
                                std::fs::read_link(&result).map_err(|error| error.to_string())?;
                            let target = if target.is_absolute() {
                                target
                            } else {
                                result.parent().unwrap_or(Path::new(".")).join(target)
                            };
                            result = resolved_depth(&target, depth + 1)?;
                        } else if result.exists() {
                            result = catalog::resolved(&result);
                        }
                    }
                }
            }
            Ok(result)
        }
        Err(error) => Err(error.to_string()),
    }
}
pub async fn adapt(
    root: &Path,
    context: &Context<'_>,
    mut normalize_hook: impl FnMut(&Value) -> Result<Value, String>,
    mut changed: impl FnMut(),
    runtime: &impl Runtime,
) -> Result<(), String> {
    let cache = resolved(&context.account.join("plugins/cache"))?;
    let root = resolved(root)?;
    if !root.starts_with(&cache) {
        return Err("Plugin fora do cache do Codex".into());
    }
    let mut paths = BTreeSet::from([root.join("hooks/hooks.json"), root.join("hooks.json")]);
    let (mut security, mut ecc) = (false, false);
    for rel in [".codex-plugin/plugin.json", ".claude-plugin/plugin.json"] {
        let manifest = root.join(rel);
        if !manifest.is_file() {
            continue;
        }
        let data = files::object(&manifest).map_err(|error| error.to_string())?;
        security |= data.get("name").and_then(Value::as_str) == Some("security-guidance");
        ecc |= matches!(
            data.get("name").and_then(Value::as_str),
            Some("ecc" | "everything-claude-code")
        );
        let refs = data.get("hooks").unwrap_or(&Value::Null);
        if refs.is_object() {
            paths.insert(manifest);
            continue;
        }
        if let Some(reference) = refs.as_str() {
            paths.insert(root.join(reference));
        } else if truthy(refs) {
            let refs = refs
                .as_array()
                .ok_or("Referências de hooks inválidas no plugin")?;
            for reference in refs {
                paths.insert(
                    root.join(
                        reference
                            .as_str()
                            .ok_or("Referências de hooks inválidas no plugin")?,
                    ),
                );
            }
        }
    }
    let wrapper = context.account.join(".hangar-hooks/codex-hook-json.py");
    if security {
        let raw = std::fs::read(context.repo.join("scripts/codex-hook-json.py"))
            .map_err(|error| error.to_string())?;
        let prior = files::read(&wrapper).map_err(|error| error.to_string())?;
        files::write(&wrapper, &raw, prior.as_deref(), Some(context.backups))
            .map_err(|error| error.to_string())?;
    }
    for path in paths {
        if !path.is_file() {
            continue;
        }
        if !resolved(&path)?.starts_with(&root) {
            return Err("Hooks fora do plugin gerenciado".into());
        }
        let touched = files::transform(&path, context.backups, |raw| {
            let raw =
                raw.ok_or_else(|| files::Error::Invalid("Arquivo de hooks desapareceu".into()))?;
            let data: Value = serde_json::from_slice(raw)
                .map_err(|error| files::Error::Invalid(error.to_string()))?;
            let mut result = normalize_hook(&data).map_err(files::Error::Invalid)?;
            if security {
                result = normalize::security(
                    &result,
                    context.python,
                    &wrapper.to_string_lossy(),
                    context.windows,
                )
                .map_err(files::Error::Invalid)?;
            }
            Ok(if data == result {
                raw.to_vec()
            } else {
                files::bytes(&result)
            })
        })
        .map_err(|error| error.to_string())?;
        if touched {
            changed();
        }
    }
    if ecc && runtime.prepare(&root, context.backups).await? {
        changed();
    }
    Ok(())
}
