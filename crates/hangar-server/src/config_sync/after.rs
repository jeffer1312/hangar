//! Pós-aplicar compartilhado; uma falha de integração não desfaz os itens gravados.
use super::apply::Apply;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin};

pub const HOOKS: &[&str] = &[
    "ensure_askq_hook_installed", "ensure_state_hooks_installed", "ensure_preview_hook_installed",
    "ensure_subagent_hook_installed", "ensure_pair_hook_installed", "ensure_nav_hook_installed",
    "ensure_guard_hooks_installed",
];

pub use crate::harness_setup::skills::RebuildReport as BridgeReport;

#[cfg(test)]
#[path="after_native_tests.rs"]
mod native_tests;
pub type Step<'a, T = Value> = Pin<Box<dyn Future<Output=Result<T,String>> + Send + 'a>>;
pub trait Actions {
    fn bridge(&self) -> Step<'_, BridgeReport>;
    fn hook(&self, name: &'static str) -> Step<'_>;
    fn codex(&self) -> Step<'_>;
}

pub struct NativeActions {
    pub state: std::sync::Arc<crate::routes::AppState>,
    pub repo: std::path::PathBuf,
}

impl Actions for NativeActions {
    fn bridge(&self) -> Step<'_, BridgeReport> {
        let env = self.state.accounts.env.clone();
        let repo = self.repo.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || crate::harness_setup::skills::rebuild_report(&env, &repo, false))
                .await.map_err(|_| "falha ao reconstruir os espelhos de skills".to_owned())
        })
    }

    fn hook(&self, name: &'static str) -> Step<'_> {
        let accounts = self.state.accounts.clone();
        let repo = self.repo.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || crate::harness_setup::hooks::sync_installer(&accounts, &repo, name))
                .await.map_err(|_| "falha ao instalar os hooks do Hangar".to_owned())?
                .map_err(|errors| errors.join("\n"))
        })
    }

    fn codex(&self) -> Step<'_> {
        Box::pin(async {
            self.state.codex_setup_service()?.start("config_sync", true).await
        })
    }
}

fn warning(context: &mut Apply, item: &str, step: &str, error: &str) {
    context.warn(item,"config_sync_after_failed",json!({"step":step,
        "error":crate::orchestration::codec::truncate_string(error,200)}));
}

pub async fn run(context: &mut Apply, items: &[String], codex_enabled: bool, actions: &impl Actions) {
    let selected = |item: &str|items.iter().any(|selected|selected == item);
    if let Some(item) = ["claude_skills","claude_hooks","claude_plugins"].into_iter().find(|item|selected(item)) {
        context.context.emit(json!({"phase":"after","item":item,"entry":"skill_bridge"}));
        match actions.bridge().await {
            Err(error) => warning(context,item,"skill_bridge",&error),
            Ok(report) => {
                let mut entries: Vec<_> = report.stats.as_object().into_iter().flatten().collect();
                entries.sort_by_key(|(name,_)|*name);
                for (name,stats) in entries {
                    if stats.is_object() && crate::orchestration::truthy(&stats["erro"]) {
                        let cause=report.errors.get(name).map(String::as_str).unwrap_or("erro");
                        warning(context,item,&format!("skill_bridge:{name}"),cause);
                    }
                }
                crate::orchestration::codec::discard(report.stats);
            }
        }
    }
    if selected("claude_hooks") {
        context.context.emit(json!({"phase":"after","item":"claude_hooks","entry":"hangar_hooks"}));
        for name in HOOKS {
            match actions.hook(name).await {
                Ok(value) => crate::orchestration::codec::discard(value),
                Err(error) => warning(context,"claude_hooks",name,&error),
            }
        }
    }
    if selected("codex") && codex_enabled {
        context.context.emit(json!({"phase":"after","item":"codex","entry":"codex_integration"}));
        match actions.codex().await {
            Ok(value) => crate::orchestration::codec::discard(value),
            Err(error) => warning(context,"codex","codex_integracao",&error),
        }
    }
}
