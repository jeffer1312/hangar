//! Catálogo real por conta; o formato público preserva capacidades de esforço e serviço.
use super::{context, process::Process};
use crate::accounts::catalog::AccountService;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Cached {
    signature: Vec<Option<Vec<u8>>>,
    at: Instant,
    models: Vec<Value>,
    raw: Vec<Value>,
    version: String,
}
#[derive(Default)]
pub struct Models {
    cache: Mutex<BTreeMap<(String, PathBuf), Cached>>,
    gate: tokio::sync::Mutex<()>,
}

fn signature(home: &Path) -> Vec<Option<Vec<u8>>> {
    [
        "auth.json",
        "config.toml",
        "settings.json",
        ".credentials.json",
    ]
    .iter()
    .map(|file| {
        std::fs::read(home.join(file)).ok().map(|bytes| {
            ring::digest::digest(&ring::digest::SHA256, &bytes)
                .as_ref()
                .to_vec()
        })
    })
    .collect()
}

pub fn codex_overrides(command: &mut tokio::process::Command) {
    let config: BTreeMap<String, Value> = serde_json::from_str(include_str!(
        "../../../../resources/dictation-codex-config.json"
    ))
    .expect("configuração mínima válida");
    for (key, value) in config {
        command.arg("-c").arg(format!("{key}={value}"));
    }
}

pub fn codex_configuration(home: &Path) -> Result<toml::Value, &'static str> {
    let mut config = match std::fs::read_to_string(home.join("config.toml")) {
        Ok(value) => {
            toml::from_str::<toml::Value>(&value).map_err(|_| "dictation_harness_unsupported")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(Default::default())
        }
        Err(_) => return Err("dictation_harness_unsupported"),
    };
    if let Some(profile) = config.get("profile").and_then(toml::Value::as_str) {
        let profile = config
            .get("profiles")
            .and_then(|p| p.get(profile))
            .and_then(toml::Value::as_table)
            .cloned()
            .ok_or("dictation_harness_unsupported")?;
        for key in [
            "model_provider",
            "cli_auth_credentials_store",
            "chatgpt_base_url",
        ] {
            if let Some(value) = profile.get(key) {
                config
                    .as_table_mut()
                    .ok_or("dictation_harness_unsupported")?
                    .insert(key.into(), value.clone());
            }
        }
    }
    Ok(config)
}

pub fn codex_auth_override(
    command: &mut tokio::process::Command,
    service: &AccountService,
    context: &context::Context,
    config: &toml::Value,
) -> Result<(), &'static str> {
    let default = service
        .env
        .codex_default
        .canonicalize()
        .unwrap_or_else(|_| service.env.codex_default.clone());
    let store = if context.home != default {
        "file"
    } else {
        config
            .get("cli_auth_credentials_store")
            .and_then(toml::Value::as_str)
            .unwrap_or("auto")
    };
    if !matches!(store, "file" | "keyring" | "auto" | "ephemeral") {
        return Err("dictation_harness_unsupported");
    }
    command
        .arg("-c")
        .arg(format!("cli_auth_credentials_store={}", json!(store)));
    Ok(())
}

impl Models {
    pub fn invalidate(&self, home: Option<&Path>) {
        let mut cache = self.cache.lock().unwrap();
        if let Some(home) = home {
            let home = home.canonicalize().unwrap_or_else(|_| home.into());
            cache.retain(|(_, path), _| path != &home);
        } else {
            cache.clear();
        }
    }
    pub async fn get(
        &self,
        service: &AccountService,
        provider: &str,
        home: &Path,
        fresh: bool,
    ) -> Result<(Vec<Value>, Vec<Value>), &'static str> {
        let context = context::account(provider, home, &service.env)?;
        self.get_context(service, &context, fresh).await
    }
    pub async fn get_context(
        &self,
        service: &AccountService,
        context: &context::Context,
        fresh: bool,
    ) -> Result<(Vec<Value>, Vec<Value>), &'static str> {
        let key = crate::accounts::AccountKey::new(
            if context.provider == "claude" {
                crate::accounts::Provider::Claude
            } else {
                crate::accounts::Provider::Codex
            },
            &context.home,
        )
        .map_err(|_| "dictation_target_missing")?;
        let account_guard = Arc::new(
            service
                .locks
                .try_acquire(&key, crate::accounts::GuardMode::Shared)
                .map_err(|_| "dictation_account_busy")?,
        );
        let provider = context.provider.as_str();
        let key = (provider.to_owned(), context.home.clone());
        let mut sig = signature(&context.home);
        let account_signature = sig.clone();
        if let Some(engine) = &context.engine {
            sig.push(Some(
                ring::digest::digest(&ring::digest::SHA256, engine.to_string().as_bytes())
                    .as_ref()
                    .to_vec(),
            ));
        }
        let _gate = self.gate.lock().await;
        let cached = self
            .cache
            .lock()
            .unwrap()
            .get(&key)
            .filter(|row| row.signature == sig)
            .cloned();
        if !fresh
            && let Some(row) = cached
                .as_ref()
                .filter(|row| row.at.elapsed() < Duration::from_secs(600))
        {
            return Ok((row.models.clone(), row.raw.clone()));
        }
        let (models, raw, version) = if provider == "claude" {
            let raw = if context.engine.is_some() {
                engine_models(context).await?
            } else {
                claude(service, context, account_guard.clone()).await?
            };
            (
                parse("claude", &json!({"models":raw}), None)
                    .map_err(|_| "dictation_catalog_unavailable")?,
                raw,
                String::new(),
            )
        } else {
            let version = version(service, context, account_guard.clone()).await?;
            match codex_http(&context.home, &version).await {
                Ok(Some(raw)) => {
                    let data = codex_http_models(&raw)?;
                    (
                        parse("codex", &json!({"data":data}), None)
                            .map_err(|_| "dictation_catalog_unavailable")?,
                        raw,
                        version,
                    )
                }
                Err("dictation_catalog_rate_limited") => {
                    if let Some(row) = cached {
                        self.cache.lock().unwrap().get_mut(&key).unwrap().at = Instant::now();
                        return Ok((row.models, row.raw));
                    }
                    return Err("dictation_catalog_rate_limited");
                }
                _ => {
                    let data = codex(service, context, account_guard.clone()).await?;
                    (
                        parse("codex", &data, None).map_err(|_| "dictation_catalog_unavailable")?,
                        Vec::new(),
                        version,
                    )
                }
            }
        };
        let models = if provider == "codex" && !fast_enabled(&context.home) {
            models
                .into_iter()
                .map(|mut model| {
                    if let Some(services) = model["service_tiers"].as_array_mut() {
                        services.retain(|s| s["id"] != "priority");
                    }
                    model
                })
                .collect()
        } else {
            models
        };
        if signature(&context.home) != account_signature {
            return Err("dictation_target_changed");
        }
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= 64 {
            let oldest = cache
                .iter()
                .min_by_key(|(_, value)| value.at)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            key,
            Cached {
                signature: sig,
                at: Instant::now(),
                models: models.clone(),
                raw: raw.clone(),
                version,
            },
        );
        Ok((models, raw))
    }
    pub async fn raw_model(
        &self,
        service: &AccountService,
        home: &Path,
        config: &Value,
        slug: &str,
        version: &str,
    ) -> Result<Value, &'static str> {
        let raw = if let Some(path) = config["model_catalog_json"].as_str() {
            let path = Path::new(path);
            if !path.is_absolute() {
                return Err("session_transfer_model_capacity_unknown");
            }
            let value: Value = serde_json::from_slice(
                &std::fs::read(path).map_err(|_| "session_transfer_model_capacity_unknown")?,
            )
            .map_err(|_| "session_transfer_model_capacity_unknown")?;
            value["models"]
                .as_array()
                .cloned()
                .ok_or("session_transfer_model_capacity_unknown")?
        } else {
            if config["model_provider"]
                .as_str()
                .is_some_and(|p| p != "openai")
                || !config["chatgpt_base_url"].is_null()
                || !config["model_providers"]["openai"].is_null()
            {
                return Err("session_transfer_model_capacity_unknown");
            }
            let home = home
                .canonicalize()
                .map_err(|_| "session_transfer_model_capacity_unknown")?;
            let sig = signature(&home);
            let cached = self
                .cache
                .lock()
                .unwrap()
                .get(&("codex".into(), home.clone()))
                .filter(|row| {
                    row.signature == sig
                        && row.version == version
                        && row.at.elapsed() < Duration::from_secs(600)
                        && !row.raw.is_empty()
                })
                .cloned();
            if let Some(row) = cached {
                row.raw
            } else {
                codex_http(&home, version)
                    .await?
                    .ok_or("session_transfer_model_capacity_unknown")?
            }
        };
        let mut selected = raw.into_iter().filter(|m| m["slug"].as_str() == Some(slug));
        let model = selected
            .next()
            .ok_or("session_transfer_model_capacity_unknown")?;
        if selected.next().is_some()
            || model["used_fallback_model_metadata"].as_bool() == Some(true)
        {
            return Err("session_transfer_model_capacity_unknown");
        }
        let _ = service;
        Ok(model)
    }
}

async fn engine_models(context: &context::Context) -> Result<Vec<Value>, &'static str> {
    let url = context
        .env
        .get("ANTHROPIC_BASE_URL")
        .ok_or("dictation_catalog_unavailable")?
        .trim_end_matches('/');
    let endpoint = if url.ends_with("/v1") {
        format!("{url}/models")
    } else {
        format!("{url}/v1/models")
    };
    let key = context
        .env
        .get("ANTHROPIC_API_KEY")
        .ok_or("dictation_catalog_unavailable")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "dictation_catalog_unavailable")?;
    let response = client
        .get(endpoint)
        .bearer_auth(key)
        .header("x-api-key", key)
        .send()
        .await
        .map_err(|_| "dictation_catalog_unavailable")?;
    if !response.status().is_success() {
        return Err("dictation_catalog_unavailable");
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "dictation_catalog_unavailable")?;
    if bytes.len() > 1024 * 1024 {
        return Err("dictation_catalog_unavailable");
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "dictation_catalog_unavailable")?;
    let prefix = context
        .engine
        .as_ref()
        .and_then(|e| e["dictation_prefix"].as_str())
        .map(|p| format!("{p}/"));
    let models=value["data"].as_array().ok_or("dictation_catalog_unavailable")?.iter().filter_map(|m|m["id"].as_str()
        .filter(|id|prefix.as_ref().is_none_or(|prefix|id.starts_with(prefix)))
        .map(|id|json!({"value":id,"displayName":m["name"].as_str().unwrap_or(id),"description":m["description"]}))).collect::<Vec<_>>();
    if models.is_empty() {
        Err("dictation_catalog_unavailable")
    } else {
        Ok(models)
    }
}

async fn version(
    service: &AccountService,
    context: &context::Context,
    guard: Arc<crate::accounts::AccountGuard>,
) -> Result<String, &'static str> {
    let mut command = service.codex_command().ok_or("dictation_cli_missing")?;
    command.arg("--version").env_clear().envs(&context.env);
    let output = super::process::limited_output_guarded(
        command,
        tempfile::tempdir().map_err(|_| "dictation_catalog_unavailable")?,
        "",
        Duration::from_secs(10),
        Some(guard),
    )
    .await?;
    let text = std::str::from_utf8(&output).map_err(|_| "dictation_catalog_unavailable")?;
    text.split_whitespace()
        .last()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or("dictation_catalog_unavailable")
}

async fn claude(
    service: &AccountService,
    context: &context::Context,
    guard: Arc<crate::accounts::AccountGuard>,
) -> Result<Vec<Value>, &'static str> {
    let mut command = service.claude_command().ok_or("dictation_cli_missing")?;
    command
        .args([
            "-p",
            "--safe-mode",
            "--no-session-persistence",
            "--tools",
            "",
            "--disable-slash-commands",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .env_clear()
        .envs(&context.env);
    let directory = tempfile::tempdir().map_err(|_| "dictation_catalog_unavailable")?;
    if !context.settings.as_object().is_none_or(|s| s.is_empty()) {
        let file = directory.path().join("auth-settings.json");
        std::fs::write(&file, context.settings.to_string())
            .map_err(|_| "dictation_catalog_unavailable")?;
        command.arg("--settings").arg(file);
    }
    let mut process = Process::open(command, directory)?;
    process.protect_account(guard);
    let outcome=tokio::time::timeout(Duration::from_secs(30),async{
        process.write(&format!("{}\n",json!({"type":"control_request","request_id":"catalog-init","request":{"subtype":"initialize"}}))).await?;
        process.response(&json!("catalog-init"),true).await?;
        process.write(&format!("{}\n",json!({"type":"control_request","request_id":"catalog-models","request":{"subtype":"list_models"}}))).await?;
        let data=process.response(&json!("catalog-models"),true).await?;
        data["models"].as_array().cloned().filter(|rows|!rows.is_empty()).ok_or("dictation_catalog_unavailable")
    }).await.unwrap_or(Err("dictation_organization_timeout"));
    process.stop().await;
    outcome
}

async fn codex(
    service: &AccountService,
    context: &context::Context,
    guard: Arc<crate::accounts::AccountGuard>,
) -> Result<Value, &'static str> {
    let mut command = service.codex_command().ok_or("dictation_cli_missing")?;
    codex_overrides(&mut command);
    codex_auth_override(
        &mut command,
        service,
        context,
        &codex_configuration(&context.home)?,
    )?;
    command.arg("app-server").env_clear().envs(&context.env);
    let mut process = Process::open(
        command,
        tempfile::tempdir().map_err(|_| "dictation_catalog_unavailable")?,
    )?;
    process.protect_account(guard);
    let outcome=tokio::time::timeout(Duration::from_secs(30),async{
        process.write(&format!("{}\n",json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"hangar_dictation","version":env!("CARGO_PKG_VERSION")}}}))).await?;
        process.response(&json!(1),false).await?;
        process.write(&format!("{}\n",json!({"method":"initialized"}))).await?;
        let mut rows=Vec::new();let mut cursor=Value::Null;
        for page in 0..32 {
            let id=page+2;
            process.write(&format!("{}\n",json!({"id":id,"method":"model/list","params":{"cursor":cursor,"limit":100,"includeHidden":false}}))).await?;
            let data=process.response(&json!(id),false).await?;
            rows.extend(data["data"].as_array().cloned().ok_or("dictation_catalog_unavailable")?);
            cursor=data["nextCursor"].clone();if cursor.is_null(){return Ok(json!({"data":rows}));}
        }
        Err("dictation_catalog_unavailable")
    }).await.unwrap_or(Err("dictation_organization_timeout"));
    process.stop().await;
    outcome
}

fn fast_enabled(home: &Path) -> bool {
    let value = match std::fs::read_to_string(home.join("config.toml")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let Ok(config) = toml::from_str::<toml::Value>(&value) else {
        return false;
    };
    let mut enabled = config
        .get("features")
        .and_then(|f| f.get("fast_mode"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);
    if let Some(profile) = config.get("profile").and_then(toml::Value::as_str) {
        let Some(profile) = config.get("profiles").and_then(|p| p.get(profile)) else {
            return false;
        };
        enabled = profile
            .get("features")
            .and_then(|f| f.get("fast_mode"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(enabled);
    }
    enabled
}

async fn codex_http(home: &Path, version: &str) -> Result<Option<Vec<Value>>, &'static str> {
    use base64::Engine;
    let config = codex_configuration(home)?;
    if config
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .is_some_and(|p| p != "openai")
        || config.get("chatgpt_base_url").is_some()
    {
        return Ok(None);
    }
    let raw = match std::fs::read(home.join("auth.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        Some(raw) => raw,
        None => return Ok(None),
    };
    let Some(token) = raw["tokens"]["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let Some(account) = raw["tokens"]["account_id"]
        .as_str()
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let payload = token
        .split('.')
        .nth(1)
        .and_then(|s| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s.trim_end_matches('='))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.);
    if payload
        .and_then(|p| p["exp"].as_f64())
        .is_none_or(|exp| exp - now <= 60.)
    {
        return Ok(None);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "dictation_catalog_unavailable")?;
    let response = match client
        .get(format!(
            "https://chatgpt.com/backend-api/codex/models?client_version={}",
            form_urlencoded::byte_serialize(version.as_bytes()).collect::<String>()
        ))
        .bearer_auth(token)
        .header("chatgpt-account-id", account)
        .header("user-agent", "codex-cli")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if response.status().as_u16() == 429 {
        return Err("dictation_catalog_rate_limited");
    }
    if !response.status().is_success() {
        return Ok(None);
    }
    let value = response
        .bytes()
        .await
        .ok()
        .filter(|bytes| bytes.len() <= 4 * 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    Ok(value.and_then(|v| v["models"].as_array().cloned()))
}

fn codex_http_models(raw: &[Value]) -> Result<Vec<Value>, &'static str> {
    let mut raw = raw.to_vec();
    raw.sort_by_key(|m| m["priority"].as_i64().unwrap_or(i64::MAX));
    raw.into_iter().filter(|m|m.is_object()).map(|m| {
        let levels=m["supported_reasoning_levels"].as_array().ok_or("dictation_catalog_unavailable")?;
        Ok(json!({"model":m["slug"],"displayName":m["display_name"],"description":m["description"],"hidden":m["visibility"]!="list",
            "supportedReasoningEfforts":levels.iter().map(|level|json!({"reasoningEffort":level["effort"]})).collect::<Vec<_>>(),
            "defaultReasoningEffort":m["default_reasoning_level"],"serviceTiers":m["service_tiers"],"defaultServiceTier":m["default_service_tier"],"additionalSpeedTiers":m["additional_speed_tiers"]}))
    }).collect()
}

pub fn parse(
    provider: &str,
    catalog: &Value,
    current: Option<&str>,
) -> Result<Vec<Value>, &'static str> {
    let rows = catalog
        .get(if provider == "claude" {
            "models"
        } else {
            "data"
        })
        .and_then(Value::as_array)
        .ok_or("Catálogo de modelos inválido.")?;
    let mut out = Vec::new();
    for model in rows {
        let Some(id) = model
            .get(if provider == "claude" {
                "value"
            } else {
                "model"
            })
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if model["hidden"].as_bool() == Some(true) {
            continue;
        }
        let name = model["displayName"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(id);
        let description = model["description"].as_str().unwrap_or("");
        if provider == "claude" {
            let active = current
                .map(|current| current == id || model["resolvedModel"].as_str() == Some(current))
                .unwrap_or(id == "default");
            out.push(json!({"id":id,"name":name,"desc":description,"active":active}));
        } else {
            let list = |key: &str| -> Result<Vec<Value>, &'static str> {
                match model.get(key) {
                    None | Some(Value::Null) => Ok(Vec::new()),
                    Some(Value::Array(rows)) => Ok(rows.clone()),
                    _ => Err("Capacidades de modelo inválidas."),
                }
            };
            let efforts: Vec<_> = list("supportedReasoningEfforts")?
                .iter()
                .filter_map(|e| e["reasoningEffort"].as_str().map(str::to_owned))
                .collect();
            let services: Vec<_> = list("serviceTiers")?
                .into_iter()
                .filter(|s| {
                    s["id"].as_str().is_some_and(|id| !id.is_empty())
                        && s["hidden"].as_bool() != Some(true)
                })
                .collect();
            out.push(json!({"id":id,"name":name,"desc":description,"efforts":efforts,
                "default_effort":model["defaultReasoningEffort"],"service_tiers":services,
                "default_service_tier":model["defaultServiceTier"],"additional_speed_tiers":list("additionalSpeedTiers")?}));
        }
    }
    if out.is_empty() {
        Err("O catálogo não devolveu nenhum modelo disponível.")
    } else {
        Ok(out)
    }
}

pub fn validate(
    models: &[Value],
    model: Option<&str>,
    effort: Option<&str>,
    tier: Option<&str>,
) -> Result<(), String> {
    if tier.is_some_and(|t| !matches!(t, "default" | "priority")) {
        return Err("service_tier: use default ou priority".into());
    }
    let Some(model) = model else {
        return if tier == Some("priority") {
            Err("service_tier priority exige modelo explícito do catálogo do Codex".into())
        } else {
            Ok(())
        };
    };
    let row = models
        .iter()
        .find(|row| row["id"].as_str() == Some(model))
        .ok_or_else(|| format!("Modelo fora do catálogo: {model}"))?;
    if let Some(effort) = effort
        && !row["efforts"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|e| e.as_str() == Some(effort)))
    {
        return Err(format!("Nível fora do suporte de {model}: {effort}"));
    }
    if tier == Some("priority")
        && !row["service_tiers"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|s| s["id"] == "priority"))
    {
        return Err(format!("service_tier priority indisponível para {model}"));
    }
    Ok(())
}
