use hangar_server::{accounts::environment::AccountEnvironment, config_sync::runtime::RuntimeConfig, orchestration::codec};
use serde_json::{Value, json};

#[test]
fn integer_preferences_preserve_python_conversion_and_rejection() {
    let fixture = codec::loads(include_str!("../../../../backend/tests/fixtures/contract/config_sync/integer-preferences.json")).unwrap();
    let mut differences = Vec::new();
    for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let mut runtime = RuntimeConfig::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into())).unwrap();
        runtime.path = home.path().join("runtime-config.json");
        std::fs::write(&runtime.path, "{}").unwrap();
        let key = row["key"].as_str().unwrap();
        let input = [(key.to_owned(), codec::clone_value(&row["input"]))].into_iter().collect();
        let actual = match runtime.apply_checked(&input, &Default::default()) {
            Ok(mut result) => json!({"ok":result.shift_remove(key).unwrap()}),
            Err(error) => json!({"kind":error.kind,"error":error.message}),
        };
        if actual != row["result"] { differences.push(format!("{index} {key}: {actual} versus {}", row["result"])); }
        let disk: Value = serde_json::from_str(&std::fs::read_to_string(&runtime.path).unwrap()).unwrap();
        let expected = row["result"].get("ok").map(|value| json!({key:codec::clone_value(value)})).unwrap_or(json!({}));
        if disk != expected { differences.push(format!("{index} {key}: efeito em disco divergente")); }
        codec::discard(Value::Object(input));
    }
    assert!(differences.is_empty(), "{differences:?}");
}
