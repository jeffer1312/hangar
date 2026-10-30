//! O interruptor continua publicado mesmo se a escrita do endereço falhar.
use serde_json::{Value, json};

#[test]
fn plugin_publish_preserves_files_and_independent_updates() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/plugin-publish.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        for name in ["zeta", "hangar", "alfa"] {
            let path = repo.join("plugins").join(name).join(".claude-plugin/plugin.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}").unwrap();
        }
        std::fs::create_dir_all(repo.join("plugins/sem-manifesto")).unwrap();
        let config = home.path().join(".hangar");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("plugin-dir"), "antigo\n").unwrap();
        if case["broken"] == true { std::fs::create_dir(config.join("plugin.json")).unwrap(); }
        let result = hangar_server::config_sync::plugin_publish::publish(
            home.path(), &repo, 8765, "fixture-owner", case["enabled"].as_bool().unwrap()
        );
        assert_eq!(result.is_ok(), case["result"]["ok"] == true, "{}", case["name"]);
        if case["broken"] == true { assert!(config.join("plugin.json").is_dir()); }
        let mut actual = serde_json::Map::new();
        for entry in std::fs::read_dir(config).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                actual.insert(path.strip_prefix(home.path()).unwrap().to_string_lossy().replace('\\', "/"),
                    json!(crate::contract_paths::text(&std::fs::read_to_string(path).unwrap(), home.path(), "⟦HOME⟧")));
            }
        }
        let mut expected = case["files"].as_object().unwrap().clone();
        if case["name"] == "address_failure" {
            // D25: o temporário da escrita recusada é limpo, preservando os arquivos finais.
            expected.remove(".hangar/plugin.json.tmp");
        }
        assert_eq!(actual, expected, "{}", case["name"]);
    }
}
