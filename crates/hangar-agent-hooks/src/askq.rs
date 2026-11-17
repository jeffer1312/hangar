//! `askq-capture`: no `PreToolUse` do `AskUserQuestion` grava o evento cru em
//! `.hangar-askq/<session_id>.json`; no `PostToolUse(Failure)` apaga o da MESMA chamada. Nunca escreve
//! no stdout (o `PreToolUse` dessa ferramenta conflita com stdout) e cala toda falha.
use crate::input::{self, file_key};
use crate::{Context, Outcome};
use hangar_core::pycompat::truthy;
use serde_json::Value;

pub fn handle(stdin: &[u8], context: &Context) -> Outcome {
    let _ = capture(stdin, context);
    Outcome::default()
}

fn capture(stdin: &[u8], context: &Context) -> Option<()> {
    let raw = input::decode(stdin);
    let event = input::parse(&raw)?;
    let event = event.as_object()?;
    if event.get("tool_name").and_then(Value::as_str) != Some("AskUserQuestion") {
        return None;
    }
    let dir = context.config_base()?.join(".hangar-askq");
    std::fs::create_dir_all(&dir).ok()?;
    let sid = event.get("session_id").filter(|sid| truthy(sid)).cloned().unwrap_or_else(|| Value::String("unknown".into()));
    let key = file_key(&sid)?;
    let path = dir.join(format!("{key}.json"));
    let name = event.get("hook_event_name").and_then(Value::as_str);
    if matches!(name, Some("PostToolUse" | "PostToolUseFailure")) {
        // Respondida ou recusada no terminal: some só a mesma pergunta, nunca uma mais nova. Sem id é
        // de versão antiga do Claude Code, anterior a esta chamada.
        let saved = hangar_core::fs::read_replaced(&path).ok()?;
        let saved = input::parse(std::str::from_utf8(&saved).ok()?)?;
        let saved_id = saved.as_object()?.get("tool_use_id").unwrap_or(&Value::Null);
        if saved_id.is_null() || Some(saved_id) == event.get("tool_use_id") {
            std::fs::remove_file(&path).ok()?;
        }
        return Some(());
    }
    crate::marker::write(&dir, key, raw.as_bytes()).ok()
}
