use hangar_server::{accounts::{aliases, catalog}, config_sync::{apply::Apply,
    bundle::Bundle, export::Context, paths::Roots}, transcript::pyjson};
use serde_json::{Value, json};

#[test]
fn config_account_alias_admission_and_reapplication_match_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/account-alias.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let home_text = home.path().to_string_lossy().into_owned();
        let account = home.path().join(".claude-fixture");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join(".hangar-conta"), "{}").unwrap();
        let id = format!("claude:{}", catalog::resolved(&account).display());
        let roots = Roots {home: home_text.clone(), hangar: home_text,
            claude: home.path().join(".claude").to_string_lossy().into_owned(),
            codex: home.path().join(".codex").to_string_lossy().into_owned()};
        let mut bundle = Bundle::default();
        bundle.items.insert("claude_accounts".into(), json!({"accounts":{
            "fixture":{"alias":case["alias"],"settings":{}}
        }}));
        let context = Context::new(roots);
        let mut apply = Apply::new(context.clone(), bundle.clone(), "fixture", Default::default());
        let path = aliases::path(&apply.accounts.env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, pyjson::dumps(&json!({id.clone():"Original"}), false)).unwrap();
        for step in case["steps"].as_array().unwrap() {
            apply.apply_item("claude_accounts").unwrap();
            let actual = apply.result("claude_accounts");
            assert_eq!(actual["changed"], step["result"]["changed"], "{}", case["alias"]);
            assert_eq!(actual["warnings"], step["result"]["warnings"], "{}", case["alias"]);
            assert_eq!(aliases::read(&apply.accounts.env).get(&id), step.get("alias"), "{}", case["alias"]);
            let disk = std::fs::read_to_string(&path).unwrap().replace(
                &serde_json::to_string(&id).unwrap(), "\"claude:<HOME>/.claude-fixture\"");
            assert_eq!(disk, step["disk"].as_str().unwrap(), "{}", case["alias"]);
            apply = Apply::new(context.clone(), bundle.clone(), "fixture", Default::default());
        }
    }
}
