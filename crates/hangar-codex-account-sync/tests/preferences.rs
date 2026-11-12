use hangar_codex_account_sync::project_preferences;
use serde_json::json;

#[test]
fn preferences_keep_service_tier_without_copying_account_identity() {
    let (value, issues) = project_preferences(&json!({
        "model":"gpt-model", "service_tier":"fast", "cli_auth_credentials_store":"keyring",
        "auth":{"token":"private"}, "projects":{"local":{}}, "model_provider":"other"
    }))
    .unwrap();
    assert_eq!(
        value,
        json!({"model":"gpt-model", "service_tier":"fast", "cli_auth_credentials_store":"file"})
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "codex_account_provider_divergence")
    );
}

#[test]
fn mcp_keeps_third_party_secrets_and_excludes_codex_identity() {
    let (value, issues) = project_preferences(&json!({"mcp_servers":{"service":{"env":{
        "JEV_API_KEY":"third-party", "OPENAI_API_KEY":"identity", "CODEX_HOME":"private", "NORMAL":"value"
    }}}})).unwrap();
    assert_eq!(
        value["mcp_servers"]["service"]["env"],
        json!({"JEV_API_KEY":"third-party","NORMAL":"value"})
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "codex_account_mcp_auth_excluded")
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "codex_account_mcp_runtime_excluded")
    );
}

proptest::proptest! {
    #[test]
    fn external_preference_names_never_panic(key in ".{0,100}", value in ".{0,100}") {
        let mut source=serde_json::Map::new();source.insert(key,serde_json::Value::String(value));
        let result=project_preferences(&serde_json::Value::Object(source));
        proptest::prop_assert!(result.is_ok());
    }
}
