use hangar_server::codex_setup::{files,status};
use serde_json::{Value,json};
#[test]
fn codex_service_status_matches_original_restart_messages_and_disk_precedence(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/status.json")).unwrap();
 let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let home=temp.path();let account=home.join(".codex");
  let root=home.join(".hangar/codex-integracao").join(&files::hash(account.to_string_lossy().as_bytes())[..24]);
  if !case["disk"].is_null(){
   std::fs::create_dir_all(&root).unwrap();
   let raw=case["disk"].as_str().map(str::to_owned).unwrap_or_else(||case["disk"].to_string());
   std::fs::write(root.join("estado.json"),raw).unwrap();
  }
  let memory=case["memory"].as_object().map(|fields|{let mut value=status::snapshot();value.as_object_mut().unwrap().extend(fields.clone());value});
  let actual=match status::read(home,&account,memory.as_ref(),1000.0){Ok(v)=>json!({"ok":v}),Err(e)=>json!({"error":e.kind,"message":e.message})};
  if actual!=case["expected"]{failures.push(json!({"name":case["name"],"actual":actual,"expected":case["expected"]}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
