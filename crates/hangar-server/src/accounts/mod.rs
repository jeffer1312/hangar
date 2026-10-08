//! Proteção de existência e fatos estritos de uso das contas.
pub mod bridge;
pub mod catalog;
pub mod environment;
pub mod storage;
pub mod native;
pub mod http;
pub use catalog::AccountService;
pub mod locks;
pub mod types;
pub use locks::{AccountGuard, AccountLocks, LockError};
pub use types::{AccountKey, GuardMode, Provider, UsageFacts};
