//! Hangar Connect sem o servidor: configuração, binários, ligação, aparelhos e supervisão do frpc e do Caddy.
//! Rota e porteiro (`AppState`, `Request`) ficam no `hangar-server`, que só liga este crate.
#![deny(unsafe_code)]

use axum::http::StatusCode;

pub mod binaries;
pub mod config;
pub mod devices;
pub mod state;
// `prctl` no filho (Linux) e `kill` do grupo (Unix).
#[allow(unsafe_code)]
pub mod supervisor;

pub const CONNECT_PORT: u16 = 8768;

/// Falha com código: vira resposta HTTP, e o `msg` aparece em `error` de `GET /api/connect`.
#[derive(Clone, Debug)]
pub struct ConnectError {
    pub status: StatusCode,
    pub code: &'static str,
    pub msg: String,
}

impl ConnectError {
    pub fn new(status: StatusCode, code: &'static str, msg: impl Into<String>) -> Self {
        Self { status, code, msg: msg.into() }
    }
}
