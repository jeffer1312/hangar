//! O escritor único preserva a leitura tolerante e a rejeição de texto impossível de gravar.
use hangar_server::accounts::{aliases, environment::AccountEnvironment};
use serde_json::Value;

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
