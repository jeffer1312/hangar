//! Um cálculo por chave; a resposta conserva o último valor durante a atualização.
use indexmap::IndexMap;
use serde_json::Value;
use std::{io,sync::{Arc,Mutex}};
pub type Work=Box<dyn FnOnce()+Send>;
pub type Clock=Arc<dyn Fn()->f64+Send+Sync>;
#[derive(Clone)]
struct Hit {at:f64,value:Value,error:Option<String>,unavailable:bool}
#[derive(Default)]
struct Entry {hit:Option<Hit>,running:bool}
#[derive(Default)]
pub struct Cache {entries:Mutex<IndexMap<String,Entry>>}
impl Cache {
    pub fn request(self:&Arc<Self>,key:String,clock:Clock,ttl:f64,retry:f64,
        compute:impl FnOnce()->Result<(Value,bool),String>+Send+'static,
        spawn:impl FnOnce(Work)->io::Result<()>,
    )->(Value,Option<String>){
        let mut entries=self.entries.lock().unwrap();
        if entries.len()>=16&&!entries.contains_key(&key){
            if let Some(i)=entries.iter().position(|(_,e)|!e.running){entries.shift_remove_index(i);}
        }
        let entry=entries.entry(key.clone()).or_default();
        let stale=entry.hit.as_ref().map(|h|(h.value.clone(),h.error.clone())).unwrap_or_default();
        if entry.running||entry.hit.as_ref().is_some_and(|h|clock()-h.at<if h.unavailable{retry}else{ttl}){return stale}
        entry.running=true;drop(entries);
        let cache=self.clone();let worker_key=key.clone();let worker_clock=clock.clone();
        let work=Box::new(move||{
            let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(compute))
                .unwrap_or_else(|_|Err("não foi possível calcular o consumo".into()));
            let mut entries=cache.entries.lock().unwrap();let entry=entries.entry(worker_key).or_default();entry.running=false;
            entry.hit=Some(match result{
                Ok((value,unavailable))=>Hit{at:worker_clock(),value,error:None,unavailable},
                Err(error)=>Hit{at:worker_clock(),value:Value::Null,error:Some(error),unavailable:false},
            });
        });
        if let Err(e)=spawn(work){
            let error=e.to_string();let mut entries=self.entries.lock().unwrap();let entry=entries.entry(key).or_default();entry.running=false;
            entry.hit=Some(Hit{at:clock(),value:Value::Null,error:Some(error.clone()),unavailable:true});
            return(Value::Null,Some(error))
        }
        stale
    }
    pub fn running(&self,key:&str)->bool {self.entries.lock().unwrap().get(key).is_some_and(|e|e.running)}
    pub fn len(&self)->usize {self.entries.lock().unwrap().len()}
}
