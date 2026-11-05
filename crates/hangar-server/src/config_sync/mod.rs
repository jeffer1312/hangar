//! Configuração compartilhada: pacote portável e aplicação com backup no destino.
pub mod after;
pub mod apply;
pub mod apply_http;
mod apply_run;
pub mod bundle;
pub use crate::cli_probe;
pub use crate::command_capture as command;
pub mod describe;
pub mod export;
pub mod files;
pub mod http;
pub mod install;
pub mod jev;
pub mod legacy;
pub mod metadata;
pub mod paths;
pub mod plugin_publish;
pub mod preferences;
#[cfg(test)]
mod review_tests;
pub mod runtime;
mod runtime_defaults;
pub mod runtime_error;
pub mod runtime_http;
pub(crate) mod runtime_values;
pub(crate) mod value_equality;
