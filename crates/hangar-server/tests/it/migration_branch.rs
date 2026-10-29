use hangar_server::migration_branch::Branch;
use serde_json::{Value,json};
use std::{collections::VecDeque,time::Duration};
#[test]
fn migration_branch_matches_captured_lifetime_and_retry() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/migration-branch.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut branch=Branch::default();
        let mut responses:VecDeque<_>=case["responses"].as_array().unwrap().iter().collect();
        let mut calls=vec![];
        let values:Vec<_>=case["times"].as_array().unwrap().iter().map(|at| {
            branch.resolve_with(Duration::from_secs(at.as_u64().unwrap()),case["manifest"]["channel"].as_str(),|args,timeout| {
                calls.push(json!({"args":args,"timeout":timeout.as_secs()}));
                let row=responses.pop_front().unwrap();Ok((row[0].as_i64().unwrap() as i32,row[1].as_str().unwrap().into()))
            })
        }).collect();
        assert_eq!(json!(values),case["values"],"{}",case["name"]);
        assert_eq!(json!(calls),case["calls"],"{}",case["name"]);
    }
}

#[test]
fn migration_branch_services_are_isolated_and_freeze_the_first_valid_manifest() {
    let root=tempfile::tempdir().unwrap();
    let manifest=|channel:&str|json!({"version":"1","commit":"fixture","channel":channel}).to_string();
    std::fs::write(root.path().join("manifest.json"),manifest("beta")).unwrap();
    let first=hangar_server::migration_branch::Service::default();
    let env=std::collections::BTreeMap::new();
    assert_eq!(first.get(root.path(),&env,Duration::ZERO),Some("beta".into()));
    std::fs::write(root.path().join("manifest.json"),manifest("changed")).unwrap();
    assert_eq!(first.get(root.path(),&env,Duration::from_secs(120)),Some("beta".into()));
    assert_eq!(hangar_server::migration_branch::Service::default().get(root.path(),&env,Duration::ZERO),Some("changed".into()));
}

proptest::proptest! {
    #[test]
    fn migration_branch_freezes_success_and_strips_python_whitespace(value in "[a-z][a-z0-9/_-]{0,100}") {
        let mut branch=Branch::default();
        let padded=format!("\u{1c} {value}\n\u{85}");
        proptest::prop_assert_eq!(branch.resolve_with(Duration::ZERO,None,|_,_|Ok((0,padded.clone()))),Some(value.clone()));
        proptest::prop_assert_eq!(branch.resolve_with(Duration::from_secs(1000),None,|_,_|panic!("branch congelada não deve executar git")),Some(value));
    }
}
