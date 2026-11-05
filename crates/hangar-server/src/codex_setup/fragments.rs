//! Mescla de fragmentos com proveniência e preservação dos complementos pessoais.
use serde_json::{Map,Value,json};
use std::collections::BTreeSet;
use crate::{config_sync::value_equality::equal,orchestration::codec};
use super::messages;
static NULL:Value=Value::Null;
enum Task<'a>{
 Copy(&'a Value),
 Prune(&'a Value,&'a Value),
 Merge(&'a Value,&'a Value,&'a Value),
 Finish(Vec<String>,bool),
}
fn transform(first:Task<'_>)->Option<Value>{
 let mut tasks=vec![first];let mut values=Vec::<Option<Value>>::new();
 while let Some(task)=tasks.pop(){match task{
  Task::Copy(value)=>values.push(Some(codec::clone_value(value))),
  Task::Prune(current,previous)=>{
   if equal(current,previous){values.push(None);continue;}
   let (Some(current),Some(previous))=(current.as_object(),previous.as_object()) else{tasks.push(Task::Copy(current));continue;};
   let keys:Vec<_>=current.keys().cloned().collect();tasks.push(Task::Finish(keys.clone(),true));
   for key in keys.iter().rev(){let value=&current[key];tasks.push(match previous.get(key){Some(old)=>Task::Prune(value,old),None=>Task::Copy(value)});}
  },
  Task::Merge(current,source,previous)=>{
   let Some(source)=source.as_object() else{tasks.push(Task::Copy(source));continue;};
   let current=current.as_object();let previous=previous.as_object();
   let mut keys:Vec<_>=current.into_iter().flat_map(|v|v.keys()).cloned().collect();
   keys.extend(source.keys().filter(|key|!current.is_some_and(|v|v.contains_key(*key))).cloned());
   tasks.push(Task::Finish(keys.clone(),false));
   for key in keys.iter().rev(){
    let value=current.and_then(|v|v.get(key)).unwrap_or(&NULL);let old=previous.and_then(|v|v.get(key));
    tasks.push(if let Some(desired)=source.get(key){Task::Merge(value,desired,old.unwrap_or(&NULL))}else if let Some(old)=old{Task::Prune(value,old)}else{Task::Copy(value)});
   }
  },
  Task::Finish(keys,prune)=>{
   let children=values.split_off(values.len()-keys.len());let mut map=Map::new();
   for (key,value) in keys.into_iter().zip(children){if let Some(value)=value{map.insert(key,value);}}
   values.push(if prune&&map.is_empty(){None}else{Some(Value::Object(map))});
  }
 }}
 values.pop().flatten()
}
fn warning(code:&str,name:&str)->Value{
 messages::message(code,&Map::from_iter([("nome".into(),json!(name))])).expect("mensagem interna de proveniência")
}
pub fn merge(current:&Value,source:&Value,previous:&Value,trusted:&BTreeSet<String>)->Result<Value,String> {
 let (Some(current_map),Some(source_map),Some(previous_map))=(current.as_object(),source.as_object(),previous.as_object()) else{return Err("Configuração e manifestos precisam ser objetos".into());};
 let mut merged=codec::clone_value(current);let merged_map=merged.as_object_mut().unwrap();let mut manifest=Map::new();let mut warnings=Vec::new();
 for (name,value) in source_map {
  let current=current_map.get(name).unwrap_or(&NULL);
  if current_map.contains_key(name)&&!previous_map.contains_key(name)&&!trusted.contains(name){
   let candidate=transform(Task::Merge(current,value,&NULL)).unwrap();let same=equal(&candidate,current);codec::discard(candidate);
   if !same{warnings.push(warning("aviso_config_sem_proveniencia",name));continue;}
  }
  let value=transform(Task::Merge(current,value,previous_map.get(name).unwrap_or(&NULL))).unwrap();
  if let Some(old)=merged_map.insert(name.clone(),value){codec::discard(old);}
  manifest.insert(name.clone(),codec::clone_value(&source_map[name]));
 }
 for (name,old) in previous_map {
  if source_map.contains_key(name){continue;}
  let Some(current)=current_map.get(name) else{continue;};
  if let Some(value)=transform(Task::Prune(current,old)){
   if let Some(old)=merged_map.insert(name.clone(),value){codec::discard(old);}
   warnings.push(warning("aviso_config_campos_particulares",name));
  }else if let Some(old)=merged_map.shift_remove(name){codec::discard(old);}
 }
 Ok(Value::Object(Map::from_iter([("merged".into(),merged),("manifest".into(),Value::Object(manifest)),("warnings".into(),Value::Array(warnings))])))
}
