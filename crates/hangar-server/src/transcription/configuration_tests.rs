use super::configuration::snapshot;
use crate::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    config_sync::runtime::RuntimeConfig,
    routes::AppState,
};
use serde_json::{Value, json};
use std::path::Path;

fn state(home: &Path, variables: &[(&str, &str)]) -> AppState {
    // Impede que a descoberta da instalação leia o .env do checkout da máquina.
    std::fs::create_dir_all(home.join(".hangar/current")).unwrap();
    let mut environment = std::collections::BTreeMap::from([
        ("HOME".into(), home.display().to_string()),
        ("USERPROFILE".into(), home.display().to_string()),
    ]);
    environment.extend(variables.iter().map(|(key, value)| (key.to_string(), value.to_string())));
    let address = "127.0.0.1:0".parse().unwrap();
    let mut state = AppState::new(Config {
        listen: address, upstream: address, internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(), log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(environment));
    state
}

fn apply(state: &AppState, changes: Value) {
    RuntimeConfig::from_state(state).unwrap()
        .apply_checked(changes.as_object().unwrap(), &Default::default()).unwrap();
}

#[tokio::test]
async fn defaults_are_available_without_an_upstream_server() {
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path(), &[]);
    let config = snapshot(&state).await.unwrap();
    assert_eq!(*config, json!({
        "providers": [], "legacy": null, "user_vocabulary": "", "dictation_max_minutes": 0,
        "state_path": home.path().join(".hangar/transcription-wait.json").to_string_lossy(),
        "organization": {
            "dictation_organization_mode": "none", "dictation_claude_model": "",
            "dictation_codex_model": "", "dictation_include_recent_messages": false,
            "ditado_estilo": "", "llm_base_url": "", "llm_api_key": "", "llm_model": "",
            "llm_reasoning_effort": "", "llm_briefing_base_url": "",
            "llm_briefing_api_key": "", "llm_briefing_model": ""
        }
    }));
}

#[tokio::test]
async fn saved_providers_override_legacy_and_are_read_after_normal_writes() {
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path(), &[
        ("CP_GROQ_API_KEY", " fixture-groq "),
        ("CP_DICTATION_ORGANIZATION_MODE", "harness"),
        ("CP_DICTATION_CLAUDE_MODEL", "fixture-claude"),
        ("CP_DICTATION_CODEX_MODEL", "fixture-codex"),
        ("CP_DICTATION_INCLUDE_RECENT_MESSAGES", "true"),
    ]);
    let initial = snapshot(&state).await.unwrap();
    assert_eq!(initial["legacy"], json!({
        "id":"legacy", "kind":"openai", "api_key":"fixture-groq", "base_url":"", "model":""
    }));
    assert_eq!(initial["organization"]["dictation_organization_mode"], "harness");
    assert_eq!(initial["organization"]["dictation_claude_model"], "fixture-claude");
    assert_eq!(initial["organization"]["dictation_codex_model"], "fixture-codex");
    assert_eq!(initial["organization"]["dictation_include_recent_messages"], true);
    apply(&state, json!({
        "transcription_providers": [
            {"id":"first", "kind":"openai", "model":"fixture-a", "api_key":"fixture-a"},
            {"id":"second", "kind":"elevenlabs", "api_key":"fixture-b"}
        ],
        "ditado_vocabulario":"Hangar, Promédico", "dictation_max_minutes":12,
        "dictation_organization_mode":"none", "dictation_include_recent_messages":false
    }));
    let updated = snapshot(&state).await.unwrap();
    assert_eq!(updated["legacy"], Value::Null);
    assert_eq!(updated["providers"][0]["id"], "first");
    assert_eq!(updated["providers"][1]["id"], "second");
    assert_eq!(updated["user_vocabulary"], "Hangar, Promédico");
    assert_eq!(updated["dictation_max_minutes"], 12);
    assert_eq!(updated["organization"]["dictation_include_recent_messages"], false);
    assert_eq!(updated["organization"]["dictation_organization_mode"], "none");
    apply(&state, json!({"transcription_providers":[], "groq_api_key":""}));
    assert_eq!(snapshot(&state).await.unwrap()["legacy"], Value::Null);
}

#[tokio::test]
async fn organization_only_exposes_external_credentials_in_external_api_mode() {
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path(), &[]);
    let organization = json!({
        "dictation_organization_mode":"external_api", "dictation_claude_model":"fixture-claude",
        "dictation_codex_model":"fixture-codex", "dictation_include_recent_messages":true,
        "ditado_estilo":"briefing", "llm_base_url":"https://example.test/v1",
        "llm_api_key":"fixture-key", "llm_model":"fixture-model", "llm_reasoning_effort":"low",
        "llm_briefing_base_url":"https://briefing.example.test/v1",
        "llm_briefing_api_key":"fixture-briefing-key", "llm_briefing_model":"fixture-briefing"
    });
    apply(&state, organization.clone());
    assert_eq!(snapshot(&state).await.unwrap()["organization"], organization);
    for mode in ["none", "harness"] {
        apply(&state, json!({"dictation_organization_mode":mode}));
        let mut expected = organization.clone();
        expected["dictation_organization_mode"] = json!(mode);
        for field in ["llm_base_url", "llm_api_key", "llm_briefing_base_url", "llm_briefing_api_key"] {
            expected[field] = json!("");
        }
        assert_eq!(snapshot(&state).await.unwrap()["organization"], expected);
    }
}
