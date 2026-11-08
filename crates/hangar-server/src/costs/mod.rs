//! Custos e uso: leitores dos transcripts, índice incremental próprio e relatórios.
//! Porte dos módulos de custos e uso do backend; o Python é a referência dos golden.
pub mod pricing;
pub mod py;
pub mod index;
pub mod rows;
pub mod areas;
pub mod uso_rules;
pub mod accumulator;
pub mod claude;
pub mod codex;
pub mod simple;
pub mod collect;
pub mod report_costs;
pub mod fx;
pub mod report_uso;
pub mod origins;
pub mod session_cost;

use std::any::Any;
use std::sync::{Arc, Mutex};
use indexmap::IndexMap;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub data_version: u64,
    pub pricing_generation: u64,
    pub area_signature: String,
    pub labels: Vec<(String, String)>,
    pub route: Vec<String>,
}

#[derive(Default)]
pub struct ReportCache {
    entries: Mutex<IndexMap<CacheKey, Arc<dyn Any + Send + Sync>>>,
}

impl ReportCache {
    pub fn get<T: Any + Send + Sync>(&self, key: &CacheKey) -> Option<Arc<T>> {
        let mut entries = self.entries.lock().unwrap();
        let value = entries.shift_remove(key)?;
        entries.insert(key.clone(), value.clone());
        value.downcast().ok()
    }

    pub fn insert<T: Any + Send + Sync>(&self, key: CacheKey, value: Arc<T>) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|old, _| old.data_version == key.data_version);
        entries.shift_remove(&key);
        entries.insert(key, value);
        while entries.len() > 8 { entries.shift_remove_index(0); }
    }
}

pub mod scopes;
