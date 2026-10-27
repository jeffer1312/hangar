//! Cache limitado pela assinatura dos arquivos; uma leitura por alteração.
use indexmap::IndexMap;
use std::{path::PathBuf,sync::Mutex};
pub struct SnapshotCache<S,V>{entries:Mutex<IndexMap<PathBuf,(S,V)>>,evict_updated:bool}
impl<S:PartialEq,V:Clone> SnapshotCache<S,V>{
    pub fn new(evict_updated:bool)->Self{Self{entries:Mutex::new(IndexMap::new()),evict_updated}}
    pub fn get(&self,key:PathBuf,signature:S,fill:impl FnOnce()->V)->V{
        let mut entries=self.entries.lock().unwrap();
        if let Some((sig,value))=entries.get(&key){if *sig==signature{return value.clone()}}
        let value=fill();
        if entries.len()>=16&&(self.evict_updated||!entries.contains_key(&key)){entries.shift_remove_index(0);}
        entries.insert(key,(signature,value.clone()));value
    }
    pub fn len(&self)->usize{self.entries.lock().unwrap().len()}
}
