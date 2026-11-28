//! Runtime exclusivo do Claude terminal/sem terminal, do Codex sem terminal e do Codex com terminal adotado.
pub mod protocol;
pub mod cano;
pub mod cano_reconnect;
pub mod websocket;
pub mod codex_terminal;
pub mod codex_sidecar;
pub mod claude_sidecar;
pub mod native_message;
pub mod codex_launch;
pub mod claude_launch;
pub mod codex_pane;
pub mod codex_plugins;
pub mod codex_relaunch;
pub mod antigravity;
pub mod antigravity_launch;
pub mod antigravity_sidecar;
pub mod queue {
    use std::{io, path::Path};

    pub use hangar_core::fs::{QueueRefusal, read_replaced, read_replaced_to_string};
    pub(crate) use hangar_core::{fs::{atomic_write, replace}, queue_row::row_waits};
    pub(super) use hangar_core::fs::invalid;
    #[cfg(test)]
    pub(crate) use hangar_core::fs::atomic_write_inspected;
    pub use hangar_runtime_queue::{
        Action, CODEX_LIFE, Lease, Operation, QueueActor, State, Status, Store,
        acquire_lease, acquire_lease_within, discard, oldest_waiting, refusal, waiting_ids,
    };
    pub(crate) use hangar_runtime_queue::{CAP, legacy_rows, projection_path, queue_text_ok};
    pub(super) use hangar_runtime_queue::{BATCH_SIZE_MESSAGE, BATCH_TOO_LARGE, BatchValidator};

    pub(crate) fn open_observed_store(path: &Path, projection: &Path, initial: State) -> io::Result<Store> {
        Ok(Store::open(path, projection, initial)?.with_delivery_observer(
            hangar_runtime_queue::DeliveryObserver {
                tracks: crate::telemetry_hooks::speed_knows,
                delivered: crate::telemetry_hooks::speed_delivered,
            },
        ))
    }

    pub(super) fn queue_problem(state: &mut hangar_api::state::StateEvent, detail: Option<String>) -> bool {
        if state.problema.as_deref().is_some_and(|code| code != BATCH_TOO_LARGE) { return false; }
        let code = detail.as_ref().map(|_| BATCH_TOO_LARGE.to_owned());
        if state.problema == code && state.problema_detalhe == detail { return false; }
        state.problema = code;
        state.problema_detalhe = detail;
        true
    }
}
pub mod receipt {
    pub use hangar_runtime_queue::{DispatchCursor, Occurrence, ReceiptIndex, ReceiptProof};
}
pub mod claude;
pub mod codex;
pub mod actor;
pub mod gateway;
pub mod ingress;
pub mod terminal;
pub mod other_terminal;
pub mod terminal_status;
pub mod local_policy;
pub mod process;
pub mod claude_relaunch;
pub mod claude_terminal;
pub mod claude_reopen;
pub mod lifecycle;
pub mod close_name;

#[derive(Default)]
pub(crate) struct LiveBuffer {
    text: String,
    last: Option<f64>,
    dirty: bool,
    blocked: bool,
    visible: bool,
}

impl LiveBuffer {
    pub fn append(&mut self, piece: &str, now: f64) -> Option<String> {
        if piece.is_empty() || self.blocked { return None; }
        self.text.push_str(piece);
        self.dirty = true;
        if self.last.is_none_or(|last| now + 1e-9 >= last + 0.15) { self.flush(now) } else { None }
    }
    pub fn flush(&mut self, now: f64) -> Option<String> {
        if !self.dirty || self.blocked { return None; }
        self.dirty = false; self.last = Some(now); self.visible = !self.text.is_empty();
        Some(self.text.clone())
    }
    pub fn tick(&mut self, now: f64) -> Option<String> {
        if self.deadline().is_some_and(|deadline| now + 1e-9 >= deadline) { self.flush(now) } else { None }
    }
    pub fn clear(&mut self) -> Option<String> {
        self.text.clear(); self.last = None; self.dirty = false; self.blocked = false;
        if std::mem::take(&mut self.visible) { Some(String::new()) } else { None }
    }
    pub fn block(&mut self) { self.clear(); self.blocked = true; }
    pub fn unblock(&mut self) { if self.blocked { self.clear(); } }
    pub fn deadline(&self) -> Option<f64> { if self.dirty && !self.blocked { self.last.map(|last|last+0.15) } else { None } }
}

/// A ferramenta ao vivo como os clientes a leem (`{"nome","input"}`): texto em outro formato derruba o chat deles.
pub(crate) fn live_tool_text(name: &str, input: &serde_json::Value) -> String {
    crate::transcript::pyjson::dumps(&serde_json::json!({"nome": name, "input": input}), false)
}
