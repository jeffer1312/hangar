use hangar_codex_account_sync::project_preferences;
use serde_json::json;

#[test]
fn project_table_without_trust_is_completed_without_overriding_explicit_choice() {
    use hangar_codex_account_sync::project_trust_edits;
    let cwd = "/example/project";
    let edits = project_trust_edits(&json!({"projects":{cwd:{"custom":"kept"}}}),cwd);
    assert_eq!(edits,json!([{"keyPath":"projects.\"/example/project\".trust_level","value":"trusted","mergeStrategy":"replace"}]).as_array().unwrap().clone());
    for choice in ["trusted","untrusted"] {
        assert!(project_trust_edits(&json!({"projects":{cwd:{"trust_level":choice}}}),cwd).is_empty());
    }
}

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

#[test]
fn shell_environment_excludes_identity_without_case_sensitivity() {
    let (value, issues) = project_preferences(&json!({"shell_environment_policy":{"set":{
        "Codex_Home":"private", "openai_base_url":"https://example.invalid",
        "OpenAI_Api_Key":"private", "JEV_API_KEY":"third-party", "NORMAL":"kept"
    }}})).unwrap();
    assert_eq!(value["shell_environment_policy"]["set"], json!({"JEV_API_KEY":"third-party","NORMAL":"kept"}));
    assert!(issues.iter().any(|issue| issue["code"] == "codex_account_provider_divergence"
        && issue["params"]["key"] == "openai_base_url"));
}

#[test]
fn model_provider_excludes_literal_tokens_and_authentication_headers() {
    let (value, issues) = project_preferences(&json!({"model_providers":{"custom":{
        "experimental_bearer_token":"private-token", "env_key":"CUSTOM_API_KEY",
        "http_headers":{"Authorization":"Bearer private", "Proxy-Authorization":"private-proxy", "X-Client":"kept"},
        "env_http_headers":{"Authorization":"CUSTOM_API_KEY"}
    }}})).unwrap();
    let provider = &value["model_providers"]["custom"];
    assert!(provider.get("experimental_bearer_token").is_none());
    assert_eq!(provider["http_headers"], json!({"X-Client":"kept"}));
    assert_eq!(provider["env_key"],"CUSTOM_API_KEY");
    assert_eq!(provider["env_http_headers"],json!({"Authorization":"CUSTOM_API_KEY"}));
    for key in ["experimental_bearer_token", "http_headers.Authorization", "http_headers.Proxy-Authorization"] {
        assert!(issues.iter().any(|issue| issue["code"] == "codex_account_provider_auth_excluded"
            && issue["params"]["provider"] == "custom" && issue["params"]["key"] == key), "{key}");
    }
}
