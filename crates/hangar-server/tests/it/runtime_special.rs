use hangar_server::{accounts::environment::AccountEnvironment, config_sync::runtime::RuntimeConfig,
    orchestration::codec};
use serde_json::{Value, json};

#[test]
fn runtime_special_stored_values_keep_python_reads_and_disk_effects() {
    let fixture = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/runtime-special.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let environment = AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(), String::new()),
        ].into());
        let service = RuntimeConfig::new(environment).unwrap();
        std::fs::create_dir_all(service.path.parent().unwrap()).unwrap();
        std::fs::write(&service.path, case["initial"].as_str().unwrap()).unwrap();
        assert_eq!(Value::Object(service.read()), case["loaded"]["ok"], "leitura {}", case["initial"]);
        if let Some(expected) = case["state"].get("ok") {
            assert_eq!(service.state(), *expected, "estado {}", case["initial"]);
        }
        let changes = json!({"finish_min_seconds":9});
        let applied = service.apply(changes.as_object().unwrap(), &Default::default());
        if let Some(expected) = case["applied"].get("ok") {
            assert_eq!(Value::Object(applied.unwrap()), *expected, "aplicação {}", case["initial"]);
        } else {
            assert!(applied.is_err(), "escrita deveria falhar: {}", case["initial"]);
        }
        assert_eq!(std::fs::read_to_string(&service.path).unwrap(), case["disk"].as_str().unwrap(),
            "arquivo {}", case["initial"]);
    }
}
