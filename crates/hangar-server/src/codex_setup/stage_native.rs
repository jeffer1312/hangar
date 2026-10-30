//! O importador temporário é encerrado antes de adotar seus arquivos na conta.
use super::{cancellation::{self,OperationError},config_edits,files,import_api,instructions,native::Native,phase::{Error,State},pipeline_hooks,plugin_native,stage_flow,stage_history};
use crate::{accounts::environment::AccountEnvironment,config_sync::{runtime::RuntimeConfig,runtime_values::ValueGuard}};
use serde_json::Value;
use std::{future::Future,path::Path,time::{Duration,SystemTime,UNIX_EPOCH}};
use tokio::sync::watch;
pub(super) struct Actions<'a>{
 pub main:&'a Native,pub stage:Option<Native>,pub env:&'a AccountEnvironment,
 pub account:&'a Path,pub repo:&'a Path,pub backups:&'a Path,pub work:&'a Path,
 pub state:&'a State,pub config:&'a RuntimeConfig,pub cancel:watch::Receiver<bool>,
}
fn cancelled()->Error{Error{kind:"CancelledError",message:"integração Codex interrompida".into()}}
fn operation(error:OperationError)->Error{match error{OperationError::Cancelled=>cancelled(),OperationError::Failed(message)=>Error{kind:"CodexNativoErro",message}}}
async fn wait<T>(signal:&mut watch::Receiver<bool>,future:impl Future<Output=Result<T,String>>)->Result<T,Error>{
 tokio::select!{biased;_=cancellation::stopped(signal)=>Err(cancelled()),result=future=>result.map_err(|message|Error{kind:"CodexNativoErro",message})}
}
impl stage_flow::Actions for Actions<'_>{
 async fn history(&mut self)->Result<Value,Error>{
  tokio::select!{biased;_=cancellation::stopped(&mut self.cancel)=>Err(cancelled()),result=stage_history::read(self.main,self.state)=>result}
 }
 fn fingerprint(&mut self)->Result<String,Error>{super::pipeline_fingerprint::compute(self.env,self.config,self.account,self.repo,None,None,true)}
 async fn open(&mut self,home:&Path,account:&Path,memory:bool)->Result<(),Error>{
  self.stage=Some(Native::open_controlled(self.env,home,account,memory,self.cancel.clone()).await.map_err(operation)?);Ok(())
 }
 async fn detect(&mut self)->Result<Vec<Value>,Error>{
  let native=self.stage.as_ref().ok_or_else(||Error{kind:"RuntimeError",message:"Importador temporário não foi aberto".into()})?;
  wait(&mut self.cancel,import_api::detect(native,Duration::from_secs(120))).await
 }
 async fn import(&mut self,items:Vec<Value>)->Result<Value,Error>{
  let native=self.stage.as_mut().ok_or_else(||Error{kind:"RuntimeError",message:"Importador temporário não foi aberto".into()})?;
  wait(&mut self.cancel,import_api::import(native,items,Duration::from_secs(120))).await
 }
 async fn close(&mut self)->Result<(),Error>{
  if let Some(mut native)=self.stage.take(){native.close().await.map_err(|message|Error{kind:"CodexNativoErro",message})?;}Ok(())
 }
 async fn configure(&mut self,config:stage_flow::Configuration<'_>,registry:&mut Value)->Result<(),Error>{
  let limit=ValueGuard(instructions::limit(self.account)?);let mut plan=None;let mut callback_error=None;
  let result=files::edit_config_controlled(self.env,&self.account.join("config.toml"),self.backups,self.work,|current|{
   let value=config_edits::prepare(current,config_edits::Sources{mcp:config.mcp,agents:config.agents,registry:Some(registry),history:config.history,env:Some(config.env),limit:&limit,hooks:config.hooks,memory:config.memory});
   match value{Ok(mut value)=>{let edits=std::mem::take(&mut value.edits);plan=Some(value);Ok(edits)},Err(error)=>{let message=error.message.clone();callback_error=Some(error);Err(message)}}
  },self.cancel.clone()).await;
  match result{Ok(())=>{plan.expect("Gravador consultou a configuração").confirm(registry,self.state);Ok(())},Err(error)=>Err(callback_error.unwrap_or_else(||operation(error)))}
 }
 fn hooks(&mut self,value:&Value,registry:&mut Value)->Result<(),Error>{
  let wrapper=super::hook_command::allow_wrapper().map_err(|message|Error{kind:"RuntimeError",message})?;
  pipeline_hooks::sync(&pipeline_hooks::Context{account:self.account,backups:self.backups,wrapper:&wrapper,windows:cfg!(windows)},value,registry,self.state)
 }
 fn checkpoint(&mut self,registry:&mut Value)->Result<(),Error>{
  let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|Error{kind:"RuntimeError",message:e.to_string()})?.as_secs_f64();
  plugin_native::checkpoint(&self.env.home,self.account,self.work,registry,self.state,now)
 }
}
