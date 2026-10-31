//! Estado persistido da integração e retrato compartilhado da rodada ativa.
use serde_json::{Map,Value,json};
use std::path::{Path,PathBuf};
use crate::orchestration::{codec,truthy};
use super::{files,messages,plugins};
use crate::config_sync::runtime_values::ValueGuard;
#[derive(Debug)]
pub struct Error {pub kind:&'static str,pub message:String}
fn error(kind:&'static str,message:impl Into<String>)->Error{Error{kind,message:message.into()}}
pub fn snapshot()->Value {
 json!({"estado":"ocioso","etapa":"","ultima_execucao":null,"proxima_atualizacao":null,"plugins":[],"erros":[],"avisos":[],"confianca_pendente":false,"progresso":null})
}
pub fn root(home:&Path,account:&Path)->Result<PathBuf,Error>{
 let resolved=plugins::resolved(account).map_err(|e|error("RuntimeError",e))?;
 Ok(home.join(".hangar/codex-integracao").join(&files::hash(resolved.to_string_lossy().as_bytes())[..24]))
}
fn record(path:&Path)->Result<Value,()> { files::object(path).map_err(|_|()) }
fn kind(value:&Value)->&'static str{match value{Value::Null=>"NoneType",Value::Bool(_)=>"bool",Value::String(_)=>"str",Value::Array(_)=>"list",Value::Object(_)=>"dict",Value::Number(v)=>if v.to_string().contains(['.','e','E']){"float"}else{"int"}}}
fn message(code:&str)->Value{messages::message(code,&Map::new()).expect("mensagem de estado versionada")}
fn unreadable()->Value {let mut state=snapshot();state["estado"]=json!("erro");state["erros"]=json!([message("erro_registro_ilegivel")]);state}
fn raw_status(path:&Path,memory:Option<&Value>)->Result<Value,Error>{
 if let Some(memory)=memory.filter(|v|v["estado"]=="executando"){return Ok(codec::clone_value(memory));}
 let mut saved=match record(path){Ok(value)=>value,Err(())=>return Ok(unreadable())};
 let status=saved.as_object_mut().unwrap().shift_remove("status").unwrap_or_else(||json!({}));codec::discard(saved);
 let Value::Object(status)=status else{let message=format!("'{}' object is not a mapping",kind(&status));codec::discard(status);return Err(error("TypeError",message));};
 let mut state=ValueGuard(snapshot());state.as_object_mut().unwrap().extend(status);
 if state["estado"]=="executando"{state["estado"]=json!("ocioso");let old=state.as_object_mut().unwrap().insert("etapa".into(),message("etapa_reinicio"));if let Some(old)=old{codec::discard(old);}}
 if let Some(memory)=memory {
  let left=&memory["ultima_execucao"];let right=&state["ultima_execucao"];
  let left=if truthy(left){left.as_str().ok_or_else(||error("TypeError","ultima_execucao precisa ser texto"))?}else{""};
  let right=if truthy(right){right.as_str().ok_or_else(||error("TypeError","ultima_execucao precisa ser texto"))?}else{""};
  if left>=right{return Ok(codec::clone_value(memory));}
 }
 Ok(std::mem::replace(&mut state.0,Value::Null))
}
fn skills(path:&Path)->Result<Value,Error>{
 let mut saved=match record(path){Ok(value)=>value,Err(())=>return Ok(json!({"ponte":0,"nativas":0}))};
 let skills=saved.as_object_mut().unwrap().shift_remove("skills").unwrap_or_else(||json!({}));codec::discard(saved);
 let Some(map)=skills.as_object() else{let message=format!("'{}' object has no attribute 'values'",kind(&skills));codec::discard(skills);return Err(error("AttributeError",message));};
 let mut bridges=0;let mut native=0;
 for item in map.values().filter(|v|v.is_object()) {match item["mode"].as_str(){Some("symlink"|"copy")=>bridges+=1,Some("native")=>native+=1,_=>{}}}
 codec::discard(skills);Ok(json!({"ponte":bridges,"nativas":native}))
}
fn serialized_list(value:Value)->Result<Value,Error>{
 let result=match &value {
  Value::Array(items)=>Ok(Value::Array(items.iter().map(messages::serialize).collect())),
  Value::Object(items)=>Ok(Value::Array(items.keys().map(|v|messages::serialize(&json!(v))).collect())),
  Value::String(text)=>Ok(Value::Array(text.chars().map(|v|messages::serialize(&json!(v.to_string()))).collect())),
  _=>Err(error("TypeError",format!("'{}' object is not iterable",kind(&value)))),
 };
 codec::discard(value);result
}
pub fn read(home:&Path,account:&Path,memory:Option<&Value>,now:f64)->Result<Value,Error> {
 let path=root(home,account)?.join("estado.json");
 let mut state=ValueGuard(raw_status(&path,memory)?);
 let summary=match skills(&path){Ok(value)=>value,Err(error)=>{return Err(error);}};
 let map=state.as_object_mut().unwrap();if let Some(old)=map.insert("skills".into(),summary){codec::discard(old);}
 let since=map.shift_remove("etapa_desde").unwrap_or(Value::Null);
 let seconds=if truthy(&since)&&map.get("estado").and_then(Value::as_str)==Some("executando") {
  let timestamp=since.as_f64().or_else(||since.as_bool().map(|b|if b{1.0}else{0.0}));
  match timestamp {
   Some(timestamp) if (now-timestamp).is_finite()=>serde_json::from_str(&format!("{:.0}",(now-timestamp).trunc())).unwrap(),
   _=>{codec::discard(since);return Err(error("TypeError","etapa_desde precisa ser um número finito"));}
  }
 }else{Value::Null};codec::discard(since);
 if let Some(old)=map.insert("etapa_segundos".into(),seconds){codec::discard(old);}
 let stage=map.shift_remove("etapa").unwrap_or(Value::Null);let serialized=messages::serialize(&stage);codec::discard(stage);map.insert("etapa".into(),serialized);
 for field in ["avisos","erros"]{
  let value=map.shift_remove(field).unwrap_or_else(||json!([]));
  match serialized_list(value){Ok(value)=>{map.insert(field.into(),value);},Err(error)=>{return Err(error);}}
 }
 Ok(std::mem::replace(&mut state.0,Value::Null))
}
