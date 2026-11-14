//! Tipos e ajudantes comuns do servidor. Nada aqui depende do `hangar-server`.
#![deny(unsafe_code)]

pub mod account_env;
pub mod auth;
pub mod clock;
pub mod config;
pub mod config_lock;
pub mod costs_failure;
pub mod credentials;
pub mod diag;
pub mod display_env;
pub mod edge;
pub mod fs;
// Troca atômica com o destino conferido: chamadas de `libc` e `windows-sys`.
#[allow(unsafe_code)]
pub mod fs_replace;
pub mod guest;
pub mod http;
pub mod http_body_input;
pub mod http_body_limit;
pub mod http_contract;
pub mod http_cors;
pub mod http_route_response;
pub mod ids;
pub mod install_root;
pub mod json_envelope;
pub mod llm_url;
pub mod log_rotate;
pub mod net;
pub mod orq_codec;
pub mod path_markers;
pub mod paths;
// Descritor de segurança do Windows: chamadas de `windows-sys`.
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod private_acl;
pub mod process;
pub mod provider_state;
pub mod pycompat;
pub mod query;
pub mod session_dirs;
pub mod queue_row;
pub mod terminal_lock;
// Grupo de processos e job object do Windows: chamadas de `libc` e `windows-sys`.
#[allow(unsafe_code)]
pub mod terminal_process;
pub mod terminal_state;
pub mod user_data;
pub mod value_coercion;
pub mod warn_limit;
