//! Ações dos tickets: recibos duráveis e despacho para a sessão pelo runtime do servidor.
#![forbid(unsafe_code)]

mod error;
mod ledger;
mod engine;
pub mod http;

pub use error::Failure;
pub use ledger::{Attempt, Ledger, Reservation, Status};
pub use engine::{Engine, Sessions, Session, Request, Task, Delivery};
