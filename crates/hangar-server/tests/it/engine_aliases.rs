//! O escritor único preserva a leitura tolerante e a rejeição de texto impossível de gravar.
use hangar_server::{
    accounts::{aliases, environment::AccountEnvironment},
    orchestration::codec,
};
use serde_json::Value;

#[test]
fn engine_aliases_match_original_read_write_and_unicode_failures() {
    let fixture = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/aliases.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_map(
            [("HOME".into(), home.path().to_string_lossy().into_owned())].into(),
        );
        let path = aliases::path(&env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(before) = case["before"].as_str() {
            std::fs::write(&path, before).unwrap();
        }
        let before = Value::Object(aliases::read(&env));
        if before != case["read"] {
            failures.push(format!(
                "{} leitura: {} != {}",
                case["name"],
                codec::dumps(&before, false),
                codec::dumps(&case["read"], false)
            ));
        }
        let result = aliases::set(&env, case["id"].as_str().unwrap(), case["value"].as_str());
        // Correção de perda de dados: a captura antiga sobrescrevia estes documentos inválidos.
        if matches!(case["name"].as_str(), Some("broken" | "list")) {
            assert!(result.is_err(), "{}", case["name"]);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                case["before"].as_str().unwrap()
            );
            continue;
        }
        match (result, case["result"].get("error")) {
            (Ok(value), None) if Value::Object(value.clone()) == case["result"]["map"] => {}
            (Err(_), Some(_)) => {}
            (actual, _) => failures.push(format!("{} escrita: {actual:?}", case["name"])),
        }
        assert_eq!(
            Value::Object(aliases::read(&env)),
            case["read_again"],
            "releitura {}",
            case["name"]
        );
        let after = std::fs::read_to_string(&path)
            .ok()
            .map(Value::String)
            .unwrap_or(Value::Null);
        if after != case["after"] {
            failures.push(format!("{} arquivo final divergiu", case["name"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

proptest::proptest! {
    #[test]
    fn engine_aliases_external_bytes_and_labels_never_panic(
        bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048),
        label in proptest::collection::vec(proptest::prelude::any::<char>(), 0..100),
    ) {
        let home = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_map(
            [("HOME".into(), home.path().to_string_lossy().into_owned())].into(),
        );
        let path = aliases::path(&env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let _ = aliases::read(&env);
        let label: String = label.into_iter().collect();
        let saved = match aliases::set(&env, "fixture", Some(&label)) {
            Ok(saved) => saved,
            Err(_) => {
                proptest::prop_assert_eq!(std::fs::read(&path).unwrap(), bytes);
                return Ok(());
            }
        };
        let _ = aliases::read(&env);
        let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        proptest::prop_assert_eq!(stored, Value::Object(saved.clone()));
        if let Some(value) = saved.get("fixture") {
            proptest::prop_assert!(value.as_str().unwrap().chars().count() <= 40);
        }
    }
}

#[test]
fn engine_aliases_preserve_unreadable_bytes_and_existing_entries() {
    let home = tempfile::tempdir().unwrap();
    let env =
        AccountEnvironment::from_pairs([("HOME", home.path().to_string_lossy().into_owned())]);
    let path = aliases::path(&env);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for raw in [b"{".as_slice(), b"[]", b"null", &[255]] {
        std::fs::write(&path, raw).unwrap();
        assert!(aliases::set(&env, "new", Some("Novo")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), raw);
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(aliases::set(&env, "new", Some("Novo")).is_err());
    assert!(path.is_dir());
    std::fs::remove_dir(&path).unwrap();
    aliases::set(&env, "first", Some("Primeiro")).unwrap();
    let saved = aliases::set(&env, "second", Some("Segundo")).unwrap();
    assert_eq!(saved["first"], "Primeiro");
    assert_eq!(saved["second"], "Segundo");
}
