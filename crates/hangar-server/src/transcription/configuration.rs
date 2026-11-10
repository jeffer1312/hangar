//! Configuração efetiva compartilhada pela transcrição e pela organização do ditado.
use crate::{config_sync::{runtime::RuntimeConfig, runtime_values::ValueGuard}, routes::AppState};
use serde_json::{Value, json};

pub(crate) async fn snapshot(state: &AppState) -> Result<ValueGuard, ()> {
    let config = RuntimeConfig::from_state(state).map_err(|_| ())?;
    tokio::task::spawn_blocking(move || assemble(&config)).await.map_err(|_| ())
}

fn assemble(config: &RuntimeConfig) -> ValueGuard {
    let value_or = |key: &str, default: Value| {
        let value = config.get(key);
        if crate::orchestration::truthy(&value) { value } else { default }
    };
    let mut providers = ValueGuard(value_or("transcription_providers", json!([])));
    let key = ValueGuard(config.get("groq_api_key"));
    let key = crate::transcript::py::strip(key.as_str().unwrap_or(""));
    let legacy = if !crate::orchestration::truthy(&providers) && !key.is_empty() {
        json!({"id":"legacy", "kind":"openai", "api_key":key,
            "base_url":value_or("transcription_base_url",json!("")),
            "model":value_or("transcription_model",json!(""))})
    } else { Value::Null };
    let mut organization = ValueGuard(json!({"dictation_include_recent_messages":
        crate::orchestration::truthy(&config.get("dictation_include_recent_messages"))}));
    for field in ["dictation_organization_mode", "dictation_claude_model", "dictation_codex_model",
        "ditado_estilo", "llm_base_url", "llm_api_key", "llm_model", "llm_reasoning_effort",
        "llm_briefing_base_url", "llm_briefing_api_key", "llm_briefing_model"] {
        organization[field] = value_or(field,json!(""));
    }
    if organization["dictation_organization_mode"] != "external_api" {
        for field in ["llm_base_url", "llm_api_key", "llm_briefing_base_url", "llm_briefing_api_key"] {
            organization[field] = json!("");
        }
    }
    ValueGuard(json!({"providers":providers.take(), "legacy":legacy,
        "organization":organization.take(),
        "user_vocabulary":value_or("ditado_vocabulario",json!("")),
        "dictation_max_minutes":value_or("dictation_max_minutes",json!(0)),
        "state_path":config.env.home.join(".hangar/transcription-wait.json").to_string_lossy()}))
}
