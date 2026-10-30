#![cfg(target_os="linux")]
use hangar_server::update::{executor::Spawner,git_engine::Service};
use serde_json::{Value,json};
use std::{sync::Mutex,os::unix::fs::PermissionsExt};
struct Recorded<'a>{name:&'a str,service:&'a Service,seen:Mutex<Vec<Value>>}
impl Spawner for Recorded<'_>{
 fn spawn(&self,args:&[String])->std::io::Result<u32>{
  let mut observed=json!({"args":args,"state":null});observed["state"]=self.service.state().read().into_value();self.seen.lock().unwrap().push(observed);
  if self.name.contains("spawn-failed"){return Err(std::io::Error::other("fixture-spawn-failed"));}
  if self.name=="child-progress"{self.service.state().write(json!({"etapa":"resguardar","passo":1,"texto":"Guardando"}));}
  Ok(std::process::id())
 }
}
fn service(root:&std::path::Path,name:&str)->Service{
 let checkout=root.join("repo");let home=root.join("home");let bin=root.join("bin");
 std::fs::create_dir_all(checkout.join("backend")).unwrap();std::fs::create_dir_all(&bin).unwrap();
 let command=bin.join("systemctl");std::fs::write(&command,if name=="manual-restart"{"#!/bin/sh\nexit 1\n"}else{"#!/bin/sh\nexit 0\n"}).unwrap();std::fs::set_permissions(command,std::fs::Permissions::from_mode(0o755)).unwrap();
 Service::new(home.clone(),checkout,home.join(".claude"),[("PATH".into(),bin.display().to_string()),("HOME".into(),home.display().to_string()),("CP_UPDATE_BRANCH".into(),if name=="channel-changed"{"beta"}else{""}.into())].into())
}
fn normalize(value:&mut Value){
 if let Some(map)=value.as_object_mut(){
  if map.get("pid")==Some(&json!(std::process::id())){map.insert("pid".into(),json!(424242));}
  if map.get("request_id").is_some_and(Value::is_string){map.insert("request_id".into(),json!("fixture-request"));}
 }
}
#[test]
fn update_launch_matches_original_state_lock_and_child_order(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/launch.json")).unwrap();
 for row in fixture["cases"].as_array().unwrap(){
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let service=service(temp.path(),name);let lock=service.state_dir.join("rodando.lock");
  if name.ends_with("lock"){std::fs::create_dir_all(&service.state_dir).unwrap();std::fs::write(&lock,if name=="live-lock"{std::process::id().to_string()}else{"999999999".into()}).unwrap();}
  let recorder=Recorded{name,service:&service,seen:Mutex::new(vec![])};
  let result=if name=="transfer-failed" {
   service.start_with_owner_writer(8765,None,false,&recorder,&|_,_|Err(std::io::Error::other("fixture-transfer-failed")))
  }else{service.start(8765,(name=="channel-changed").then_some("main"),name.contains("restart"),&recorder)};
  if name=="transfer-failed" {
   assert_eq!(row["result"]["value"]["ok"],true,"D62: captura original anuncia sucesso após falha de transferência");
   assert_eq!(result.unwrap_err(),"update_lock_unavailable: fixture-transfer-failed");
   assert!(!lock.exists());
   assert_eq!(service.state().read()["ok"],false);
   continue;
  }
  if row["result"].get("exception").is_some(){assert_eq!(result.unwrap_err(),row["result"]["message"].as_str().unwrap());}
  else {let mut result=result.unwrap();normalize(&mut result);assert_eq!(result,row["result"]["value"],"{name}");}
  if name=="spawn-failed" {
   assert_eq!(row["lock"],"<PARENT>","D53: o original deixava a vez presa");
   assert!(!lock.exists());let state=service.state().read();assert_eq!(state["fase"],"pronto");assert_eq!(state["ok"],false);assert_eq!(state["erro"],"não consegui lançar o processo da atualização");
   let retry=Recorded{name:"retry",service:&service,seen:Mutex::new(vec![])};assert_eq!(service.start(8765,None,false,&retry).unwrap()["ok"],true);
   continue;
  }
  let actual_lock=std::fs::read_to_string(&lock).ok().map(|value|if value==std::process::id().to_string(){"424242".into()}else{value});assert_eq!(json!(actual_lock),row["lock"],"{name}");
  let mut state=service.state().read().into_value();normalize(&mut state);
  for (key,expected) in row["state"].as_object().unwrap(){assert_eq!(&state[key],expected,"{name}:{key}");}
  let calls=recorder.seen.into_inner().unwrap();let expected_calls=row["events"].as_array().unwrap().iter().filter(|event|event[0]=="spawn").collect::<Vec<_>>();assert_eq!(calls.len(),expected_calls.len());
  for (call,expected) in calls.iter().zip(expected_calls){
   assert_eq!(call["args"][0],"checkout-update");assert_eq!(call["args"][4],"8765");
   let mut before=call["state"].clone();normalize(&mut before);
   for (key,value) in expected[1]["state_before_spawn"].as_object().unwrap(){assert_eq!(&before[key],value,"{name}: antes de spawn:{key}");}
  }
 }
}
#[test]
fn update_expected_branch_never_ignores_unreadable_live_source(){
 let temp=tempfile::tempdir().unwrap();let mut service=service(temp.path(),"unreadable");service.environment.remove("CP_UPDATE_BRANCH");
 std::fs::create_dir(service.root.join("backend/.env")).unwrap();
 let recorder=Recorded{name:"must-not-spawn",service:&service,seen:Mutex::new(vec![])};
 assert!(service.start(8765,Some("main"),false,&recorder).is_err());
 assert!(recorder.seen.lock().unwrap().is_empty());
 assert!(!service.state_dir.join("rodando.lock").exists());
}
