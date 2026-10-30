//! Operações da reconciliação executadas pelo mesmo cliente nativo da rodada.
use super::{files,import_api,messages,native::Native,normalize,phase::{Error,State},plugin_edits,plugin_sync,plugins,status};
use crate::{accounts::environment::AccountEnvironment,config_sync::runtime_values::ValueGuard,orchestration::codec};
use serde_json::{Map,Value,json};
use std::{path::Path,time::{Duration,SystemTime,UNIX_EPOCH}};

pub struct Actions<'a>{
 pub native:&'a mut Native,
 pub env:&'a AccountEnvironment,
 pub home:&'a Path,
 pub account:&'a Path,
 pub repo:&'a Path,
 pub backups:&'a Path,
 pub work:&'a Path,
 pub state:&'a State,
}
fn native_error(message:String)->Error{Error{kind:"CodexNativoErro",message}}
fn runtime_error(message:String)->Error{Error{kind:"RuntimeError",message}}
const TIMEOUT:Duration=Duration::from_secs(120);
impl plugin_sync::Actions for Actions<'_>{
 async fn installed(&mut self)->Result<Vec<Value>,Error>{self.native.cli().plugins(TIMEOUT).await.map_err(native_error)}
 async fn detect(&mut self)->Result<Vec<Value>,Error>{import_api::detect(self.native,TIMEOUT).await.map_err(native_error)}
 async fn import(&mut self,items:Vec<Value>)->Result<Value,Error>{import_api::import(self.native,items,TIMEOUT).await.map_err(native_error)}
 async fn upgrade(&mut self,market:&str)->Result<Value,Error>{self.native.cli().upgrade(market,TIMEOUT).await.map_err(native_error)}
 async fn add(&mut self,id:&str)->Result<Value,Error>{self.native.cli().add(id,TIMEOUT).await.map_err(native_error)}
 async fn edit(&mut self,choices:Vec<(String,bool)>)->Result<(),Error>{plugin_edits::apply(self.env,self.account,self.backups,self.work,&choices).await}
 async fn hooks(&mut self,path:&Path)->Result<(),Error>{
  let (server,wrapper)=super::hook_command::allow_wrapper().map_err(runtime_error)?;
  let context=plugins::Context{account:self.account,backups:self.backups,server:&server,windows:cfg!(windows)};
  plugins::adapt(path,&context,|data|Ok(normalize::hooks(data,&server,&wrapper,cfg!(windows))),||mark_changed(self.state),&plugins::NativeRuntime(self.env.clone())).await.map_err(runtime_error)
 }
 fn checkpoint(&mut self,registry:&mut Value,state:&State)->Result<(),Error>{
  checkpoint(self.home,self.account,self.work,registry,state,self.now())
 }
 fn now(&self)->f64{SystemTime::now().duration_since(UNIX_EPOCH).map(|duration|duration.as_secs_f64()).unwrap_or_else(|error|-error.duration().as_secs_f64())}
}
pub(crate) fn checkpoint(home:&Path,account:&Path,work:&Path,registry:&mut Value,state:&State,now:f64)->Result<(),Error>{
 let snapshot=ValueGuard(state.snapshot());
 let view=status::read(home,account,Some(&snapshot),now).map_err(|error|Error{kind:error.kind,message:error.message})?;
 let map=registry.as_object_mut().ok_or_else(||Error{kind:"TypeError",message:"Registro da integração precisa ser um objeto".into()})?;
 if let Some(old)=map.insert("status".into(),view){codec::discard(old);}
 let path=work.join("estado.json");let previous=files::read(&path)?;
 files::write(&path,&files::bytes(registry)?,previous.as_deref(),None)?;Ok(())
}
fn mark_changed(state:&State){
 state.update(|value|{
  value["confianca_pendente"]=json!(true);
  let warnings=value["avisos"].as_array_mut().expect("Estado inicializado");
  if !warnings.iter().any(|item|{let serialized=ValueGuard(messages::serialize(item));serialized["codigo"]=="aviso_hooks_alterados"}){
   warnings.push(messages::message("aviso_hooks_alterados",&Map::new()).expect("Mensagem versionada"));
  }
 });
}
