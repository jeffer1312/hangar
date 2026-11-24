//! O `agy` com terminal ignora o `GEMINI_HOME`: o pedido nunca leva uma conta adicional.
use std::collections::BTreeMap;

#[test]
fn gemini_home_does_not_become_an_account() {
    let vars = BTreeMap::from([("GEMINI_HOME".to_owned(), "/home/x/.antigravity-trabalho".to_owned())]);
    let body = hangar_open_cli::request("antigravity", &["--mode".to_owned(), "plan".to_owned()], &vars, std::path::Path::new("/tmp"), &mut Vec::new()).unwrap();
    assert!(!body.contains_key("config_dir"), "{body:?}");
    assert_eq!(body["permission_mode"], "plan");
}
