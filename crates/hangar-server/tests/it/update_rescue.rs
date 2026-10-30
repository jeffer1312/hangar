//! Resgate em Git real, com falhas injetadas somente na operação indicada.
#![cfg(unix)]
use std::{path::Path, process::Command, os::unix::fs::PermissionsExt};
use serde_json::{Value,json};
fn git(root:&Path,args:&[&str])->std::process::Output {
 Command::new("/usr/bin/git").arg("-C").arg(root).args(args)
  .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL","/dev/null")
  .env("GIT_AUTHOR_NAME","Fixture").env("GIT_AUTHOR_EMAIL","fixture@example.invalid")
  .env("GIT_COMMITTER_NAME","Fixture").env("GIT_COMMITTER_EMAIL","fixture@example.invalid")
  .output().unwrap()
}
fn checked(root:&Path,args:&[&str])->String {
 let output=git(root,args);assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
 String::from_utf8(output.stdout).unwrap().trim().into()
}
fn normalize(value:&mut Value) {
 match value {
  Value::String(text)=>*text=regex::Regex::new(r"resgate/\d{4}-\d{2}-\d{2}-\d{4}").unwrap().replace_all(text,"resgate/<MINUTE>").into_owned(),
  Value::Array(values)=>values.iter_mut().for_each(normalize),
  Value::Object(values)=>values.values_mut().for_each(normalize),
  _=>{}
 }
}
#[test]
fn update_rescue_preserves_refs_stash_files_and_failures() {
 let result=Command::new(std::env::current_exe().unwrap()).args(["--exact","update_rescue::update_rescue_contract_child","--ignored","--nocapture"]).output().unwrap();
 assert!(result.status.success(),"{}{}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
}
#[test]
#[ignore="Filho isolado com Git e executável de falhas próprios."]
fn update_rescue_contract_child() {
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/rescue.json")).unwrap();
 for row in fixture["cases"].as_array().unwrap() {
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let repo=temp.path().join("repo");let bin=temp.path().join("bin");let home=temp.path().join("home");let calls=temp.path().join("calls");
  std::fs::create_dir_all(&repo).unwrap();std::fs::create_dir_all(&bin).unwrap();
  checked(&repo,&["init","-b","main"]);checked(&repo,&["config","user.name","Fixture"]);checked(&repo,&["config","user.email","fixture@example.invalid"]);
  std::fs::write(repo.join("a.txt"),"original\n").unwrap();checked(&repo,&["add","."]);checked(&repo,&["commit","-m","original"]);
  if ["dirty","dirty-untracked","stash-failed"].contains(&name){std::fs::write(repo.join("a.txt"),"alterado\n").unwrap();}
  if name=="dirty-untracked"{std::fs::write(repo.join("solto.txt"),"solto\n").unwrap();}
  let fake=bin.join("git");std::fs::write(&fake,r#"#!/bin/sh
repo="$2"
shift 2
printf '%s\t' "$@" >> "$FIXTURE_CALLS"
printf '\n' >> "$FIXTURE_CALLS"
if { [ "$FIXTURE_CASE" = branch-failed ] && [ "$1" = branch ]; } ||
   { [ "$FIXTURE_CASE" = stash-failed ] && [ "$1" = stash ]; } ||
   { [ "$FIXTURE_CASE" = verify-failed ] && [ "$1" = rev-parse ] && [ "$2" = --verify ]; }; then
 printf fixture-git-failed >&2
 exit 1
fi
exec /usr/bin/git -C "$repo" "$@"
"#).unwrap();std::fs::set_permissions(&fake,std::fs::Permissions::from_mode(0o755)).unwrap();
  let service=hangar_server::update::git_engine::Service::new(home.clone(),repo.clone(),home.join(".claude"),[
   ("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("HOME".into(),home.display().to_string()),
   ("FIXTURE_CASE".into(),name.into()),("FIXTURE_CALLS".into(),calls.display().to_string()),
   ("GIT_CONFIG_NOSYSTEM".into(),"1".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into())
  ].into());
  let actual_result=match service.rescue(&row["pre"]){Ok(value)=>json!({"value":value}),Err(error)=>json!({"error":error})};
  let refs=checked(&repo,&["for-each-ref","--format=%(refname:short)","refs/heads/resgate/"]);
  let head=checked(&repo,&["rev-parse","HEAD"]);
  let refs=refs.lines().filter(|line|!line.is_empty()).map(|name|json!({"name":name,"at_head":checked(&repo,&["rev-parse",name])==head})).collect::<Vec<_>>();
  let stash=checked(&repo,&["stash","list","--format=%s"]).lines().map(str::to_owned).collect::<Vec<_>>();
  let stashed=|reference:&str|{let output=git(&repo,&["show",reference]);output.status.success().then(||String::from_utf8(output.stdout).unwrap())};
  let actual_calls=std::fs::read_to_string(calls).unwrap_or_default().lines().map(|line|json!({"args":line.trim_end_matches('\t').split('\t').collect::<Vec<_>>()})).collect::<Vec<_>>();
  let mut actual=json!({"result":actual_result,"calls":actual_calls,"refs":refs,"file":std::fs::read_to_string(repo.join("a.txt")).unwrap(),
   "untracked_exists":repo.join("solto.txt").exists(),"stash":stash,"stashed_file":stashed("stash@{0}:a.txt"),"stashed_untracked":stashed("stash@{0}^3:solto.txt")});
  normalize(&mut actual);
  let mut expected=row.clone();expected.as_object_mut().unwrap().remove("name");expected.as_object_mut().unwrap().remove("pre");expected["result"].as_object_mut().unwrap().remove("type");
  for call in expected["calls"].as_array_mut().unwrap(){call.as_object_mut().unwrap().remove("timeout");}
  assert_eq!(actual,expected,"{name}");
 }
}
