//! Ponte de skills com proveniência por arquivo e preservação de conteúdo pessoal.
use super::{artifacts,files,messages,plugins};
use crate::{accounts::environment::AccountEnvironment,config_sync::runtime_values::ValueGuard,orchestration::codec};
use serde_json::{Value,Map,json};
use std::{collections::{BTreeMap,BTreeSet},path::{Path,PathBuf},fs};
pub trait Storage:artifacts::Storage{
 fn replace_link(&mut self,destination:&Path,source:&Path,backups:&Path,expected:Option<&Path>)->Result<(),files::Error>;
}
pub struct Disk;
impl artifacts::Storage for Disk{
 fn read(&mut self,path:&Path)->Result<Option<Vec<u8>>,files::Error>{artifacts::Disk.read(path)}
 fn write(&mut self,path:&Path,data:&[u8],expected:Option<&[u8]>,backups:&Path)->Result<bool,files::Error>{artifacts::Disk.write(path,data,expected,backups)}
 fn backup(&mut self,path:&Path,data:&[u8],backups:&Path)->Result<(),files::Error>{artifacts::Disk.backup(path,data,backups)}
 fn remove(&mut self,path:&Path)->Result<(),files::Error>{artifacts::Disk.remove(path)}
}
fn invalid(text:impl Into<String>)->files::Error{files::Error::Invalid(text.into())}
fn exists(path:&Path)->bool{fs::symlink_metadata(path).is_ok()}
fn resolved(path:&Path)->Result<PathBuf,files::Error>{plugins::resolved(path).map_err(invalid)}
impl Storage for Disk{
 fn replace_link(&mut self,destination:&Path,source:&Path,backups:&Path,expected:Option<&Path>)->Result<(),files::Error>{
  let temp=tempfile::Builder::new().prefix(&format!(".{}.hangar-",destination.file_name().unwrap_or_default().to_string_lossy())).tempfile_in(destination.parent().unwrap())?;
  let path=temp.into_temp_path();fs::remove_file(&path)?;
  #[cfg(unix)] std::os::unix::fs::symlink(source,&path)?;
  #[cfg(windows)] std::os::windows::fs::symlink_dir(source,&path)?;
  if exists(destination){
   let actual=fs::read_link(destination).ok();if actual.as_deref()!=expected||actual.is_none(){return Err(invalid(format!("Entrada deixou de ser link: {}",destination.display())));}
   files::backup(destination,actual.unwrap().to_string_lossy().as_bytes(),backups)?;
  }
  if fs::read_link(destination).ok().as_deref()!=expected||(exists(destination)&&!destination.is_symlink()){return Err(invalid(format!("Entrada alterada durante a troca: {}",destination.display())));}
  crate::runtime::queue::replace(&path,destination)?;Ok(())
 }
}
fn normalized(value:&str,windows:bool)->String{
 let mut value=if windows{value.replace('/',"\\").to_lowercase()}else{value.into()};
 if windows{if value.starts_with("\\\\?\\unc\\"){value=format!("\\\\{}",&value[8..]);}else if value.starts_with("\\\\?\\"){value=value[4..].into();}}
 let separator=if windows{'\\'}else{'/'};let prefix=if value.starts_with(&format!("{separator}{separator}")){format!("{separator}{separator}")}else if value.starts_with(separator){separator.to_string()}else{String::new()};
 let mut parts:Vec<&str>=Vec::new();for part in value.split(separator){match part{""|"."=>{},".."=>{if parts.last().is_some_and(|v|*v!=".."&&!v.ends_with(':')){parts.pop();}else if prefix.is_empty(){parts.push(part);}},_=>parts.push(part)}}
 format!("{prefix}{}",parts.join(&separator.to_string()))
}
pub fn managed_link(path:&Path,roots:&[PathBuf],windows:bool)->bool{
 let Ok(target)=fs::read_link(path)else{return false;};let target=target.to_string_lossy();
 let absolute=if windows{target.starts_with(['\\','/'])||target.as_bytes().get(1)==Some(&b':')}else{target.starts_with('/')};
 let target=if absolute{target.into_owned()}else{format!("{}{}{}",path.parent().unwrap().display(),if windows{'\\'}else{'/'},target)};
 let target=normalized(&target,windows);let separator=if windows{'\\'}else{'/'};
 roots.iter().any(|root|{let root=normalized(&root.to_string_lossy(),windows);target==root||target.strip_prefix(&root).is_some_and(|suffix|suffix.starts_with(separator))})
}
fn children(path:&Path)->Result<Vec<PathBuf>,files::Error>{let mut entries=fs::read_dir(path)?.map(|entry|entry.map(|entry|entry.path())).collect::<Result<Vec<_>,_>>()?;entries.sort();Ok(entries)}
fn tree(root:&Path)->Result<BTreeMap<String,Vec<u8>>,files::Error>{
 fn walk(root:&Path,path:&Path,out:&mut BTreeMap<String,Vec<u8>>)->Result<(),files::Error>{
  if !path.is_dir(){return Ok(());}
  for child in children(path)?{if child.is_dir(){if child.is_symlink(){return Err(invalid(format!("Subdiretório ligado externamente: {}",child.display())));}walk(root,&child,out)?;}else{out.insert(child.strip_prefix(root).unwrap().to_string_lossy().replace('\\',"/"),fs::read(&child)?);}}Ok(())
 }
 let mut out=BTreeMap::new();walk(root,root,&mut out)?;Ok(out)
}
fn names(path:&Path)->Result<BTreeSet<String>,files::Error>{
 let bytes=fs::read(path.join("SKILL.md"))?;let text=std::str::from_utf8(&bytes).map_err(|_|invalid("arquivo SKILL.md não está em UTF-8"))?;
 if crate::accounts::quotas::other::py_strip(text).is_empty(){return Ok(BTreeSet::new());}
 let mut names=BTreeSet::from([path.file_name().unwrap_or_default().to_string_lossy().into_owned()]);
 if text.starts_with("---\n"){
  let header=text.split_once("\n---").map(|(head,_)|head).unwrap_or(text);
  static NAME:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(r#"(?m)^name:\s*['\"]?([\w.:-]+)['\"]?\s*$"#).unwrap());
  if let Some(found)=NAME.captures(header){names.insert(found[1].into());}
 }Ok(names)
}
fn warning(warnings:&mut Vec<Value>,code:&str,params:Value){warnings.push(messages::message(code,params.as_object().unwrap()).expect("mensagem de skills versionada"));}
fn native(plugins:&Value,warnings:&mut Vec<Value>)->Result<Vec<(String,BTreeSet<String>)>,files::Error>{
 let mut out=Vec::new();let plugins=plugins.as_object().ok_or_else(||invalid("Plugins precisam ser um objeto"))?;
 for(id,plugin)in plugins{
  let result=(||->Result<(),files::Error>{
   let root=Path::new(plugin.get("path").and_then(Value::as_str).ok_or_else(||invalid("Plugin sem caminho"))?);let mut manifest=ValueGuard(Value::Null);
   for relative in [".codex-plugin/plugin.json",".claude-plugin/plugin.json"]{let path=root.join(relative);if path.is_file(){manifest=ValueGuard(files::object(&path)?);break;}}
   let declared=manifest.get("skills").unwrap_or(&Value::Null);let defaults=json!(["skills"]);
   let roots=if declared.is_null(){&defaults}else{declared};let paths:Vec<_>=if let Some(path)=roots.as_str(){vec![path]}else{roots.as_array().ok_or_else(||invalid("Raízes de skills inválidas"))?.iter().map(|value|value.as_str().ok_or_else(||invalid("Raízes de skills inválidas"))).collect::<Result<_,_>>()?};
   for relative in paths{let relative=Path::new(relative);if relative.is_absolute()||relative.components().any(|p|p==std::path::Component::ParentDir){return Err(invalid("Raiz de skill fora do plugin"));}
    let path=root.join(relative);let skills=if path.join("SKILL.md").is_file(){vec![path]}else{crate::harness_setup::skills::skills(&path)?};
    for skill in skills{let names=names(&skill)?;if !names.is_empty(){out.push((id.clone(),names));}}
   }Ok(())
  })();if let Err(error)=result{warning(warnings,"aviso_plugin_skills",json!({"id":id,"erro":error.to_string()}));}
 }Ok(out)
}
fn same_plugin(source:&Path,id:&str,home:&Path)->Result<bool,files::Error>{
 let Some((name,market))=id.rsplit_once('@').filter(|(a,b)|!a.is_empty()&&!b.is_empty())else{return Ok(false);};
 let real=resolved(source)?;if real.starts_with(resolved(&home.join(".claude/plugins/cache").join(market).join(name))?){return Ok(true);}
 let market=resolved(&home.join(".claude/plugins/marketplaces").join(market))?;if !real.starts_with(&market){return Ok(false);}
 for path in real.ancestors(){if !path.starts_with(&market){break;}let manifest=path.join(".claude-plugin/plugin.json");if manifest.is_file(){return Ok(files::object(&manifest).ok().map(ValueGuard).is_some_and(|data|data["name"]==name));}}
 let Ok(data)=files::object(&market.join(".claude-plugin/marketplace.json"))else{return Ok(false);};let data=ValueGuard(data);
 for plugin in data["plugins"].as_array().into_iter().flatten(){if plugin["name"]!=name||plugin["strict"]!=false{continue;}
  let Some(source)=plugin["source"].as_str().filter(|s|s.starts_with("./"))else{continue;};let root=resolved(&market.join(source))?;
  if !root.starts_with(&market)||!real.starts_with(&root){continue;}
  let default=json!(["./skills"]);let skills=plugin.get("skills").unwrap_or(&default);let paths:Vec<_>=if let Some(path)=skills.as_str(){vec![path]}else{skills.as_array().into_iter().flatten().filter_map(Value::as_str).collect()};
  for path in paths{let path=resolved(&root.join(path))?;if path.starts_with(&root)&&real.starts_with(path){return Ok(true);}}return Ok(false);
 }Ok(false)
}
fn previous(value:&Value,destination:&Path)->Value{
 let Some(map)=value.as_object().filter(|map|!map.is_empty())else{return json!({});};
 if map.get("path").is_some_and(|value|value.as_str()!=Some(destination.to_string_lossy().as_ref())){return json!({});}
 let mut result=codec::clone_value(value);
 if !map.contains_key("mode"){result["mode"]=match value["tipo"].as_str(){Some("link")=>json!("symlink"),Some("copia")=>json!("copy"),_=>Value::Null};}
 if !map.contains_key("files"){result["files"]=map.get("arquivos").map(codec::clone_value).unwrap_or_else(||json!({}));}result
}
fn safe(destination:&Path,relative:&str)->Result<PathBuf,files::Error>{
 let relative=Path::new(relative);if relative.is_absolute()||relative.components().any(|p|p==std::path::Component::ParentDir){return Err(invalid("Caminho inválido no manifesto de skills"));}
 let path=destination.join(relative);for parent in path.ancestors(){if parent==destination{break;}if parent.is_symlink(){return Err(invalid(format!("Arquivo da skill virou link: {}",parent.display())));}}Ok(path)
}
fn copy(destination:&Path,source:&Path,previous:&Value,backups:&Path,warnings:&mut Vec<Value>,storage:&mut impl Storage)->Result<Value,files::Error>{
 let current=tree(source)?;let empty=Map::new();let old=previous["files"].as_object().unwrap_or(&empty);let mut managed=Map::new();fs::create_dir_all(destination)?;
 for(relative,data)in &current{let result=(||->Result<(),files::Error>{let path=safe(destination,relative)?;
  for attempt in 0..3{let bytes=storage.read(&path)?;if bytes.is_some()&&!old.contains_key(relative){warning(warnings,"aviso_arquivo_exclusivo",json!({"path":path}));break;}
   match storage.write(&path,data,bytes.as_deref(),backups){Ok(_)=>{managed.insert(relative.clone(),json!(files::hash(data)));break;},Err(files::Error::Changed(_))if attempt<2=>{},Err(error)=>return Err(error)}
  }Ok(())})();if let Err(error)=result{warning(warnings,"aviso_arquivo_falha",json!({"path":destination.join(relative),"erro":error.to_string()}));if let Some(old)=old.get(relative){managed.insert(relative.clone(),codec::clone_value(old));}}
 }
 for(relative,hash)in old{if current.contains_key(relative){continue;}let result=(||->Result<(),files::Error>{let path=safe(destination,relative)?;let Some(bytes)=storage.read(&path)?else{return Ok(());};
  if Some(files::hash(&bytes).as_str())!=hash.as_str(){warning(warnings,"aviso_arquivo_removido_alterado",json!({"path":path}));managed.insert(relative.clone(),codec::clone_value(hash));return Ok(());}
  storage.backup(&path,&bytes,backups)?;if storage.read(&path)?.as_deref()!=Some(&bytes){return Err(invalid(format!("Arquivo alterado durante a retirada: {}",path.display())));}storage.remove(&path)
 })();if let Err(error)=result{warning(warnings,"aviso_arquivo_falha",json!({"path":destination.join(relative),"erro":error.to_string()}));managed.insert(relative.clone(),codec::clone_value(hash));}}
 Ok(Value::Object(Map::from_iter([("path".into(),json!(destination)),("mode".into(),json!("copy")),("source".into(),json!(source)),("files".into(),Value::Object(managed))])))
}
fn prune(path:&Path)->Result<(),files::Error>{if !path.is_dir()||path.is_symlink(){return Ok(());}for child in children(path)?{if child.is_dir()&&!child.is_symlink(){prune(&child)?;}}if fs::read_dir(path)?.next().is_none(){fs::remove_dir(path)?;}Ok(())}
fn remove_copy(destination:&Path,previous:&Value,backups:&Path,warnings:&mut Vec<Value>,storage:&mut impl Storage)->Result<Value,files::Error>{
 let mut remaining=Map::new();for(relative,hash)in previous["files"].as_object().into_iter().flatten(){let path=safe(destination,relative)?;let Some(bytes)=storage.read(&path)?else{continue;};if Some(files::hash(&bytes).as_str())!=hash.as_str(){remaining.insert(relative.clone(),codec::clone_value(hash));warning(warnings,"aviso_ponte_alteracao_local",json!({"path":path}));continue;}
  storage.backup(&path,&bytes,backups)?;if storage.read(&path)?.as_deref()!=Some(&bytes){return Err(invalid(format!("Arquivo alterado durante a retirada: {}",path.display())));}storage.remove(&path)?;
 }prune(destination)?;if destination.exists(){warning(warnings,"aviso_ponte_conteudo_pessoal",json!({"destino":destination}));}let mut result=codec::clone_value(previous);let old=result.as_object_mut().unwrap().insert("files".into(),Value::Object(remaining));if let Some(old)=old{codec::discard(old);}Ok(result)
}
fn duplicate(name:&str,source:&Path,home:&Path,previous_value:&Value,backups:&Path,warnings:&mut Vec<Value>,storage:&mut impl Storage)->Result<(),files::Error>{
 let destination=home.join(".agents/skills").join(name);if !exists(&destination)||resolved(&destination)?==resolved(source)?{return Ok(());}
 if destination.is_symlink()||!destination.is_dir(){warning(warnings,"aviso_skill_nativa_sem_proveniencia",json!({"destino":destination}));return Ok(());}
 let bytes=tree(&destination)?;let hashes:Map<_,_>=bytes.iter().map(|(name,data)|(name.clone(),json!(files::hash(data)))).collect();let previous=ValueGuard(previous(previous_value,&destination));
 if previous["mode"]!="copy"||previous["files"]!=Value::Object(hashes)||previous.get("source").or_else(||previous.get("origem")).and_then(Value::as_str)!=Some(source.to_string_lossy().as_ref()){warning(warnings,"aviso_skill_duplicata_plugin",json!({"nome":name}));return Ok(());}
 for(relative,data)in &bytes{storage.backup(&destination.join(relative),data,backups)?;}
 if tree(&destination)?!=bytes{return Err(invalid(format!("Skill nativa alterada durante a reconciliação: {}",destination.display())));}
 for relative in bytes.keys(){storage.remove(&destination.join(relative))?;}prune(&destination)
}
pub fn reconcile(env:&AccountEnvironment,account:&Path,repo:&Path,plugins:&Value,registry:&Value,backups:&Path,windows:bool,storage:&mut impl Storage)->Result<Value,files::Error>{
 let mut warnings=Vec::new();let mut sources=crate::harness_setup::skills::sources(env,repo)?;let available=!sources.is_empty();let keep=env.home.join(".claude/ecc-slim-keep.txt");
 if keep.is_file(){let text=fs::read_to_string(keep)?;let allowed:BTreeSet<_>=text.lines().collect();let mut filtered=BTreeMap::new();for(name,path)in sources{if allowed.contains(name.as_str())||!same_plugin(&path,"ecc@ecc",&env.home)?{filtered.insert(name,path);}}sources=filtered;}
 let native=native(plugins,&mut warnings)?;let bridge=account.join("skills");fs::create_dir_all(&bridge)?;let roots=crate::harness_setup::skills::roots(env,repo);let mut manifest=Map::new();
 for(name,source)in &sources{if name==".system"{continue;}let destination=bridge.join(name);let old=ValueGuard(previous(&registry[name],&destination));
  let result=(||->Result<(),files::Error>{let names=names(source)?;let mut native_id=None;for(id,native_names)in &native{if !names.is_disjoint(native_names)&&same_plugin(source,id,&env.home)?{native_id=Some(id);break;}}
   let agents=env.home.join(".agents/skills");let already_native=resolved(source)?.starts_with(resolved(&agents)?);let in_agents=!already_native&&exists(&agents.join(name));
   if native_id.is_some()||already_native||in_agents{
    if managed_link(&destination,&roots,cfg!(windows)){storage.backup(&destination,fs::read_link(&destination)?.to_string_lossy().as_bytes(),backups)?;storage.remove(&destination)?;}
    manifest.insert(name.clone(),json!({"path":destination,"mode":"native","source":source,"plugin":native_id,"files":{}}));
    if !destination.is_symlink()&&old["mode"]=="copy"{let rest=remove_copy(&destination,&old,backups,&mut warnings,storage)?;if destination.exists(){manifest.insert(name.clone(),rest);}else{codec::discard(rest);}}
    if native_id.is_some(){duplicate(name,source,&env.home,&registry[name],backups,&mut warnings,storage)?;}else if in_agents&&agents.join(name).is_dir()&&tree(&agents.join(name))?!=tree(source)?{warning(&mut warnings,"aviso_skill_agents_difere",json!({"nome":name}));}return Ok(());
   }
   if exists(&destination){if destination.is_symlink(){if !managed_link(&destination,&roots,cfg!(windows)){if resolved(&destination)?!=resolved(source)?{warning(&mut warnings,"aviso_link_pessoal",json!({"destino":destination}));}return Ok(());}if resolved(&destination)?==resolved(source)?{manifest.insert(name.clone(),json!({"path":destination,"mode":"symlink","source":source,"files":{}}));return Ok(());}}
    else if old["mode"]!="copy"{warning(&mut warnings,"aviso_skill_pessoal",json!({"destino":destination}));return Ok(());}else{manifest.insert(name.clone(),copy(&destination,source,&old,backups,&mut warnings,storage)?);return Ok(());}}
   let expected=fs::read_link(&destination).ok();match storage.replace_link(&destination,source,backups,expected.as_deref()){
    Ok(())=>{manifest.insert(name.clone(),json!({"path":destination,"mode":"symlink","source":source,"files":{}}));},
    Err(files::Error::Io(_))if windows&&!exists(&destination)=>{manifest.insert(name.clone(),copy(&destination,source,&old,backups,&mut warnings,storage)?);},
    Err(error)=>return Err(error),
   }Ok(())
  })();if let Err(error)=result{warning(&mut warnings,"aviso_skill_falha",json!({"nome":name,"erro":error.to_string()}));if !manifest.contains_key(name)&&old.as_object().is_some_and(|v|!v.is_empty()){manifest.insert(name.clone(),codec::clone_value(&old));}}
 }
 if available{
  for destination in children(&bridge)?{let name=destination.file_name().unwrap().to_string_lossy();if name!=".system"&&!sources.contains_key(name.as_ref())&&managed_link(&destination,&roots,cfg!(windows)){storage.backup(&destination,fs::read_link(&destination)?.to_string_lossy().as_bytes(),backups)?;storage.remove(&destination)?;}}
  for(name,value)in registry.as_object().ok_or_else(||invalid("Manifesto de skills precisa ser um objeto"))?{
   if Path::new(name).file_name().and_then(|v|v.to_str())!=Some(name)||matches!(name.as_str(),"."|".."){warning(&mut warnings,"aviso_skill_nome_invalido",json!({"nome":name}));continue;}
   let destination=bridge.join(name);let old=ValueGuard(previous(value,&destination));if !sources.contains_key(name)&&name!=".system"&&old["mode"]=="copy"&&destination.is_dir()&&!destination.is_symlink(){match remove_copy(&destination,&old,backups,&mut warnings,storage){Ok(rest)=>{if destination.exists(){manifest.insert(name.clone(),rest);}else{codec::discard(rest);}},Err(error)=>{manifest.insert(name.clone(),codec::clone_value(&old));warning(&mut warnings,"aviso_copia_obsoleta_falha",json!({"nome":name,"erro":error.to_string()}));}}}
  }
 }else{for(_,value)in std::mem::take(&mut manifest){codec::discard(value);}manifest=registry.as_object().ok_or_else(||invalid("Manifesto de skills precisa ser um objeto"))?.iter().map(|(key,value)|(key.clone(),codec::clone_value(value))).collect();warning(&mut warnings,"aviso_skills_sem_fonte",json!({}));}
 Ok(Value::Object(Map::from_iter([("manifest".into(),Value::Object(manifest)),("warnings".into(),Value::Array(warnings))])))
}
