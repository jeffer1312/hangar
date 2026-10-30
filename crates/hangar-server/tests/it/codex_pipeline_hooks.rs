use hangar_server::codex_setup::{pipeline_hooks::{self,Context},hook_files,phase::{Error,State},status};
use serde_json::{Value,json};
use std::path::Path;
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}
fn bytes(text:&str)->Vec<u8>{(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).unwrap()).collect()}
fn paths(root:&Path)->Vec<std::path::PathBuf>{let mut out=vec![];let mut pending=vec![root.to_path_buf()];while let Some(path)=pending.pop(){if let Ok(entries)=std::fs::read_dir(path){for entry in entries{let path=entry.unwrap().path();if path.is_dir()&&!path.is_symlink(){pending.push(path.clone());}out.push(path);}}}out.sort();out}
#[test]
fn codex_service_pipeline_hooks_preserves_backups_manifest_and_install_order(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/pipeline-hooks.json")).unwrap();let mut differences=vec![];
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let home=root.join("home");let account=home.join(".codex");std::fs::create_dir_all(&account).unwrap();let backups=status::root(&home,&account).unwrap().join("backups");
  let input=&case["input"];for(key,name)in [("current","hooks.json"),("mirror",".hangar-hooks.json")]{if let Some(value)=input.get(key){std::fs::write(account.join(name),hangar_server::transcript::pyjson::dumps_indent(value,1,true)).unwrap();}}
  let state=State::new(status::snapshot());let mut registry=json!({"hooks":input.get("previous").cloned().unwrap_or(json!({}))});let mut events=vec![];
  let wrapper=("python-fixture".into(),root.join("repo/scripts/codex-hook-allow.py").to_string_lossy().into_owned());let context=Context{account:&account,backups:&backups,wrapper:&wrapper,windows:false};
  let result=if input["legacy"]==true{pipeline_hooks::legacy(&context,&state,&mut|path|{events.push(json!(["install",path.to_string_lossy().replace(root.to_str().unwrap(),"ROOT")]));if input["install_error"]==true{Err(Error{kind:"OSError",message:"fixture".into()})}else{Ok(())}})}else{pipeline_hooks::sync(&context,&input["source"],&mut registry,&state)};
  let mut actual=match result{Ok(())=>json!({"ok":true}),Err(error)=>json!({"error":error.kind,"message":error.message})};
  let files=paths(&account).into_iter().filter(|p|p.is_file()).map(|p|(p.strip_prefix(&account).unwrap().to_string_lossy().into_owned(),json!(hex(&std::fs::read(p).unwrap())))).collect::<serde_json::Map<_,_>>();
  let mut backup=paths(&backups).into_iter().filter(|p|p.is_file()).map(|p|hex(std::fs::read_to_string(p).unwrap().replace(root.to_str().unwrap(),"ROOT").as_bytes())).collect::<Vec<_>>();backup.sort();let snapshot=state.snapshot();
  actual["events"]=json!(events);actual["files"]=json!(files);actual["backups"]=json!(backup);actual["registry"]=registry;actual["pending"]=snapshot["confianca_pendente"].clone();actual["warnings"]=snapshot["avisos"].clone();
  if actual!=case["result"]{differences.push(json!({"case":input["name"],"actual":actual,"expected":case["result"]}));}
 }
 assert!(differences.is_empty(),"{differences:#?}");
}
#[cfg(unix)]
#[test]
fn codex_service_pipeline_hook_files_match_original_links_copies_and_invalid_reads(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/hook-files.json")).unwrap();let mut differences=vec![];
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let home=root.join("home");let account=root.join("account");let input=&case["input"];let name=input["namepath"].as_str().unwrap_or("x.sh");let source=home.join(".claude/hooks").join(name);let target=account.join("hooks").join(name);std::fs::create_dir_all(source.parent().unwrap()).unwrap();std::fs::create_dir_all(target.parent().unwrap()).unwrap();
  if let Some(value)=input["source"].as_str(){let raw=bytes(value);if input["source_link"]==true{let actual=root.join("real.sh");std::fs::write(&actual,raw).unwrap();std::os::unix::fs::symlink(actual,&source).unwrap();}else{std::fs::write(&source,raw).unwrap();}}
  if input["source_directory"]==true{std::fs::create_dir(&source).unwrap();}
  if let Some(raw)=input["target"].as_str(){std::fs::write(&target,bytes(raw)).unwrap();}
  if input["target_broken"]==true{std::os::unix::fs::symlink(root.join("missing"),&target).unwrap();}
  let command=if input["relative"]==true{"x.sh".into()}else if input["outside"]==true{root.join("outside.sh").to_string_lossy().into_owned()}else{target.to_string_lossy().into_owned()};
  let document=if input["invalid"]==true{json!([])}else{json!({"hooks":{"PreToolUse":[{"hooks":[{"command":format!("python \"{command}\"")},{"command":format!("python '{command}'")}]}]}})};
  let document=if let Some(kind)=input["file"].as_str(){let raw=serde_json::to_vec(&document).unwrap();let raw=match kind{"utf8"=>vec![255],"json"=>b"{".to_vec(),"bom"=>[&[239,187,191][..],&raw].concat(),_=>raw};std::fs::write(account.join("hooks.json"),raw).unwrap();None}else{Some(&document)};
  let(created,orphans)=hook_files::materialize(&account,&home,document,case["windows"].as_bool().unwrap()).unwrap();
  let mut files=serde_json::Map::new();for path in paths(&account.join("hooks")){let name=path.strip_prefix(&account).unwrap().to_string_lossy().into_owned();if path.is_symlink(){files.insert(name,json!({"link":std::fs::read_link(path).unwrap().to_string_lossy().replace(root.to_str().unwrap(),"ROOT")}));}else if path.is_file(){files.insert(name,json!({"hex":hex(&std::fs::read(path).unwrap())}));}}
  let actual=json!({"created":created,"orphans":orphans,"files":files});if actual!=case["result"]{differences.push(json!({"case":input["name"],"windows":case["windows"],"actual":actual,"expected":case["result"]}));}
 }
 assert!(differences.is_empty(),"{differences:#?}");
}
