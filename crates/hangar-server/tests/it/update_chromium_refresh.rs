//! Renovação do Chromium respeita a marca e usa somente instalador falso.
#![cfg(target_os="linux")]
use hangar_server::update::git_engine::Service;
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt,time::{SystemTime,Duration}};
#[test]
fn update_chromium_refresh_matches_original_markers_and_retry(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/chromium-refresh.json")).unwrap();
 for row in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let home=temp.path().join("home");let calls=temp.path().join("calls");std::fs::create_dir_all(root.join("scripts")).unwrap();let script=root.join("scripts/install-chromium.sh");std::fs::write(&script,format!("#!/bin/sh\nprintf called > \"$CALLS\"\nexit {}\n",row["code"])).unwrap();std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o755)).unwrap();let marker=home.join(".hangar/native/chromium-ok");
  if let Some(text)=row["marker"].as_str(){std::fs::create_dir_all(marker.parent().unwrap()).unwrap();std::fs::write(&marker,text).unwrap();std::fs::File::options().write(true).open(&marker).unwrap().set_modified(SystemTime::now()-Duration::from_secs(row["age_hours"].as_u64().unwrap()*3600)).unwrap();}
  let service=Service::new(home,root,temp.path().join("claude"),[("CALLS".into(),calls.display().to_string())].into());service.refresh_chromium();assert_eq!(calls.exists(),!row["calls"].as_array().unwrap().is_empty(),"{}",row["name"]);if let Some(text)=row["marker"].as_str(){assert_eq!(std::fs::read_to_string(marker).unwrap(),text);}
 }
}
