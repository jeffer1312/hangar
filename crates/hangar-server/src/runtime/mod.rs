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
pub mod queue;
pub mod receipt;
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
