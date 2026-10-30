use hangar_server::codex_setup::phase::{self,State};
use serde_json::{Value,json};
struct Actions {name:String,outcome:String,reads:usize,calls:Vec<Value>}
impl phase::Actions for Actions {
 async fn fingerprint(&mut self,source:bool,_registry:&Value)->Result<String,phase::Error>{self.calls.push(json!(["fingerprint",source]));if source{self.reads+=1;Ok(if self.outcome=="source-changed"&&self.reads>1{"changed"}else{"source"}.into())}else{Ok("same".into())}}
 async fn execute(&mut self,_registry:&mut Value,state:&State)->Result<(),phase::Error>{self.calls.push(json!(["action"]));state.update(|value|{value["erros"].as_array_mut().unwrap().push(json!("new error"));value["avisos"].as_array_mut().unwrap().push(json!("new warning"));if self.name=="plugins"{value["plugins"]=json!(["new plugin"]);value["proxima_atualizacao"]=json!("new date");}});if self.name=="plugins"{state.set_confirmed(["a".into(),"b".into()].into());}if self.outcome=="error"{Err(phase::Error{kind:"RuntimeError",message:"fixture action".into()})}else{Ok(())}}
 fn now(&self)->f64{1000.0}
}
#[tokio::test]
async fn codex_service_phase_matches_original_reuse_invalidation_and_checkpoint(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/phases.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let name=case["phase"].as_str().unwrap();let mut registry=case["initial"].clone();let state=State::new(json!({"erros":["existing error"],"avisos":["existing warning"],"plugins":[],"proxima_atualizacao":null}));
  let mut actions=Actions{name:name.into(),outcome:case["outcome"].as_str().unwrap().into(),reads:0,calls:vec![]};
  let result=phase::run(name,&mut registry,&state,case["force"].as_bool().unwrap(),&mut actions).await;
  let result=match result{Ok(())=>json!({"ok":null}),Err(error)=>json!({"error":error.kind,"message":error.message})};
  let actual=json!({"result":result,"registry":registry,"state":state.snapshot(),"confirmed":state.confirmed(),"calls":actions.calls});
  let expected=json!({"result":case["result"],"registry":case["registry"],"state":case["state"],"confirmed":case["confirmed"],"calls":case["calls"]});
  if actual!=expected{failures.push(json!({"phase":name,"force":case["force"],"previous":case["previous"],"outcome":case["outcome"],"actual":actual,"expected":expected}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
