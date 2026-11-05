use hangar_server::codex_setup::trust;
use proptest::prelude::*;
use serde_json::json;
use std::path::Path;

proptest! {
    #[test]
    fn codex_trust_never_approves_an_unrelated_project(key in ".{0,100}", enabled in any::<bool>(), reload in any::<bool>()) {
        let data = json!({"data":[{"hooks":[{"source":"project","enabled":enabled,"trustStatus":"untrusted","key":format!("project:{key}"),"currentHash":"fixture"}]}]});
        let (edits,_) = trust::edits(&data,Path::new("/fixture/home"),Path::new("/fixture/account"),reload).unwrap();
        prop_assert!(edits.is_empty());
    }
}
