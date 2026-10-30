use super::super::runtime::RuntimeConfig;
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Value,json};
use std::collections::{BTreeMap,BTreeSet};

#[test]
fn codex_context_preserves_upstream_defaults_and_saved_choice() {
    let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/codex-context.json")).unwrap();
    let mut differences=Vec::new();
    for (index,case) in corpus["cases"].as_array().unwrap().iter().enumerate() {
        let home=tempfile::tempdir().unwrap();
        let mut environment=BTreeMap::from([
            ("HOME".into(),home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(),home.path().to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(),String::new()),
        ]);
        if let Some(value)=case["env"].as_str() {environment.insert("CP_CODEX_EXTENDED_CONTEXT".into(),value.into());}
        let runtime=RuntimeConfig::new(AccountEnvironment::from_map(environment)).unwrap();
        std::fs::create_dir_all(runtime.path.parent().unwrap()).unwrap();
        std::fs::write(&runtime.path,crate::transcript::pyjson::dumps(&case["initial"],false)).unwrap();
        let before=runtime.state().unwrap()["codex_extended_context"].clone();
        let remove=if case["remove"]==true {BTreeSet::from(["codex_extended_context".into()])}else{BTreeSet::new()};
        let result=match runtime.apply(case["changes"].as_object().unwrap(),&remove) {
            Ok(value)=>json!({"ok":value}),Err(error)=>json!({"error":error}),
        };
        let actual=json!({"before":before,"result":result,
            "after":runtime.state().unwrap()["codex_extended_context"],
            "disk":std::fs::read_to_string(&runtime.path).unwrap()});
        let expected=json!({"before":case["before"],"result":case["result"],"after":case["after"],"disk":case["disk"]});
        if actual!=expected {differences.push(json!({"case":index,"actual":actual,"expected":expected}));}
    }
    assert!(differences.is_empty(),"Contexto Codex divergente: {differences:#?}");
}
