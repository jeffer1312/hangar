//! Configuração compartilhada: pacote portável e aplicação com backup no destino.
pub mod apply;
pub mod bundle;
pub use crate::command_capture as command;
pub use crate::cli_probe;
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
pub mod runtime;
mod runtime_defaults;
pub mod translate;
