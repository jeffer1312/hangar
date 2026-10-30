use super::*;
use std::{fs,os::unix::fs::PermissionsExt};
#[tokio::test]
async fn codex_service_native_cancel_waits_for_cleanup_before_releasing_the_caller(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-cancellation.json")).unwrap();
 let mut differences=vec![];
 for case in fixture["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let home=root.join("home");let codex=home.join(".codex");let bin=root.join("bin");fs::create_dir_all(&codex).unwrap();fs::create_dir_all(&bin).unwrap();let config=codex.join("config.toml");fs::write(&config,"model=\"fixture\"\n").unwrap();let seen=root.join("seen.json");let work=root.join("work");
  let program=format!("#!/usr/bin/env python3\nimport sys\nsys.argv[1:1]=[{:?},{:?}]\n{}",seen.to_str().unwrap(),case["method"].as_str().unwrap(),fixture["program"].as_str().unwrap());let executable=bin.join("codex");fs::write(&executable,program).unwrap();fs::set_permissions(&executable,fs::Permissions::from_mode(0o700)).unwrap();
  let env=AccountEnvironment::from_map([("HOME".into(),home.to_string_lossy().into_owned()),("PATH".into(),format!("{}:{}",bin.display(),std::env::var("PATH").unwrap()))].into());
  let (sender,cancel)=tokio::sync::watch::channel(false);let backup=root.join("backups");let writer=case["writer"]==true;let call_config=config.clone();let call_work=work.clone();
  let task=tokio::spawn(async move{if writer{crate::codex_setup::files::edit_config_controlled(&env,&call_config,&backup,&call_work,|_|Ok(vec![json!({"keyPath":"model","value":"changed","mergeStrategy":"replace"})]),cancel).await}else{let mut native=Native::open_controlled(&env,&home,&codex,false,cancel).await?;native.close().await.map_err(OperationError::Failed)}});
  tokio::time::timeout(Duration::from_secs(3),async{while !seen.exists(){tokio::time::sleep(Duration::from_millis(2)).await;}}).await.unwrap();let observed:Value=serde_json::from_slice(&fs::read(&seen).unwrap()).unwrap();sender.send_replace(true);
  let result=tokio::time::timeout(Duration::from_secs(8),task).await.unwrap().unwrap();
  let pid=observed["pid"].as_u64().unwrap() as u32;let cwd=PathBuf::from(observed["cwd"].as_str().unwrap());let actual=json!({"method":case["method"],"writer":writer,"result":match result{Err(OperationError::Cancelled)=>"CancelledError".to_owned(),Err(OperationError::Failed(e))=>e,Ok(())=>"ok".into()},"alive":crate::terminal_process::pid_alive(pid).unwrap(),"cwd_removed":!cwd.exists(),"work_empty":!work.exists()||fs::read_dir(&work).unwrap().next().is_none(),"config":fs::read_to_string(&config).unwrap(),"confirmed":[]});
  if actual!=*case{differences.push(json!({"actual":actual,"expected":case}));}
  // A rodada vermelha também aguarda o supervisor antes de descartar o sandbox.
  tokio::time::timeout(Duration::from_secs(8),async{while crate::terminal_process::pid_alive(pid).unwrap()||cwd.exists(){tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
 }
 assert!(differences.is_empty(),"Cancelamento divergente: {differences:#?}");
}
