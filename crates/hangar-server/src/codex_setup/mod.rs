//! Reconciliação nativa do Codex e escritas compartilhadas de configuração.
pub mod files;
pub mod native;
pub mod native_cli;
mod native_shutdown;
pub mod import_api;

pub mod ecc;
pub mod hooks;
pub mod instructions;
mod instructions_alias;
pub(crate) mod instructions_imports;
mod instructions_lock;
mod instructions_projects;
mod instructions_toml;
pub mod normalize;
pub mod plugins;
pub mod plugin_selection;
pub mod plugin_sync;
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
    if !crate::orchestration::truthy(&sync) {
        return false;
    }
    let automations = crate::config_sync::runtime_values::ValueGuard(config.get("automations"));
    crate::orchestration::truthy(&automations)
}

pub mod repair;
pub mod status;
pub mod fragments;
pub mod messages;
pub mod service;
mod service_binding;
pub mod hook_wrappers;
pub mod hook_command;
pub mod artifacts;
pub mod batch;
mod batch_native;
pub mod reload;

pub mod phase;

pub mod skills;

pub mod config_edits;

pub mod fingerprints;
pub mod stage_sources;

pub mod plugin_edits;

pub mod plugin_native;

pub mod stage_history;

pub mod stage_flow;

mod cancellation;
pub mod timestamp;
pub mod pipeline_hooks;
pub mod hook_files;

pub mod pipeline_round;

mod pipeline_native;
mod pipeline_fingerprint;
mod stage_native;
mod plugin_controlled;
mod reconcile;
mod legacy_instructions;

pub mod options;
pub mod options_http;
