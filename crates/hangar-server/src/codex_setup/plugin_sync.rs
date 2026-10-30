//! Reconciliação de plugins: a fonte é relida antes de confirmar a seleção.
use super::phase::{Error,State};
use serde_json::Value;
use std::{future::Future,path::Path};

pub struct Paths<'a> {
    pub home:&'a Path,
    pub codex:&'a Path,
    pub backups:&'a Path,
}

pub trait Actions {
    fn installed(&mut self)->impl Future<Output=Result<Vec<Value>,Error>>+Send;
    fn detect(&mut self)->impl Future<Output=Result<Vec<Value>,Error>>+Send;
    fn import(&mut self,items:Vec<Value>)->impl Future<Output=Result<Value,Error>>+Send;
    fn upgrade(&mut self,market:&str)->impl Future<Output=Result<Value,Error>>+Send;
    fn add(&mut self,id:&str)->impl Future<Output=Result<Value,Error>>+Send;
    fn hooks(&mut self,path:&Path)->impl Future<Output=Result<(),Error>>+Send;
    fn edit(&mut self,choices:Vec<(String,bool)>)->impl Future<Output=Result<(),Error>>+Send;
    fn checkpoint(&mut self,registry:&mut Value,state:&State)->Result<(),Error>;
    fn now(&self)->f64;
}

pub async fn run(paths:&Paths<'_>,registry:&mut Value,state:&State,force:bool,actions:&mut impl Actions)->Result<(),Error> {
    implementation::run(paths,registry,state,force,actions).await
}

#[path="plugin_sync_impl.rs"]
mod implementation;

#[cfg(all(test,unix))]
#[path="plugin_sync_tests.rs"]
mod tests;
