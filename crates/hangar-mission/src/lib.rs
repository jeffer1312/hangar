//! Tickets: etapas do trabalho por ticket, sessão ligada, portão do dono e evidências de validação.
//!
//! Não conhece fornecedor nem conector: guarda o que o dono e as sessões informam sobre cada ticket.
pub mod http;
pub mod model;
pub mod store;

pub use store::Store;
