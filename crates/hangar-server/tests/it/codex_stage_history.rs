use hangar_server::codex_setup::{stage_history,phase::State};
use serde_json::{Value,json};
#[test]
fn codex_service_stage_history_matches_original_provenance_and_failures(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/stage-history.json")).unwrap();let mut failures=Vec::new();
 for(index,case)in corpus["cases"].as_array().unwrap().iter().enumerate(){
  let state=State::new(json!({"avisos":[]}));
  let entries=if case["input"]=="native-error"{Err("fixture".to_owned())}else{Ok(case["input"].as_array().unwrap().as_slice())};
  let result=stage_history::collect(entries,&state,false);
  let snapshot=state.snapshot();let warnings=snapshot["avisos"].as_array().unwrap().iter().map(|v|json!({"codigo":v["codigo"]})).collect::<Vec<_>>();
  let actual=match result{Ok(value)=>json!({"ok":value,"warnings":warnings}),Err(error)=>json!({"error":error.kind,"message":error.message,"warnings":warnings})};
  if actual!=case["result"]{failures.push(format!("caso {index}: {actual} != {}",case["result"]));}
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}
