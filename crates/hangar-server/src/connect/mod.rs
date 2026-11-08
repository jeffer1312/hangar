//! Entrada do Hangar Connect: o Caddy desta máquina entrega na 8768 o que veio da internet.
//! O que não depende do servidor mora no `hangar-connect`; aqui ficam o porteiro e as rotas.
pub use hangar_connect::{CONNECT_PORT, ConnectError, GUEST_PORT, binaries, config, devices, peers, preview, state, supervisor};

pub mod gate;
pub mod routes;
