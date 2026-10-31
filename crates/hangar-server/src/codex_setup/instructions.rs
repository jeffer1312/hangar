//! Aliases de instruções, com autoria durável e preservação de alterações externas.
use super::files::Error;
use crate::config_sync::runtime::RuntimeConfig;
use std::path::Path;
pub use super::instructions_imports::imported;

pub async fn prepare(
    config: &RuntimeConfig,
    codex_home: &Path,
    cwd: Option<&Path>,
    force: bool,
) -> Result<(), Error> {
    if !force && !super::sync_enabled(config) {
        return Ok(());
    }
    let _lock = lock(codex_home).await?;
    let source = super::instructions_alias::source(&config.env.home.join(".claude"))?;
    super::instructions_alias::apply(
        &codex_home.join("AGENTS.override.md"),
        source.as_deref(),
        &codex_home.join(".hangar-instrucoes"),
        &config.env,
    )?;
    super::instructions_projects::prepare(&config.env, codex_home, cwd).await
}

pub async fn prepare_project(
    config: &RuntimeConfig,
    codex_home: &Path,
    cwd: &Path,
    force: bool,
) -> Result<(), Error> {
    if !force && !super::sync_enabled(config) {
        return Ok(());
    }
    let _lock = lock(codex_home).await?;
    super::instructions_projects::prepare(&config.env, codex_home, Some(cwd)).await
}

pub fn limit(codex_home: &Path) -> Result<serde_json::Value, Error> {
    use crate::config_sync::runtime_values::ValueGuard;
    let mut total = 0u128;
    for path in super::instructions_projects::records(&codex_home.join(".hangar-instrucoes"))? {
        let record = ValueGuard(super::files::object(&path)?);
        let source = record["fonte"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Fonte de instruções inválida".into()))?;
        let source = Path::new(source);
        if source.is_file() {
            total += u128::from(std::fs::metadata(source)?.len()) + 1024;
        }
    }
    let path = codex_home.join("config.toml");
    let config = ValueGuard(if path.exists() {
        super::instructions_toml::read(&path)?
    } else {
        serde_json::json!({})
    });
    let default = serde_json::json!(32768);
    let current = config.get("project_doc_max_bytes").unwrap_or(&default);
    let number = current
        .as_number()
        .map(ToString::to_string)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| Error::Invalid("project_doc_max_bytes inválido".into()))?;
    let total = total.max(1048576).to_string();
    let maximum = if (number.len(), number.as_str()) > (total.len(), total.as_str()) {
        number
    } else {
        total
    };
    serde_json::from_str(&maximum).map_err(|error| Error::Invalid(error.to_string()))
}

async fn lock(codex_home: &Path) -> Result<std::fs::File, Error> {
    super::instructions_lock::acquire(codex_home, tokio::time::sleep)
        .await
        .map_err(Into::into)
}
