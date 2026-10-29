#[test]
fn cliproxy_client_key_matches_python_line_reader() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/client-keys.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let env = hangar_server::accounts::environment::AccountEnvironment::from_map(
            [
                ("HOME".into(), root.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    root.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
        );
        let path = root.path().join(".cli-proxy-api/config.yaml");
        if let Some(text) = case["text"].as_str() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        } else if case["invalid_utf8"] == true {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, [255]).unwrap();
        }
        let expected = case["keys"]
            .as_array()
            .unwrap()
            .first()
            .and_then(serde_json::Value::as_str);
        assert_eq!(
            hangar_server::cliproxy_config::client_key(&env).as_deref(),
            expected,
            "{}",
            case["name"]
        );
    }
}
