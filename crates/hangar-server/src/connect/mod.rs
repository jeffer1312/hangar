//! Entrada do Hangar Connect: o Caddy desta máquina entrega na 8768 o que veio da internet.
pub mod devices;
pub mod gate;
pub mod routes;

pub const CONNECT_PORT: u16 = 8768;
