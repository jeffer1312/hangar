//! Conversa do Antigravity (`agy`): o `transcript_full.jsonl` que o próprio CLI grava, uma linha por passo.
//! O arquivo só cresce e cada linha entra já concluída; a saída de ferramenta (`GENERIC`) não traz o id da
//! chamada e chega na ordem das chamadas da resposta anterior, então o pareamento guarda estado entre linhas.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use crate::{event, finish, py};

mod tools;

const REQUEST_OPEN: &str = "<USER_REQUEST>";
const REQUEST_CLOSE: &str = "</USER_REQUEST>";
static EXIT_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Command exited with code (-?\d+)").unwrap());
static LAUNCH_UNREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static EXIT_UNREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Como aparece, nos recados dentro de um subagente, a conversa que o lançou.
pub const MAIN_SENDER: &str = "principal";
const SUBAGENT_CREATED: &str = "Created the following subagent:";
const BACKGROUND_TASK: &str = "Tool is running as a background task with task id: ";
const SUBAGENTS_KILLED: &str = "Successfully killed ";

/// Recado entre conversas do `agy` (relatório de subagente, pedido do principal, fim de tarefa de fundo), dentro de
/// um `SYSTEM_MESSAGE`.
static MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\[Message\] timestamp=\S+ sender=(\S+) priority=\S+ content=(.*)").unwrap()
});

/// Pareia saída de ferramenta com a chamada pela ordem. Leitor que começa no meio do arquivo pode ver uma saída
/// sem chamada: ela sai sem `tool_use_id`, nunca ligada à chamada errada.
pub struct AgyStream {
    /// Id e nome no `agy` de cada chamada da última resposta ainda sem saída, e os subagentes que ela encerra.
    pending: VecDeque<(String, Option<String>, Vec<String>)>,
    /// A pasta `brain` e o id desta conversa: dizem se quem manda um recado é subagente dela ou o principal.
    place: Option<(PathBuf, String)>,
}

impl AgyStream {
    /// O leitor de uma conversa conhecida: o caminho do transcript diz qual é a conversa e onde estão os subagentes.
    pub fn for_transcript(path: Option<&Path>) -> Self {
        Self { pending: VecDeque::new(), place: path.and_then(hangar_antigravity::paths::place_of_transcript) }
    }

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
                    self.pending.push_back((id.clone(), call["name"].as_str().map(str::to_owned), killed_subagents(call)));
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
                let (call, tool, killed) = match self.pending.pop_front() {
                    Some((call, tool, killed)) => (Some(call), tool, killed),
                    None => (None, None, Vec::new()),
                };
                let failed = value["status"] == "ERROR" || value["exit_code"].as_i64().is_some_and(|code| code != 0);
                // Só a saída da própria chamada vale: um `cat` de arquivo com o mesmo texto não lança nada.
                let launched = tool.as_deref().filter(|t| *t == "invoke_subagent").and_then(|_| {
                    let launched = launched_subagent(content);
                    // O `agy` mudou o texto da saída: o subagente deixaria de ser acompanhado sem ninguém saber.
                    if launched.is_none() && !content.contains("error") && !LAUNCH_UNREAD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        tracing::warn!(code = "agy_subagent_launch_unread", "saída do invoke_subagent sem o id do subagente");
                    }
                    launched
                });
                events.push(ChatEvent { ts, tool_use_id: call, result: Some(content.to_owned()), is_error: Some(failed),
                    bg_agent_id: launched, ..event(ChatKind::ToolResult, format!("res:agy:{step}")) });
                // Subagente encerrado não manda relatório: o cartão dele fecha aqui.
                if content.contains(SUBAGENTS_KILLED) {
                    for (n, id) in killed.iter().enumerate() {
                        events.push(ChatEvent { ts, ..crate::claude::task_result(format!("agy:{step}:kill:{n}"), id) });
                    }
                }
                // O comando foi para o fundo depois de chamado: os clientes só acompanham um `Bash` de fundo.
                if let Some((task, command)) = tool.as_deref().filter(|t| *t == "run_command").and_then(|_| background_task(content)) {
                    let id = format!("agy:{step}:bg");
                    let input = Map::from_iter([("command".to_owned(), command.into()), ("run_in_background".to_owned(), true.into())]);
                    events.push(ChatEvent { ts, tool_name: Some("Bash".into()), tool_input: Some(input), tool_use_id: Some(id.clone()),
                        ..event(ChatKind::ToolUse, id.clone()) });
                    events.push(ChatEvent { ts, tool_use_id: Some(id), result: Some(format!("Command running in background with ID: {task}")),
                        is_error: Some(false), ..event(ChatKind::ToolResult, format!("res:agy:{step}:bg")) });
                }
            }
            Some("SYSTEM_MESSAGE") => self.message(step, ts, content, &mut events),
            // O `agy` resumiu a conversa para caber no contexto: o mesmo aviso da compactação do Claude.
            Some("CHECKPOINT") => {
                self.pending.clear();
                events.push(ChatEvent { ts, text: Some("compacted".into()), ..event(ChatKind::Notice, format!("agy:{step}")) });
            }
            _ => {}
        }
        events.iter_mut().for_each(finish);
        events
    }
}

impl AgyStream {
    /// Relatório de subagente vira recado com o nome dele e encerra o subagente; fim de tarefa de fundo encerra o
    /// comando; o pedido do principal, dentro do subagente, vira recado do principal. Os outros avisos ficam fora.
    fn message(&self, step: u64, ts: Option<f64>, content: &str, events: &mut Vec<ChatEvent>) {
        let Some(found) = MESSAGE.captures(content) else { return };
        let sender = &found[1];
        let text = found[2].trim_end().trim_end_matches("</SYSTEM_MESSAGE>").trim().to_owned();
        let finish = |id: &str| ChatEvent { ts, ..crate::claude::task_result(format!("agy:{step}:end"), id) };
        if let Some((_, task)) = sender.rsplit_once('/') {
            // O fim traz o código de saída do comando: diferente de zero é falha, sem código fica como antes.
            let code = EXIT_CODE.captures(&text).and_then(|found| found[1].parse::<i64>().ok());
            if code.is_none() && task.starts_with("task-") && !EXIT_UNREAD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(code = "agy_task_exit_unread", "fim de tarefa de fundo sem o código de saída");
            }
            if task.starts_with("task-") { events.push(ChatEvent { is_error: code.map(|code| code != 0), ..finish(task) }); }
            return;
        }
        if !hangar_antigravity::paths::valid_conversation_id(sender) { return; }
        use hangar_antigravity::paths::{subagent_descriptor, subagent_name};
        let mine = self.place.as_ref().and_then(|(brain, me)| subagent_descriptor(brain, me, sender));
        // Um subagente meu nunca é meu pai: o segundo arquivo só é lido quando o primeiro não existe.
        let parent = mine.is_none() && self.place.as_ref().is_some_and(|(brain, me)| subagent_descriptor(brain, sender, me).is_some());
        let name = match &mine {
            Some(descriptor) => subagent_name(descriptor).unwrap_or_else(|| "subagente".to_owned()),
            None if parent => MAIN_SENDER.to_owned(),
            None => "subagente".to_owned(),
        };
        if !text.is_empty() {
            events.push(ChatEvent { ts, text: Some(format!("[de: {name}] {text}")), ..event(ChatKind::UserMsg, format!("agy:{step}")) });
        }
        if !parent { events.push(finish(sender)); }
    }
}

/// Os subagentes que uma chamada `manage_subagents` com `kill` encerra.
fn killed_subagents(call: &Value) -> Vec<String> {
    let args = &call["args"];
    if call["name"] != "manage_subagents" || args["Action"] != "kill" { return Vec::new(); }
    args["ConversationIds"].as_array().into_iter().flatten().filter_map(Value::as_str)
        .filter(|id| hangar_antigravity::paths::valid_conversation_id(id)).map(str::to_owned).collect()
}

/// A conversa do subagente que a saída do `invoke_subagent` informa (`Created the following subagent: {…}`).
fn launched_subagent(content: &str) -> Option<String> {
    let rest = &content[content.find(SUBAGENT_CREATED)? + SUBAGENT_CREATED.len()..];
    let json = &rest[rest.find('{')?..];
    let value = serde_json::Deserializer::from_str(json).into_iter::<Value>().next()?.ok()?;
    value["conversationId"].as_str().filter(|id| hangar_antigravity::paths::valid_conversation_id(id)).map(str::to_owned)
}

/// `(task-N, comando)` quando o comando foi para o fundo; o id sem a conversa, que os clientes aceitam.
fn background_task(content: &str) -> Option<(String, String)> {
    let rest = &content[content.find(BACKGROUND_TASK)? + BACKGROUND_TASK.len()..];
    let full = rest.split_whitespace().next()?;
    let task = full.rsplit('/').next().filter(|t| t.strip_prefix("task-").is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())))?;
    let command = content.lines().find_map(|line| line.strip_prefix("Task Description: ")).unwrap_or(full).trim().to_owned();
    Some((task.to_owned(), command))
}

/// O texto da pessoa vem dentro de `<USER_REQUEST>`, seguido de metadados que o CLI acrescenta (hora, troca de
/// modelo). Sem a marca, o conteúdo inteiro é o pedido.
fn user_request(content: &str) -> String {
    let Some(start) = content.find(REQUEST_OPEN) else { return content.to_owned() };
    let rest = &content[start + REQUEST_OPEN.len()..];
    let end = rest.find(REQUEST_CLOSE).unwrap_or(rest.len());
    py::strip(&rest[..end]).to_owned()
}

/// Mesmo nome e entrada do transcript, para o cliente casar o bloco ao vivo com o definitivo.
pub fn live_tool(name: &str, input: &Value) -> (String, Map<String, Value>) { tools::canonical(name, args(input)) }

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

    const PARENT: &str = "d4260961-71cd-4a60-9ae1-410cae6b046d";
    const SUB: &str = "d5194f5d-0b70-49c7-9c27-566b54db74d5";

    fn real(conversation: &str, from_step: u64) -> Vec<ChatEvent> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agy-subagent").join(conversation)
            .join(".system_generated/logs/transcript_full.jsonl");
        let mut stream = AgyStream::for_transcript(Some(&path));
        std::fs::read_to_string(&path).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|l| l["step_index"].as_u64().is_some_and(|s| s >= from_step))
            .flat_map(|l| stream.feed(&l)).collect()
    }

    fn task_end(events: &[ChatEvent], id: &str) -> bool {
        events.iter().any(|e| e.kind == ChatKind::ToolResult && e.tool_use_id.as_deref() == Some(&format!("task:{id}")) && e.result.as_deref() == Some("task-notification"))
    }

    /// Conversa real do `agy`: tarefa de fundo e subagente lançados juntos, relatório e fim da tarefa depois.
    #[test]
    fn a_real_subagent_and_background_task_launch_report_and_finish() {
        let events = real(PARENT, 0);
        let launch = events.iter().find(|e| e.kind == ChatKind::ToolResult && e.bg_agent_id.as_deref() == Some(SUB)).expect("lançamento do subagente");
        let agent = events.iter().find(|e| e.kind == ChatKind::ToolUse && e.tool_use_id == launch.tool_use_id).unwrap();
        assert_eq!((agent.tool_name.as_deref(), agent.tool_input.as_ref().unwrap()["run_in_background"].as_bool()), (Some("Agent"), Some(true)));
        let shell = events.iter().find(|e| e.kind == ChatKind::ToolUse && e.tool_name.as_deref() == Some("Bash")
            && e.tool_input.as_ref().is_some_and(|i| i.get("run_in_background") == Some(&Value::Bool(true)))).expect("comando de fundo");
        assert_eq!(shell.tool_input.as_ref().unwrap()["command"], "sleep 20; echo pronto");
        assert!(events.iter().any(|e| e.kind == ChatKind::ToolResult && e.tool_use_id == shell.tool_use_id
            && e.result.as_deref() == Some("Command running in background with ID: task-5")));
        let report = events.iter().find(|e| e.kind == ChatKind::UserMsg && e.text.as_deref().is_some_and(|t| t.starts_with("[de: Directory Lister] "))).expect("relatório");
        assert!(!report.text.as_deref().unwrap().contains("SYSTEM_MESSAGE"));
        assert!(task_end(&events, SUB) && task_end(&events, "task-5"));
        let ids: std::collections::HashSet<_> = events.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids.len(), events.len(), "ids repetidos");
    }

    /// Subagente encerrado pelo principal (`manage_subagents` com `kill`) fecha o cartão dele, sem relatório.
    #[test]
    fn a_subagent_killed_by_the_main_agent_finishes_its_card() {
        let call = json!({"step_index": 10, "type": "PLANNER_RESPONSE", "content": "",
            "tool_calls": [{"name": "manage_subagents", "args": {"Action": "kill", "ConversationIds": [SUB]}}]});
        let done = json!({"step_index": 11, "type": "GENERIC", "content": "Successfully killed 1 subagent(s) and their descendants. Killed roles: Revisor"});
        assert!(task_end(&feed_all(&[call.clone(), done]), SUB));
        let refused = json!({"step_index": 11, "type": "GENERIC", "content": "Error: no subagent with that id"});
        assert!(!task_end(&feed_all(&[call, refused]), SUB));
    }

    /// Tarefa de fundo que acaba com código diferente de zero fecha como falha.
    #[test]
    fn a_background_task_that_exits_non_zero_finishes_as_an_error() {
        let message = |step: u64, code: i64| json!({"step_index": step, "type": "SYSTEM_MESSAGE",
            "content": format!("<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-25T12:22:24Z sender={PARENT}/task-{step} priority=MESSAGE_PRIORITY_HIGH content=Task id \"{PARENT}/task-{step}\" finished with result:\nCommand exited with code {code}.\n</SYSTEM_MESSAGE>")});
        let events = feed_all(&[message(63, 101), message(64, 0)]);
        let end = |task: &str| events.iter().find(|e| e.tool_use_id.as_deref() == Some(&format!("task:{task}"))).map(|e| e.is_error).unwrap();
        assert_eq!((end("task-63"), end("task-64")), (Some(true), Some(false)));
    }

    /// A compactação sai como o aviso do Claude; os avisos do modo planejador/executor continuam fora.
    #[test]
    fn a_checkpoint_is_the_compaction_notice_and_plan_reviews_stay_out() {
        let events = feed_all(&[
            json!({"step_index": 322, "type": "CHECKPOINT", "content": "# Resuming from a compaction\n\nYou are continuing work…"}),
            json!({"step_index": 323, "type": "SYSTEM_MESSAGE", "content": "The following is a <SYSTEM_MESSAGE> not actually sent by the user.\n\n<SYSTEM_MESSAGE>\n[plan review due] 30 invocations of the execution model have run since the plan was handed off.\n</SYSTEM_MESSAGE>"}),
        ]);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!((&events[0].kind, events[0].id.as_str(), events[0].text.as_deref()), (&ChatKind::Notice, "agy:322", Some("compacted")));
    }

    /// Saída de outra ferramenta com o mesmo texto (ler um arquivo de prova) não lança subagente nem comando de fundo.
    #[test]
    fn launch_markers_only_count_in_the_output_of_their_own_call() {
        let content = format!("Created the following subagent:\n{{\"conversationId\": \"{SUB}\"}}\nTool is running as a background task with task id: {PARENT}/task-5\nTask Description: sleep 20");
        let events = feed_all(&[
            json!({"step_index": 1, "type": "PLANNER_RESPONSE", "content": "", "tool_calls": [{"name": "read_file", "args": {"Path": "/w/prova.txt"}}]}),
            json!({"step_index": 2, "type": "GENERIC", "content": content}),
        ]);
        assert!(events.iter().all(|e| e.bg_agent_id.is_none()));
        assert!(!events.iter().any(|e| e.id.ends_with(":bg")));
    }

    /// O histórico paginado começa no meio do arquivo: o relatório ainda sai com o nome do subagente.
    #[test]
    fn a_report_read_from_the_middle_keeps_the_subagent_name() {
        let events = real(PARENT, 8);
        assert!(events.iter().any(|e| e.text.as_deref().is_some_and(|t| t.starts_with("[de: Directory Lister] "))));
        assert!(task_end(&events, SUB));
    }

    /// Na conversa do subagente, o pedido chega do principal e não encerra nada.
    #[test]
    fn inside_the_subagent_the_request_comes_from_the_main_agent() {
        let events = real(SUB, 0);
        let first = events.iter().find(|e| e.kind == ChatKind::UserMsg).expect("pedido");
        assert!(first.text.as_deref().unwrap().starts_with("[de: principal] Liste os arquivos"), "{:?}", first.text);
        assert!(!events.iter().any(|e| e.tool_use_id.as_deref().is_some_and(|t| t.starts_with("task:"))));
    }

    fn feed_all(lines: &[Value]) -> Vec<ChatEvent> {
        let mut stream = AgyStream::for_transcript(None);
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
