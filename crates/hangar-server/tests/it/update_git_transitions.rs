//! Efeitos completos das transições de Git, isolados dos serviços instalados.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::{Value,json};
use std::{path::Path,process::Command,sync::{Arc,Mutex},os::unix::fs::PermissionsExt};
fn git(root:&Path,args:&[&str])->std::process::Output {
 Command::new("/usr/bin/git").arg("-C").arg(root).args(args)
  .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL","/dev/null")
  .env("GIT_AUTHOR_NAME","Fixture").env("GIT_AUTHOR_EMAIL","fixture@example.invalid")
  .env("GIT_COMMITTER_NAME","Fixture").env("GIT_COMMITTER_EMAIL","fixture@example.invalid").output().unwrap()
}
fn checked(root:&Path,args:&[&str])->String {let output=git(root,args);assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));String::from_utf8(output.stdout).unwrap().trim().into()}
fn lines(root:&Path,args:&[&str])->Vec<String>{String::from_utf8(git(root,args).stdout).unwrap().lines().map(str::to_owned).collect()}
fn show(root:&Path,reference:&str)->Option<String>{let out=git(root,&["show",reference]);out.status.success().then(||String::from_utf8(out.stdout).unwrap())}
#[derive(Clone)]struct Log(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Log {fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{self.0.lock().unwrap().extend_from_slice(bytes);Ok(bytes.len())}fn flush(&mut self)->std::io::Result<()>{Ok(())}}
fn capture<T>(call:impl FnOnce()->T)->(T,Vec<String>){
 let log=Log(Default::default());let writer=log.clone();
 let subscriber=tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).with_ansi(false).without_time().with_target(false).with_writer(move||writer.clone()).finish();
 let result=tracing::subscriber::with_default(subscriber,call);
 let lines=String::from_utf8(log.0.lock().unwrap().clone()).unwrap().lines().filter_map(|line|line.split_once("WARN").map(|(_,text)|text.trim().to_owned())).collect();(result,lines)
}
fn service(base:&Path,repo:&Path,group:&str,name:&str)->(Service,std::path::PathBuf) {
 let bin=base.join("bin");let home=base.join("home");let calls=base.join("calls");std::fs::create_dir_all(&bin).unwrap();
 let fake=bin.join("git");std::fs::write(&fake,r#"#!/bin/sh
repo="$2"
shift 2
printf '%s\t' "$@" >> "$FIXTURE_CALLS"
printf '\n' >> "$FIXTURE_CALLS"
fail=0
if [ "$FIXTURE_GROUP" = pull ]; then
 if { [ "$FIXTURE_CASE" = fetch-failed ] && [ "$1" = fetch ] && [ "$#" = 2 ]; } ||
    { [ "$FIXTURE_CASE" = tag-failed ] && [ "$1" = fetch ] && [ "$3" = --force ]; } ||
    { { [ "$FIXTURE_CASE" = stash-failed ] || [ "$FIXTURE_CASE" = reset-failed ]; } && [ "$1" = merge ]; } ||
    { [ "$FIXTURE_CASE" = stash-failed ] && [ "$1" = stash ]; } ||
    { [ "$FIXTURE_CASE" = reset-failed ] && [ "$1" = reset ]; }; then fail=1; fi
else
 if { [ "$FIXTURE_CASE" = stash-failed ] && { [ "$1" = checkout ] || [ "$1" = stash ]; }; } ||
    { [ "$FIXTURE_CASE" = retry-failed ] && [ "$1" = checkout ]; } ||
    { [ "$FIXTURE_CASE" = rescue-failed ] && [ "$1" = branch ] && [ "$2" = --force ]; } ||
    { [ "$FIXTURE_CASE" = upstream-failed ] && [ "$1" = branch ] && [ "$2" = --set-upstream-to ]; }; then fail=1; fi
fi
if [ "$fail" = 1 ]; then printf fixture-git-failed >&2;exit 1;fi
exec /usr/bin/git -C "$repo" "$@"
"#).unwrap();std::fs::set_permissions(&fake,std::fs::Permissions::from_mode(0o755)).unwrap();
 (Service::new(home.clone(),repo.into(),home.join(".claude"),[
  ("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("HOME".into(),home.display().to_string()),("CP_RUST_SERVER".into(),"0".into()),
  ("FIXTURE_GROUP".into(),group.into()),("FIXTURE_CASE".into(),name.into()),("FIXTURE_CALLS".into(),calls.display().to_string()),
  ("GIT_CONFIG_NOSYSTEM".into(),"1".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into())
 ].into()),calls)
}
fn normalize(value:&mut Value,repo:&Path,state:&Path,old:&str) {
 match value {
  Value::String(text)=>{*text=text.replace(&state.display().to_string(),"<STATE>").replace(&repo.display().to_string(),"<REPO>").replace(old,"<OLD>");*text=regex::Regex::new(r"resgate/\d{4}-\d{2}-\d{2}-\d{4}").unwrap().replace_all(text,"resgate/<MINUTE>").into_owned();},
  Value::Array(values)=>values.iter_mut().for_each(|v|normalize(v,repo,state,old)),
  Value::Object(values)=>values.values_mut().for_each(|v|normalize(v,repo,state,old)),_=>{}
 }
}
fn calls(path:&Path)->Value {json!(std::fs::read_to_string(path).unwrap_or_default().lines().map(|line|json!({"args":line.trim_end_matches('\t').split('\t').collect::<Vec<_>>()})).collect::<Vec<_>>())}
fn compare(name:&str,actual:Value,mut expected:Value,failures:&mut Vec<String>) {
 expected.as_object_mut().unwrap().remove("name");expected["result"].as_object_mut().unwrap().remove("type");
 for call in expected["calls"].as_array_mut().unwrap(){call.as_object_mut().unwrap().remove("timeout");}
 for (key,value) in expected.as_object().unwrap(){if actual.get(key)!=Some(value){failures.push(format!("{name}:{key}: atual={:?}; esperado={value:?}",actual.get(key)));}}
}
fn run_child(kind:&str) {
 let result=Command::new(std::env::current_exe().unwrap()).args(["--exact","update_git_transitions::update_git_transition_child","--ignored","--nocapture"]).env("HANGAR_TRANSITION_PROOF",kind).output().unwrap();
 assert!(result.status.success(),"{}{}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
}
#[test]fn update_git_pull_preserves_untracked_files_and_failure_order(){run_child("pull");}
#[test]fn update_git_switch_preserves_refs_marker_bytes_and_warnings(){run_child("switch");}
#[test]
#[ignore="Filho isolado para comandos e captura do diagnóstico sem interferência de outros testes."]
fn update_git_transition_child() {
 let kind=std::env::var("HANGAR_TRANSITION_PROOF").unwrap();
 let fixture:Value=serde_json::from_str(if kind=="pull"{include_str!("../../../../backend/tests/fixtures/contract/update-git/pull.json")}else{include_str!("../../../../backend/tests/fixtures/contract/update-git/switch.json")}).unwrap();
 let mut failures=vec![];
 for row in fixture["cases"].as_array().unwrap() {
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let repo=temp.path().join("repo");std::fs::create_dir_all(&repo).unwrap();
  checked(&repo,&["init","-b","main"]);std::fs::write(repo.join("content"),"old\n").unwrap();checked(&repo,&["add","."]);checked(&repo,&["commit","-m","old"]);let old=checked(&repo,&["rev-parse","HEAD"]);
  let (service,log)=service(temp.path(),&repo,&kind,name);
  let mut actual=if kind=="pull" {
   let remote=temp.path().join("remote");std::fs::create_dir(&remote).unwrap();checked(&remote,&["init","--bare"]);checked(&repo,&["remote","add","origin",remote.to_str().unwrap()]);checked(&repo,&["push","-u","origin","main"]);
   std::fs::write(repo.join("content"),"new\n").unwrap();std::fs::write(repo.join("collision.txt"),"remote\n").unwrap();checked(&repo,&["add","."]);checked(&repo,&["commit","-m","new"]);let new=checked(&repo,&["rev-parse","HEAD"]);checked(&repo,&["push","origin","main"]);checked(&repo,&["reset","--hard",&old]);
   if name=="diverged"{std::fs::write(repo.join("local.txt"),"local\n").unwrap();checked(&repo,&["add","."]);checked(&repo,&["commit","-m","local"]);}
   if ["untracked-collision","stash-failed","reset-failed"].contains(&name){std::fs::write(repo.join("collision.txt"),"untracked\n").unwrap();}
   struct NoNetwork;impl Source for NoNetwork{fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("pull sem binário não consulta rede");}}
   let (result,warnings)=capture(||service.pull(&json!({"branch":"main","alvo":"main","sujo":0}),&NoNetwork,None));
   let result=match result{Ok(value)=>{assert!(value.is_none());json!({"value":null})},Err(error)=>json!({"error":error})};
   let head=checked(&repo,&["rev-parse","HEAD"]);
   json!({"result":result,"calls":calls(&log),"warnings":warnings,"head":if head==old{"old"}else if head==new{"new"}else{"local"},
    "content":std::fs::read_to_string(repo.join("content")).unwrap(),"collision":std::fs::read_to_string(repo.join("collision.txt")).ok(),
    "stash":lines(&repo,&["stash","list","--format=%s"]),"stashed_untracked":show(&repo,"stash@{0}^3:collision.txt")})
  }else{
   checked(&repo,&["config","remote.origin.url",repo.to_str().unwrap()]);checked(&repo,&["config","remote.origin.fetch","+refs/heads/*:refs/remotes/origin/*"]);checked(&repo,&["update-ref","refs/remotes/origin/main",&old]);checked(&repo,&["update-ref","refs/remotes/origin/master",&old]);
   checked(&repo,&["checkout","-b","remote-build"]);std::fs::write(repo.join("beta.txt"),"remote\n").unwrap();checked(&repo,&["add","."]);checked(&repo,&["commit","-m","remote"]);let remote=checked(&repo,&["rev-parse","HEAD"]);checked(&repo,&["update-ref","refs/remotes/origin/beta",&remote]);checked(&repo,&["checkout","main"]);
   let mut local=None;
   if ["existing","local-ahead","rescue-failed","return-main","return-master"].contains(&name){
    checked(&repo,&["checkout","-b","beta",&old]);
    if ["local-ahead","rescue-failed"].contains(&name){std::fs::write(repo.join("local.txt"),"local\n").unwrap();checked(&repo,&["add","."]);checked(&repo,&["commit","-m","local"]);local=Some(checked(&repo,&["rev-parse","HEAD"]));}
    if !name.starts_with("return-"){checked(&repo,&["checkout","main"]);}
   }
   if name=="return-master"{checked(&repo,&["branch","master",&old]);}
   std::fs::create_dir_all(&service.state_dir).unwrap();let marker=service.state_dir.join("branch");
   if name.starts_with("return-"){std::fs::write(&marker,"beta").unwrap();}
   if ["collision","stash-failed","retry-failed"].contains(&name){std::fs::write(repo.join("beta.txt"),"untracked\n").unwrap();}
   if name=="marker-failed"{std::fs::create_dir(&marker).unwrap();}
   let target=row["target"].as_str().unwrap();let reference=row["reference"].as_str().unwrap().replace("<OLD>",&old);
   let (result,warnings)=capture(||service.switch_branch(target,&reference));
   let result=match result{Ok(())=>json!({"ok":true}),Err(error)=>json!({"error":error})};
   let refs=lines(&repo,&["for-each-ref","--format=%(refname:short)","refs/heads/resgate/"]).iter().map(|name|json!({"name":name,"at_local":Some(checked(&repo,&["rev-parse",name]))==local})).collect::<Vec<_>>();
   let head=checked(&repo,&["rev-parse","HEAD"]);
   json!({"target":target,"reference":reference,"result":result,"calls":calls(&log),"warnings":warnings,"state_warnings":service.state().read().get("avisos").cloned().unwrap_or(json!([])),
    "branch":checked(&repo,&["rev-parse","--abbrev-ref","HEAD"]),"head":if head==old{"old"}else if head==remote{"remote"}else{"local"},
    "upstream":String::from_utf8(git(&repo,&["rev-parse","--abbrev-ref","@{upstream}"]).stdout).unwrap().trim(),
    "refs":refs,"marker":std::fs::read_to_string(&marker).ok(),"marker_directory":marker.is_dir(),
    "beta_file":std::fs::read_to_string(repo.join("beta.txt")).ok(),"stashed_untracked":show(&repo,"stash@{0}^3:beta.txt")})
  };
  normalize(&mut actual,&repo,&service.state_dir,&old);
  let mut expected=row.clone();
  if kind=="switch"&&name=="marker-failed"{
   let original="nao consegui gravar a branch da atualizacao (beta): [Errno 21] Is a directory: '<STATE>/branch'";
   let native="nao consegui gravar a branch da atualizacao (beta): Is a directory (os error 21): '<STATE>/branch'";
   for key in ["warnings","state_warnings"]{assert_eq!(expected[key],json!([original]));expected[key]=json!([native]);}
  }
  compare(name,actual,expected,&mut failures);
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}
