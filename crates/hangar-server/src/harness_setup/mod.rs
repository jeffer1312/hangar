//! Instalação compartilhada dos recursos dos harnesses, sem processo auxiliar Python.
pub mod cli;
pub mod codex_hooks;
pub mod files;
pub mod hooks;
pub mod kimi_hooks;
pub mod persona;
pub mod skills;
use crate::accounts::{AccountService, environment::AccountEnvironment};
use serde_json::Value;
use std::path::Path;
pub fn sync_skills(env: &AccountEnvironment, repo: &Path) -> Value {
    skills::rebuild(env, repo, false)
}
/// Hooks que a subida confere: os do Claude e, à parte, os do Codex.
const STARTUP_HOOKS: &[&str] = &[
    "ensure_askq_hook_installed", "ensure_state_hooks_installed", "ensure_preview_hook_installed",
    "ensure_subagent_hook_installed", "ensure_pair_hook_installed", "ensure_nav_hook_installed",
];

/// A cada subida: versão nova de plugin muda o caminho do cache, e checkout ou pacote trocado muda o comando dos hooks.
pub fn sync_startup(service: &AccountService) {
    let Some(repo) = crate::install_root::root(&service.env.home) else {
        tracing::warn!(code = "harness_startup_no_root", "raiz da instalação não encontrada; hooks e skills não conferidos na subida");
        return;
    };
    skills::rebuild_report(&service.env, &repo, false);
    for name in STARTUP_HOOKS {
        if let Err(errors) = hooks::sync_installer(service, &repo, name) {
            tracing::warn!(code = "harness_startup_hooks_failed", installer = name, ?errors, "hooks do Claude não conferidos na subida");
        }
    }
    if let Err(error) = codex_hooks::sync(&service.env.codex_default, &repo, cfg!(windows)) {
        tracing::warn!(code = "harness_startup_codex_hooks_failed", %error, "hooks do Codex não conferidos na subida");
    }
}

pub fn sync_hooks(service: &AccountService, repo: &Path) -> Value {
    let mut result = hooks::sync(service, repo);
    match kimi_hooks::sync(&service.env, repo) {
        Ok(paths) => result["kimi"] = serde_json::json!(paths),
        Err(error) => {
            tracing::warn!(%error,"Hooks Kimi não atualizados");
            result["kimi"] = serde_json::json!([]);
        }
    }
    match codex_hooks::sync(&service.env.codex_default, repo, cfg!(windows)) {
        Ok(events) => result["codex"] = serde_json::json!(events),
        Err(error) => {
            tracing::warn!(%error,"Hooks Codex não atualizados");
            result["codex"] = serde_json::json!([]);
        }
    }
    result
}

pub mod health;

pub mod commands;
mod health_details;
pub mod service;
mod repair;
pub mod installer;
pub mod http;
pub mod routing;
