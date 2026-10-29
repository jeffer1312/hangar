//! Login próprio, propriedade das sessões e cofre de sincronização, com um escritor por arquivo.
mod http;
pub mod store;
mod sync;
mod validation;
pub use http::{Service, Settings, route};

pub mod gate;
