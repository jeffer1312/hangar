//! Tipos e ajudantes comuns do servidor. Nada aqui depende do `hangar-server`.
#![forbid(unsafe_code)]

pub mod costs_failure;
pub mod http;
pub mod http_body_input;
pub mod http_body_limit;
pub mod http_contract;
pub mod http_cors;
pub mod http_route_response;
pub mod json_envelope;
pub mod net;
pub mod orq_codec;
pub mod pycompat;
pub mod query;
pub mod terminal_lock;
pub mod terminal_state;
pub mod value_coercion;
pub mod warn_limit;
