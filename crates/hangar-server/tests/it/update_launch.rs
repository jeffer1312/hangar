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
#[test]
fn update_expected_branch_never_ignores_unreadable_live_source(){
 let temp=tempfile::tempdir().unwrap();let mut service=service(temp.path(),"unreadable");service.environment.remove("CP_UPDATE_BRANCH");
 std::fs::create_dir(service.root.join("backend/.env")).unwrap();
 let recorder=Recorded{name:"must-not-spawn",service:&service,seen:Mutex::new(vec![])};
 assert!(service.start(8765,Some("main"),false,&recorder).is_err());
 assert!(recorder.seen.lock().unwrap().is_empty());
 assert!(!service.state_dir.join("rodando.lock").exists());
}
