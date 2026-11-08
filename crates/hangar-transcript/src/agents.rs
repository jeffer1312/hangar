//! Conversa estruturada do Pi, omp e Kimi, sem dependência do terminal.
use std::sync::LazyLock;
use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};
use crate::{event, finish, py, pyjson};

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:?]*[ -/]*[@-~]").unwrap());

fn clean(text: &str) -> String {
    let mut text = ANSI.replace_all(text, "").into_owned();
    py::scrub_str(&mut text);
    text
}

fn block_text<'a>(block: &'a Value, field: &str) -> Option<&'a str> {
    block.get(field)?.as_str().filter(|text| !py::strip(text).is_empty())
}

fn sub_id(id: &str, index: usize) -> String {
    if index == 0 { id.to_owned() } else { format!("{id}:{index}") }
}

fn args(value: &Value) -> Option<Map<String, Value>> { Some(value.as_object().cloned().unwrap_or_default()) }
fn string(value: &Value) -> Option<String> { value.as_str().map(str::to_owned) }

/// Um objeto completo; o pareamento de contexto de hook pertence ao `PiStream`.
pub fn parse_pi_object(value: &Value) -> Vec<ChatEvent> {
    if value["type"] != "message" { return Vec::new(); }
    let message = &value["message"];
    let Some(content) = message["content"].as_array() else { return Vec::new() };
    let id = value["id"].as_str().unwrap_or("");
    let ts = py::number(&message["timestamp"]).map(|n| n / 1000.);
    let mut events = Vec::new();
    match message["role"].as_str() {
        Some("toolResult") => {
            let result = content.iter().filter_map(|block| match block["type"].as_str() {
                Some("text") => Some(clean(block["text"].as_str().unwrap_or(""))),
                Some("image") => Some(format!("[imagem {}]", block["mimeType"].as_str().filter(|s| !s.is_empty()).unwrap_or("desconhecida"))),
                _ => None,
            }).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
            events.push(ChatEvent { ts, tool_use_id: string(&message["toolCallId"]), result: Some(result),
                is_error: Some(py::truthy(message.get("isError"))), ..event(ChatKind::ToolResult, id.to_owned()) });
        }
        Some(role @ ("user" | "assistant")) => {
            let cache_read = message["usage"]["cacheRead"].as_u64().or_else(|| message["usage"]["cacheRead"].as_bool().map(u64::from));
            for (index, block) in content.iter().enumerate() {
                let eid = sub_id(id, index);
                match block["type"].as_str() {
                    Some("text") => if let Some(text) = block_text(block, "text") {
                        events.push(ChatEvent { ts, text: Some(clean(text)), cache_read: if role == "assistant" { cache_read } else { None },
                            ..event(if role == "user" { ChatKind::UserMsg } else { ChatKind::AssistantMsg }, eid) });
                    },
                    Some("thinking") if role == "assistant" => if let Some(text) = block_text(block, "thinking") {
                        events.push(ChatEvent { ts, text: Some(clean(text)), ..event(ChatKind::Thinking, eid) });
                    },
                    Some("toolCall") if role == "assistant" => events.push(ChatEvent { ts,
                        tool_name: string(&block["name"]), tool_input: args(&block["arguments"]), tool_use_id: string(&block["id"]),
                        ..event(ChatKind::ToolUse, eid) }),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    events.iter_mut().for_each(finish);
    events
}

/// A mensagem fica retida só até a próxima linha válida ou o fim do lote.
#[derive(Default)]
pub struct PiStream { held: Vec<ChatEvent>, id: String }

impl PiStream {
    pub fn has_pending(&self) -> bool { !self.held.is_empty() }
    pub fn pending_offset(&self) -> Option<u64> { self.held.first().and_then(|event| event.offset) }
    pub fn flush(&mut self) -> Vec<ChatEvent> {
        self.id.clear();
        std::mem::take(&mut self.held)
    }
    pub fn feed(&mut self, value: &Value) -> Vec<ChatEvent> { self.feed_with_offset(value, None) }
    pub(crate) fn feed_with_offset(&mut self, value: &Value, offset: Option<u64>) -> Vec<ChatEvent> {
        if value["type"] == "custom_message" && value["customType"] == "claude-hook-context"
            && self.has_pending() && value["parentId"].as_str() == Some(self.id.as_str()) {
            let context = clean(value["content"].as_str().unwrap_or(""));
            let context = py::strip(&context);
            if !context.is_empty() {
                self.held.retain_mut(|event| {
                    if let Some(text) = &mut event.text {
                        if let Some(rest) = py::strip(text).strip_prefix(context) { *text = py::strip(rest).to_owned(); }
                        return !text.is_empty();
                    }
                    true
                });
            }
            return self.flush();
        }
        let mut events = self.flush();
        let mut parsed = parse_pi_object(value);
        for event in &mut parsed { event.offset = offset; }
        self.id = value["id"].as_str().unwrap_or("").to_owned();
        if !self.id.is_empty() && !parsed.is_empty() && parsed.iter().all(|event| event.kind == ChatKind::UserMsg) {
            self.held = parsed;
        } else { events.extend(parsed); }
        events
    }
}

/// O wire não repete UUID nos resultados: a ligação à chamada é também a identidade do resultado.
pub fn parse_kimi_object(value: &Value) -> Vec<ChatEvent> {
    let ts = py::number(&value["time"]).map(|n| n / 1000.);
    let mut events = Vec::new();
    if value["type"] == "context.append_message" {
        let message = &value["message"];
        if message["role"] != "user" || message["origin"]["kind"] != "user" { return events; }
        let id = message["id"].as_str().unwrap_or("");
        for (index, block) in message["content"].as_array().into_iter().flatten().enumerate() {
            if block["type"] == "text" && let Some(text) = block_text(block, "text") {
                events.push(ChatEvent { ts, text: Some(text.to_owned()), ..event(ChatKind::UserMsg, sub_id(id, index)) });
            }
        }
    } else if value["type"] == "context.append_loop_event" {
        let wire = &value["event"];
        let id = wire["uuid"].as_str().unwrap_or("");
        match wire["type"].as_str() {
            Some("content.part") => {
                let part = &wire["part"];
                let (kind, field) = match part["type"].as_str() {
                    Some("text") => (ChatKind::AssistantMsg, "text"),
                    Some("think") => (ChatKind::Thinking, "think"),
                    _ => return events,
                };
                if let Some(text) = block_text(part, field) { events.push(ChatEvent { ts, text: Some(text.to_owned()), ..event(kind, id.to_owned()) }); }
            }
            Some("tool.call") => events.push(ChatEvent { ts, tool_name: string(&wire["name"]), tool_input: args(&wire["args"]),
                tool_use_id: string(&wire["toolCallId"]), ..event(ChatKind::ToolUse, id.to_owned()) }),
            Some("tool.result") => {
                let result = &wire["result"];
                let text = match result {
                    Value::Object(_) => result["output"].as_str().map(str::to_owned).unwrap_or_else(|| pyjson::dumps_unicode(result, false)),
                    Value::String(text) => text.clone(),
                    _ => String::new(),
                };
                let cid = [&wire["toolCallId"], &wire["parentUuid"], &wire["uuid"]].into_iter().filter_map(Value::as_str).find(|s| !s.is_empty());
                if cid.is_none() { tracing::warn!(code = "kimi_tool_result_sem_id", "Kimi: resultado sem identificador de chamada nem UUID"); }
                events.push(ChatEvent { ts, tool_use_id: string(&wire["toolCallId"]), result: Some(text),
                    is_error: Some(py::truthy(result.get("isError"))), ..event(ChatKind::ToolResult, cid.map(|s| format!("res:{s}")).unwrap_or_default()) });
            }
            _ => {}
        }
    }
    events.iter_mut().for_each(finish);
    events
}
