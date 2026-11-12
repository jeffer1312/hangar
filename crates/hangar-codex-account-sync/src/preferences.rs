//! Projeção das preferências compartilháveis, sem a identidade da conta principal.
use serde_json::{Value, json};

const PREFERENCES: &[&str] = &[
    "model",
    "model_reasoning_effort",
    "model_reasoning_summary",
    "model_verbosity",
    "service_tier",
    "model_context_window",
    "model_auto_compact_token_limit",
    "model_catalog_json",
    "personality",
    "features",
    "tui",
    "desktop",
    "project_doc_fallback_filenames",
    "project_doc_max_bytes",
    "model_instructions_file",
    "agents",
    "mcp_servers",
    "shell_environment_policy",
    "model_providers",
    "approval_policy",
    "approvals_reviewer",
    "sandbox_mode",
    "sandbox_workspace_write",
    "permissions",
    "auto_review",
    "web_search",
    "notify",
    "otel",
    "analytics",
];
const IGNORED: &[&str] = &[
    "cli_auth_credentials_store",
    "projects",
    "notice",
    "history",
    "sqlite_home",
    "auth",
    "auth_file",
    "credentials",
    "forced_login_method",
    "forced_chatgpt_workspace_id",
    "marketplaces",
    "plugins",
];
const PROVIDER_ENV: &[&str] = &[
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
    "OPENAI_ORGANIZATION",
    "OPENAI_ORG_ID",
    "OPENAI_PROJECT",
    "OPENAI_PROJECT_ID",
    "OPENAI_ENDPOINT",
];
const RUNTIME_ENV: &[&str] = &[
    "CODEX_HOME",
    "CODEX_CONFIG_HOME",
    "CODEX_SQLITE_HOME",
    "HOME",
    "USERPROFILE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
];
fn auth_name(name: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(||regex::Regex::new(r"(?i)^(?:OPENAI|CODEX|CHATGPT)_.*(?:(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)(?:_FILE)?|ACCOUNT_ID|ORG_ID|PROJECT_ID)$").expect("padrão constante")).is_match(name)
}
fn excluded(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    auth_name(&upper)
        || PROVIDER_ENV.contains(&upper.as_str())
        || RUNTIME_ENV.contains(&upper.as_str())
}
fn issue(code: &str, params: Value) -> Value {
    json!({"code":code,"params":params})
}

/// Seleciona preferências herdáveis; identidade e estado privado nunca entram na projeção.
///
/// # Errors
/// Retorna erro quando a fonte não é um objeto TOML.
pub fn project_preferences(source: &Value) -> Result<(Value, Vec<Value>), String> {
    let entries = source
        .as_object()
        .ok_or("config.toml da conta padrão não é um objeto")?;
    let mut out = serde_json::Map::new();
    let mut issues = Vec::new();
    for (key, value) in entries {
        if IGNORED.contains(&key.as_str()) {
            continue;
        }
        if key == "model_provider" {
            issues.push(issue(
                "codex_account_provider_divergence",
                json!({"key":key}),
            ));
            continue;
        }
        if key == "hooks" {
            if let Some(values) = value.as_object() {
                let mut definitions = values.clone();
                definitions.remove("state");
                if !definitions.is_empty() {
                    out.insert(key.clone(), Value::Object(definitions));
                }
            } else {
                issues.push(issue(
                    "codex_account_preference_invalid",
                    json!({"key":key}),
                ));
            }
            continue;
        }
        if !PREFERENCES.contains(&key.as_str()) {
            issues.push(issue(
                "codex_account_unknown_preference",
                json!({"key":key}),
            ));
            continue;
        }
        let mut projected = value.clone();
        if key == "shell_environment_policy" {
            if let Some(variables) = projected.get_mut("set").and_then(Value::as_object_mut) {
                for variable in variables.keys() {
                    if PROVIDER_ENV.contains(&variable.as_str()) {
                        issues.push(issue(
                            "codex_account_provider_divergence",
                            json!({"key":variable}),
                        ));
                    }
                }
                variables.retain(|name, _| {
                    !auth_name(name)
                        && !PROVIDER_ENV.contains(&name.as_str())
                        && !RUNTIME_ENV.contains(&name.as_str())
                });
            }
        } else if key == "mcp_servers" {
            if let Some(servers) = projected.as_object_mut() {
                for (server, config) in servers {
                    if let Some(env) = config.get_mut("env").and_then(Value::as_object_mut) {
                        env.retain(|name, _| {
                            if excluded(name) {
                                issues.push(issue(
                                    if RUNTIME_ENV.contains(&name.as_str()) {
                                        "codex_account_mcp_runtime_excluded"
                                    } else {
                                        "codex_account_mcp_auth_excluded"
                                    },
                                    json!({"server":server,"variable":name}),
                                ));
                                false
                            } else {
                                true
                            }
                        });
                    }
                }
            }
        } else if key == "model_providers"
            && let Some(providers) = projected.as_object_mut()
        {
            for (provider, config) in providers {
                if let Some(fields) = config.as_object_mut() {
                    fields.retain(|name, _| {
                        if [
                            "api_key",
                            "access_token",
                            "refresh_token",
                            "password",
                            "secret",
                        ]
                        .contains(&name.to_lowercase().as_str())
                        {
                            issues.push(issue(
                                "codex_account_provider_auth_excluded",
                                json!({"provider":provider,"key":name}),
                            ));
                            false
                        } else {
                            true
                        }
                    });
                }
            }
        }
        out.insert(key.clone(), projected);
    }
    out.insert("cli_auth_credentials_store".into(), json!("file"));
    Ok((Value::Object(out), issues))
}
