// crates/hangar-server/src/transcript/codex.rs
//! Porte de `parse_rollout_obj` e `_event_id` (backend/app/adapters/codex/rollout.py).
//! Campo com tipo errado, que no Python levanta exceção, aqui conta como ausente.

use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use super::py::{self, py_re, strip};
use super::{event, finish, pyjson};

// rollout.py:20-33
static CONTEXT_WRAPPER: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"\A(<(?:environment_context|recommended_plugins|[a-z][a-z_ ]*instructions)>|# AGENTS\.md instructions(?: for |[ \t]*\r?\n\s*<INSTRUCTIONS>))")
});
static TURN_ABORTED: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)\A<turn_aborted>.*</turn_aborted>\z"));
static HOOK_PROMPT: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)\A<hook_prompt\b[^>]*>(.*)</hook_prompt>\z"));
static SKILL: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"(?s)\A<skill>\s*<name>([^<]*)</name>\s*(?:<path>([^<]*)</path>)?\s*(.*?)\s*</skill>\z")
});
// rollout.py:67-96
static TOOL_IN_CODE: LazyLock<Regex> = LazyLock::new(|| py_re(r"\btools\.(\w+)\s*\("));
static FIRST_STRING: LazyLock<Regex> = LazyLock::new(|| py_re(r#""((?:[^"\\]|\\.)*)""#));
static STEP: LazyLock<Regex> = LazyLock::new(|| py_re(r#"\bstep\s*:\s*"((?:[^"\\]|\\.)*)""#));
static STATUS: LazyLock<Regex> = LazyLock::new(|| py_re(r#"\bstatus\s*:\s*"(\w+)""#));
static CMD: LazyLock<Regex> = LazyLock::new(|| py_re(r#"\bcmd"?\s*:\s*"((?:[^"\\]|\\.)*)""#));
static PATCH_FILE: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?m)^\*{3} (?:Add|Update|Delete) File: (.+)$"));
// rollout.py:147
static SCRIPT_HEADER: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"\AScript (completed|failed)\nWall time [\d.]+ seconds\nOutput:\n"));

/// `_event_id` (rollout.py:40): sha1 do `json.dumps(obj, sort_keys=True)` da linha inteira.
pub(crate) fn event_id(line: &Value) -> String {
    sha1_smol::Sha1::from(pyjson::dumps(line, true)).digest().to_string()
}

/// `parse_rollout_obj` (rollout.py:222), já com o `scrub_surrogates` do `ChatEvent`.
pub(crate) fn parse_rollout_obj(line: &Value) -> Vec<ChatEvent> {
    let mut out = parse_raw(line);
    out.iter_mut().for_each(finish);
    out
}

fn parse_raw(line: &Value) -> Vec<ChatEvent> {
    let Some(obj) = line.as_object() else { return Vec::new() };
    if obj.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(payload) = obj.get("payload").and_then(Value::as_object) else { return Vec::new() };
    let id = || event_id(line);
    match payload.get("type").and_then(Value::as_str) {
        Some("message") => message(payload, id),
        Some("function_call") => {
            let args = payload.get("arguments");
            // `json.loads(arguments or "{}")`, e o que não for objeto vira {}.
            let tool_input = match args {
                Some(Value::String(s)) if py::truthy(args) => match pyjson::loads_lossless(s) {
                    Some(Value::Object(m)) => m,
                    _ => Map::new(),
                },
                _ => Map::new(),
            };
            vec![ChatEvent {
                tool_name: str_field(payload, "name"),
                tool_use_id: str_field(payload, "call_id"),
                tool_input: Some(tool_input),
                ..event(ChatKind::ToolUse, id())
            }]
        }
        Some("custom_tool_call") => vec![custom_tool_call(payload, id())],
        Some("custom_tool_call_output" | "function_call_output") => {
            let (result, failed) = output_result(payload.get("output"));
            vec![ChatEvent {
                tool_use_id: str_field(payload, "call_id"),
                result,
                is_error: Some(failed),
                ..event(ChatKind::ToolResult, id())
            }]
        }
        _ => Vec::new(),
    }
}

fn str_field(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).map(str::to_string)
}

fn text_event(kind: ChatKind, id: String, text: String) -> ChatEvent {
    ChatEvent { text: Some(text), ..event(kind, id) }
}

/// `_blocks_text` (rollout.py:46).
fn blocks_text(content: Option<&Value>, block_type: &str) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(Value::as_object)
            .filter(|b| b.get("type").and_then(Value::as_str) == Some(block_type))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect(),
        _ => String::new(),
    }
}

fn message(payload: &Map<String, Value>, id: impl Fn() -> String) -> Vec<ChatEvent> {
    match payload.get("role").and_then(Value::as_str) {
        Some("user") => {
            let text = blocks_text(payload.get("content"), "input_text");
            let t = strip(&text);
            if CONTEXT_WRAPPER.is_match(t) {
                return Vec::new();
            }
            if TURN_ABORTED.is_match(t) {
                return vec![text_event(ChatKind::Notice, id(), "turn_aborted".into())];
            }
            if let Some(h) = HOOK_PROMPT.captures(t) {
                return vec![ChatEvent {
                    hook_error: Some(strip(&h[1]).to_string()),
                    ..text_event(ChatKind::Notice, id(), "hook_prompt".into())
                }];
            }
            if let Some(s) = SKILL.captures(t) {
                let path = s.get(2).map_or("", |m| strip(m.as_str()));
                let mut skill = Map::new();
                skill.insert("name".into(), strip(&s[1]).into());
                skill.insert("path".into(), if path.is_empty() { Value::Null } else { path.into() });
                skill.insert("body".into(), s[3].into());
                return vec![ChatEvent { skill: Some(skill), ..text_event(ChatKind::Notice, id(), "skill_loaded".into()) }];
            }
            vec![text_event(ChatKind::UserMsg, id(), text)]
        }
        Some("assistant") => {
            vec![text_event(ChatKind::AssistantMsg, id(), blocks_text(payload.get("content"), "output_text"))]
        }
        _ => Vec::new(),
    }
}

/// `_unescape_js` (rollout.py:99).
fn unescape_js(raw: &str) -> String {
    match pyjson::loads_lossless(&format!("\"{raw}\"")) {
        Some(Value::String(s)) => s,
        _ => raw.to_string(),
    }
}

fn js_string(text: &str) -> String {
    FIRST_STRING.captures(text).map(|m| unescape_js(&m[1])).unwrap_or_default()
}

fn command_from_code(code: &str) -> String {
    CMD.captures(code).map(|m| unescape_js(&m[1])).unwrap_or_default()
}

/// `_plan_from_code` (rollout.py:117): o status de cada passo é o primeiro antes do passo seguinte.
fn plan_from_code(code: &str) -> Vec<Value> {
    let steps: Vec<_> = STEP.captures_iter(code).collect();
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let end = steps.get(i + 1).map_or(code.len(), |n| n.get(0).unwrap().start());
            let status = STATUS
                .captures_at(&code[..end], step.get(0).unwrap().end())
                .map_or_else(|| "pending".to_string(), |m| m[1].to_string());
            let mut item = Map::new();
            item.insert("step".into(), unescape_js(&step[1]).into());
            item.insert("status".into(), status.into());
            Value::Object(item)
        })
        .collect()
}

/// Ramo `custom_tool_call` (rollout.py:273-322).
fn custom_tool_call(payload: &Map<String, Value>, id: String) -> ChatEvent {
    let code = payload.get("input").and_then(Value::as_str).unwrap_or("");
    let mut name = str_field(payload, "name");
    let mut tool_input = Map::new();
    tool_input.insert("code".into(), code.into());
    let calls: Vec<_> =
        if name.as_deref() == Some("exec") { TOOL_IN_CODE.captures_iter(code).collect() } else { Vec::new() };
    let inner = if calls.len() == 1 { calls.first() } else { None };
    if let Some(c) = inner {
        name = Some(c[1].to_string());
    }
    let patch = match (name.as_deref(), inner) {
        (Some("apply_patch"), Some(c)) => js_string(&code[c.get(0).unwrap().end()..]),
        (Some("apply_patch"), None) => code.to_string(),
        _ => String::new(),
    };
    if name.as_deref() == Some("update_plan") {
        let plan = plan_from_code(code);
        if !plan.is_empty() {
            tool_input.insert("plan".into(), Value::Array(plan));
        }
    } else if !patch.is_empty() {
        let files: Vec<Value> = PATCH_FILE.captures_iter(&patch).map(|m| m[1].into()).collect();
        tool_input.insert("patch".into(), patch.into());
        if !files.is_empty() {
            tool_input.insert("file_path".into(), Value::Array(files));
        }
    } else if calls.len() > 1 {
        let cmds = CMD.captures_iter(code).map(|m| unescape_js(&m[1])).collect::<Vec<_>>().join("\n");
        let command = if cmds.is_empty() {
            let mut names: Vec<&str> = Vec::new();
            for c in &calls {
                let n = c.get(1).unwrap().as_str();
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            names.join(", ")
        } else {
            cmds
        };
        tool_input.insert("command".into(), command.into());
    } else {
        let command = command_from_code(code);
        if !command.is_empty() {
            tool_input.insert("command".into(), command.into());
        }
    }
    ChatEvent {
        tool_name: name,
        tool_use_id: str_field(payload, "call_id"),
        tool_input: Some(tool_input),
        ..event(ChatKind::ToolUse, id)
    }
}

/// `_command_output` (rollout.py:151).
fn command_output(value: &Value) -> Option<(String, bool)> {
    match value {
        Value::Array(items) => {
            let parts = items.iter().map(command_output).collect::<Option<Vec<_>>>()?;
            if parts.is_empty() {
                return None;
            }
            let failed = parts.iter().any(|p| p.1);
            Some((parts.into_iter().map(|p| p.0).collect::<Vec<_>>().join("\n\n"), failed))
        }
        Value::Object(m) => {
            let status = m.get("status").and_then(Value::as_str);
            let only = |a: &str, b: &str| m.len() == 2 && m.contains_key(a) && m.contains_key(b);
            if status == Some("fulfilled") && only("status", "value") {
                return command_output(&m["value"]);
            }
            if status == Some("rejected") && only("status", "reason") {
                let reason = &m["reason"];
                return Some((reason.as_str().map_or_else(|| pyjson::dumps_unicode(reason, false), str::to_string), true));
            }
            let Some(Value::String(output)) = m.get("output") else { return None };
            if !(m.contains_key("chunk_id") && m.contains_key("wall_time_seconds")) {
                return None;
            }
            let failed = m.get("exit_code").and_then(py::int_of).is_some_and(|c| c != 0);
            let text = if failed {
                format!("exit_code: {}\n{output}", py::py_str(&m["exit_code"]))
            } else if let Some(sid) = m.get("session_id").filter(|v| !v.is_null()) {
                format!("session_id: {}\n{output}", py::py_str(sid))
            } else {
                output.clone()
            };
            Some((text, failed))
        }
        _ => None,
    }
}

/// `_output_text` (rollout.py:208).
fn output_text(output: Option<&Value>) -> Option<String> {
    match output? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        v @ Value::Array(_) => Some(blocks_text(Some(v), "input_text")),
        other => Some(py::py_str(other)),
    }
}

/// `_output_result` (rollout.py:177).
fn output_result(output: Option<&Value>) -> (Option<String>, bool) {
    let Some(raw) = output_text(output) else { return (None, false) };
    let Some(header) = SCRIPT_HEADER.captures(&raw) else { return (Some(raw), false) };
    let mut failed = &header[1] == "failed";
    let mut blocks: Vec<String> = match output {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(Value::as_object)
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("input_text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string))
            .collect(),
        _ => vec![raw.clone()],
    };
    // O Python corta pelo índice de caractere do cabeçalho no texto juntado.
    let skip = raw[..header.get(0).unwrap().end()].chars().count();
    if let Some(first) = blocks.first_mut() {
        *first = first.chars().skip(skip).collect();
    }
    let mut parts = Vec::new();
    for block in blocks.into_iter().filter(|b| !b.is_empty()) {
        match pyjson::loads_lossless(&block).as_ref().and_then(command_output) {
            Some((text, f)) => {
                parts.push(text);
                failed = failed || f;
            }
            None => parts.push(block),
        }
    }
    (Some(parts.join("\n\n")), failed)
}
