//! Login próprio, propriedade das sessões e cofre de sincronização, com um escritor por arquivo.
mod http;
mod diary;
pub mod store;
mod sync;
mod validation;
pub(crate) mod schema;
pub use http::{Service, Settings, route, body_limit, owns_path, public_contract};

pub mod gate;

/// Árvore externa pode exceder a pilha; até caminhos de erro descartam sem recursão.
pub(super) struct Tree(pub serde_json::Value);
impl std::ops::Deref for Tree {
    type Target=serde_json::Value;
    fn deref(&self)->&Self::Target {&self.0}
}
impl std::ops::DerefMut for Tree {
    fn deref_mut(&mut self)->&mut Self::Target {&mut self.0}
}
impl Drop for Tree {
    fn drop(&mut self){crate::orchestration::codec::discard(std::mem::take(&mut self.0));}
}
pub(super) fn object<const N:usize>(entries:[(&str,serde_json::Value);N])->serde_json::Value {
    serde_json::Value::Object(entries.into_iter().map(|(key,value)|(key.into(),value)).collect())
}
