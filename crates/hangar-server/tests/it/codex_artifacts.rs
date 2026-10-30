use hangar_server::codex_setup::{artifacts::{self,Disk,Storage},files};
use serde_json::{Value,json};
use std::{collections::{BTreeMap,BTreeSet},path::Path};
fn unhex(text:&str)->Vec<u8>{(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).unwrap()).collect()}
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}
struct Races {name:String,writes:usize,reads:usize}
impl Storage for Races {
 fn read(&mut self,path:&Path)->Result<Option<Vec<u8>>,files::Error>{self.reads+=1;if self.name=="stale-race"&&self.reads==2{std::fs::write(path,b"race")?;}Disk.read(path)}
 fn write(&mut self,path:&Path,data:&[u8],expected:Option<&[u8]>,backups:&Path)->Result<bool,files::Error>{self.writes+=1;if (self.name=="retry-ok"&&self.writes<=2)||(self.name=="retry-exhausted"&&self.writes<=3){std::fs::write(path,format!("race-{}",self.writes))?;return Err(files::Error::Changed("fixture conflict".into()));}Disk.write(path,data,expected,backups)}
 fn backup(&mut self,path:&Path,data:&[u8],backups:&Path)->Result<(),files::Error>{Disk.backup(path,data,backups)}
 fn remove(&mut self,path:&Path)->Result<(),files::Error>{Disk.remove(path)}
}
#[test]
fn codex_service_artifacts_match_original_files_backups_and_races(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/artifacts.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let name=case["name"].as_str().unwrap();let temp=tempfile::tempdir().unwrap();let root=temp.path();let path=root.join("account/artifact.md");std::fs::create_dir_all(path.parent().unwrap()).unwrap();let backups=root.join("backups");let external=root.join("external.md");
  if let Some(raw)=case["original_hex"].as_str(){let data=unhex(raw);if name.contains("symlink"){
   std::fs::write(&external,&data).unwrap();
   #[cfg(unix)] std::os::unix::fs::symlink(&external,&path).unwrap();
   #[cfg(windows)] std::os::windows::fs::symlink_file(&external,&path).unwrap();
  }else{std::fs::write(&path,data).unwrap();}}
  let previous:Value=serde_json::from_str(&case["previous"].to_string().replace("$ROOT",&root.to_string_lossy().replace('\\',"\\\\"))).unwrap();
  let desired=if case["desired"]==true{BTreeMap::from([(path.clone(),b"source".to_vec())])}else{BTreeMap::new()};let trusted=if case["trusted"]==true{BTreeSet::from([path.clone()])}else{BTreeSet::new()};
  let mut storage=Races{name:name.into(),writes:0,reads:0};let result=artifacts::reconcile(&desired,&previous,&backups,&trusted,&mut storage).unwrap();
  let mut saved:Vec<_>=std::fs::read_dir(&backups).into_iter().flatten().map(|p|serde_json::from_slice::<Value>(&std::fs::read(p.unwrap().path()).unwrap()).unwrap()).collect();saved.sort_by_key(Value::to_string);
  let actual=json!({"manifest":result["manifest"],"warnings":result["warnings"],"backups":saved,"writes":storage.writes,"final_hex":std::fs::read(&path).ok().map(|v|hex(&v)),"link":std::fs::read_link(&path).ok(),"external_hex":std::fs::read(&external).ok().map(|v|hex(&v))});
  let actual:Value=serde_json::from_str(&actual.to_string().replace(&root.to_string_lossy().replace('\\',"\\\\"),"$ROOT")).unwrap();
  if actual!=case["result"]{failures.push(json!({"name":name,"actual":actual,"expected":case["result"]}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
