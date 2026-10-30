//! A URL publicada usa a chave exata das fontes e dependências calculada pelo Python e CI.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::Value;
use std::{sync::Mutex,os::unix::fs::PermissionsExt};
use base64::Engine;
use super::update_local_dist::git;
struct Missing(Mutex<Vec<String>>);
impl Source for Missing{fn get(&self,url:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{self.0.lock().unwrap().push(url.into());Err(DownloadError{kind:"HTTPError",status:Some(404),message:"HTTP Error 404: Not Found".into()})}}
#[test]
fn update_dist_key_tracks_only_sources_and_matches_original_ci(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/dist-key.json")).unwrap();
 for row in fixture["cases"].as_array().unwrap(){for call in row["calls"].as_array().unwrap(){let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let bin=temp.path().join("bin");std::fs::create_dir(&root).unwrap();std::fs::create_dir(&bin).unwrap();
  for(name,bytes)in call["files"].as_object().unwrap(){let path=root.join(name);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,base64::engine::general_purpose::STANDARD.decode(bytes.as_str().unwrap()).unwrap()).unwrap();}
  git(&root,&["init","-b","main"]);git(&root,&["add","."]);git(&root,&["commit","-m","fixture"]);let head=git(&root,&["rev-parse","HEAD"]);git(&root,&["commit","--allow-empty","-m","adiante"]);git(&root,&["update-ref","refs/remotes/origin/main","HEAD"]);git(&root,&["reset","--hard",&head]);
  let npm=bin.join("npm");std::fs::write(&npm,"#!/bin/sh\n/bin/mkdir -p frontend/dist\nprintf screen > frontend/dist/index.html\n").unwrap();std::fs::set_permissions(npm,std::fs::Permissions::from_mode(0o755)).unwrap();
  let service=Service::new(temp.path().join("home"),root,temp.path().join("claude"),[("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("CP_UPDATE_BRANCH".into(),"".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into())].into());let source=Missing(Mutex::new(vec![]));assert_eq!(service.update_dist(&source).unwrap(),None);let urls=source.0.into_inner().unwrap();assert_eq!(urls,[format!("https://github.com/jeffer1312/hangar/releases/download/dist-commits/frontend-dist-{}.tar.gz.sha256",call["key"].as_str().unwrap())],"{}",row["name"]);
 }}
}
