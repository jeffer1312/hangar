//! O driver aplica os passos reais antes da preparação, com posse e diagnóstico publicados.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::{Value,json};
use std::{os::unix::fs::PermissionsExt,sync::atomic::{AtomicBool,Ordering}};
use super::update_local_dist::git;
struct Unused;
impl Source for Unused {fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("Rust desativado não consulta publicação");}}
#[test]
fn update_steps_receive_ownership_and_publish_invalid_documents_before_prepare(){
 let captured:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/step-effects.json")).unwrap();
 let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let origin=temp.path().join("origin");let bin=temp.path().join("bin");let docs=root.join("docs/atualizacoes");std::fs::create_dir_all(&docs).unwrap();std::fs::create_dir(&bin).unwrap();
 std::fs::write(docs.join("2026-01-01-posse.md"),"---\nid: 2026-01-01-posse\ntitulo: Posse do atualizador\ntipo: maquina\ncomando: printf %s \"$HANGAR_UPDATE_OWNS_RUST\" > ownership\nprova: ownership\n---\n").unwrap();
 std::fs::write(docs.join("2026-01-01-torto.md"),"---\nid: torto\n---\n").unwrap();
 git(&root,&["init","-b","main"]);git(&root,&["add","."]);git(&root,&["commit","-m","fixture"]);git(temp.path(),&["clone","--bare",root.to_str().unwrap(),origin.to_str().unwrap()]);git(&root,&["remote","add","origin",origin.to_str().unwrap()]);git(&root,&["fetch","origin"]);git(&root,&["branch","--set-upstream-to=origin/main"]);
 for name in ["uv","node","npm","tmux","systemctl"]{let path=bin.join(name);std::fs::write(&path,if name=="systemctl"{"#!/bin/sh\nexit 1\n"}else{"#!/bin/sh\nexit 0\n"}).unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();}
 let home=temp.path().join("home");let service=Service::new(home.clone(),root.clone(),temp.path().join("claude"),[("HOME".into(),home.display().to_string()),("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("CP_RUST_SERVER".into(),"0".into()),("CP_UPDATE_BRANCH".into(),"".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into())].into());
 let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();let prepared=AtomicBool::new(false);let result=service.execute_with_preparer(listener.local_addr().unwrap().port(),&Unused,None,&|s,_,_,_,_,_|{assert_eq!(std::fs::read(root.join("ownership")).unwrap(),b"1");assert_eq!(s.state().read()["passos_invalidos"],captured["cases"][1]["invalid_steps"]);prepared.store(true,Ordering::SeqCst);Ok(())});
 assert!(prepared.load(Ordering::SeqCst));let manual:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/manual-no-probe.json")).unwrap();let expected=&manual["cases"][0]["calls"][1]["result"];for key in ["ok","reiniciar_manual"]{assert_eq!(result[key],expected[key]);}assert_eq!(listener.accept().unwrap_err().kind(),std::io::ErrorKind::WouldBlock,"reinício manual não consulta saúde");assert_eq!(result["ok"],json!(true));assert_eq!(result["passos_invalidos"],captured["cases"][1]["invalid_steps"]);
}
