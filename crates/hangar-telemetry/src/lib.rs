//! Telemetria anônima de uso do Hangar: contadores por dia (UTC), sem texto livre por construção, e
//! as linhas de aviso e erro do servidor, que o servidor limpa antes de `Telemetry::log`.
//! `wire`, `events` e `buckets` compilam sem a feature `runtime`: o receptor
//! (`services/telemetry-ingest`) usa só eles.
pub mod buckets;
pub mod events;
pub mod wire;

#[cfg(feature = "runtime")]
mod aggregate;
#[cfg(feature = "runtime")]
mod queue;
#[cfg(feature = "runtime")]
mod send;
#[cfg(feature = "runtime")]
mod state;
#[cfg(feature = "runtime")]
mod switch;

#[cfg(feature = "runtime")]
pub use aggregate::{Clock, Config, EnvLookup, Telemetry};
#[cfg(feature = "runtime")]
pub use send::{DEFAULT_URL, Failure, Post, PostFuture};
#[cfg(feature = "runtime")]
pub use state::SETTINGS_CONSENT_SCOPE;
#[cfg(feature = "runtime")]
pub use switch::Disabled;
