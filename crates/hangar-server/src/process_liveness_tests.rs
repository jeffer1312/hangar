use super::{pid_alive,pid_alive_with};
use serde_json::{Value,json};
use std::io::{BufRead,Write};
#[test]
fn codex_service_pid_probe_matches_original_permission_and_missing_classes(){
 // A captura usa EIO do POSIX (5); no Windows 5 é acesso negado, outra classe.
 #[cfg(unix)] let failure_code = 5;
 #[cfg(windows)] let failure_code = windows_sys::Win32::Foundation::ERROR_IO_DEVICE as i32;
 assert_ne!(std::io::Error::from_raw_os_error(failure_code).kind(), std::io::ErrorKind::PermissionDenied);
 assert_ne!(std::io::Error::from_raw_os_error(failure_code).kind(), std::io::ErrorKind::NotFound);
 let corpus:Value=serde_json::from_str(include_str!("../../../backend/tests/fixtures/contract/codex-integration/liveness.json")).unwrap();
 for case in corpus["cases"].as_array().unwrap(){
  let mut called=false;
  let result=pid_alive_with(case["pid"].as_i64().unwrap(),||{called=true;match case["probe"].as_str().unwrap(){"ok"=>Ok(()),"missing"=>Err(std::io::ErrorKind::NotFound.into()),"denied"=>Err(std::io::ErrorKind::PermissionDenied.into()),_=>Err(std::io::Error::from_raw_os_error(failure_code))}});
  assert_eq!(called,!case["calls"].as_array().unwrap().is_empty(),"{case}");
  if case["error"].is_null(){assert_eq!(json!(result.unwrap()),case["expected"],"{case}");}else{assert_eq!(case["error"]["errno"], 5, "a referência precisa continuar sendo EIO");assert_eq!(result.unwrap_err().raw_os_error(),Some(failure_code));}
 }
}
#[test]
#[ignore]
fn codex_service_pid_child(){println!("ready");std::io::stdout().flush().unwrap();let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();}
#[test]
fn codex_service_pid_queries_do_not_stop_live_child(){
 struct Guard(std::process::Child);
 impl Drop for Guard{fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}
 let mut child=Guard(std::process::Command::new(std::env::current_exe().unwrap()).args(["--ignored","--exact","terminal_process::liveness_tests::codex_service_pid_child","--nocapture"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap());
 let stdout=child.0.stdout.take().unwrap();let mut reader=std::io::BufReader::new(stdout);let mut line=String::new();
 loop{line.clear();assert_ne!(reader.read_line(&mut line).unwrap(),0);if line.trim()=="ready"{break;}}
 let fixture:Value=serde_json::from_str(include_str!("../../../backend/tests/fixtures/contract/codex-integration/liveness.json")).unwrap();
 for _ in 0..3{assert_eq!(json!(pid_alive(child.0.id()).unwrap()),fixture["child"]["alive"]);assert!(child.0.try_wait().unwrap().is_none());}
 drop(child.0.stdin.take());assert!(child.0.wait().unwrap().success());
 assert_eq!(json!(pid_alive(child.0.id()).unwrap()),fixture["child"]["dead"]);
}
