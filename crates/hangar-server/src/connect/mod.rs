//! Entrada do Hangar Connect: o Caddy desta máquina entrega na 8768 o que veio da internet.
use axum::http::StatusCode;

pub mod binaries;
pub mod config;
pub mod devices;
pub mod gate;
pub mod routes;
pub mod state;
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
