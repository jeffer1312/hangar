use hangar_server::codex_setup::repair::{self, Actions};
use serde_json::{Value,json};
use std::path::{Path,PathBuf};
struct Fixture { name:String, data:Value, calls:Vec<Value>, root:PathBuf }
impl Fixture {
 fn path(&self,p:&Path)->String { p.to_string_lossy().replace(&*self.root.to_string_lossy(),"$ROOT").replace('\\',"/") }
}
impl Actions for Fixture {
 async fn install(&mut self, account:&Path)->Result<(),String> {
  self.calls.push(json!(["hooks",self.path(account)]));
  std::fs::write(account.join("hooks.json"),b"installed").unwrap(); Ok(())
 }
 fn available(&self)->bool {self.name!="no-cli"}
 async fn plugin(&mut self,root:&Path,_backups:&Path)->Result<(),String> {
  self.calls.push(json!(["plugin",self.path(root)]));
  if self.name=="plugin-failure" {Err("fixture plugin".into())}else{Ok(())}
 }
 async fn open(&mut self)->Result<(),String>{self.calls.push(json!(["open"]));Ok(())}
 async fn request(&mut self,method:&str,params:Value)->Result<Value,String>{
  self.calls.push(json!([method,params]));
  if method=="config/batchWrite" && self.name=="batch-failure" {Err("fixture batch".into())}
  else if method=="hooks/list" {Ok(self.data.clone())}else{Ok(json!({}))}
 }
 async fn close(&mut self)->Result<(),String>{self.calls.push(json!(["close"]));Ok(())}
}
fn decode(s:&str)->Vec<u8>{(0..s.len()).step_by(2).map(|i|u8::from_str_radix(&s[i..i+2],16).unwrap()).collect()}
fn hex(path:&Path)->Value{std::fs::read(path).map(|b|Value::String(b.iter().map(|x|format!("{x:02x}")).collect())).unwrap_or(Value::Null)}
#[cfg(unix)]
#[tokio::test]
async fn codex_service_matches_original_order_backup_and_partial_effects(){
 use std::os::unix::fs::PermissionsExt;
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/repair.json")).unwrap();
 let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let home=temp.path();let account=home.join("account");std::fs::create_dir(&account).unwrap();
  let backup=home.join(".hangar/codex-prepare-once/backups/fixture/hooks.json");
  for (field,path) in [("original_hex",account.join("hooks.json")),("backup_hex",backup.clone())]{
   if let Some(s)=case[field].as_str(){std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,decode(s)).unwrap();}
  }
  for p in case["plugins"].as_array().unwrap(){std::fs::create_dir_all(account.join("plugins/cache/ecc/ecc").join(p.as_str().unwrap())).unwrap();}
  let mut actions=Fixture{name:case["name"].as_str().unwrap().into(),data:case["data"].clone(),calls:vec![],root:home.to_owned()};
  let result=repair::repair_locked(home,&account,"fixture",&mut actions).await;
  let actual=match result {Ok(v)=>json!({"ok":v}),Err(e)=>json!({"message":e})};
  let expected=if case["result"].get("error").is_some(){json!({"message":case["result"]["message"]})}else{case["result"].clone()};
  let private_mode=std::fs::metadata(home.join(".hangar/codex-prepare-once")).ok().map(|m|m.permissions().mode()&0o777);
  if actual!=expected || json!(actions.calls)!=case["calls"] || hex(&backup)!=case["final_backup_hex"] || hex(&account.join("hooks.json"))!=case["final_hooks_hex"] || json!(private_mode)!=case["private_mode"]{
   failures.push(json!({"case":case["name"],"actual":actual,"expected":expected,"calls":actions.calls,"expected_calls":case["calls"],"mode":private_mode}));
  }
 }
 assert!(failures.is_empty(),"{failures:#?}");
}

#[test]
fn codex_service_legacy_matches_original_preserving_personal_commands() {
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/legacy-hooks.json")).unwrap();
 let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let repo=root.join("repo");let python=root.join("python");
  let windows=case["windows"].as_bool().unwrap();
  // O instalador Windows não produz comandos para shlex POSIX.
  if cfg!(windows)&&!windows{continue;}
  std::fs::create_dir_all(repo.join("backend/hooks")).unwrap();
  if case["current_exists"]==true {std::fs::write(&python,[]).unwrap();}
  if case["legacy_exists"]==true {
   let path=repo.join(if windows{"backend/.venv/Scripts/python.exe"}else{"backend/.venv/bin/python"});
   std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,[]).unwrap();
  }
  let mut hooks=hangar_server::codex_setup::files::remap(&case["before"],Path::new("$ROOT"),root);
  let touched=hangar_server::harness_setup::codex_hooks::repair_legacy(&mut hooks,&repo,&python,windows);
  let hooks=crate::contract_paths::value(&hooks,root,"$ROOT");
  if hooks!=case["after"] || json!(touched)!=case["result"]["ok"] {
   failures.push(json!({"windows":windows,"current":case["current_exists"],"legacy":case["legacy_exists"],"form":case["form"],"hooks":hooks,"expected":case["after"],"touched":touched,"expected_touched":case["result"]}));
  }
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
