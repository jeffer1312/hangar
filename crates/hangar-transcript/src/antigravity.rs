//! Conversa do Antigravity (`agy`): o `transcript_full.jsonl` que o próprio CLI grava, uma linha por passo.
//! O arquivo só cresce e cada linha entra já concluída; a saída de ferramenta (`GENERIC`) não traz o id da
//! chamada e chega na ordem das chamadas da resposta anterior, então o pareamento guarda estado entre linhas.
use std::collections::VecDeque;

use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::{Map, Value};

use crate::{event, finish, py};

mod tools;

const REQUEST_OPEN: &str = "<USER_REQUEST>";
const REQUEST_CLOSE: &str = "</USER_REQUEST>";

/// Pareia saída de ferramenta com a chamada pela ordem. Leitor que começa no meio do arquivo pode ver uma saída
/// sem chamada: ela sai sem `tool_use_id`, nunca ligada à chamada errada.
#[derive(Default)]
pub struct AgyStream { pending: VecDeque<String> }

impl AgyStream {
    pub fn feed(&mut self, value: &Value) -> Vec<ChatEvent> {
        let Some(step) = value["step_index"].as_u64() else { return Vec::new() };
        let ts = value["created_at"].as_str().and_then(|t| py::iso_timestamp(&t.replace('Z', "+00:00")));
        let content = value["content"].as_str().unwrap_or("");
        let mut events = Vec::new();
        match value["type"].as_str() {
            Some("USER_INPUT") => {
                self.pending.clear();
                let text = user_request(content);
                if !py::strip(&text).is_empty() {
                    events.push(ChatEvent { ts, text: Some(text), ..event(ChatKind::UserMsg, format!("agy:{step}")) });
                }
            }
            Some("PLANNER_RESPONSE") => {
                self.pending.clear();
                if !py::strip(content).is_empty() {
                    events.push(ChatEvent { ts, text: Some(content.to_owned()), cache_read: value["cache_read_tokens"].as_u64(),
                        ..event(ChatKind::AssistantMsg, format!("agy:{step}")) });
                }
                for (n, call) in value["tool_calls"].as_array().into_iter().flatten().enumerate() {
                    let id = format!("agy:{step}:{n}");
                    self.pending.push_back(id.clone());
                    let (tool_name, tool_input) = match call["name"].as_str() {
                        Some(name) => { let (name, input) = tools::canonical(name, args(&call["args"])); (Some(name), input) }
                        None => (None, args(&call["args"])),
                    };
                    events.push(ChatEvent { ts, tool_name, tool_input: Some(tool_input),
                        tool_use_id: Some(id.clone()), ..event(ChatKind::ToolUse, id) });
                }
            }
            // Versão do agy que grava a saída do comando em passo próprio, com o código de saída.
            Some("GENERIC" | "RUN_COMMAND") => {
                let call = self.pending.pop_front();
                let failed = value["status"] == "ERROR" || value["exit_code"].as_i64().is_some_and(|code| code != 0);
                events.push(ChatEvent { ts, tool_use_id: call, result: Some(content.to_owned()),
                    is_error: Some(failed), ..event(ChatKind::ToolResult, format!("res:agy:{step}")) });
            }
            _ => {}
        }
        events.iter_mut().for_each(finish);
        events
    }
}

/// O texto da pessoa vem dentro de `<USER_REQUEST>`, seguido de metadados que o CLI acrescenta (hora, troca de
/// modelo). Sem a marca, o conteúdo inteiro é o pedido.
fn user_request(content: &str) -> String {
    let Some(start) = content.find(REQUEST_OPEN) else { return content.to_owned() };
    let rest = &content[start + REQUEST_OPEN.len()..];
    let end = rest.find(REQUEST_CLOSE).unwrap_or(rest.len());
    py::strip(&rest[..end]).to_owned()
}

/// Os argumentos às vezes chegam com o valor em texto JSON (`"\"ls -la\""`): desembrulha só quando o texto
/// inteiro é uma string JSON, senão fica como veio.
fn args(value: &Value) -> Map<String, Value> {
    let Some(map) = value.as_object() else { return Map::new() };
    map.iter().map(|(k, v)| {
        let v = match v.as_str().map(|s| serde_json::from_str::<Value>(s)) {
            Some(Ok(inner @ Value::String(_))) => inner,
            _ => v.clone(),
        };
        (k.clone(), v)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed_all(lines: &[Value]) -> Vec<ChatEvent> {
        let mut stream = AgyStream::default();
        lines.iter().flat_map(|l| stream.feed(l)).collect()
    }

    #[test]
    fn a_user_line_keeps_only_the_request() {
        let line = json!({"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-11-23T11:59:12Z",
            "content":"<USER_REQUEST>\nResponda só: ok prova 1\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: x.\n</ADDITIONAL_METADATA>"});
        let events = feed_all(&[line]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ChatKind::UserMsg);
        assert_eq!(events[0].text.as_deref(), Some("Responda só: ok prova 1"));
        assert_eq!(events[0].id, "agy:0");
    }

    #[test]
    fn two_calls_in_one_response_pair_with_the_outputs_in_call_order() {
        let events = feed_all(&[
            json!({"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[
                {"name":"run_command","args":{"CommandLine":"echo primeiro; sleep 3"}},
                {"name":"run_command","args":{"CommandLine":"echo segundo"}}]}),
            json!({"step_index":2,"type":"GENERIC","status":"DONE","content":"Command output:\nprimeiro\r\n"}),
            json!({"step_index":3,"type":"GENERIC","status":"DONE","content":"Command output:\nsegundo\r\n"}),
        ]);
        let calls: Vec<_> = events.iter().filter(|e| e.kind == ChatKind::ToolUse).map(|e| e.tool_use_id.clone().unwrap()).collect();
        let results: Vec<_> = events.iter().filter(|e| e.kind == ChatKind::ToolResult).map(|e| (e.tool_use_id.clone(), e.result.clone().unwrap())).collect();
        assert_eq!(calls, ["agy:1:0", "agy:1:1"]);
        assert_eq!(results[0].0.as_deref(), Some("agy:1:0"));
        assert!(results[0].1.contains("primeiro"));
        assert_eq!(results[1].0.as_deref(), Some("agy:1:1"));
        assert!(results[1].1.contains("segundo"));
    }

    #[test]
    fn a_failed_tool_is_an_error_and_an_output_without_call_has_no_link() {
        let events = feed_all(&[json!({"step_index":2,"type":"GENERIC","status":"ERROR","content":"tool call denied by pre-tool hook"})]);
        assert_eq!(events[0].kind, ChatKind::ToolResult);
        assert_eq!(events[0].is_error, Some(true));
        assert_eq!(events[0].tool_use_id, None);
    }

    #[test]
    fn a_command_output_step_closes_the_call_and_a_nonzero_exit_is_an_error() {
        let events = feed_all(&[
            json!({"step_index":7,"type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"run_command","args":{"CommandLine":"false"}}]}),
            json!({"step_index":8,"type":"RUN_COMMAND","status":"DONE","exit_code":1,"content":"Command output:\n"}),
        ]);
        assert_eq!(events[1].kind, ChatKind::ToolResult);
        assert_eq!(events[1].tool_use_id.as_deref(), Some("agy:7:0"));
        assert_eq!(events[1].is_error, Some(true));
    }

    #[test]
    fn system_messages_stay_out_and_quoted_args_are_unwrapped() {
        let events = feed_all(&[
            json!({"step_index":3,"type":"SYSTEM_MESSAGE","status":"DONE","content":"<SYSTEM_MESSAGE>x</SYSTEM_MESSAGE>"}),
            json!({"step_index":7,"type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"run_command","args":{"CommandLine":"\"ls -la\"","WaitMsBeforeAsync":"5000"}},
                {"name":"command_status","args":{"CommandId":"\"c1\"","WaitDurationSeconds":"5000"}}]}),
        ]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].tool_name.as_deref(), Some("Bash"));
        assert_eq!(events[0].tool_input.as_ref().unwrap()["command"], "ls -la");
        let raw = events[1].tool_input.as_ref().unwrap();
        assert_eq!(events[1].tool_name.as_deref(), Some("command_status"));
        assert_eq!(raw["CommandId"], "c1");
        assert_eq!(raw["WaitDurationSeconds"], "5000");
    }

    #[test]
    fn a_response_with_text_carries_the_cache_read() {
        let events = feed_all(&[json!({"step_index":4,"type":"PLANNER_RESPONSE","status":"DONE","cache_read_tokens":12,"content":"ok prova 1"})]);
        assert_eq!(events[0].kind, ChatKind::AssistantMsg);
        assert_eq!(events[0].text.as_deref(), Some("ok prova 1"));
        assert_eq!(events[0].cache_read, Some(12));
    }
}
