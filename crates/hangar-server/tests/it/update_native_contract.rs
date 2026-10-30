//! Instalação nativa mantém consulta, recusa e diário do atualizador original.
#![cfg(unix)]
use hangar_server::{accounts::environment::AccountEnvironment,update::git_engine::{Service,releases::{Source,DownloadError}}};
use serde_json::{Value,json};
use std::{sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::Duration,io::{Read,Write}};
struct Manifest { input:Value,calls:Arc<Mutex<Vec<Value>>> }
impl Source for Manifest {
 fn get(&self,url:&str,timeout:Duration)->Result<Vec<u8>,DownloadError>{
  assert_eq!(url,"https://native.example/release/native-latest.json");self.calls.lock().unwrap().push(json!({"source":"manifest","deadline":timeout.as_secs_f64()}));
  if self.input["source"]=="manifest" {match self.input["failure"].as_str().unwrap(){"503"=>return Err(DownloadError { kind: "URLError",status:Some(503),message:"HTTP 503".into()}),"unreadable"=>return Ok(b"{quebrado".to_vec()),"not-object"=>return Ok(b"[]".to_vec()),_=>unreachable!()}}
  Ok(if self.input["required"]=="missing"{json!({})}else{json!({"requires_api":self.input.get("required").cloned().unwrap_or(json!(2))})}.to_string().into_bytes())
 }
}
#[test]
fn update_native_contract_keeps_requests_commands_and_diagnostics() {
 let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_native_contract::update_native_contract_child","--ignored","--nocapture"]).output().unwrap();
 assert!(output.status.success(),"{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
#[test]
#[ignore="Instalador falso isolado de fork concorrente."]
fn update_native_contract_child(){
 use std::os::unix::fs::PermissionsExt;
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/native-update.json")).unwrap();let mut differences=vec![];
 for row in corpus["cases"].as_array().unwrap(){
  let input=&row["input"];let name=input["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path().join("root");let home=temp.path().join("home");let marker=home.join(".hangar/native/release.json");std::fs::create_dir_all(marker.parent().unwrap()).unwrap();std::fs::create_dir_all(root.join("scripts")).unwrap();
  if input.get("marker")!=Some(&Value::Null){std::fs::write(&marker,input["marker"].as_str().unwrap_or("{}")).unwrap();}
  let calls=Arc::new(Mutex::new(vec![]));let stop=Arc::new(AtomicBool::new(false));let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let port=listener.local_addr().unwrap().port();listener.set_nonblocking(true).unwrap();let source=Manifest{input:input.clone(),calls:calls.clone()};let serving=input.clone();let recorded=calls.clone();let done=stop.clone();
  let server=std::thread::spawn(move||{while !done.load(Ordering::Acquire){match listener.accept(){Ok((mut socket,_))=>{socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();let mut bytes=[0;4096];let n=socket.read(&mut bytes).unwrap();let request=String::from_utf8_lossy(&bytes[..n]);assert!(request.contains("/api/client-api"));assert!(request.to_lowercase().contains("authorization: bearer fixture-token"));recorded.lock().unwrap().push(json!({"source":"api"}));let mut status="200 OK";let body=if serving["source"]=="api"{match serving["failure"].as_str().unwrap(){"503"=>{status="503 Unavailable";"{}".into()},"404"=>{status="404 Not Found";"{}".into()},"unreadable"=>"{quebrado".into(),"not-object"=>"[]".into(),_=>unreachable!()}}else{json!({"api":serving.get("api").cloned().unwrap_or(json!(2))}).to_string()};let response=format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());socket.write_all(response.as_bytes()).unwrap();break;},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(2)),Err(e)=>panic!("{e}")}}});
  let trace=temp.path().join("native-call");let script=root.join("scripts/install-native.sh");std::fs::write(&script,"#!/bin/sh\nprintf called > \"$NATIVE_CALL\"\n").unwrap();std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o755)).unwrap();
  let environment:std::collections::BTreeMap<String,String>=[("HOME".into(),home.display().to_string()),("CP_AUTH_TOKEN".into(),"fixture-token".into()),("HANGAR_NATIVE_UPDATE_URL".into(),"https://native.example/release".into()),("NATIVE_CALL".into(),trace.display().to_string())].into();let diary=hangar_server::diag::diary_dir_in(&AccountEnvironment::from_map(environment.clone())).unwrap();let service=Service::new(home,root.clone(),temp.path().join("claude"),environment);
  if let Some(command)=row["commands"].as_array().unwrap().first(){let actual=service.native_command(input["windows"].as_bool().unwrap_or(false)).iter().map(|v|v.replace(root.to_str().unwrap(),"<ROOT>")).collect::<Vec<_>>();assert_eq!(json!(actual),command["args"],"{name}");}
  service.update_native(input["target"].as_str().unwrap_or("main"),port,&source);stop.store(true,Ordering::Release);server.join().unwrap();
  let expected_requests=row["requests"].as_array().unwrap().iter().map(|request|if request["source"]=="manifest"{json!({"source":"manifest","deadline":row["downloads"][0]["deadline"].as_f64().unwrap()})}else{json!({"source":"api"})}).collect::<Vec<_>>();
  if *calls.lock().unwrap()!=expected_requests{differences.push(format!("{name}: ordem de consultas {:?} != {expected_requests:?}",calls.lock().unwrap()));}
  assert_eq!(trace.exists(),!row["commands"].as_array().unwrap().is_empty(),"{name}");assert!(!hangar_server::machine_peers::store::truthy(service.state().read().get("avisos").unwrap_or(&Value::Null)));
  let mut events=vec![];if let Ok(files)=std::fs::read_dir(diary){for file in files{for line in std::fs::read_to_string(file.unwrap().path()).unwrap().lines(){let mut event:Value=serde_json::from_str(line).unwrap();for key in ["ts","backend","pid_backend","origem"]{event.as_object_mut().unwrap().remove(key);}events.push(event);}}}
  if events!=*row["events"].as_array().unwrap(){differences.push(format!("{name}: diário {events:?} != {}",row["events"]));}
 }
 assert!(differences.is_empty(),"{}",differences.join("\n"));
}
