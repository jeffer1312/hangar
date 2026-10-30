//! Reconciliação nativa do Codex e escritas compartilhadas de configuração.
pub mod files;
pub mod native;
pub mod native_cli;
pub mod import_api;

pub mod ecc;
pub mod hooks;
pub mod instructions;
mod instructions_alias;
mod instructions_imports;
mod instructions_lock;
mod instructions_projects;
mod instructions_toml;
pub mod normalize;
pub mod plugins;
pub mod plugin_selection;
pub mod trust;

pub fn sync_enabled(config: &crate::config_sync::runtime::RuntimeConfig) -> bool {
    if config
        .env
        .base
        .get("CP_CODEX_SYNC_ENABLED")
        .is_some_and(|value| matches!(value.to_lowercase().as_str(), "0" | "false" | "no"))
    {
        return false;
    }
    let sync = crate::config_sync::runtime_values::ValueGuard(config.get("codex_sync"));
    if !crate::config_sync::apply::truthy(&sync) {
        return false;
    }
    let automations = crate::config_sync::runtime_values::ValueGuard(config.get("automations"));
    crate::config_sync::apply::truthy(&automations)
}

#[cfg(test)]
#[path = "sync_gate_tests.rs"]
mod sync_gate_tests;

pub mod repair;
pub mod status;
pub mod fragments;
pub mod messages;
pub mod service;
pub mod artifacts;
pub mod batch;
pub mod reload;

pub mod phase;

pub mod skills;

pub mod config_edits;
