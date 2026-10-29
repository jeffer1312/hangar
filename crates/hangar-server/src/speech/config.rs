//! O arquivo vivo da configuração é a fonte; nenhuma chamada ao Python busca credenciais.
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::SpeechError;

#[derive(Clone)]
pub struct SpeechConfig {
    pub values: Value,
    pub home: PathBuf,
    pub cache_dir: PathBuf,
}

impl SpeechConfig {
    pub fn load(home: &Path, claude_dir: &Path, backend: Option<&Path>) -> Result<Self, SpeechError> {
        let path = claude_dir.join("runtime-config.json");
        let mut values = match crate::runtime::queue::read_replaced(&path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(map)) => Value::Object(map),
                _ => return Err(SpeechError::new(503, "A configuração de voz está inválida.")),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(_) => return Err(SpeechError::new(503, "Não foi possível ler a configuração de voz.")),
        };
        let environment = |names: &[&str]| -> Result<Option<String>, SpeechError> {
            for name in names {
                if let Ok(value) = std::env::var(name) { return Ok(Some(value)); }
            }
            for name in names {
                let value = crate::user_data::env_value(home, backend, name)
                    .map_err(|_| SpeechError::new(503, "Não foi possível ler o ambiente da configuração de voz."))?;
                if value.is_some() { return Ok(value); }
            }
            Ok(None)
        };
        if values.get("groq_api_key").is_none() {
            values["groq_api_key"] = json!(environment(&["CP_GROQ_API_KEY", "GROQ_API_KEY"])?.unwrap_or_default());
        }
        if values.get("traduzir_pensamento").is_none() {
            let enabled = environment(&["CP_TRADUZIR_PENSAMENTO"])?.map_or(true, |v|
                matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "t" | "yes" | "y" | "on"));
            values["traduzir_pensamento"] = json!(enabled);
        }
        let projects = environment(&["CP_PROJECTS_DIR"])?.filter(|v| !v.is_empty())
            .map(PathBuf::from).unwrap_or_else(|| claude_dir.join("projects"));
        let cache_dir = projects.parent().unwrap_or(claude_dir).join(".hangar-tts");
        Ok(Self { values, home: home.to_owned(), cache_dir })
    }

    pub fn string(&self, key: &str) -> String {
        self.values.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
    }

    pub fn integer(&self, key: &str, default: i64) -> i64 {
        match self.values.get(key) {
            Some(Value::Bool(value)) => i64::from(*value),
            Some(Value::Number(value)) => value.as_i64().or_else(|| value.as_f64().filter(|v| v.is_finite())
                .map(|v| v.clamp(i64::MIN as f64, i64::MAX as f64) as i64)).unwrap_or(default),
            Some(Value::String(value)) => value.trim().parse().unwrap_or(default),
            _ => default,
        }
    }

    pub fn boolean(&self, key: &str, default: bool) -> bool {
        self.values.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn thought_available(&self) -> bool {
        self.boolean("traduzir_pensamento", true) && !self.text_provider().1.is_empty()
    }

    /// A chave legada do áudio só pertence ao LLM quando ambos usam o mesmo serviço padrão.
    pub fn text_provider(&self) -> (String, String, String) {
        let configured = self.string("llm_base_url");
        let endpoint = configured.trim().trim_end_matches('/');
        let endpoint = if endpoint.is_empty() { "https://api.groq.com/openai/v1" } else { endpoint };
        let key = if endpoint == "https://api.groq.com/openai/v1" {
            if self.string("transcription_base_url").trim().is_empty() { self.string("groq_api_key") } else { String::new() }
        } else { self.string("llm_api_key") };
        let model = self.string("llm_model");
        (endpoint.to_owned(), key.trim().to_owned(),
            if model.trim().is_empty() { "openai/gpt-oss-120b".to_owned() } else { model.trim().to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_the_live_file_never_requires_an_organization_mode_or_python() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude");
        std::fs::create_dir_all(&config).unwrap();
        let path = config.join("runtime-config.json");
        std::fs::write(&path, r#"{"llm_base_url":"https://model.test/v1", "llm_api_key":"fixture", "dictation_organization_mode":"none"}"#).unwrap();
        let first = SpeechConfig::load(dir.path(), &config, None).unwrap();
        assert_eq!(first.text_provider().1, "fixture");
        std::fs::write(path, r#"{"llm_base_url":"https://other.test/v1", "llm_api_key":"new"}"#).unwrap();
        let second = SpeechConfig::load(dir.path(), &config, None).unwrap();
        assert_eq!(second.text_provider().1, "new");
        assert_eq!(first.cache_dir, config.join(".hangar-tts"));
    }

    #[test]
    fn a_key_never_crosses_the_transcription_and_text_hosts() {
        let mut config = SpeechConfig { values: json!({"groq_api_key":"audio", "llm_api_key":"text"}),
            home: PathBuf::new(), cache_dir: PathBuf::new() };
        assert_eq!(config.text_provider().1, "audio");
        config.values["transcription_base_url"] = json!("https://audio.test");
        assert!(config.text_provider().1.is_empty());
        config.values["llm_base_url"] = json!("https://text.test/v1");
        assert_eq!(config.text_provider().1, "text");
    }
}
