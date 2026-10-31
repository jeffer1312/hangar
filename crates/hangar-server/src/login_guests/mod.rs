//! Login próprio, propriedade das sessões e cofre de sincronização, com um escritor por arquivo.
mod http;
mod diary;
pub mod store;
mod sync;
mod validation;
pub(crate) mod schema;
pub use http::{Service, Settings, route, body_limit, owns_path, public_contract};

pub mod gate;

pub(super) fn object<const N:usize>(entries:[(&str,serde_json::Value);N])->serde_json::Value {
    serde_json::Value::Object(entries.into_iter().map(|(key,value)|(key.into(),value)).collect())
}
