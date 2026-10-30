use hangar_server::{codex_setup::batch,accounts::catalog::Account};
use serde_json::{Value,json};
struct Actions {name:String,calls:Vec<Value>,sessions:Vec<Value>}
impl batch::Actions for Actions {
 async fn repair(&mut self,account:&Account)->Result<(),String>{self.calls.push(json!(["repair",account.id,{"adotar_preparo":true}]));if self.name=="repair-failure"{Err("fixture repair".into())}else{Ok(())}}
 fn sessions(&mut self)->Result<Vec<Value>,String>{self.calls.push(json!(["sessions",{"include_incomplete":true}]));Ok(self.sessions.clone())}
 fn alive(&self,_pid:&Value)->bool{self.name!="dead"}
 async fn reload(&mut self,endpoint:&str,cwd:&str,account:&Account)->Result<bool,String>{self.calls.push(json!(["reload",endpoint,cwd,{"recarregar":true,"codex_home":account.home}]));Ok(self.name!="reload-false")}
}
fn replace(value:&Value,old:&str,new:&str)->Value{serde_json::from_str(&value.to_string().replace(&old.replace('\\',"\\\\"),&new.replace('\\',"\\\\"))).unwrap()}
#[tokio::test]
async fn codex_service_batch_matches_original_selection_and_proof(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/repair-all.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let home=temp.path();let name=case["name"].as_str().unwrap();let account=home.join("account");std::fs::create_dir_all(&account).unwrap();
  let mut accounts=if name=="empty"{vec![]}else{vec![Account{id:"fixture".into(),home:account,is_default:false}]};
  if name=="missing-account-dir"{accounts.push(Account{id:"missing".into(),home:home.join("missing"),is_default:false});}
  let sessions=replace(&case["sessions"],"$ROOT",&home.to_string_lossy());let mut actions=Actions{name:name.into(),calls:vec![],sessions:sessions.as_array().unwrap().clone()};
  let result=batch::run(home,&accounts,&mut actions).await;
  let expected=case["result"].get("error").map(|_|Err(case["result"]["message"].as_str().unwrap().to_owned())).unwrap_or(Ok(()));
  let actual=json!({"calls":actions.calls,"proof":std::fs::read_to_string(home.join(".hangar/codex-prepare-once-v1.json")).ok()});let actual=crate::contract_paths::value(&actual,home,"$ROOT");
  if result!=expected || actual["calls"]!=case["calls"] || actual["proof"]!=case["proof"]{failures.push(json!({"name":name,"result":format!("{result:?}"),"expected":format!("{expected:?}"),"actual":actual,"case":case}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
