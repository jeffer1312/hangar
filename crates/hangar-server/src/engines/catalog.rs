//! Catálogo do motor: cache para leitura, consulta nova antes de uma troca.
use super::{probe, proxy, store};
use crate::{accounts::AccountService, orchestration::truthy, provider_models::ModelError};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};
use tokio::time::Instant;
type Cache = HashMap<(std::path::PathBuf, String), (Instant, Vec<Value>)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn supports_fast(service: &AccountService, engine: &str, model: Option<&str>) -> bool {
    allows_fast(FastSupport::read(service, engine).as_ref(), model)
}
/// O motor salvo e o "CLIProxy local" valem para todos os modelos: lidos uma vez por consulta.
struct FastSupport {
    saved_model: String,
    local: bool,
}
impl FastSupport {
    fn read(service: &AccountService, engine: &str) -> Option<Self> {
        let entries = store::state(&service.env).0;
        let saved = entries.get(engine)?;
        Some(Self {
            saved_model: saved["model"].as_str().unwrap_or("").to_owned(),
            local: proxy::is_local(&service.env, saved["base_url"].as_str().unwrap_or("")).unwrap_or(false),
        })
    }
}
fn allows_fast(support: Option<&FastSupport>, model: Option<&str>) -> bool {
    let Some(support) = support else {
        return false;
    };
    let model = model
        .filter(|s| !s.is_empty())
        .unwrap_or(&support.saved_model)
        .rsplit('/')
        .next()
        .unwrap_or("");
    model
        .strip_prefix("gpt-")
        .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()))
        && support.local
}
pub fn invalidate(env: &crate::accounts::environment::AccountEnvironment, engine: &str) {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(store::path(env), engine.to_owned()));
}
pub async fn models(
    service: &AccountService,
    engine: &str,
    account: Option<&str>,
    fresh: bool,
) -> Result<Vec<Value>, ModelError> {
    // A conta fixa é validada antes do cache; a leitura sem conta conserva o catálogo publicado.
    let prefix = if let Some(account) = account {
        let entries = store::state(&service.env).0;
        let saved = entries
            .get(engine)
            .filter(|v| truthy(v))
            .ok_or_else(|| {
                ModelError::coded(400, "erro_motor_invalido", "motor inválido", json!({}))
            })?;
        if !proxy::is_local(&service.env, saved["base_url"].as_str().unwrap_or("")).unwrap_or(false)
        {
            return Err(ModelError::coded(
                400,
                "erro_cliproxy_conta",
                "a conta ChatGPT só pode ser fixada no CLIProxyAPI local",
                json!({}),
            ));
        }
        Some(proxy::resolve(service,account,None).map_err(|e|ModelError::coded(400,"erro_cliproxy_conta",&e,json!({})))?["prefix"].as_str().unwrap_or("").to_owned())
    } else {
        None
    };
    let cache_key = (store::path(&service.env), engine.to_owned());
    let cached = if fresh {
        None
    } else {
        CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&cache_key)
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(300))
            .map(|(_, v)| v.clone())
    };
    let models = if let Some(models) = cached {
        models
    } else {
        let entries = store::state(&service.env).0;
        let saved = entries
            .get(engine)
            .filter(|v| truthy(v))
            .ok_or_else(|| {
                ModelError::coded(
                    409,
                    "erro_motor_ausente",
                    &format!(
                        "motor {} nao esta mais no engines.json",
                        crate::model_picker::py_repr(engine)
                    ),
                    json!({"nome":engine}),
                )
            })?;
        let (base, key) = (
            saved["base_url"].as_str().unwrap_or(""),
            saved["api_key"].as_str().unwrap_or(""),
        );
        let models = probe::fetch(base, key).await.map_err(|error| {
            let message = error
                .detail
                .as_str()
                .unwrap_or("resposta inválida do provedor");
            let message = if key.is_empty() {
                message.to_owned()
            } else {
                message.replace(key, "***")
            };
            ModelError::coded(
                502,
                "erro_provedor_offline",
                &format!(
                    "o provedor do motor {} nao respondeu: {message}",
                    crate::model_picker::py_repr(engine)
                ),
                json!({"nome":engine,"erro":message}),
            )
        })?;
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
        cache.insert(cache_key, (Instant::now(), models.clone()));
        models
    };
    Ok(models
        .into_iter()
        .filter_map(|mut model| {
            if let Some(prefix) = &prefix {
                let id = model["id"]
                    .as_str()?
                    .strip_prefix(&format!("{prefix}/"))?
                    .to_owned();
                if id.starts_with("gpt-image-") || proxy::prefix_model(&id, prefix).is_err() {
                    return None;
                }
                model["id"] = json!(id);
            }
            Some(model)
        })
        .collect())
}
pub fn picker(service: &AccountService, engine: &str, models: Vec<Value>) -> Vec<Value> {
    let fast = FastSupport::read(service, engine);
    models.into_iter().map(|model|json!({"id":model["id"],"context_length":model["context_length"],"vision":model["vision"],"supports_fast":allows_fast(fast.as_ref(),model["id"].as_str())})).collect()
}
