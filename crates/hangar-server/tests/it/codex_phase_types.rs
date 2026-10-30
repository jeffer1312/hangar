use hangar_server::codex_setup::phase::{self,State};
use serde_json::{Value,json};
struct Actions{calls:Vec<Value>}
impl phase::Actions for Actions{
 async fn fingerprint(&mut self,source:bool,_registry:&Value)->Result<String,phase::Error>{self.calls.push(json!(["fingerprint",source]));Ok("same".into())}
 async fn execute(&mut self,_registry:&mut Value,_state:&State)->Result<(),phase::Error>{panic!("checkpoint coincidente não executa a fase")}
 fn now(&self)->f64{1000.0}
}
#[tokio::test]
async fn codex_service_phase_types_match_original_partial_effect_order(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/phase-types.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let mut registry=case["initial"].clone();let state=State::new(json!({"erros":["existing error"],"avisos":["existing warning"],"plugins":[],"proxima_atualizacao":null}));let mut actions=Actions{calls:vec![]};
  let result=match phase::run("plugins",&mut registry,&state,false,&mut actions).await{Ok(())=>json!({"ok":null}),Err(error)=>json!({"error":error.kind,"message":error.message})};
  let actual=json!({"result":result,"registry":registry,"state":state.snapshot(),"confirmed":state.confirmed(),"calls":actions.calls});
  let expected=json!({"result":case["result"],"registry":case["registry"],"state":case["state"],"confirmed":case["confirmed"],"calls":case["calls"]});
  if actual!=expected{failures.push(json!({"field":case["field"],"value":case["value"],"actual":actual,"expected":expected}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
