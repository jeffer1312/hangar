use hangar_server::{accounts::environment::AccountEnvironment,
    config_sync::runtime::RuntimeConfig, orchestration::codec};
use serde_json::Value;

#[test]
fn runtime_apply_errors_preserve_precedence_mask_and_both_files() {
    let fixture = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/runtime-apply-errors.json")).unwrap();
    let mut differences = Vec::new();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let service = RuntimeConfig::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(), String::new()),
        ].into())).unwrap();
        std::fs::create_dir_all(service.path.parent().unwrap()).unwrap();
        let initial = codec::dumps(&case["initial"], false);
        std::fs::write(&service.path, &initial).unwrap();
        let thinking = service.path.parent().unwrap().join("settings.json");
        if let Some(text) = case["thinking"].as_str() {
            std::fs::write(&thinking, text).unwrap();
        }
        let actual_state = service.state().map(|value| value["groq_api_key"].clone());
        let expected_state = case["state_secret"].get("ok").cloned()
            .ok_or_else(|| case["state_secret"]["error"].as_str().unwrap().to_owned());
        if actual_state != expected_state {differences.push(format!("{index}: máscara"));}
        let actual = service.apply(case["changes"].as_object().unwrap(), &Default::default()).map(Value::Object);
        let expected = case["result"].get("ok").cloned()
            .ok_or_else(|| case["result"]["error"].as_str().unwrap().to_owned());
        if actual != expected {differences.push(format!("{index}: resultado {actual:?} versus {expected:?}"));}
        if std::fs::read_to_string(&service.path).unwrap() != case["runtime"].as_str().unwrap() {
            differences.push(format!("{index}: arquivo de preferências"));
        }
        if std::fs::read_to_string(thinking).ok().as_deref() != case["settings"].as_str() {
            differences.push(format!("{index}: arquivo de pensamento"));
        }
    }
    assert!(differences.is_empty(), "CFG-064/065: {differences:?}");
}
