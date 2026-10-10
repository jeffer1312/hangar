//! Identidade e autenticação vêm do destino; nunca da sessão que ficou na tela.
use super::model::OrganizationRequest;
use crate::{accounts::environment::AccountEnvironment, routes::AppState};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Context {
    pub provider: String,
    pub home: PathBuf,
    pub generation: Option<String>,
    pub session: Option<String>,
    pub env: BTreeMap<String, String>,
    pub settings: Value,
    pub engine: Option<Value>,
    pub jsonl: Option<PathBuf>,
}

fn read_json(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .filter(|bytes| bytes.len() <= 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

pub fn account(
    provider: &str,
    home: &Path,
    environment: &AccountEnvironment,
) -> Result<Context, &'static str> {
    if !matches!(provider, "claude" | "codex") {
        return Err("dictation_harness_unsupported");
    }
    let home = home
        .canonicalize()
        .map_err(|_| "dictation_target_missing")?;
    let mut env: BTreeMap<_, _> = environment
        .base
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.to_ascii_uppercase().as_str(),
                "PATH"
                    | "SYSTEMROOT"
                    | "WINDIR"
                    | "TEMP"
                    | "TMP"
                    | "LANG"
                    | "LC_ALL"
                    | "SSL_CERT_FILE"
                    | "SSL_CERT_DIR"
                    | "NODE_EXTRA_CA_CERTS"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // A conta lê a autenticação no caminho declarado; o auxiliar não descobre o projeto.
    env.insert("HOME".into(), environment.home.to_string_lossy().into());
    env.insert(
        "USERPROFILE".into(),
        environment.home.to_string_lossy().into(),
    );
    env.insert(
        if provider == "codex" {
            "CODEX_HOME"
        } else {
            "CLAUDE_CONFIG_DIR"
        }
        .into(),
        home.to_string_lossy().into(),
    );
    let mut settings = json!({});
    if provider == "claude" {
        let stored = read_json(&home.join("settings.json"));
        for key in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_CUSTOM_HEADERS",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ] {
            let value = stored["env"][key]
                .as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    (home
                        == environment
                            .claude_base
                            .canonicalize()
                            .unwrap_or_else(|_| environment.claude_base.clone()))
                    .then(|| environment.base.get(key).cloned())
                    .flatten()
                });
            if let Some(value) = value {
                env.insert(key.into(), value);
            }
        }
        if let Some(helper) = stored["apiKeyHelper"].as_str().filter(|h| !h.is_empty()) {
            settings["apiKeyHelper"] = json!(helper);
        }
    }
    Ok(Context {
        provider: provider.into(),
        home,
        generation: None,
        session: None,
        env,
        settings,
        engine: None,
        jsonl: None,
    })
}

pub async fn resolve(
    state: &AppState,
    request: &OrganizationRequest,
) -> Result<Context, &'static str> {
    let name = request
        .session
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("dictation_target_missing")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.);
    let rows = state
        .list
        .discover(Some(now))
        .await
        .map_err(|_| "dictation_target_missing")?;
    let row = rows
        .iter()
        .find(|row| row.name == name)
        .ok_or("dictation_target_missing")?;
    let generation = row.lifecycle_id.as_ref().or(row.jsonl.as_ref()).cloned();
    if request.generation.is_some() && request.generation != generation {
        return Err("dictation_target_changed");
    }
    if request.account.is_some() && request.account != row.conta {
        return Err("dictation_target_changed");
    }
    let home = if row.provider == "codex" {
        row.codex_home
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| state.accounts.env.codex_default.clone())
    } else {
        crate::list::context::config_dir_of(row.conta.as_deref())
            .unwrap_or_else(|| state.accounts.env.claude_base.clone())
    };
    let mut context = account(&row.provider, &home, &state.accounts.env)?;
    context.generation = generation;
    context.session = Some(name.into());
    context.jsonl = row.jsonl.as_ref().map(PathBuf::from);
    if let Some(engine) = &row.engine {
        let source = state
            .accounts
            .env
            .base
            .get("CP_ENGINES_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| state.accounts.env.home.join(".claude/engines.json"));
        let engines = read_json(&source);
        let data = engines
            .get(engine)
            .filter(|e| e.is_object())
            .ok_or("dictation_harness_unsupported")?;
        let key = data["api_key"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("dictation_organization_failed")?;
        let url = data["base_url"]
            .as_str()
            .filter(|s| s.starts_with("https://") || s.starts_with("http://"))
            .ok_or("dictation_organization_failed")?;
        context.env.insert("ANTHROPIC_API_KEY".into(), key.into());
        context.env.insert("ANTHROPIC_BASE_URL".into(), url.into());
        let mut data = data.clone();
        if row.engine_account.is_some() {
            let home = row
                .conta
                .as_deref()
                .and_then(|id| id.strip_prefix("codex:"))
                .map(PathBuf::from)
                .ok_or("dictation_harness_unsupported")?;
            let auth = read_json(&home.join("auth.json"));
            let id = auth["tokens"]["account_id"]
                .as_str()
                .ok_or("dictation_organization_failed")?;
            let mut matches = Vec::new();
            for file in std::fs::read_dir(state.accounts.env.home.join(".cli-proxy-api"))
                .map_err(|_| "dictation_organization_failed")?
            {
                let file = file.map_err(|_| "dictation_organization_failed")?;
                if file.path().extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                if file
                    .file_type()
                    .map_err(|_| "dictation_organization_failed")?
                    .is_symlink()
                {
                    return Err("dictation_organization_failed");
                }
                let credential = read_json(&file.path());
                if credential["type"] == "codex"
                    && credential["account_id"].as_str() == Some(id)
                    && credential["disabled"].as_bool() != Some(true)
                {
                    matches.push(credential);
                }
            }
            if matches.len() != 1 {
                return Err("dictation_organization_failed");
            }
            let prefix = matches[0]["prefix"]
                .as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
                })
                .ok_or("dictation_organization_failed")?;
            data["dictation_prefix"] = json!(prefix);
        }
        context.engine = Some(data);
    }
    Ok(context)
}
