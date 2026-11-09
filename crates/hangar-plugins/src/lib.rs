//! Plugins do Claude Code da pasta principal: modelo do contrato HTTP, leitura do disco e operações pela CLI.
#![forbid(unsafe_code)]

mod cli;
mod details;
mod disk;
mod model;
mod ops;

pub use cli::{CliError, CliOutput, CliRunner};
pub use disk::{AutoUpdateError, CliInstalled, apply_auto_update, read_state, source_version};
pub use model::{
    AvailablePlugin, InputError, InstalledPlugin, Inventory, Marketplace, MarketplaceName,
    MarketplaceSource, OperationResult, PluginDetails, PluginId, PluginsState,
};
pub use ops::{Plugins, PluginsError};
