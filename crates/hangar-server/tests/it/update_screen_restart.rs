//! O servidor novo conclui a atualização interrompida somente no commit esperado.
#![cfg(unix)]
use hangar_server::update::git_engine::Service;
use serde_json::{Value,json};
use super::update_local_dist::git;
fn normalize(mut value:Value,head:&str,root:&std::path::Path)->Value{
 if let Some(lines)=value.get_mut("log").and_then(Value::as_array_mut){for line in lines {if let Some(text)=line.as_str(){*line=json!(text.replace(&root.display().to_string(),"<REPO>").replace(head,"<HEAD>"));}}}
 if value.get("ts").is_some(){value["ts"]=json!("<TS>");}if value.get("pid").is_some_and(Value::is_number){value["pid"]=json!("<DEAD>");}
 if let Some(pid)=value["backend_pid"].as_u64(){value["backend_pid"]=json!(if pid==u64::from(std::process::id()){"<SELF>"}else{"<OTHER>"});}
 if value["commit_para"]==head{value["commit_para"]=json!("<HEAD>");}value
}
#[test]
fn update_screen_restart_matches_original_completion_and_idempotence(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/screen-restart.json")).unwrap();
 for row in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let home=temp.path().join("home");std::fs::create_dir(&root).unwrap();git(&root,&["init","-b","main"]);std::fs::write(root.join("file"),"fixture").unwrap();git(&root,&["add","."]);git(&root,&["commit","-m","fixture"]);let head=git(&root,&["rev-parse","HEAD"]);
  if let Some(raw)=row["marker"].as_str(){let marker=home.join(".hangar/native/release.json");std::fs::create_dir_all(marker.parent().unwrap()).unwrap();std::fs::write(marker,raw).unwrap();}
  let mut service=Service::new(home,root.clone(),temp.path().join("claude"),[("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into()),("PATH".into(),"/usr/bin:/bin".into())].into());service.writing=row["motor"].as_bool().unwrap();std::fs::create_dir_all(&service.state_dir).unwrap();
  for call in row["calls"].as_array().unwrap(){let mut input=call["input"].clone();input["ts"]=json!(chrono::Local::now().to_rfc3339());if input["pid"]=="<DEAD>"{input["pid"]=json!(999999999_u32);}if input["backend_pid"]=="<SELF>"{input["backend_pid"]=json!(std::process::id());}else if input["backend_pid"]=="<OTHER>"{input["backend_pid"]=json!(u64::from(std::process::id())+1);}if input["commit_para"]=="<HEAD>"{input["commit_para"]=json!(head);}
   std::fs::write(service.state().path(),input.to_string().replace("<REPO>",&root.display().to_string()).replace("<HEAD>",&head)).unwrap();let actual=normalize(service.screen_state().into_value(),&head,&root);assert_eq!(actual,call["result"],"{} {}",row["name"],row["stage"]);let stored:Value=serde_json::from_slice(&std::fs::read(service.state().path()).unwrap()).unwrap();assert_eq!(normalize(stored,&head,&root),call["stored"]);assert_eq!(normalize(service.screen_state().into_value(),&head,&root),actual,"segunda consulta não pode acrescentar aviso");
  }
 }
}
