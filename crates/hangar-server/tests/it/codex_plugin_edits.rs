use hangar_server::codex_setup::plugin_edits;
use serde_json::{Value,json};

#[test]
fn codex_service_plugin_edits_match_original_types_order_and_empty_selection(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/plugin-edits.json")).unwrap();
 let mut failures=Vec::new();
 for(index,case)in fixture["cases"].as_array().unwrap().iter().enumerate(){
  let choices=case["choices"].as_object().unwrap().iter().map(|(id,value)|(id.clone(),value.as_bool().unwrap())).collect::<Vec<_>>();
  let actual=match plugin_edits::prepare(&case["current"],&choices){
   Ok(edits)=>json!({"ok":true,"calls":["edit_config",edits]}),
   Err(error)=>json!({"error":error.kind,"message":error.message,"calls":["edit_config"]}),
  };
  if actual!=case["result"]{failures.push(format!("caso {index}: {actual} != {}",case["result"]));}
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[tokio::test]
async fn codex_service_plugin_empty_selection_still_reads_shared_writer(){
 use hangar_server::accounts::environment::AccountEnvironment;
 use std::collections::BTreeMap;
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/plugin-edits.json")).unwrap();
 for case in corpus["file_cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let account=root.join(".codex");std::fs::create_dir(&account).unwrap();
  let env=AccountEnvironment::from_map(BTreeMap::from([("HOME".into(),root.to_string_lossy().into_owned()),("USERPROFILE".into(),root.to_string_lossy().into_owned()),("PATH".into(),String::new())]));
  let path=account.join("config.toml");let raw=case["toml"].as_str().unwrap();std::fs::write(&path,raw).unwrap();
  plugin_edits::apply(&env,&account,&root.join("backup"),&root.join("work"),&[]).await.unwrap();
  assert_eq!(std::fs::read_to_string(&path).unwrap(),raw);
  assert_eq!(std::fs::read_dir(root.join("work")).unwrap().count(),0);
 }
}
