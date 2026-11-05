//! Motores locais: o mesmo catálogo serve HTTP, abertura e o CLI sem serviço.
pub mod store;
pub mod probe;
pub mod proxy;
pub mod environment;
pub mod cli;
pub mod settings;
mod routes;
pub use routes::{cors_layer, matches, serve};

pub mod mirrors;

pub mod catalog;

pub mod model_http;

pub(crate) mod decimal;


pub mod body;

pub mod providers;
