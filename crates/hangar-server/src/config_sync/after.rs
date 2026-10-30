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
                    if stats.is_object() && super::apply::truthy(&stats["erro"]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_sync::{bundle::Bundle, export::Context, paths::Roots};
    use serde_json::json;
    use std::sync::Mutex;

    struct Recorder { failure: String, calls: Mutex<Vec<Value>> }
    impl Actions for Recorder {
        fn bridge(&self) -> Step<'_, BridgeReport> { Box::pin(async {
            self.calls.lock().unwrap().push(json!("bridge"));
            if self.failure == "bridge" { return Err("falha de ponte".into()); }
            Ok(BridgeReport {stats:if self.failure == "bridge_report" { json!({"kimi":{"erro":1}}) } else { json!({}) },errors:Default::default()})
        }) }
        fn hook(&self,name:&'static str) -> Step<'_> { Box::pin(async move {
            self.calls.lock().unwrap().push(json!(name));
            if self.failure == name { Err("falha de hook".into()) } else { Ok(Value::Null) }
        }) }
        fn codex(&self) -> Step<'_> { Box::pin(async {
            self.calls.lock().unwrap().push(json!(["codex","config_sync",true]));
            if self.failure == "codex" { Err("falha de integração".into()) } else { Ok(Value::Null) }
        }) }
    }

    #[tokio::test]
    async fn after_apply_preserves_selected_order_and_partial_failures() {
        let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/after-apply.json")).unwrap();
        let mut differences = Vec::new();
        for (index,row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().to_string_lossy().into_owned();
            let mut context = Context::new(Roots {home:root.clone(),claude:format!("{root}/.claude"),codex:format!("{root}/.codex"),hangar:format!("{root}/repo")});
            let (sender,mut receiver) = tokio::sync::mpsc::unbounded_channel();
            context.progress = Some(sender);
            let mut apply = Apply::new(context,Bundle::default(),"fixture",Default::default());
            let actions = Recorder {failure:row["failure"].as_str().unwrap().into(),calls:Mutex::new(Vec::new())};
            let items: Vec<String> = row["items"].as_array().unwrap().iter().map(|v|v.as_str().unwrap().into()).collect();
            run(&mut apply,&items,row["enabled"].as_bool().unwrap(),&actions).await;
            let mut events = Vec::new();
            while let Ok(event) = receiver.try_recv() { events.push(event); }
            if json!(*actions.calls.lock().unwrap()) != row["calls"] || json!(events) != row["events"] || json!(apply.report) != row["report"] {
                differences.push(index);
            }
        }
        assert!(differences.is_empty(),"pós-aplicar divergente nos casos {differences:?}");
    }
}
