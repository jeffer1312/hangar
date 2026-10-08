//! Proteção de existência e fatos estritos de uso das contas.
pub mod bridge;
pub mod locks;
pub mod types;
pub use locks::{AccountGuard, AccountLocks, LockError};
pub use types::{AccountKey, GuardMode, Provider, UsageFacts};
