//! Tela local e compensação em checkout descartável, sem npm ou rede reais.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::{Value,json};
use std::path::Path;
struct NoNetwork;
impl Source for NoNetwork {fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("build local não consulta release");}}
pub(super) fn git(root:&Path,args:&[&str]) -> String {
 let output=std::process::Command::new("/usr/bin/git").arg("-C").arg(root).args(args)
  .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL","/dev/null")
  .env("GIT_AUTHOR_NAME","Fixture").env("GIT_AUTHOR_EMAIL","fixture@example.invalid")
  .env("GIT_COMMITTER_NAME","Fixture").env("GIT_COMMITTER_EMAIL","fixture@example.invalid").output().unwrap();
 assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
 String::from_utf8(output.stdout).unwrap().trim().into()
}
fn text(path:&Path)->Value {std::fs::read_to_string(path).ok().map(Value::String).unwrap_or(Value::Null)}
#[test]
fn update_local_dist_preserves_previous_screen_and_build_outcome() {
 let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_local_dist::update_local_dist_child","--ignored","--nocapture"]).output().unwrap();
 assert!(output.status.success(),"{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
#[test]
#[ignore="Executável falso isolado de fork concorrente de outros testes."]
fn update_local_dist_child() {
 use std::os::unix::fs::PermissionsExt;
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/local-dist.json")).unwrap();
 for row in fixture["cases"].as_array().unwrap() {
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let bin=temp.path().join("bin");let calls=temp.path().join("calls");let dist=root.join("frontend/dist");let previous=root.join("frontend/.dist-velho");std::fs::create_dir_all(&dist).unwrap();std::fs::create_dir(&bin).unwrap();
  std::fs::write(dist.join("index.html"),"old-screen").unwrap();std::fs::write(root.join("frontend/source.ts"),"old-source").unwrap();std::fs::write(root.join(".gitignore"),"frontend/dist/\nfrontend/.dist-velho/\n").unwrap();
  git(&root,&["init","-b",if name=="dirty-main"{"main"}else{"beta"}]);git(&root,&["add","."]);git(&root,&["commit","-m","fixture"]);
  if name=="dirty-main" {std::fs::write(root.join("frontend/source.ts"),"changed-source").unwrap();}
  std::os::unix::fs::symlink("/usr/bin/git",bin.join("git")).unwrap();
  if name!="missing-npm" {
   let script=bin.join("npm");std::fs::write(&script,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CALLS\"\nif [ \"$CASE\" = incomplete ]; then /bin/rm frontend/dist/index.html; else printf new-screen > frontend/dist/index.html; fi\nif [ \"$CASE\" = build-failed ]; then printf fixture-build-failed >&2; exit 1; fi\n").unwrap();std::fs::set_permissions(script,std::fs::Permissions::from_mode(0o755)).unwrap();
  }
  let service=Service::new(temp.path().join("home"),root.clone(),temp.path().join("claude"),[("PATH".into(),bin.display().to_string()),("CALLS".into(),calls.display().to_string()),("CASE".into(),name.into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into())].into());
  let actual=match service.update_dist(&NoNetwork){Ok(warning)=>json!({"warning":warning}),Err(message)=>json!({"error_type":"RuntimeError","message":message})};assert_eq!(actual,row["result"],"{name}");
  let actual_calls=std::fs::read_to_string(&calls).ok().map(|raw|{let args=std::iter::once("npm").chain(raw.lines()).collect::<Vec<_>>();vec![json!({"args":args,"cwd":"repo"})]}).unwrap_or_default();let expected_calls=row["calls"].as_array().unwrap().iter().map(|call|json!({"args":call["args"],"cwd":call["cwd"]})).collect::<Vec<_>>();assert_eq!(actual_calls,expected_calls,"{name}");
  assert_eq!(json!({"screen":text(&dist.join("index.html")),"previous":text(&previous.join("index.html")),"previous_exists":previous.exists()}),row["before_restore"],"{name}");service.restore_dist().unwrap();assert_eq!(json!({"screen":text(&dist.join("index.html")),"previous_exists":previous.exists()}),row["after_restore"],"{name}");
 }
}
