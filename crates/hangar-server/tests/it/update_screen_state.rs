//! A consulta do estado conserva o nascimento e conclui o abandono no arquivo.
use hangar_server::update::git_engine::Service;
use serde_json::{Value,json};
#[test]
fn update_screen_state_preserves_original_owner_lifecycle() {
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/screen-state.json")).unwrap();
 for row in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let service=Service::new(temp.path().join("home"),temp.path().join("repo"),temp.path().join("claude"),Default::default());let mut input=row["input"].clone();
  if input["pid"]=="<SELF>"{input["pid"]=json!(std::process::id());}
  input["ts"]=json!((chrono::Local::now()-chrono::TimeDelta::seconds(if row["name"]=="recent-no-pid"{0}else{600})).to_rfc3339());
  std::fs::create_dir_all(&service.state_dir).unwrap();std::fs::write(service.state().path(),input.to_string()).unwrap();
  let mut result=service.screen_state().into_value();let mut stored:Value=serde_json::from_slice(&std::fs::read(service.state().path()).unwrap()).unwrap();
  for value in [&mut result,&mut stored]{
   if value["pid"]==std::process::id(){value["pid"]=json!("<SELF>");}
   value["ts"]=if value["fase"]=="pronto"{json!("updated")}else{row["input"]["ts"].clone()};
  }
  assert_eq!(result,row["result"],"{}: resposta",row["name"]);assert_eq!(stored,row["stored"],"{}: persistência",row["name"]);assert_eq!(service.state_dir.join("rodando.lock").exists(),row["lock_exists"].as_bool().unwrap());
 }
}
