//! Cache por fase só é publicado quando a fonte permanece igual durante a execução.
use crate::config_sync::runtime_values::ValueGuard;
use serde_json::Value;
use std::{collections::BTreeSet,future::Future,sync::{Arc,Mutex}};
#[derive(Clone)]
pub struct State {
    data:Arc<Mutex<ValueGuard>>,
    confirmed:Arc<Mutex<BTreeSet<String>>>,
}
impl State {
    pub fn new(value:Value)->Self{Self{data:Arc::new(Mutex::new(ValueGuard(value))),confirmed:Arc::new(Mutex::new(BTreeSet::new()))}}
    pub fn snapshot(&self)->Value{crate::orchestration::codec::clone_value(&self.data.lock().unwrap_or_else(|e|e.into_inner()))}
    pub fn update(&self,apply:impl FnOnce(&mut Value)){apply(&mut self.data.lock().unwrap_or_else(|e|e.into_inner()));}
    pub fn hooks_changed(&self){
        self.update(|value|{
            value["confianca_pendente"]=serde_json::json!(true);
            let warnings=value["avisos"].as_array_mut().expect("Estado inicializado");
            if !warnings.iter().any(|item|{let serialized=ValueGuard(super::messages::serialize(item));serialized["codigo"]=="aviso_hooks_alterados"}){
                warnings.push(super::messages::message("aviso_hooks_alterados",&serde_json::Map::new()).expect("Mensagem versionada"));
            }
        });
    }
    pub fn confirmed(&self)->BTreeSet<String>{self.confirmed.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub fn set_confirmed(&self,value:BTreeSet<String>){*self.confirmed.lock().unwrap_or_else(|e|e.into_inner())=value;}
}
#[derive(Debug)]
pub struct Error {pub kind:&'static str,pub message:String}
impl From<super::files::Error> for Error {
 fn from(error:super::files::Error)->Self{let kind=match &error{super::files::Error::Io(_)=>"OSError",super::files::Error::Invalid(_)=>"ValueError",super::files::Error::Changed(_)=>"AlteradoExternamente"};Self{kind,message:error.to_string()}}
}
pub trait Actions {
    fn fingerprint(&mut self,source:bool,registry:&Value)->impl Future<Output=Result<String,Error>>+Send;
    fn execute(&mut self,registry:&mut Value,state:&State)->impl Future<Output=Result<(),Error>>+Send;
    fn now(&self)->f64;
}
fn invalid(message:&str)->Error{Error{kind:"TypeError",message:message.into()}}
fn replace(map:&mut serde_json::Map<String,Value>,key:&str,value:Value){
 if let Some(old)=map.insert(key.into(),value){crate::orchestration::codec::discard(old);}
}
pub async fn run(name:&str,registry:&mut Value,state:&State,force:bool,actions:&mut impl Actions)->Result<(),Error>{
 use crate::orchestration::codec;
 use serde_json::json;
 let current=actions.fingerprint(false,registry).await?;
 let map=registry.as_object_mut().ok_or_else(||invalid("Registro de fases precisa ser um objeto"))?;
 let phases=map.entry("phases").or_insert_with(||json!({})).as_object_mut().ok_or_else(||invalid("Fases precisam ser um objeto"))?;
 if let Some(previous)=phases.get(name){
  let previous=previous.as_object().ok_or_else(||invalid("Checkpoint de fase precisa ser um objeto"))?;
  if !force&&previous.get("fingerprint").and_then(Value::as_str)==Some(&current){
   for (field,key) in [("erros","errors"),("avisos","warnings")]{
    if let Some(items)=previous.get(key){let items=iterable(items)?;state.update(|value|value[field].as_array_mut().unwrap().extend(items));}
   }
   if name=="plugins"{
    let confirmed=match previous.get("confirmed"){Some(value)=>iterable(value)?,None=>vec![]};
    let mut ids=BTreeSet::new();for value in confirmed{let Value::String(value)=value else{return Err(invalid("Identificador de plugin confirmado precisa ser texto"));};ids.insert(value);}
    state.set_confirmed(ids);
    state.update(|value|{let map=value.as_object_mut().unwrap();replace(map,"plugins",previous.get("plugins").map(codec::clone_value).unwrap_or_else(||json!([])));replace(map,"proxima_atualizacao",previous.get("next_update").map(codec::clone_value).unwrap_or(Value::Null));});
   }
   return Ok(());
  }
 }
 if let Some(old)=phases.shift_remove(name){codec::discard(old);}
 let source=actions.fingerprint(true,registry).await?;
 let before=ValueGuard(state.snapshot());let errors=before["erros"].as_array().unwrap().len();let warnings=before["avisos"].as_array().unwrap().len();drop(before);
 actions.execute(registry,state).await?;
 if source!=actions.fingerprint(true,registry).await?{return Ok(());}
 let fingerprint=actions.fingerprint(false,registry).await?;
 let after=ValueGuard(state.snapshot());
 let mut result=serde_json::Map::from_iter([
  ("fingerprint".into(),json!(fingerprint)),("attempted_at".into(),json!(actions.now())),
  ("errors".into(),Value::Array(after["erros"].as_array().unwrap().iter().skip(errors).map(super::messages::serialize).collect())),
  ("warnings".into(),Value::Array(after["avisos"].as_array().unwrap().iter().skip(warnings).map(super::messages::serialize).collect())),
 ]);
 if name=="plugins"{result.insert("confirmed".into(),json!(state.confirmed()));result.insert("plugins".into(),codec::clone_value(&after["plugins"]));result.insert("next_update".into(),codec::clone_value(&after["proxima_atualizacao"]));}
 let phases=registry.get_mut("phases").and_then(Value::as_object_mut).ok_or_else(||invalid("Fases precisam ser um objeto"))?;
 replace(phases,name,Value::Object(result));Ok(())
}

fn iterable(value:&Value)->Result<Vec<Value>,Error>{
 use crate::orchestration::codec;
 match value{
  Value::Array(values)=>Ok(values.iter().map(codec::clone_value).collect()),
  Value::Object(values)=>Ok(values.keys().cloned().map(Value::String).collect()),
  Value::String(text) if !codec::contains_nonfinite(value)=>Ok(text.chars().map(|c|Value::String(c.to_string())).collect()),
  _=>Err(invalid(&format!("'{}' object is not iterable",match value{Value::Null=>"NoneType",Value::Bool(_)=>"bool",Value::Number(v) if !v.to_string().contains(['.','e','E'])=>"int",_=>"float"}))),
 }
}
