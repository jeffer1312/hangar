//! Conectores do Hangar: contrato por capacidade, registro por servidor e host de processos.
//!
//! O host não conhece fornecedor (Help, Jira, Linear): só o manifesto e o contrato de cada capacidade.
pub mod error;
pub mod manifest;
pub mod protocol;
pub mod registry;
pub mod work_tracking;

pub use error::{AdminError, CallError};
