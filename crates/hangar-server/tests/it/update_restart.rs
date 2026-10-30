//! Reinício isolado: somente executáveis falsos, sem gerenciador de serviço real.
#![cfg(unix)]
use hangar_server::update::git_engine::Service;
use serde_json::{Value,json};
#[test]
fn update_restart_matches_commands_literal_port_and_state() {
 let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_restart::update_restart_child","--ignored","--nocapture"]).output().unwrap();
 assert!(output.status.success(),"{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
#[test]
#[ignore="Executáveis e guardas isolados dos filhos de outros testes."]
fn update_restart_child() {
 use std::os::unix::fs::PermissionsExt;
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/restart.json")).unwrap();
 let mut differences=Vec::new();
 for row in fixture["cases"].as_array().unwrap() {
  let name=row["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let bin=temp.path().join("bin");let calls=temp.path().join("calls");std::fs::create_dir_all(root.join("backend")).unwrap();std::fs::create_dir(&bin).unwrap();
  if let Some(raw)=row["dotenv"].as_str(){std::fs::write(root.join("backend/.env"),raw).unwrap();}
  for program in ["systemctl","powershell","launchctl"] {
   let file=bin.join(program);let body=format!("#!/bin/sh\nprintf '%s' '{program}' >> \"$CALLS\"\nfor arg in \"$@\"; do printf '\\t%s' \"$arg\" >> \"$CALLS\"; done\nprintf '\\n' >> \"$CALLS\"\ncase \"$CASE\" in *-failed) printf fixture-failed; exit 1;; systemd-only) if [ \"$2\" = list-unit-files ]; then printf fixture-failed; exit 1; fi;; esac\nexit 0\n");std::fs::write(&file,body).unwrap();std::fs::set_permissions(file,std::fs::Permissions::from_mode(0o755)).unwrap();
  }
  let service=Service::new(temp.path().join("home"),root.clone(),temp.path().join("claude"),[("PATH".into(),bin.display().to_string()),("CALLS".into(),calls.display().to_string()),("CASE".into(),name.into()),("CP_FRONT_PORT".into(),"9911".into())].into());
  let result=service.restart_service(row["topology"].as_str().unwrap(),8123);
  let actual_result=match result{Ok(())=>json!({"ok":true}),Err(message)=>json!({"error_type":"RuntimeError","message":message})};
  let raw=std::fs::read_to_string(&calls).unwrap_or_default().replace(&root.display().to_string(),"<REPO>").replace(&format!("gui/{}/",unsafe{libc::getuid()}),"gui/<UID>/");
  let actual_calls:Vec<Value>=raw.lines().map(|line|json!(line.split('\t').collect::<Vec<_>>())).collect();let expected_calls:Vec<Value>=row["calls"].as_array().unwrap().iter().map(|call|call["args"].clone()).collect();
  let mut state=service.state().read();state.remove("ts");
  if actual_calls!=expected_calls{differences.push(format!("{name}: chamadas {actual_calls:?} != {expected_calls:?}"));}
  if actual_result!=row["result"]{differences.push(format!("{name}: resultado {actual_result} != {}",row["result"]));}
  if state.clone().into_value()!=row["state"]{differences.push(format!("{name}: estado {state:?} != {}",row["state"]));}
 }
 assert!(differences.is_empty(),"{}",differences.join("\n"));
}
#[test]
fn update_manual_restart_propagates_state_refusal_and_preserves_bytes() {
 let temp=tempfile::tempdir().unwrap();let service=Service::new(temp.path().join("home"),temp.path().join("repo"),temp.path().join("claude"),Default::default());
 std::fs::create_dir_all(&service.state_dir).unwrap();let path=service.state().path().to_path_buf();let raw=br#"{"ok":true,"extra":"\ud800"}"#;std::fs::write(&path,raw).unwrap();
 assert_eq!(service.restart_service("manual",8123).unwrap_err(),"update_state_invalid_unicode: texto contém surrogate Unicode isolado");assert_eq!(std::fs::read(path).unwrap(),raw);
}
