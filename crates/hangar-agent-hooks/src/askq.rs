//! `askq-capture`: no `PreToolUse` do `AskUserQuestion` grava o evento cru em
//! `.hangar-askq/<session_id>.json`; no `PostToolUse(Failure)` apaga o da MESMA chamada. Nunca escreve
//! no stdout (o `PreToolUse` dessa ferramenta conflita com stdout) e sai 0 em toda falha; a de disco
//! deixa a linha no log privado.
use crate::input::{self, file_key};
use crate::{Context, Outcome};
use hangar_core::pycompat::truthy;
use serde_json::Value;

pub const LOG: &str = "askq_capture-falhas.log";

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
    let failed = |what: &str, error: &std::io::Error| crate::log::record(context, LOG, &format!("{what} ({error})"));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        failed(".hangar-askq não criada", &error);
        return None;
    }
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
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    failed("pergunta respondida não apagada", &error);
                    return None;
                }
                _ => {}
            }
        }
        return Some(());
    }
    crate::marker::write(&dir, key, raw.as_bytes()).inspect_err(|error| failed("pergunta não gravada", error)).ok()
}
