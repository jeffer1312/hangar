use hangar_server::codex_setup::{config_edits::{self,Sources},phase::State};
use serde_json::{Value,json};
#[test]
fn codex_service_config_edits_match_original_confirmation_and_types(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/config-edits.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let mut registry=json!({"mcp_servers":{"old":{"command":"old"}}});let state=State::new(json!({"avisos":[]}));let empty=json!({});let env=json!({"NEW":"source"});let limit=json!(40000);
  let sources=Sources{mcp:&empty,agents:&empty,registry:Some(&registry),history:&empty,env:Some(&env),limit:&limit,hooks:true,memory:true};
  let mut edits=vec![];
  let result=match config_edits::prepare(&case["current"],sources){Ok(mut plan)=>{edits=std::mem::take(&mut plan.edits);if case["confirm"]==true{plan.confirm(&mut registry,&state);}json!({"ok":null})},Err(error)=>json!({"error":error.kind,"message":error.message})};
  let actual=json!({"result":result,"edits":edits,"registry":registry,"warnings":state.snapshot()["avisos"]});let expected=json!({"result":case["result"],"edits":case["edits"],"registry":case["registry"],"warnings":case["warnings"]});
  if actual!=expected{failures.push(json!({"name":case["name"],"current":case["current"],"confirm":case["confirm"],"actual":actual,"expected":expected}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
