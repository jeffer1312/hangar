use hangar_server::{accounts::environment::AccountEnvironment, config_sync::runtime::RuntimeConfig};
use serde_json::Value;

#[test]
fn config_metadata_matches_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/metadata.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let mut pairs = vec![("HOME".into(), root.clone()), ("USERPROFILE".into(), root.clone())];
        pairs.extend(case["env"].as_object().unwrap().iter().map(|(key, value)|
            (key.clone(), value.as_str().unwrap().to_owned())));
        let service = RuntimeConfig::new(AccountEnvironment::from_pairs(pairs)).unwrap();
        let result = service.metadata();
        if case["invalid"] == true {
            assert!(result.is_err(), "entrada inválida aceita: {}", case["env"]);
            continue;
        }
        let actual = result.unwrap_or_else(|error| panic!("{}: {error}", case["env"]));
        assert_eq!(normalize_home(&actual, home.path()), case["metadata"]);
    }
}

/// Só caminhos sob o HOME temporário mudam de separador; texto arbitrário permanece intacto.
pub(crate) fn normalize_home(value: &Value, home: &std::path::Path) -> Value {
    let root = home.to_string_lossy();
    hangar_server::config_sync::paths::map_strings(value, &|text| {
        for prefix in [root.to_string(), root.replace('\\', "/")] {
            if let Some(tail) = text.strip_prefix(&prefix)
                .filter(|tail| tail.is_empty() || tail.starts_with(['/', '\\'])) {
                return format!("⟦HOME⟧{}", tail.replace('\\', "/"));
            }
        }
        text.to_owned()
    })
}
