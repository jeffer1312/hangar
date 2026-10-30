//! Seleção de plugins no escritor nativo compartilhado.
use super::{files,phase::Error};
use crate::{accounts::environment::AccountEnvironment,config_sync::value_equality::equal,transcript::pyjson};
use serde_json::{Value,json};
use std::path::Path;

fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{
 let kind=match value{Value::Null=>"NoneType",Value::Bool(_)=>"bool",Value::String(_)=>"str",Value::Array(_)=>"list",Value::Number(n) if n.to_string().contains(['.','e','E'])=>"float",Value::Number(_)=>"int",Value::Object(_)=>"dict"};
 value.as_object().map(|map|map.get(key).unwrap_or(default)).ok_or_else(||Error{kind:"AttributeError",message:format!("'{kind}' object has no attribute 'get'")})
}
pub fn prepare(current:&Value,choices:&[(String,bool)])->Result<Vec<Value>,Error>{
 let empty=json!({});let mut edits=Vec::new();
 // A seleção vazia não consulta os campos; ainda passa pelo gravador para validar a leitura.
 for (id,enabled) in choices {
  let old=get(get(get(current,"plugins",&empty)?,id,&empty)?,"enabled",&Value::Null)?;
  if !equal(old,&Value::Bool(*enabled)){
   let key=["plugins",id,"enabled"].map(|part|pyjson::dumps_unicode(&json!(part),false)).join(".");
   edits.push(json!({"keyPath":key,"value":enabled,"mergeStrategy":"replace"}));
  }
 }
 Ok(edits)
}
pub async fn apply(env:&AccountEnvironment,account:&Path,backups:&Path,work:&Path,choices:&[(String,bool)])->Result<(),Error>{
 let mut callback_error=None;
 let result=files::edit_config(env,&account.join("config.toml"),backups,work,|current|{
  prepare(current,choices).map_err(|error|{let message=error.message.clone();callback_error=Some(error);message})
 }).await;
 match result{Ok(())=>Ok(()),Err(message)=>Err(callback_error.unwrap_or(Error{kind:"RuntimeError",message}))}
}
