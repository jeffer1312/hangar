//! Tipos e ajudantes comuns do servidor. Nada aqui depende do `hangar-server`.
#![deny(unsafe_code)]

pub mod account_env;
pub mod clock;
pub mod costs_failure;
pub mod credentials;
pub mod display_env;
pub mod fs;
pub mod http;
pub mod http_body_input;
pub mod http_body_limit;
pub mod http_contract;
pub mod http_cors;
pub mod http_route_response;
pub mod ids;
pub mod json_envelope;
pub mod log_rotate;
pub mod net;
pub mod orq_codec;
// Descritor de segurança do Windows: chamadas de `windows-sys`.
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod private_acl;
pub mod process;
pub mod pycompat;
pub mod query;
pub mod queue_row;
pub mod terminal_lock;
// Grupo de processos e job object do Windows: chamadas de `libc` e `windows-sys`.
#[allow(unsafe_code)]
pub mod terminal_process;
pub mod terminal_state;
pub mod value_coercion;
pub mod warn_limit;
