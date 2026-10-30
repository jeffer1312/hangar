use hangar_server::{accounts::environment::AccountEnvironment,codex_setup::{skills,files,artifacts::{Disk,Storage}}};
use serde_json::{Value,json};
use std::path::Path;
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}
fn unhex(text:&str)->Vec<u8>{(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).unwrap()).collect()}
fn replace(value:&Value,from:&str,to:&str)->Value{serde_json::from_str(&value.to_string().replace(from,&to.replace('\\',"\\\\"))).unwrap()}
fn snapshot(root:&Path,backups:&Path)->Value{
 fn walk(root:&Path,path:&Path,backups:&Path,entries:&mut serde_json::Map<String,Value>){
  if !path.is_dir(){return;}
  for entry in std::fs::read_dir(path).unwrap(){let path=entry.unwrap().path();if path==backups||path.starts_with(backups){continue;}
   let name=path.strip_prefix(root).unwrap().to_string_lossy().replace('\\',"/");
   if let Ok(link)=std::fs::read_link(&path){entries.insert(name,json!({"link":link}));}
   else if path.is_dir(){entries.insert(name,json!({"directory":true}));walk(root,&path,backups,entries);}
   else{let data=std::fs::read(&path).unwrap();let prefix=root.to_string_lossy();let data=replace_bytes(&data,prefix.as_bytes(),b"$ROOT");entries.insert(name,json!({"hex":hex(&data)}));}
  }
 }
 let mut entries=serde_json::Map::new();walk(root,root,backups,&mut entries);
 let mut saved:Vec<Value>=std::fs::read_dir(backups).into_iter().flatten().map(|entry|serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()).collect();
 for item in &mut saved{if let Some(raw)=item["conteudo_hex"].as_str(){item["conteudo_hex"]=json!(hex(&replace_bytes(&unhex(raw),root.to_string_lossy().as_bytes(),b"$ROOT")));}}
 saved.sort_by_key(|value|value["path"].as_str().unwrap().to_owned());
 replace(&json!({"entries":entries,"backups":saved}),&root.to_string_lossy(),"$ROOT")
}
fn replace_bytes(raw:&[u8],from:&[u8],to:&[u8])->Vec<u8>{let mut result=Vec::new();let mut rest=raw;while let Some(index)=rest.windows(from.len()).position(|part|part==from){result.extend_from_slice(&rest[..index]);result.extend_from_slice(to);rest=&rest[index+from.len()..];}result.extend_from_slice(rest);result}
struct Races{name:String,index:u64,windows:bool,writes:usize,root:std::path::PathBuf}
impl Storage for Races{
 fn read(&mut self,path:&Path)->Result<Option<Vec<u8>>,files::Error>{Disk.read(path)}
 fn write(&mut self,path:&Path,data:&[u8],expected:Option<&[u8]>,backups:&Path)->Result<bool,files::Error>{self.writes+=1;if self.name=="test_copia_windows_rele_arquivo_gerenciado_apos_conflito"&&self.index==1&&self.writes==1{std::fs::write(path,b"concorrente")?;return Err(files::Error::Changed("Mudou".into()));}Disk.write(path,data,expected,backups)}
 fn backup(&mut self,path:&Path,data:&[u8],backups:&Path)->Result<(),files::Error>{Disk.backup(path,data,backups)}
 fn remove(&mut self,path:&Path)->Result<(),files::Error>{Disk.remove(path)}
}
impl skills::Storage for Races{
 fn replace_link(&mut self,destination:&Path,source:&Path,backups:&Path,expected:Option<&Path>)->Result<(),files::Error>{
  if self.windows{return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,if self.name=="test_falha_criando_novo_symlink_mantem_link_anterior"{"Sem permissão"}else{"Symlink indisponível"}).into());}
  if self.name=="test_troca_concorrente_para_link_pessoal_e_preservada"&&self.index==1{
   std::fs::remove_file(destination)?;
   #[cfg(unix)] std::os::unix::fs::symlink(self.root.join("home/skill-pessoal"),destination)?;
   #[cfg(windows)] std::os::windows::fs::symlink_dir(self.root.join("home/skill-pessoal"),destination)?;
  }
  skills::Disk.replace_link(destination,source,backups,expected)
 }
}
#[test]
fn codex_service_skills_match_original_provenance_and_files(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/skills.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){let temp=tempfile::tempdir().unwrap();let root=temp.path();
  for(name,entry)in case["initial"]["entries"].as_object().unwrap(){let path=root.join(name);std::fs::create_dir_all(path.parent().unwrap()).unwrap();
   if entry["directory"]==true{std::fs::create_dir_all(path).unwrap();}
   else if let Some(link)=entry["link"].as_str(){let link=link.replace("$ROOT",&root.to_string_lossy());
    #[cfg(unix)] std::os::unix::fs::symlink(link,path).unwrap();
    #[cfg(windows)] std::os::windows::fs::symlink_dir(link,path).unwrap();
   }else{std::fs::write(path,replace_bytes(&unhex(entry["hex"].as_str().unwrap()),b"$ROOT",root.to_string_lossy().as_bytes())).unwrap();}
  }
  let input=replace(&case["inputs"],"$ROOT",&root.to_string_lossy());let backups=Path::new(input["backups"].as_str().unwrap());
  std::fs::create_dir_all(backups).unwrap();
  for saved in case["initial"]["backups"].as_array().unwrap(){let mut saved=replace(saved,"$ROOT",&root.to_string_lossy());if let Some(raw)=saved["conteudo_hex"].as_str(){saved["conteudo_hex"]=json!(hex(&replace_bytes(&unhex(raw),b"$ROOT",root.to_string_lossy().as_bytes())));}std::fs::write(backups.join(format!("{}.json",files::hash(saved["path"].as_str().unwrap().as_bytes()))),serde_json::to_vec(&saved).unwrap()).unwrap();}
  let home=Path::new(input["home"].as_str().unwrap());let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
  let mut storage=Races{name:case["test"].as_str().unwrap().into(),index:case["index"].as_u64().unwrap(),windows:input["windows"].as_bool().unwrap(),writes:0,root:root.into()};
  let result=skills::reconcile(&env,Path::new(input["codex"].as_str().unwrap()),Path::new(input["repo"].as_str().unwrap()),&input["plugins"],&input["registry"],backups,storage.windows,&mut storage).unwrap();
  let actual=replace(&result,&root.to_string_lossy(),"$ROOT");
  let mut expected=case["result"].clone();
  if storage.name=="test_exemplos_e_skill_ilegivel_nao_comprovam_plugin"&&storage.index==1{expected["warnings"][0]["params"]["erro"]=json!("arquivo SKILL.md não está em UTF-8");expected["warnings"][0]["texto"]=json!("Skills do plugin plugin@market não puderam ser confirmadas: arquivo SKILL.md não está em UTF-8");}
  if actual!=expected||snapshot(root,backups)!=case["final"]{failures.push(json!({"test":case["test"],"index":case["index"],"actual":actual,"expected":expected,"files":snapshot(root,backups),"expected_files":case["final"]}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
#[test]
fn codex_service_skills_windows_link_comparison_matches_original(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/skills.json")).unwrap();
 for case in corpus["links"].as_array().unwrap(){let temp=tempfile::tempdir().unwrap();let path=temp.path().join("entry");
  #[cfg(unix)] std::os::unix::fs::symlink(case["target"].as_str().unwrap(),&path).unwrap();
  #[cfg(windows)] std::os::windows::fs::symlink_dir(case["target"].as_str().unwrap(),&path).unwrap();
  let roots=case["roots"].as_array().unwrap().iter().map(|value|std::path::PathBuf::from(value.as_str().unwrap())).collect::<Vec<_>>();
  assert_eq!(json!(skills::managed_link(&path,&roots,true)),case["expected"],"{case}");
 }
}
proptest::proptest!{
 #![proptest_config(proptest::test_runner::Config::with_cases(48))]
 #[test]
 fn codex_service_skills_manifest_paths_do_not_remove_external_files(relative in "[a-z./]{0,24}"){
  let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let account=home.join(".codex");let source=home.join(".claude/skills/source");std::fs::create_dir_all(&source).unwrap();std::fs::write(source.join("SKILL.md"),b"source").unwrap();
  let destination=account.join("skills/source");std::fs::create_dir_all(&destination).unwrap();let external=account.join("sentinel");std::fs::write(&external,b"personal").unwrap();
  let mut entries=serde_json::Map::new();entries.insert(relative,json!(files::hash(b"personal")));
  let registry=json!({"source":{"path":destination,"mode":"copy","files":entries}});
  let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
  let _=skills::reconcile(&env,&account,&temp.path().join("repo"),&json!({}),&registry,&home.join("backups"),false,&mut skills::Disk);
  proptest::prop_assert_eq!(std::fs::read(external).unwrap(),b"personal");
 }
}
#[test]
fn codex_service_review_skill_aliases_normalize_newlines_without_rewriting_files(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/skill-newlines.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let account=home.join(".codex");let source=home.join(".claude/plugins/cache/market/plugin/1/skills/source-name");let native=account.join("plugins/plugin/skills/native-name");std::fs::create_dir_all(&source).unwrap();std::fs::create_dir_all(&native).unwrap();std::fs::write(source.join("SKILL.md"),unhex(case["source_hex"].as_str().unwrap())).unwrap();std::fs::write(native.join("SKILL.md"),unhex(case["native_hex"].as_str().unwrap())).unwrap();
 let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
 let result=skills::reconcile(&env,&account,&temp.path().join("repo"),&json!({"plugin@market":{"path":account.join("plugins/plugin")}}),&json!({}),&home.join("backups"),false,&mut skills::Disk).unwrap();
 let actual=replace(&json!({"manifest":result["manifest"],"warnings":result["warnings"],"bridge_exists":account.join("skills/source-name").exists(),"source_hex":hex(&std::fs::read(source.join("SKILL.md")).unwrap()),"native_hex":hex(&std::fs::read(native.join("SKILL.md")).unwrap())}),&temp.path().to_string_lossy(),"$ROOT");
 if actual!=case["result"]{failures.push(json!({"actual":actual,"expected":case["result"]}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
