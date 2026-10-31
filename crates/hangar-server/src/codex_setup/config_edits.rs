//! Plano de configuração cujo manifesto só é adotado após confirmar a escrita nativa.
use super::{phase::{State,Error},fragments};
use crate::{config_sync::{runtime_values::ValueGuard,value_equality::equal},orchestration::codec};
use serde_json::{Value,Map,json};
use std::collections::BTreeSet;
pub struct Plan {pub edits:Vec<Value>,pub manifest:Value,pub warnings:Vec<Value>}
impl Plan{
 pub fn confirm(self,registry:&mut Value,state:&State){
  if let Value::Object(manifest)=self.manifest{for(key,value)in manifest{if let Some(old)=registry.as_object_mut().unwrap().insert(key,value){codec::discard(old);}}}
  state.update(|value|value["avisos"].as_array_mut().unwrap().extend(self.warnings));
 }
}
pub struct Sources<'a>{
 pub mcp:&'a Value,pub agents:&'a Value,pub registry:Option<&'a Value>,pub history:&'a Value,
 pub env:Option<&'a Value>,pub limit:&'a Value,pub hooks:bool,pub memory:bool,
}
fn error(kind:&'static str,message:impl Into<String>)->Error{Error{kind,message:message.into()}}
fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{
 let map=value.as_object().ok_or_else(||error("AttributeError",format!("'{}' object has no attribute 'get'",match value{Value::Null=>"NoneType",Value::Bool(_)=>"bool",Value::Number(v) if !v.to_string().contains(['.','e','E'])=>"int",Value::Number(_)=>"float",Value::String(_)=>"str",Value::Array(_)=>"list",_=>"dict"})))?;Ok(map.get(key).unwrap_or(default))
}
fn integer(value:&Value)->Option<num_bigint::BigInt>{if !value.is_number(){return None;}let text=value.to_string();if text.contains(['.','e','E']){return None;}num_bigint::BigInt::parse_bytes(text.as_bytes(),10)}
fn edit(key:String,value:Value)->Value{Value::Object(Map::from_iter([("keyPath".into(),Value::String(key)),("value".into(),value),("mergeStrategy".into(),json!("replace"))]))}
fn key(parts:&[&str])->String{parts.iter().map(|part|crate::transcript::pyjson::dumps_unicode(&json!(part),false)).collect::<Vec<_>>().join(".")}
pub fn prepare(current:&Value,sources:Sources<'_>)->Result<Plan,Error>{
 let empty=json!({});let empty_list=json!([]);let default_limit=json!(32768);
 let fallbacks=get(current,"project_doc_fallback_filenames",&empty_list)?;
 let fallbacks=fallbacks.as_array().ok_or_else(||error("ValueError","project_doc_fallback_filenames inválido"))?;
 let mut names=Vec::new();for value in fallbacks.iter().chain([&json!("CLAUDE.md"),&json!("CLAUDE.MD")]){if value.is_array()||value.is_object(){return Err(error("TypeError",format!("unhashable type: '{}'",if value.is_array(){"list"}else{"dict"})));}if !names.iter().any(|old|equal(old,value)){names.push(codec::clone_value(value));}}
 let limit=get(current,"project_doc_max_bytes",&default_limit)?;let limit=integer(limit).filter(|v|v>=&num_bigint::BigInt::from(0)).ok_or_else(||error("ValueError","project_doc_max_bytes inválido"))?;
 let needed=integer(sources.limit).ok_or_else(||error("ValueError","project_doc_max_bytes inválido"))?;
 let mut edits=Vec::new();if limit<needed{edits.push(edit("project_doc_max_bytes".into(),codec::clone_value(sources.limit)));}
 let names=ValueGuard(Value::Array(names));if !equal(&names,&Value::Array(fallbacks.iter().map(codec::clone_value).collect())){edits.push(edit("project_doc_fallback_filenames".into(),codec::clone_value(&names)));}
 if sources.hooks&&get(get(current,"features",&empty)?,"hooks",&Value::Null)?!=&Value::Bool(true){edits.push(edit("features.hooks".into(),json!(true)));}
 if sources.memory&&get(get(current,"features",&empty)?,"external_agent_memory_import",&Value::Null)?!=&Value::Bool(true){edits.push(edit("features.external_agent_memory_import".into(),json!(true)));}
 let mut manifests=Map::new();let mut warnings=Vec::new();
 if let Some(registry)=sources.registry{
  for(section,source)in [("mcp_servers",sources.mcp),("agents",sources.agents)]{
   let before=get(current,section,&empty)?;let previous=get(registry,section,&empty)?;
   let trusted:BTreeSet<_>=sources.history.get(section).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
   let mut merged=ValueGuard(fragments::merge(before,source,previous,&trusted).map_err(|e|error("ValueError",e))?);
   let mut removed=BTreeSet::new();if section=="mcp_servers"{for name in previous.as_object().unwrap().keys(){let value=&merged["merged"][name];if source.get(name).is_none()&&value.is_object()&&!crate::orchestration::truthy(&value["command"])&&!crate::orchestration::truthy(&value["url"]){removed.insert(name.clone());}}for name in &removed{if let Some(old)=merged["merged"].as_object_mut().unwrap().shift_remove(name){codec::discard(old);}}}
   warnings.extend(merged["warnings"].as_array().unwrap().iter().filter(|warning|!warning["params"]["nome"].as_str().is_some_and(|name|removed.contains(name))).map(codec::clone_value));
   for name in before.as_object().unwrap().keys().chain(merged["merged"].as_object().unwrap().keys()).collect::<BTreeSet<_>>(){let before=before.get(name).unwrap_or(&Value::Null);let after=merged["merged"].get(name).unwrap_or(&Value::Null);if !equal(before,after){edits.push(edit(key(&[section,name]),codec::clone_value(after)));}}
   manifests.insert(section.into(),codec::clone_value(&merged["manifest"]));
  }
  if let Some(source)=sources.env{
   let before=get(get(current,"shell_environment_policy",&empty)?,"set",&empty)?;
   let merged=ValueGuard(fragments::merge(before,source,get(registry,"env",&empty)?,&BTreeSet::new()).map_err(|e|error("ValueError",e))?);
   warnings.extend(merged["warnings"].as_array().unwrap().iter().map(codec::clone_value));
   for name in before.as_object().unwrap().keys().chain(merged["merged"].as_object().unwrap().keys()).collect::<BTreeSet<_>>(){let before=before.get(name).unwrap_or(&Value::Null);let after=merged["merged"].get(name).unwrap_or(&Value::Null);if !equal(before,after){edits.push(edit(key(&["shell_environment_policy","set",name]),codec::clone_value(after)));}}
   manifests.insert("env".into(),codec::clone_value(&merged["manifest"]));
  }
 }
 Ok(Plan{edits,manifest:if sources.registry.is_some(){Value::Object(manifests)}else{Value::Null},warnings})
}
