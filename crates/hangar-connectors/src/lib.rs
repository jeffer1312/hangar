//! Conectores do Hangar: contrato por capacidade, registro por servidor e host de processos.
//!
//! O host não conhece fornecedor (Help, Jira, Linear): só o manifesto e o contrato de cada capacidade.
pub mod cli;
pub mod error;
pub mod host;
pub mod http;
pub mod manifest;
mod process;
pub mod protocol;
pub mod registry;
pub mod rpc;
pub mod work_actions;
pub mod work_tracking;

pub use error::{AdminError, AdminKind, CallError};
pub use host::{Host, HostOptions};
