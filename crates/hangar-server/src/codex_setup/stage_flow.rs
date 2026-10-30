//! Importação isolada de fragmentos antes da adoção dos arquivos e da configuração.
use super::phase::{Error,State};
use crate::accounts::environment::AccountEnvironment;
use serde_json::Value;
use std::{future::Future,path::Path};
pub struct Paths<'a>{pub env:&'a AccountEnvironment,pub home:&'a Path,pub account:&'a Path,pub work:&'a Path,pub backups:&'a Path}
pub struct Configuration<'a>{pub mcp:&'a Value,pub agents:&'a Value,pub hooks:bool,pub memory:bool,pub history:&'a Value,pub env:&'a Value}
pub trait Actions {
 fn history(&mut self)->impl Future<Output=Result<Value,Error>>+Send;
 fn fingerprint(&mut self)->Result<String,Error>;
 fn open(&mut self,home:&Path,account:&Path,memory:bool)->impl Future<Output=Result<(),Error>>+Send;
 fn detect(&mut self)->impl Future<Output=Result<Vec<Value>,Error>>+Send;
 fn import(&mut self,items:Vec<Value>)->impl Future<Output=Result<Value,Error>>+Send;
 fn close(&mut self)->impl Future<Output=Result<(),Error>>+Send;
 fn configure(&mut self,config:Configuration<'_>,registry:&mut Value)->impl Future<Output=Result<(),Error>>+Send;
 fn hooks(&mut self,value:&Value,registry:&mut Value)->Result<(),Error>;
 fn checkpoint(&mut self,registry:&mut Value)->Result<(),Error>;
}
use super::{files,stage_sources,artifacts,hooks,messages,instructions_toml};
use crate::{config_sync::{runtime_values::ValueGuard,value_equality::equal},orchestration::codec};
use serde_json::{Map,json};
use std::{collections::{BTreeMap,BTreeSet},path::PathBuf};
fn error(kind:&'static str,message:impl Into<String>)->Error{Error{kind,message:message.into()}}
fn io(error:std::io::Error)->Error{Error{kind:"OSError",message:error.to_string()}}
fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{value.as_object().map(|v|v.get(key).unwrap_or(default)).ok_or_else(||error("AttributeError","Configuração precisa ser um objeto"))}
fn read_json(path:&Path)->Result<Value,Error>{Ok(files::object(path)?)}
fn toml(path:&Path)->Result<Value,Error>{match files::read(path)?{None=>Ok(json!({})),Some(raw)=>instructions_toml::parse(std::str::from_utf8(&raw).map_err(|_|error("UnicodeDecodeError","Configuração TOML não está em UTF-8"))?).map_err(Into::into)}}
fn warning(state:&State,code:&str,params:Value){state.update(|v|v["avisos"].as_array_mut().expect("Estado inicializado").push(messages::message(code,params.as_object().unwrap()).expect("Mensagem versionada")));}
fn replace(value:&mut Value,key:&str,new:Value)->Result<(),Error>{let map=value.as_object_mut().ok_or_else(||error("TypeError","Registro precisa ser um objeto"))?;if let Some(old)=map.insert(key.into(),new){codec::discard(old);}Ok(())}
fn descendants(root:&Path)->Vec<PathBuf>{
 let mut out=Vec::new();let mut pending=vec![root.to_path_buf()];
 while let Some(path)=pending.pop(){let Ok(entries)=std::fs::read_dir(path)else{continue;};for entry in entries.flatten(){let path=entry.path();if path.is_dir()&&!path.is_symlink(){pending.push(path.clone());}out.push(path);}}
 out.sort();out
}
fn remap(value:&Value,stage:&Path,codex:&Path,home:&Path,account:&Path)->Value{
 let intermediate=ValueGuard(files::remap(value,codex,account));files::remap(&intermediate,stage,home)
}
fn string(value:&Value)->bool{value.is_string()&&!codec::contains_nonfinite(value)}
fn env_valid(value:&Value)->bool{value.as_object().is_some_and(|map|map.iter().all(|(key,value)|!key.is_empty()&&!key.contains(['=','\0'])&&string(value)&&!codec::python_string(value).contains('\0')))}
fn mcp_valid(value:&Value)->bool{
 value.as_object().is_some_and(|entries|entries.values().all(|v|{
  let Some(v)=v.as_object()else{return false;};
  v.get("command").or_else(||v.get("url")).is_some_and(string)&&v.get("args").is_none_or(|v|v.as_array().is_some_and(|items|items.iter().all(string)))&&v.get("env").is_none_or(|v|v.as_object().is_some_and(|map|map.values().all(string)))
 }))
}
fn item_type(item:&Value)->Result<Option<&str>,Error>{
 let value=get(item,"itemType",&Value::Null)?;
 if value.is_array()||value.is_object(){return Err(error("TypeError","Tipo de item não pode ser usado como chave"));}
 Ok(value.as_str())
}
pub async fn run(paths:&Paths<'_>,registry:&mut Value,state:&State,memory:bool,actions:&mut impl Actions)->Result<(),Error>{
 let history=ValueGuard(actions.history().await?);let beginning=actions.fingerprint()?;
 let raw=files::read(&paths.home.join(".claude/settings.json"))?.ok_or_else(||error("ValueError","Fonte do Claude indisponível"))?;
 let settings=ValueGuard(crate::orchestration::body::parse(&raw).map_err(|_|error("ValueError","Fonte do Claude inválida"))?);
 let temp=tempfile::Builder::new().prefix("import-").tempdir_in(paths.work).map_err(io)?;
 let result=import_stage(paths,registry,state,memory,actions,&history,&beginning,&settings,temp.path()).await;
 // A guarda de integração do chamador só sai depois da limpeza explícita do stage.
 temp.close().map_err(io)?;result
}
async fn import_stage(paths:&Paths<'_>,registry:&mut Value,state:&State,memory:bool,actions:&mut impl Actions,history:&Value,beginning:&str,settings:&Value,stage:&Path)->Result<(),Error>{
 let empty=json!({});let cc=stage.join(".claude");let cx=stage.join(".codex");std::fs::create_dir(&cc).map_err(io)?;std::fs::create_dir(&cx).map_err(io)?;
 let env=get(settings,"env",&empty)?;if !env_valid(env){return Err(error("ValueError","settings.env inválido; variáveis existentes preservadas"));}
 let config=ValueGuard(json!({"hooks":stage_sources::without_app_hooks(get(settings,"hooks",&empty)?),"env":codec::clone_value(env)}));
 let checked=ValueGuard(hooks::merge(&empty,&config,&empty).map_err(|message|error("ValueError",message))?);drop(checked);
 files::write(&cc.join("settings.json"),&files::bytes(&config)?,None,None)?;
 for name in ["commands","agents"]{let source=paths.home.join(".claude").join(name);if source.is_dir(){stage_sources::copy_tree(&source,&cc.join(name)).map_err(io)?;}}
 let imported=get(registry,"artefatos",&empty)?;
 std::fs::create_dir_all(cc.join("agents")).map_err(io)?;
 let (agents,unreadable)=stage_sources::agents(paths.home,settings)?;
 for path in unreadable{warning(state,"aviso_agente_plugin_ilegivel",json!({"path":path.to_string_lossy()}));}
 for(name,text)in agents{
  let target=paths.account.join("agents").join(format!("{name}.toml"));let source=cc.join("agents").join(format!("{name}.md"));
  if target.exists()&&imported.get(target.to_string_lossy().as_ref()).is_none()||source.exists(){continue;}
  std::fs::write(source,text.as_bytes()).map_err(io)?;
 }
 if memory{
  let projects=paths.home.join(".claude/projects");if !projects.is_dir(){warning(state,"aviso_memoria_sem_fonte",json!({}));}
  let errors=stage_sources::copy_memory(&projects,&cc.join("projects"));if !errors.is_empty(){warning(state,"aviso_memoria_erro",json!({"n":errors.len(),"exemplos":errors.iter().take(3).cloned().collect::<Vec<_>>().join(", ")}));}
 }
 let mcp_path=paths.home.join(".claude.json");
 if files::read(&mcp_path)?.is_some(){
  let data=ValueGuard(read_json(&mcp_path)?);let mcp=get(&data,"mcpServers",&empty)?;
  if !mcp.as_object().is_some_and(|m|m.values().all(Value::is_object)){return Err(error("ValueError","Configuração MCP inválida; servidores existentes preservados"));}
  if !mcp_valid(mcp){return Err(error("ValueError","Entrada MCP inválida; servidores existentes preservados"));}
  let source=ValueGuard(json!({"mcpServers":codec::clone_value(mcp)}));files::write(&stage.join(".claude.json"),&files::bytes(&source)?,None,None)?;
 }
 actions.open(stage,&cx,memory).await?;
 let imported=import_items(actions,state,&cc,memory).await;
 actions.close().await?;let frozen=imported?;
 let config=ValueGuard(toml(&cx.join("config.toml"))?);let native=ValueGuard(remap(&config,stage,&cx,paths.home,paths.account));
 let native_env=get(get(&native,"shell_environment_policy",&empty)?,"set",&empty)?;
 if !equal(native_env,env){return Err(error("ValueError","Importação nativa de env incompleta; variáveis existentes preservadas"));}
 let raw_hooks=ValueGuard(read_json(&cx.join("hooks.json"))?);let mut normalized_hooks=ValueGuard(remap(&raw_hooks,stage,&cx,paths.home,paths.account));
 if normalized_hooks.get("hooks").is_none(){codec::discard(std::mem::replace(&mut normalized_hooks.0,json!({"hooks":{}})));}
 if actions.fingerprint()?!=beginning{return Err(error("AlteradoExternamente","Fontes do Claude mudaram durante a importação"));}
 let mut roots=vec![(cx.join("agents"),paths.account.join("agents")),(stage.join(".agents/skills"),paths.home.join(".agents/skills"))];
 if memory{let extension="memories/extensions/external_agent_import";roots.push((cx.join(extension),paths.account.join(extension)));}
 let mut desired=BTreeMap::new();let mut trusted=BTreeSet::new();
 for(index,(source,target))in roots.iter().enumerate(){
  if !source.is_dir(){continue;}
  for path in descendants(source){if !path.is_file(){continue;}let relative=path.strip_prefix(source).unwrap();let destination=target.join(relative);let bytes=std::fs::read(&path).map_err(io)?;
   let bytes=if let Ok(text)=std::str::from_utf8(&bytes){let value=ValueGuard(remap(&json!(text),stage,&cx,paths.home,paths.account));codec::python_string(&value).into_bytes()}else{bytes};desired.insert(destination.clone(),bytes);
   let name=if index==0{path.file_stem().map(|v|v.to_string_lossy().into_owned())}else{relative.components().next().map(|v|v.as_os_str().to_string_lossy().into_owned())};
   let section=if index==0{"agents"}else{"commands"};if index<2&&name.is_some_and(|name|history[section].as_array().is_some_and(|items|items.iter().any(|item|item==&name))){trusted.insert(destination);}
  }
 }
 let previous=get(registry,"artefatos",&empty)?.as_object().ok_or_else(||error("AttributeError","Manifesto de artefatos precisa ser um objeto"))?;
 let (mut saved,mut selected)=(Map::new(),Map::new());for(name,value)in previous{let keep=frozen_artifact(Path::new(name),paths.account,paths.home,&frozen);if keep{saved.insert(name.clone(),codec::clone_value(value));}else{selected.insert(name.clone(),codec::clone_value(value));}}
 let selected=ValueGuard(Value::Object(selected));let mut reconciled=ValueGuard(artifacts::reconcile(&desired,&selected,paths.backups,&trusted,&mut artifacts::Disk)?);
 let mut manifest=reconciled["manifest"].take();manifest.as_object_mut().unwrap().extend(saved);replace(registry,"artefatos",manifest)?;
 let warnings=reconciled["warnings"].take();if let Value::Array(warnings)=warnings{state.update(|v|v["avisos"].as_array_mut().unwrap().extend(warnings));}
 actions.checkpoint(registry)?;actions.hooks(&normalized_hooks,registry)?;
 let agents=get(&native,"agents",&empty)?.as_object().ok_or_else(||error("AttributeError","Configuração de agentes precisa ser um objeto"))?;
 let agents=ValueGuard(Value::Object(agents.iter().filter(|(_,v)|!v.is_object()||!crate::engines::probe::truthy(&v["config_file"])||registry["artefatos"].get(v["config_file"].as_str().unwrap_or("")).is_some()).map(|(name,value)|(name.clone(),codec::clone_value(value))).collect()));
 actions.configure(Configuration{mcp:get(&native,"mcp_servers",&empty)?,agents:&agents,hooks:get(get(&native,"features",&empty)?,"hooks",&Value::Null)?==&Value::Bool(true),memory,history,env:native_env},registry).await
}
async fn import_items(actions:&mut impl Actions,state:&State,cc:&Path,memory:bool)->Result<BTreeSet<(String,PathBuf)>,Error>{
 let detected=ValueGuard(Value::Array(actions.detect().await?));let mut items=Vec::new();
 for item in detected.as_array().unwrap(){if item_type(item)?.is_some_and(|kind|matches!(kind,"CONFIG"|"HOOKS"|"MCP_SERVER_CONFIG"|"COMMANDS"|"SUBAGENTS")||memory&&kind=="MEMORY"){items.push(codec::clone_value(item));}}
 let items=ValueGuard(Value::Array(items));let empty=json!({});let empty_list=json!([]);let mut frozen=BTreeSet::new();
 for(folder,kind,detail)in [("agents","SUBAGENTS","subagents"),("commands","COMMANDS","commands")]{
  let mut names=BTreeSet::new();for item in items.as_array().unwrap(){if item_type(item)?!=Some(kind){continue;}
   let entries=get(get(item,"details",&empty)?,detail,&empty_list)?.as_array().ok_or_else(||error("TypeError","Detalhes da importação precisam ser uma lista"))?;
   for entry in entries{if entry.is_object(){let name=get(entry,"name",&json!(""))?.as_str().ok_or_else(||error("TypeError","Nome importado precisa ser texto"))?.to_owned();names.insert(name);}}
  }
  let root=cc.join(folder);let ignored=descendants(&root).into_iter().filter(|path|path.file_name().is_some_and(|name|name.to_string_lossy().ends_with(".md")))
   .filter(|path|{let stem=path.file_stem().unwrap().to_string_lossy();!names.iter().any(|name|name.as_str()==stem.as_ref()||name.contains(stem.as_ref()))}).collect::<Vec<_>>();
  if !ignored.is_empty(){for path in &ignored{frozen.insert((folder.to_owned(),path.strip_prefix(&root).unwrap().to_owned()));}
   warning(state,"aviso_ignorados",json!({"pasta":folder,"arquivos":ignored.iter().map(|path|path.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")}));}
 }
 let essential=items.as_array().unwrap().iter().filter(|item|item.get("itemType").and_then(Value::as_str)!=Some("MEMORY")).map(codec::clone_value).collect::<Vec<_>>();
 let memories=items.as_array().unwrap().iter().filter(|item|item.get("itemType").and_then(Value::as_str)==Some("MEMORY")).map(codec::clone_value).collect::<Vec<_>>();
 if memory{
  let mut seen=BTreeSet::new();for item in &memories{
   let entries=get(get(item,"details",&empty)?,"memory",&empty_list)?.as_array().ok_or_else(||error("TypeError","Detalhes de memória precisam ser uma lista"))?;
   seen.extend(entries.iter().filter_map(Value::as_str).map(str::to_owned));
  }
  let copied=std::fs::read_dir(cc.join("projects")).into_iter().flatten().flatten().filter(|entry|entry.path().is_dir()).map(|entry|entry.file_name().to_string_lossy().into_owned()).collect::<BTreeSet<_>>();
  let ignored=copied.difference(&seen).cloned().collect::<Vec<_>>();if !ignored.is_empty(){warning(state,"aviso_memoria_nao_reconhecida",json!({"n":ignored.len(),"exemplos":ignored.iter().take(3).cloned().collect::<Vec<_>>().join(", ")}));}
 }
 if !essential.is_empty(){
  let result=ValueGuard(actions.import(essential).await?);let results=get(&result,"itemTypeResults",&empty_list)?.as_array().ok_or_else(||error("TypeError","Resultado de importação precisa ser uma lista"))?;
  for item in results{if crate::engines::probe::truthy(get(item,"failures",&Value::Null)?){return Err(error("CodexNativoErro","Importação de fragmentos incompleta"));}}
 }
 if !memories.is_empty(){
  let failures=match actions.import(memories).await{
   Err(error) if error.kind=="CodexNativoErro"=>1,
   Err(error)=>return Err(error),
   Ok(result)=>{let result=ValueGuard(result);let results=get(&result,"itemTypeResults",&empty_list)?.as_array().ok_or_else(||error("TypeError","Resultado de memória precisa ser uma lista"))?;let mut count=0;for item in results{let value=get(item,"failures",&Value::Null)?;if crate::engines::probe::truthy(value){count+=match value{Value::Array(items)=>items.len(),Value::Object(items)=>items.len(),Value::String(text)=>text.chars().count(),_=>return Err(error("TypeError","Falhas de memória precisam ser iteráveis"))};}}count},
  };
  if failures>0{warning(state,"aviso_memoria_incompleta",json!({"n":failures}));}
 }
 Ok(frozen)
}

fn frozen_artifact(path:&Path,account:&Path,home:&Path,frozen:&BTreeSet<(String,PathBuf)>)->bool {
 frozen.iter().any(|(section,relative)| {
  if section=="agents" {path==account.join("agents").join(relative).with_extension("toml")}
  else {path.starts_with(home.join(".agents/skills").join(relative.with_extension("")))}
 })
}
#[cfg(test)]
#[test]
fn ignored_artifacts_preserve_only_their_section_and_relative_path() {
 let home=Path::new("home");let account=Path::new("account");
 let frozen=BTreeSet::from([("agents".into(),PathBuf::from("sub/duplicate.md"))]);
 assert!(frozen_artifact(&account.join("agents/sub/duplicate.toml"),account,home,&frozen));
 assert!(!frozen_artifact(&account.join("agents/duplicate.toml"),account,home,&frozen));
 assert!(!frozen_artifact(&home.join(".agents/skills/sub/duplicate/SKILL.md"),account,home,&frozen));
}
