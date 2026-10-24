//! Fonte de mentira ligada a um hub de verdade: quadro fixo, sem Python, tmux nem disco.
use std::sync::{Arc, Weak};

use hangar_api::ask::AskQuestion;
use hangar_api::preview::PreviewEvent;
use hangar_api::state::StateEvent;
use serde_json::Value;
use tokio::sync::Notify;

use super::facts::Dead;
use super::monitor::{CaptureFailed, FileFacts, Frame, RoundFacts, Sources, wall_now};
use super::preview::HookFile;
use crate::side::Hub;

pub struct HubFake { hub: Weak<Hub>, pane: &'static str, wake: Arc<Notify>, hub_wake: Arc<Notify> }

impl HubFake {
    pub fn new(hub: &Arc<Hub>, pane: &'static str) -> Self {
        Self { hub: Arc::downgrade(hub), pane, wake: Arc::default(), hub_wake: hub.wake() }
    }

    fn publish_raw(&self, event: &str, data: &str) -> bool { self.hub.upgrade().is_some_and(|h| h.publish_own(event, data)) }
}

impl Sources for HubFake {
    fn name(&self) -> &str { "s" }
    fn sid(&self) -> Option<String> { self.hub.upgrade()?.session_key() }
    fn epoch(&self) -> u64 { self.hub.upgrade().and_then(|h| h.generation()).unwrap_or(u64::MAX) }
    fn wake(&self) -> Arc<Notify> { self.wake.clone() }
    fn hub_wake(&self) -> Arc<Notify> { self.hub_wake.clone() }
    async fn facts(&self) -> RoundFacts { RoundFacts::default() }
    async fn capture(&self) -> Result<Frame, CaptureFailed> {
        Ok(Frame { text: self.pane.into(), analysis: crate::terminal_state::analyze(self.pane) })
    }
    async fn has_session(&self) -> Option<bool> { Some(true) }
    async fn dead(&self) -> Result<Dead, String> { Ok(Dead::Ok) }
    async fn observe_permission(&self, _: &str, mode: &str) -> Result<(String, String), String> { Ok((mode.into(), "manual".into())) }
    async fn files(&self, _: Option<&str>) -> FileFacts { FileFacts::default() }
    async fn publish(&self, event: StateEvent) -> bool { self.publish_raw("state", &serde_json::to_string(&event).unwrap()) }
    async fn emit(&self, event: &'static str, data: Value) -> bool { self.publish_raw(event, &data.to_string()) }
    fn runtime_wake(&self) -> Arc<Notify> { Arc::default() }
    fn runtime_problem(&self) -> Option<(String, String)> { None }
    async fn ask_payload(&self) -> Result<Option<AskQuestion>, String> { Ok(None) }
    fn deliverable(&self) {}
    fn trust_dialog(&self) {}
    async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { None }
    async fn preview_files(&self, _: &str) -> Vec<HookFile> { Vec::new() }
    fn committed(&self) -> Option<Arc<str>> { self.hub.upgrade()?.committed() }
    async fn publish_preview(&self, event: PreviewEvent) -> bool { self.publish_raw("preview", &serde_json::to_string(&event).unwrap()) }
    fn wall(&self) -> f64 { wall_now() }
}
