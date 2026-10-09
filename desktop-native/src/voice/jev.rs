//! A configuração única do Jev que o servidor grava: uma chave, um endereço e um modelo para navegador,
//! orquestração, voz e Computer Use.
use serde_json::Value;

pub const TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";
pub const TYPESAFE_MODEL: &str = "jev-latest";
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/alpha/decisions";
pub const OPENROUTER_MODEL: &str = "~typesafe/jev-latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Config { pub key: String, pub url: String, pub model: String }

fn text(value: &Value) -> Option<String> { value.as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned) }

pub fn is_openrouter(key: &str, endpoint: Option<&str>) -> bool {
    match endpoint.map(str::trim).filter(|v| !v.is_empty()) {
        Some(endpoint) => endpoint.contains("openrouter.ai"),
        None => key.trim().starts_with("sk-or-"),
    }
}

/// Endereço e modelo efetivos, pela mesma regra do servidor (`runtime_config.destino_jev`). No OpenRouter,
/// `typesafe/jev-latest` ganha o `~` do apelido, sem o qual ele responde "does not exist".
pub fn destination(key: &str, endpoint: Option<&str>, model: Option<&str>) -> (Option<String>, Option<String>) {
    let (endpoint, model) = (endpoint.map(str::trim).filter(|v| !v.is_empty()), model.map(str::trim).filter(|v| !v.is_empty()));
    if is_openrouter(key, endpoint) {
        let mut model = model.unwrap_or(OPENROUTER_MODEL).to_owned();
        if model.starts_with("typesafe/") && model.ends_with("-latest") { model.insert(0, '~'); }
        return (Some(endpoint.unwrap_or(OPENROUTER_URL).to_owned()), Some(model));
    }
    (Some(endpoint.unwrap_or(TYPESAFE_URL).to_owned()), Some(model.unwrap_or(TYPESAFE_MODEL).to_owned()))
}

/// O que a tela do Jev grava no servidor (`runtime-config.json`), senão o ambiente.
pub fn config_from(runtime: &Value, settings_env: &Value, env_key: Option<String>) -> Option<Config> {
    let key = text(&runtime["jev_api_key"]).or(env_key).or_else(|| text(&settings_env["TYPESAFE_API_KEY"]))?;
    let (url, model) = destination(&key, text(&runtime["jev_endpoint"]).as_deref(), text(&runtime["jev_model"]).as_deref());
    Some(Config { key, url: url.unwrap_or_else(|| TYPESAFE_URL.to_owned()), model: model.unwrap_or_else(|| TYPESAFE_MODEL.to_owned()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_key_picks_the_provider_and_its_defaults() {
        assert_eq!(destination("sk-or-v1-x", None, None), (Some(OPENROUTER_URL.into()), Some(OPENROUTER_MODEL.into())));
        assert_eq!(destination("apik-x", None, None), (Some(TYPESAFE_URL.into()), Some(TYPESAFE_MODEL.into())));
        assert_eq!(destination("sk-or-v1-x", None, Some("typesafe/jev-latest")).1.as_deref(), Some("~typesafe/jev-latest"), "sem o til o OpenRouter recusa");
        assert_eq!(destination("apik-x", Some("https://proxy/v1"), Some("jev-1.13.0")), (Some("https://proxy/v1".into()), Some("jev-1.13.0".into())));
    }

    #[test]
    fn the_saved_config_wins_over_the_environment() {
        let runtime = json!({"jev_api_key": "sk-or-saved", "jev_endpoint": "", "jev_model": ""});
        let config = config_from(&runtime, &Value::Null, Some("env-key".into())).unwrap();
        assert_eq!((config.key.as_str(), config.url.as_str()), ("sk-or-saved", OPENROUTER_URL));
        assert_eq!(config_from(&Value::Null, &Value::Null, None), None);
    }
}
