use hangar_server::codex_setup::trust;
use proptest::prelude::*;
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn codex_trust_matches_original_preparation() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/hook-trust.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let result = trust::edits(
            &case["data"],
            Path::new("/fixture/home"),
            Path::new("/fixture/account"),
            case["reload"].as_bool().unwrap(),
        );
        let matches = match result {
            Ok((edits, manual)) => case["result"] == json!({"edits":edits,"manual":manual}),
            Err(ref error) => {
                case["result"]["error"] == "ValueError" && case["result"]["message"] == *error
            }
        };
        if !matches {
            failures.push(index);
        }
    }
    assert!(failures.is_empty(), "cenários divergentes: {failures:?}");
}
proptest! {
    #[test]
    fn codex_trust_never_approves_an_unrelated_project(key in ".{0,100}", enabled in any::<bool>(), reload in any::<bool>()) {
        let data = json!({"data":[{"hooks":[{"source":"project","enabled":enabled,"trustStatus":"untrusted","key":format!("project:{key}"),"currentHash":"fixture"}]}]});
        let (edits,_) = trust::edits(&data,Path::new("/fixture/home"),Path::new("/fixture/account"),reload).unwrap();
        prop_assert!(edits.is_empty());
    }
}
