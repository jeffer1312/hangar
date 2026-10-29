//! O ambiente é congelado, mas os overrides do app continuam vivos.
use hangar_server::{accounts::environment::AccountEnvironment, config_sync::runtime::RuntimeConfig};
use serde_json::{Value, json};

#[test]
fn runtime_environment_preserves_settings_lifetime_and_alias_priority() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/runtime-environment.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let mut pairs = vec![("HOME".into(), root.clone()), ("USERPROFILE".into(), root)];
        pairs.extend(case["pairs"].as_array().unwrap().iter().map(|pair| (
            pair[0].as_str().unwrap().to_owned(), pair[1].as_str().unwrap().to_owned()
        )));
        let dotenv = home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(dotenv.parent().unwrap()).unwrap();
        std::fs::write(&dotenv, case["file"].as_str().unwrap()).unwrap();
        let mut service = RuntimeConfig::new(AccountEnvironment::from_pairs(pairs)).unwrap();
        let values = |service: &RuntimeConfig| json!({
            "editor": service.get("editor"), "shortcuts": service.get("shortcuts"),
            "groq_api_key": service.get("groq_api_key")
        });
        assert_eq!(values(&service), case["before"], "antes: {}", case["name"]);
        std::fs::write(&dotenv, "CP_EDITOR=late-file\nCP_GROQ_API_KEY=late-file\n").unwrap();
        service.env.base.insert("CP_EDITOR".into(), "late-env".into());
        assert_eq!(values(&service), case["changed"], "ambiente alterado: {}", case["name"]);
        std::fs::create_dir_all(service.path.parent().unwrap()).unwrap();
        std::fs::write(&service.path, r#"{"editor":"runtime-editor"}"#).unwrap();
        assert_eq!(values(&service), case["overridden"], "override: {}", case["name"]);
        std::fs::write(&service.path, "{}").unwrap();
        assert_eq!(values(&service), case["removed"], "removido: {}", case["name"]);
    }
}
