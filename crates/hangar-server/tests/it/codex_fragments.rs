use hangar_server::codex_setup::fragments;
use serde_json::json;
proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(48))]
 #[test]
 fn codex_service_fragments_preserve_unmanaged_names_and_local_fields(name in "[a-z]{1,24}", text in ".{0,64}", number in proptest::num::u64::ANY) {
  proptest::prop_assume!(name!="managed");
  let current=json!({name.clone():text.clone(),"managed":{"local":number}});
  let source=json!({"managed":{"requested":true}});
  let merged=fragments::merge(&current,&source,&json!({}),&std::collections::BTreeSet::from(["managed".into()])).unwrap();
  proptest::prop_assert_eq!(&merged["merged"][&name],&json!(text));
  proptest::prop_assert_eq!(&merged["merged"]["managed"],&json!({"local":number,"requested":true}));
  proptest::prop_assert_eq!(&merged["manifest"],&source);
 }
}

/// D4.3: o espelho do Codex segue a entrada do principal. Desligada (fora do `mcpServers`) e já no
/// manifesto, ela sai do `config.toml`, mesmo com campo que só o Codex tem; reescrita (D4.2), o comando
/// novo chega lá.
#[test]
fn computer_control_entry_follows_the_main_file_into_codex_config() {
 use hangar_server::codex_setup::config_edits::{Sources,prepare};
 let name="hangar-computer-control";
 let entry=json!({"command":"/h/.hangar/computer-control/hangar-computer-control","args":[],"env":{"HCC_AGENTS_DIR":"/h/t"}});
 let other=json!({"command":"o"});
 let edits_for=|current:&serde_json::Value,source:&serde_json::Value|{
  let registry=json!({"mcp_servers":{name:entry.clone(),"outro":other.clone()},"agents":{}});
  let plan=prepare(current,Sources{mcp:source,agents:&json!({}),registry:Some(&registry),history:&json!({}),env:None,limit:&json!(32768),hooks:false,memory:false}).unwrap();
  let edit=plan.edits.iter().find(|edit|edit["keyPath"].as_str().is_some_and(|key|key.contains(name))).cloned();
  (edit,plan.manifest)
 };
 for local in [json!({}),json!({"startup_timeout_sec":30})] {
  let mut installed=entry.clone();installed.as_object_mut().unwrap().extend(local.as_object().unwrap().clone());
  let current=json!({"mcp_servers":{name:installed,"outro":other.clone()}});
  let (edit,manifest)=edits_for(&current,&json!({"outro":other.clone()}));
  assert_eq!(edit.map(|edit|edit["value"].clone()),Some(serde_json::Value::Null),"{local}");
  assert!(manifest["mcp_servers"].get(name).is_none(),"{manifest}");
 }
 let current=json!({"mcp_servers":{name:{"command":"uvx","args":["hcc"],"env":{"HCC_AGENTS_DIR":"/h/t"}},"outro":other.clone()}});
 let (edit,_)=edits_for(&current,&json!({name:entry.clone(),"outro":other.clone()}));
 assert_eq!(edit.map(|edit|edit["value"]["command"].clone()),Some(entry["command"].clone()));
}
