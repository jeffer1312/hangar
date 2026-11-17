//! `state-hook`: três marcadores, nenhuma saída, sempre 0.
//!  - `.hangar-state/<session_id>.json` `{state, ts}`: o estado da lista sem ler o pane.
//!  - `.hangar-prompt/<session_id>.json` `{sha1, ts}`: o prompt passou da recusa e os hooks começaram;
//!    pasta própria porque o servidor regrava o `.hangar-state` sem campos extras.
//!  - `.hangar-active/<boot_id>.json` `{jsonl, ts, cwd, pid}`: o transcript ATIVO. Sessão aberta com
//!    `--session-id X` que retomou Y escreve em Y.jsonl e X.jsonl nunca nasce; só o hook conhece o par.
use crate::input::{self, file_key};
use crate::tree::{Processes, boot_claude};
use crate::{Context, Outcome};
use hangar_core::pycompat::{pyjson, truthy};
use serde_json::{Map, Value, json};
use std::path::Path;

pub const LOG: &str = "state_hook-falhas.log";

fn state_of(event: &str) -> Option<&'static str> {
    Some(match event {
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => "working",
        "Notification" => "awaiting_input",
        "Stop" => "idle",
        _ => return None,
    })
}

/// Falha calada em qualquer ponto: o primeiro erro encerra o resto, como a exceção do Python.
pub fn handle(stdin: &[u8], context: &Context, processes: &impl Processes, parent: Option<u32>) -> Outcome {
    let _ = write_markers(stdin, context, processes, parent);
    Outcome::default()
}

fn write_markers(stdin: &[u8], context: &Context, processes: &impl Processes, parent: Option<u32>) -> Option<()> {
    let event = input::parse(&input::decode(stdin))?;
    let event = event.as_object()?;
    let name = event.get("hook_event_name").unwrap_or(&Value::Null);
    // Evento lista ou objeto não servia de chave no dicionário do Python: nada era gravado.
    if name.is_array() || name.is_object() {
        return None;
    }
    let sid = event.get("session_id").unwrap_or(&Value::Null);
    let base = context.config_base()?;
    let name = name.as_str().unwrap_or_default();
    if let Some(state) = state_of(name)
        && truthy(sid)
    {
        marker(context, &base, ".hangar-state", sid, &json!({"state": state, "ts": input::now()}))?;
    }
    if name == "UserPromptSubmit"
        && truthy(sid)
        && let Some(prompt) = event.get("prompt").and_then(Value::as_str)
    {
        // Surrogate solto não vira UTF-8: o Python parava aqui, sem o marcador ativo.
        if input::has_surrogate(prompt) {
            return None;
        }
        let digest = sha1_smol::Sha1::from(hangar_core::pycompat::py::strip(prompt).as_bytes()).digest().to_string();
        marker(context, &base, ".hangar-prompt", sid, &json!({"sha1": digest, "ts": input::now()}))?;
    }
    let transcript = event.get("transcript_path").unwrap_or(&Value::Null);
    if truthy(transcript) {
        let (boot, pid) = parent.map_or((None, None), |parent| boot_claude(parent, processes));
        let boot = boot.map(Value::String).unwrap_or_else(|| sid.clone());
        // Sem ancestral claude provado (o Codex roda o mesmo hook com o mesmo payload) o marcador não
        // sai: o rollout dele seria adotado como transcript de uma sessão Claude.
        if truthy(&boot) && let Some(pid) = pid {
            let mut active = Map::new();
            active.insert("jsonl".into(), transcript.clone());
            active.insert("ts".into(), json!(input::now()));
            active.insert("cwd".into(), event.get("cwd").cloned().unwrap_or(Value::Null));
            active.insert("pid".into(), json!(pid));
            marker(context, &base, ".hangar-active", &boot, &Value::Object(active))?;
        }
    }
    Some(())
}

/// A pasta nasce antes de conferir a chave, como no Python (chave que não é texto deixava a pasta).
/// Falha de disco segue sem saída e com 0, mas deixa a linha no log privado: sem ela, a lista ficava
/// sem estado e sem pista.
fn marker(context: &Context, base: &Path, folder: &str, key: &Value, payload: &Value) -> Option<()> {
    let dir = base.join(folder);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        crate::log::record(context, LOG, &format!("{folder} não criada ({error})"));
        return None;
    }
    let key = file_key(key)?;
    crate::marker::write(&dir, key, pyjson::dumps(payload, false).as_bytes())
        .inspect_err(|error| crate::log::record(context, LOG, &format!("{folder} não gravado ({error})")))
        .ok()
}
