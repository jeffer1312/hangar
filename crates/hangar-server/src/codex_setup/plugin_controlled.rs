//! Cancelamento dos pedidos preserva o cliente dono até o fechamento da rodada.
use super::{cancellation,phase::{Error,State},plugin_edits,plugin_native,plugin_sync};
use serde_json::Value;
use std::{future::Future,path::Path};
use tokio::sync::watch;
pub(super) struct Actions<'a>{pub inner:plugin_native::Actions<'a>,pub cancel:watch::Receiver<bool>}
fn interrupted()->Error{Error{kind:"CancelledError",message:"integração Codex interrompida".into()}}
async fn wait<T>(signal:&mut watch::Receiver<bool>,future:impl Future<Output=Result<T,Error>>)->Result<T,Error>{
 tokio::select!{biased;_=cancellation::stopped(signal)=>Err(interrupted()),result=future=>result}
}
impl plugin_sync::Actions for Actions<'_>{
 async fn installed(&mut self)->Result<Vec<Value>,Error>{wait(&mut self.cancel,self.inner.installed()).await}
 async fn detect(&mut self)->Result<Vec<Value>,Error>{wait(&mut self.cancel,self.inner.detect()).await}
 async fn import(&mut self,items:Vec<Value>)->Result<Value,Error>{wait(&mut self.cancel,self.inner.import(items)).await}
 async fn upgrade(&mut self,market:&str)->Result<Value,Error>{wait(&mut self.cancel,self.inner.upgrade(market)).await}
 async fn add(&mut self,id:&str)->Result<Value,Error>{wait(&mut self.cancel,self.inner.add(id)).await}
 async fn edit(&mut self,choices:Vec<(String,bool)>)->Result<(),Error>{plugin_edits::apply_controlled(self.inner.env,self.inner.account,self.inner.backups,self.inner.work,&choices,self.cancel.clone()).await}
 async fn hooks(&mut self,path:&Path)->Result<(),Error>{
  if *self.cancel.borrow(){return Err(interrupted());}
  // Como a mutação protegida original, termina os arquivos e subprocessos antes de interromper.
  self.inner.hooks(path).await?;if *self.cancel.borrow(){Err(interrupted())}else{Ok(())}
 }
 fn checkpoint(&mut self,registry:&mut Value,state:&State)->Result<(),Error>{self.inner.checkpoint(registry,state)}
 fn now(&self)->f64{self.inner.now()}
}
