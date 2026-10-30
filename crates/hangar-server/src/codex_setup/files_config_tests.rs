use super::*;

#[tokio::test]
async fn config_without_edits_preserves_wide_integers_before_confirmation() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/plugin-edits.json"
    )).unwrap();
    let mut differences = Vec::new();
    for case in fixture["file_cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let original = case["toml"].as_str().unwrap().as_bytes();
        fs::write(&path,original).unwrap();
        let env = AccountEnvironment::from_map([
            ("HOME".into(),root.path().display().to_string()),
            ("PATH".into(),String::new()),
        ].into());
        let mut events = Vec::new();
        let result = edit_config(&env,&path,&root.path().join("backups"),&root.path().join("work"),|value| {
            events.push(json!({"prepare":crate::orchestration::codec::clone_value(value)}));
            Ok(vec![])
        }).await;
        if result.is_ok() {events.push(json!({"confirm":true}));}
        let actual = json!({"ok":result.is_ok(),"events":events,"unchanged":fs::read(&path).unwrap()==original});
        if actual != case["result"] {differences.push((actual,case["result"].clone(),result.err()));}
    }
    assert!(differences.is_empty(),"Configuração sem edição divergente: {differences:?}");
}
