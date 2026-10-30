//! As trocas de canal preservam o trabalho local e as referências de resgate do original.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::{Value,json};
use std::{path::Path,os::unix::fs::PermissionsExt,collections::BTreeMap};
use super::update_local_dist::git;
use base64::Engine;
struct Unused;impl Source for Unused{fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("canal sem Rust não consulta release");}}
fn files(root:&Path,current:&Path,result:&mut BTreeMap<String,String>){for entry in std::fs::read_dir(current).unwrap(){let entry=entry.unwrap();if entry.file_name()==".git"{continue;}let path=entry.path();if path.is_dir(){files(root,&path,result);}else{result.insert(path.strip_prefix(root).unwrap().to_string_lossy().into_owned(),base64::engine::general_purpose::STANDARD.encode(std::fs::read(path).unwrap()));}}}
fn normalize(value:&Value)->Value{let text=serde_json::to_string(value).unwrap();let pattern=regex::Regex::new(r"resgate/\d{4}-\d{2}-\d{2}-\d{4}").unwrap();serde_json::from_str(&pattern.replace_all(&text,"resgate/<DATA>")).unwrap()}
#[test]
fn update_channel_pipeline_preserves_dirty_files_local_commits_and_rescue_refs(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/channel-pipeline.json")).unwrap();let mut errors=vec![];
 for row in fixture["cases"].as_array().unwrap(){let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let origin=temp.path().join("origin");let bin=temp.path().join("bin");std::fs::create_dir(&bin).unwrap();
  for(key,path)in [("repo",&root),("origin",&origin)]{let bundle=temp.path().join(format!("{key}.bundle"));std::fs::write(&bundle,base64::engine::general_purpose::STANDARD.decode(row["bundles"][key].as_str().unwrap()).unwrap()).unwrap();let mut args=vec!["clone"];if key=="origin"{args.push("--bare");}args.extend([bundle.to_str().unwrap(),path.to_str().unwrap()]);git(temp.path(),&args);}
  for reference in git(&root,&["for-each-ref","--format=%(refname)","refs/remotes"]).lines(){git(&root,&["update-ref","--no-deref","-d",reference]);}
  for line in row["before"]["refs"].as_array().unwrap(){let line=line.as_str().unwrap();let (reference,sha)=line.split_once(' ').unwrap();git(&root,&["update-ref",reference,sha]);}
  git(&root,&["remote","set-url","origin",origin.to_str().unwrap()]);git(&root,&["checkout","-B",row["before"]["branch"].as_str().unwrap(),row["before"]["head"].as_str().unwrap()]);git(&root,&["config","user.name","Fixture"]);git(&root,&["config","user.email","fixture@example.invalid"]);
  let branches=git(&root,&["for-each-ref","--format=%(refname:short)","refs/heads"]);for branch in branches.lines(){let _=std::process::Command::new("/usr/bin/git").arg("-C").arg(&root).args(["config","--remove-section",&format!("branch.{branch}")]).output().unwrap();}
  for setting in row["before"]["config"].as_array().unwrap(){let (key,value)=setting.as_str().unwrap().split_once(' ').unwrap();git(&root,&["config",key,value]);}

  for(name,bytes)in row["before"]["files"].as_object().unwrap(){let path=root.join(name);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,base64::engine::general_purpose::STANDARD.decode(bytes.as_str().unwrap()).unwrap()).unwrap();}
  for name in ["uv","node","npm","tmux","systemctl"]{let path=bin.join(name);std::fs::write(&path,if name=="systemctl"{"#!/bin/sh\nexit 1\n"}else{"#!/bin/sh\nexit 0\n"}).unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();}
  let home=temp.path().join("home");let service=Service::new(home.clone(),root.clone(),temp.path().join("claude"),[("HOME".into(),home.display().to_string()),("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("CP_RUST_SERVER".into(),"0".into()),("CP_UPDATE_BRANCH".into(),row["target"].as_str().unwrap().into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into())].into());
  std::fs::create_dir_all(&service.state_dir).unwrap();if let Some(marker)=row["before"]["marker"].as_str(){std::fs::write(service.state_dir.join("branch"),base64::engine::general_purpose::STANDARD.decode(marker).unwrap()).unwrap();}
  let actual=service.execute_with_preparer(1,&Unused,None,&|_,_,_,_,_,_|Ok(()));let state:serde_json::Map<_,_>=row["state"].as_object().unwrap().keys().map(|key|(key.clone(),actual.get(key).cloned().unwrap_or(Value::Null))).collect();
  if normalize(&json!(state))!=normalize(&row["state"]){errors.push(format!("{name}: estado {:?} != {}",state,row["state"]));}
  let mut current=BTreeMap::new();files(&root,&root,&mut current);if let Some(lock)=current.remove("backend/.env.lock"){assert_eq!(lock,"","trava nativa não contém dados");assert_eq!(git(&root,&["check-ignore","backend/.env.lock"]),"backend/.env.lock","trava fica fora de stash e publicação");}assert_eq!(json!(current),row["after"]["files"],"{name}: bytes do checkout");
  for(key,actual)in [("head",git(&root,&["rev-parse","HEAD"])),("branch",git(&root,&["rev-parse","--abbrev-ref","HEAD"]))]{assert_eq!(actual,row["after"][key].as_str().unwrap(),"{name}: {key}");}
  let marker=std::fs::read(service.state_dir.join("branch")).ok().map(|bytes|base64::engine::general_purpose::STANDARD.encode(bytes));assert_eq!(json!(marker),row["after"]["marker"],"{name}: marcador");
  let refs=git(&root,&["for-each-ref","--format=%(refname) %(objectname)","refs/heads"]);let refs=refs.lines().map(str::to_owned).collect::<Vec<_>>();let expected=row["after"]["refs"].as_array().unwrap().iter().filter(|v|v.as_str().unwrap().starts_with("refs/heads/")).cloned().collect::<Vec<_>>();assert_eq!(normalize(&json!(refs)),normalize(&json!(expected)),"{name}: referências de resgate");
  assert_eq!(json!(git(&root,&["stash","list","--format=%gd"]).lines().collect::<Vec<_>>()),row["after"]["stash"],"{name}: stash");
 }
 assert!(errors.is_empty(),"{}",errors.join("\n"));
}
