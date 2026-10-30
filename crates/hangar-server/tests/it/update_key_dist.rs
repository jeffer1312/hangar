//! Download da tela por chave: decisão, bytes e diário no mesmo serviço público.
#![cfg(unix)]
use hangar_server::{accounts::environment::AccountEnvironment,update::git_engine::{Service,releases::{self,Source,DownloadError}}};
use serde_json::{Value,json};
use std::{sync::Mutex,time::Duration};
use super::update_local_dist::git;
struct FixtureSource {name:String,data:Vec<u8>,calls:Mutex<Vec<Value>>}
impl Source for FixtureSource {
 fn get(&self,url:&str,timeout:Duration)->Result<Vec<u8>,DownloadError>{
  self.calls.lock().unwrap().push(json!({"url":url,"timeout":timeout.as_secs()}));
  if self.name=="key-missing" {return Err(DownloadError { kind: "URLError",status:Some(404),message:"HTTP Error 404: Not Found".into()});}
  if url.ends_with(".sha256") {return Ok(if self.name=="key-hash" {b"bad".to_vec()}else{releases::digest(&self.data).into_bytes()});}
  if self.name=="key-cut" {return Err(DownloadError { kind: "URLError",status:None,message:"IncompleteRead(3 bytes read, 7 more expected)".into()});}
  Ok(self.data.clone())
 }
}
#[test]
fn update_key_dist_matches_fallback_files_and_exported_diagnostic() {
 let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_key_dist::update_key_dist_child","--ignored","--nocapture"]).output().unwrap();
 assert!(output.status.success(),"{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
#[test]
#[ignore="Executável falso isolado dos demais testes."]
fn update_key_dist_child() {
 use std::os::unix::fs::PermissionsExt;
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/key-dist.json")).unwrap();let mut differences=Vec::new();
 for row in fixture["cases"].as_array().unwrap(){
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let home=temp.path().join("home");let bin=temp.path().join("bin");let calls=temp.path().join("calls");std::fs::create_dir_all(&root).unwrap();std::fs::create_dir(&bin).unwrap();
  for (folder,file) in [("frontend","a.ts"),("packages/core","b.ts"),("messages","en.json"),("project.inlang","settings.json")] {std::fs::create_dir_all(root.join(folder)).unwrap();std::fs::write(root.join(folder).join(file),file).unwrap();}
  for file in ["package.json","package-lock.json"] {std::fs::write(root.join(file),"{}").unwrap();}
  std::fs::write(root.join(".gitignore"),"frontend/dist/\nfrontend/.dist-velho/\n").unwrap();let dist=root.join("frontend/dist");std::fs::create_dir(&dist).unwrap();std::fs::write(dist.join("index.html"),"old-screen").unwrap();
  git(&root,&["init","-b","main"]);git(&root,&["add","."]);git(&root,&["commit","-m","old"]);let old=git(&root,&["rev-parse","HEAD"]);std::fs::write(root.join("frontend/a.ts"),"next").unwrap();git(&root,&["add","."]);git(&root,&["commit","-m","new"]);git(&root,&["update-ref","refs/remotes/origin/main","HEAD"]);git(&root,&["reset","--hard",&old]);
  std::os::unix::fs::symlink("/usr/bin/git",bin.join("git")).unwrap();let npm=bin.join("npm");std::fs::write(&npm,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CALLS\"\nprintf local-screen > frontend/dist/index.html\n").unwrap();std::fs::set_permissions(npm,std::fs::Permissions::from_mode(0o755)).unwrap();
  let environment:std::collections::BTreeMap<String,String>=[("HOME".into(),home.display().to_string()),("USERPROFILE".into(),home.display().to_string()),("PATH".into(),bin.display().to_string()),("CALLS".into(),calls.display().to_string()),("CP_UPDATE_BRANCH".into(),"".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into())].into();let diary=hangar_server::diag::diary_dir_in(&AccountEnvironment::from_map(environment.clone())).unwrap();let service=Service::new(home,root.clone(),temp.path().join("claude"),environment);
  let encoder=flate2::write::GzEncoder::new(Vec::new(),flate2::Compression::default());let mut tar=tar::Builder::new(encoder);let mut header=tar::Header::new_gnu();header.set_size(10);header.set_mode(0o644);header.set_cksum();tar.append_data(&mut header,if name=="key-incomplete"{"other.html"}else{"index.html"},&b"new-screen"[..]).unwrap();let data=tar.into_inner().unwrap().finish().unwrap();let source=FixtureSource{name:name.into(),data:if name=="key-truncated"{b"not a gzip".to_vec()}else{data},calls:Mutex::new(vec![])};
  assert_eq!(json!(service.update_dist(&source).unwrap()),row["warning"],"{name}");assert_eq!(*source.calls.lock().unwrap(),*row["urls"].as_array().unwrap(),"{name}");
  let actual_calls=std::fs::read_to_string(&calls).ok().map(|raw|vec![json!({"args":std::iter::once("npm").chain(raw.lines()).collect::<Vec<_>>()})]).unwrap_or_default();let expected_calls=row["commands"].as_array().unwrap().iter().map(|call|json!({"args":call["args"]})).collect::<Vec<_>>();assert_eq!(actual_calls,expected_calls,"{name}");assert_eq!(std::fs::read_to_string(dist.join("index.html")).unwrap(),row["screen"].as_str().unwrap(),"{name}");assert_eq!(json!(std::fs::read_to_string(root.join("frontend/.dist-velho/index.html")).ok()),row["previous"],"{name}");
  assert!(!std::fs::read_dir(root.join("frontend")).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with(".dist-baixado.")),"{name}: temporário órfão");
  let mut actual_events=Vec::new();if let Ok(files)=std::fs::read_dir(&diary){for file in files {for line in std::fs::read_to_string(file.unwrap().path()).unwrap().lines(){let mut value:Value=serde_json::from_str(line).unwrap();for key in ["ts","backend","pid_backend","origem"]{value.as_object_mut().unwrap().remove(key);}actual_events.push(value);}}}
  let mut expected_events=row["events"].as_array().unwrap().iter().map(|event|{let mut value=event["fields"].clone();value["evento"]=event["args"][0].clone();value["nivel"]=event["args"][1].clone();value.as_object_mut().unwrap().retain(|_,v|v.as_str()!=Some(""));value}).collect::<Vec<_>>();
  if name=="key-truncated" {assert_eq!(expected_events[0]["detalhe"],"ReadError('not a gzip file')");expected_events[0]["detalhe"]=json!("ui_archive_invalid: invalid gzip header");}
  if actual_events!=expected_events {differences.push(format!("{name}: diário {actual_events:?} != {expected_events:?}"));}
 }
 assert!(differences.is_empty(),"{}",differences.join("\n"));
}
