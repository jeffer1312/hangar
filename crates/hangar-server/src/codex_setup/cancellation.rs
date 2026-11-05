//! Sinal cooperativo: a tarefa dona conserva recursos até confirmar o encerramento.
use tokio::sync::watch;
#[derive(Debug,Clone,PartialEq,Eq)]
pub enum OperationError { Cancelled, Failed(String) }
impl From<String> for OperationError {fn from(value:String)->Self{Self::Failed(value)}}
impl std::fmt::Display for OperationError {
 fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{match self{Self::Cancelled=>f.write_str("integração Codex interrompida"),Self::Failed(message)=>f.write_str(message)}}
}
impl std::error::Error for OperationError {}
pub async fn stopped(signal:&mut watch::Receiver<bool>){
 loop{if *signal.borrow(){return;}if signal.changed().await.is_err(){std::future::pending::<()>().await;}}
}

impl From<&str> for OperationError {fn from(value:&str)->Self{Self::Failed(value.into())}}
pub fn check(signal:&Option<watch::Receiver<bool>>)->Result<(),OperationError>{
 if signal.as_ref().is_some_and(|signal|*signal.borrow()){Err(OperationError::Cancelled)}else{Ok(())}
}
pub async fn run<T>(signal:&mut Option<watch::Receiver<bool>>,future:impl std::future::Future<Output=Result<T,String>>)->Result<T,OperationError>{
 match signal{None=>future.await.map_err(OperationError::Failed),Some(signal)=>tokio::select!{biased;_=stopped(signal)=>Err(OperationError::Cancelled),result=future=>result.map_err(OperationError::Failed)}}
}
