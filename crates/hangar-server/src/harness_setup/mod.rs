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
