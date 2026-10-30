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

#[cfg(unix)]
#[tokio::test]
async fn codex_service_plugin_selection_persists_through_the_real_writer() {
 use hangar_server::accounts::environment::AccountEnvironment;
 use std::os::unix::fs::PermissionsExt;
 let root=tempfile::tempdir().unwrap();let account=root.path().join("account");let bin=root.path().join("bin");
 std::fs::create_dir(&account).unwrap();std::fs::create_dir(&bin).unwrap();let script=bin.join("codex");
 std::fs::write(&script,r##"#!/bin/sh
while IFS= read -r message; do
 id=$(printf '%s' "$message" | /usr/bin/sed -n 's/.*"id":\([0-9]*\).*/\1/p')
 case "$message" in
 *'"method":"config/batchWrite"'*)
  printf '%s' "$message" > "$FIXTURE_REQUEST"
  printf 'model = "personal"\n[plugins."a@m"]\nenabled = true\n' > "$CODEX_HOME/config.toml"
  printf '{"id":%s,"result":{}}\n' "$id";;
 *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id";;
 esac
done
"##).unwrap();std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o700)).unwrap();
 let request=root.path().join("request.json");let env=AccountEnvironment::from_pairs([("HOME",root.path().to_string_lossy().into_owned()),("PATH",bin.to_string_lossy().into_owned()),("FIXTURE_REQUEST",request.to_string_lossy().into_owned())]);
 let original="model = \"personal\"\n[plugins.\"a@m\"]\nenabled = false\n";std::fs::write(account.join("config.toml"),original).unwrap();
 plugin_edits::apply(&env,&account,&root.path().join("backups"),&root.path().join("work"),&[("a@m".into(),true)]).await.unwrap();
 let sent:Value=serde_json::from_slice(&std::fs::read(request).unwrap()).unwrap();assert_eq!(sent["params"]["edits"],json!([{"keyPath":"\"plugins\".\"a@m\".\"enabled\"","value":true,"mergeStrategy":"replace"}]));
 let actual:Value=toml::from_str(&std::fs::read_to_string(account.join("config.toml")).unwrap()).unwrap();assert_eq!(actual["plugins"]["a@m"]["enabled"],true);assert_eq!(actual["model"],"personal");
 let backup=std::fs::read_dir(root.path().join("backups")).unwrap().next().unwrap().unwrap().path();let saved:Value=serde_json::from_slice(&std::fs::read(backup).unwrap()).unwrap();assert_eq!(saved["conteudo_hex"],original.as_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>());
}
