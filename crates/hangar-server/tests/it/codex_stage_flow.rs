use hangar_server::{accounts::environment::AccountEnvironment,codex_setup::{stage_flow::{self,Actions,Configuration},phase::{Error,State}}};
use serde_json::{Value,json};
use std::path::{Path,PathBuf};
fn clean(value:Value,root:&Path)->Value{
 match value{Value::String(s)=>json!(s.replace(root.to_str().unwrap(),"ROOT")),Value::Array(items)=>Value::Array(items.into_iter().map(|v|clean(v,root)).collect()),Value::Object(items)=>Value::Object(items.into_iter().map(|(k,v)|(k.replace(root.to_str().unwrap(),"ROOT"),clean(v,root))).collect()),value=>value}
}
fn files(root:&Path)->Value{
 let mut result=serde_json::Map::new();let mut pending=vec![root.to_path_buf()];
 while let Some(path)=pending.pop(){if !path.exists(){continue;}for entry in std::fs::read_dir(path).unwrap(){let path=entry.unwrap().path();if path.is_dir(){pending.push(path);}else{result.insert(path.strip_prefix(root).unwrap().to_str().unwrap().into(),json!(std::fs::read(path).unwrap().iter().map(|b|format!("{b:02x}")).collect::<String>()));}}}
 Value::Object(result)
}
struct Fake{input:Value,root:PathBuf,account:PathBuf,events:Vec<Value>,fingerprints:usize}
impl Actions for Fake{
 async fn history(&mut self)->Result<Value,Error>{self.events.push(json!(["history"]));Ok(json!({"mcp_servers":[],"agents":[],"commands":[]}))}
 fn fingerprint(&mut self)->Result<String,Error>{self.fingerprints+=1;self.events.push(json!(["fingerprint",{"fontes":true}]));Ok(if self.input["changed"]==true&&self.fingerprints>1{"changed"}else{"same"}.into())}
 async fn open(&mut self,home:&Path,account:&Path,memory:bool)->Result<(),Error>{
  self.account=account.to_path_buf();self.events.push(json!(["open",{"memoria":memory}]));self.events.push(json!(["stage",files(home)]));
  std::fs::write(account.join("config.toml"),self.input["native_config"].as_str().unwrap_or("").replace("@CODEX@",account.to_str().unwrap())).unwrap();Ok(())
 }
 async fn detect(&mut self)->Result<Vec<Value>,Error>{self.events.push(json!(["detect"]));Ok(self.input["items"].as_array().cloned().unwrap_or_default())}
 async fn import(&mut self,items:Vec<Value>)->Result<Value,Error>{
  self.events.push(json!(["import",items]));if self.input["native_error"]==true{return Err(Error{kind:"CodexNativoErro",message:"fixture".into()});}
  if let Some(outputs)=self.input["outputs"].as_object(){for(name,text)in outputs{let path=self.account.join(name);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,text.as_str().unwrap()).unwrap();}}
  Ok(json!({"itemTypeResults":[{"failures":if self.input["failures"]==true{json!(["fixture"])}else{json!([])}}]}))
 }
 async fn close(&mut self)->Result<(),Error>{self.events.push(json!(["close"]));Ok(())}
 async fn configure(&mut self,config:Configuration<'_>,registry:&mut Value)->Result<(),Error>{self.events.push(clean(json!(["config",config.mcp,config.agents,{"hooks":config.hooks,"memoria":config.memory,"registro":registry,"historico":config.history,"env":config.env}]),&self.root));Ok(())}
 fn hooks(&mut self,value:&Value,_registry:&mut Value)->Result<(),Error>{self.events.push(clean(json!(["hooks",value]),&self.root));Ok(())}
 fn checkpoint(&mut self,registry:&mut Value)->Result<(),Error>{self.events.push(clean(json!(["checkpoint",registry]),&self.root));Ok(())}
}
#[cfg(unix)]
#[tokio::test]
async fn codex_service_stage_flow_matches_original_order_files_and_partial_failures(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/stage-flow.json")).unwrap();let mut differences=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let home=root.join("home");let account=root.join("account");let work=root.join("work");let backups=work.join("backups");
  for path in [home.join(".claude"),account.clone(),work.clone()]{std::fs::create_dir_all(path).unwrap();}
  let input=&case["input"];
  if input["missing"]!=true{std::fs::write(home.join(".claude/settings.json"),serde_json::to_vec(input.get("settings").unwrap_or(&json!({}))).unwrap()).unwrap();}
  if let Some(mcp)=input.get("mcp"){std::fs::write(home.join(".claude.json"),serde_json::to_vec(mcp).unwrap()).unwrap();}
  if let Some(files)=input["files"].as_object(){for(name,text)in files{let path=home.join(".claude").join(name);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,text.as_str().unwrap()).unwrap();}}
  let env=AccountEnvironment::from_map([("HOME".into(),home.to_string_lossy().into_owned()),("PATH".into(),String::new())].into());
  let mut fake=Fake{input:input.clone(),root:root.into(),account:PathBuf::new(),events:vec![],fingerprints:0};let state=State::new(json!({"avisos":[]}));let mut registry=json!({});
  let result=stage_flow::run(&stage_flow::Paths{env:&env,home:&home,account:&account,work:&work,backups:&backups},&mut registry,&state,input["memory"]==true,&mut fake).await;
  let mut actual=match result{Ok(())=>json!({"ok":true}),Err(error)=>json!({"error":error.kind,"message":error.message})};
  actual["events"]=json!(fake.events);actual["registry"]=registry;actual["warnings"]=state.snapshot()["avisos"].clone();actual["files"]=files(&account);let actual=clean(actual,root);
  if actual!=case["result"]{differences.push(json!({"case":input["name"],"actual":actual,"expected":case["result"]}));}
 }
 assert!(differences.is_empty(),"{differences:#?}");
}
