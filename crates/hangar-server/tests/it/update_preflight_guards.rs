//! Recusas anteriores ao primeiro efeito no checkout usam dependências e Git isolados.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::{Value,json};
use std::{os::unix::fs::PermissionsExt,path::Path};
fn git(root:&Path,args:&[&str])->String{let output=std::process::Command::new("/usr/bin/git").arg("-C").arg(root).args(args).env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").env("GIT_AUTHOR_NAME","Fixture").env("GIT_AUTHOR_EMAIL","fixture@example.invalid").env("GIT_COMMITTER_NAME","Fixture").env("GIT_COMMITTER_EMAIL","fixture@example.invalid").output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));String::from_utf8(output.stdout).unwrap().trim().into()}
struct Unused;impl Source for Unused{fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("recusa anterior à rede");}}
#[test]
fn update_preflight_guards_match_original_without_touching_checkout(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/preflight-guards.json")).unwrap();
 for row in corpus["cases"].as_array().unwrap(){
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let bin=temp.path().join("bin");std::fs::create_dir(&root).unwrap();std::fs::create_dir(&bin).unwrap();git(&root,&["init","-b","main"]);std::fs::write(root.join("a.txt"),"um\n").unwrap();git(&root,&["add","."]);git(&root,&["commit","-m","primeiro"]);
  let missing=if name=="test_dependencia_faltando_recusa"{"uv"}else if name=="test_dependencia_faltando_nao_toca_no_repo"{"npm"}else{""};
  std::os::unix::fs::symlink("/usr/bin/git",bin.join("git")).unwrap();for name in ["node","npm","uv"]{if name!=missing{let path=bin.join(name);std::fs::write(&path,"#!/bin/sh\nexit 0\n").unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();}}
  if name=="test_branch_de_trabalho_recusa_sem_tocar_no_repo"{git(&root,&["checkout","-b","mobile-expo"]);}
  let service=Service::new(temp.path().join("home"),root.clone(),temp.path().join("claude"),[("PATH".into(),bin.display().to_string()),("CP_UPDATE_BRANCH".into(),"".into()),("CP_RUST_SERVER".into(),"0".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into())].into());
  let before=git(&root,&["show-ref"]);let mut actual=vec![];
  for call in row["calls"].as_array().unwrap(){
   let value=if call["method"]=="checar"{if call["result"]["branch"]=="master"{git(&root,&["checkout","-b","master"]);}service.preflight().unwrap()}else{service.execute_with_preparer(1,&Unused,None,&|_,_,_,_,_,_|panic!("recusa anterior à preparação")).into_value()};
   let result=call["result"].as_object().unwrap().keys().map(|key|(key.clone(),value.get(key).cloned().unwrap_or(Value::Null))).collect::<serde_json::Map<_,_>>();actual.push(json!({"method":call["method"],"result":result}));
  }
  assert_eq!(actual,*row["calls"].as_array().unwrap(),"{name}");assert_eq!(git(&root,&["rev-parse","--abbrev-ref","HEAD"]),row["branch"].as_str().unwrap());assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(),"um\n");if name!="test_main_e_master_seguem_atualizando"{assert_eq!(git(&root,&["show-ref"]),before,"{name}: referências alteradas");}
 }
}
