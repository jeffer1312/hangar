//! Plugins do Claude Code da pasta principal: modelo do contrato HTTP, leitura do disco e operações pela CLI.
#![forbid(unsafe_code)]

mod cli;
mod details;
mod disk;
mod model;
mod ops;
pub mod repair;

pub use cli::{CliError, CliOutput, CliRunner};
pub use disk::{AutoUpdateError, apply_auto_update, read_state};
pub use model::{InputError, MarketplaceName, MarketplaceSource, OperationResult, PluginId, PluginsState};
pub use ops::{Plugins, PluginsError};
