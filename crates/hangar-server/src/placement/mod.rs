//! Balanceamento de sessões entre máquinas: o servidor liga o crate `hangar-placement` (rotas, fatos,
//! escolha antes da abertura e encaminhamento).
pub mod routes;
pub mod context;
pub mod facts;
pub mod forward;
pub mod suggest;
