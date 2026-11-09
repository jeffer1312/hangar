//! Pedidos pendentes e prova positiva de resposta nos transcripts Pi, omp e Kimi.
use std::{collections::HashSet, fs::File, io::{self, Read, Seek, SeekFrom}, path::{Path, PathBuf}, sync::{LazyLock, Mutex}};
use serde_json::{Map, Value, json};
use crate::{py, pyjson};

#[derive(Clone, Debug, PartialEq)]
pub struct PendingCall { pub id: String, pub args: Map<String, Value> }

const TAIL_BYTES: u64 = 512 * 1024;

fn warn_once(path: &Path, code: &'static str) -> bool {
    static WARNED: LazyLock<Mutex<HashSet<(PathBuf, &'static str)>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    WARNED.lock().unwrap_or_else(|error| error.into_inner()).insert((path.to_owned(), code))
}

/// Erro de leitura não vira "sem pendência" nem confirmação por ausência.
fn tail_objects(path: &Path, marker: &str, warn_cut: bool) -> io::Result<Vec<Value>> {
    let read = || -> io::Result<(Vec<u8>, bool)> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        file.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok((bytes, size > TAIL_BYTES))
    };
    let (bytes, cut) = read().inspect_err(|error| {
        if warn_once(path, "kimi_wire_read") {
            tracing::warn!(code = "kimi_wire_read", path = %path.display(), io_kind = ?error.kind(), "não consegui ler a cauda da conversa");
        }
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = py::splitlines(&text).into_iter();
    if cut && let Some(first) = lines.next() {
        if warn_cut && first.contains(marker) && warn_once(path, "kimi_wire_cut") {
            tracing::warn!(code = "kimi_wire_cut", path = %path.display(), "a cauda cortou um pedido de interação; ele pode não aparecer");
        }
    }
    Ok(lines.filter(|line| line.contains(marker)).filter_map(|line| pyjson::loads_lossless(py::strip(line))).filter(Value::is_object).collect())
}

pub fn read_pending_pi_call(path: &Path, tool: &str) -> io::Result<Option<PendingCall>> {
    let objects = tail_objects(path, &format!("\"{tool}\""), false)?;
    let mut last = None;
    let mut answered = HashSet::new();
    for object in &objects {
        let message = &object["message"];
        if message["role"] == "toolResult" && message["toolName"] == tool {
            if let Some(id) = message["toolCallId"].as_str().filter(|id| !id.is_empty()) { answered.insert(id.to_owned()); }
        } else if message["role"] == "assistant" {
            for block in message["content"].as_array().into_iter().flatten() {
                if block["type"] == "toolCall" && block["name"] == tool {
                    if let (Some(id), Some(args)) = (block["id"].as_str().filter(|id| !id.is_empty()), block["arguments"].as_object()) {
                        last = Some(PendingCall { id: id.to_owned(), args: args.clone() });
                    }
                }
            }
        }
    }
    let Some(mut last) = last.filter(|call| !answered.contains(&call.id)) else { return Ok(None) };
    if tool == "ask" {
        let Some(question) = last.args.get("questions").and_then(Value::as_array).and_then(|questions| questions.iter().find(|question| question.is_object() && py::truthy(question.get("question")))) else { return Ok(None) };
        let options = question["options"].as_array().into_iter().flatten().filter(|option| option.is_object() && py::truthy(option.get("label"))).cloned().collect::<Vec<_>>();
        last.args = json!({"question":py::py_str(&question["question"]), "header":question.get("header").filter(|value| py::truthy(Some(value))).map(py::py_str).unwrap_or_default(), "options":options}).as_object().unwrap().clone();
    }
    Ok(Some(last))
}

pub fn read_pending_kimi_call(path: &Path) -> io::Result<Option<PendingCall>> {
    let mut last = None;
    let mut answered = HashSet::new();
    for object in tail_objects(path, "\"context.append_loop_event\"", false)? {
        if object["type"] != "context.append_loop_event" { continue; }
        let event = &object["event"];
        if event["type"] == "tool.result" {
            if let Some(id) = event["toolCallId"].as_str().filter(|id| !id.is_empty()) { answered.insert(id.to_owned()); }
        } else if event["type"] == "tool.call" && event["name"] == "AskUserQuestion" {
            if let (Some(id), Some(args)) = (event["toolCallId"].as_str().filter(|id| !id.is_empty()), event["args"].as_object()) {
                last = Some(PendingCall { id: id.to_owned(), args: args.clone() });
            }
        }
    }
    Ok(last.filter(|call| !answered.contains(&call.id)))
}

/// O mesmo shape publicado pelo leitor anterior; posições e feedback pertencem à TUI.
pub fn read_pending_kimi_interaction(path: &Path) -> io::Result<Option<Value>> {
    let mut last = None;
    let mut resolved = HashSet::new();
    for object in tail_objects(path, "\"interaction.", true)? {
        if object["type"] == "interaction.resolved" {
            if let Some(id) = object["id"].as_str() { resolved.insert(id.to_owned()); }
        } else if object["type"] == "interaction.request" && object["kind"] == "approval" {
            if let (Some(id), Some(request)) = (object["id"].as_str(), object["request"].as_object()) {
                last = Some((id.to_owned(), request.clone()));
            }
        }
    }
    let Some((id, request)) = last.filter(|(id, _)| !resolved.contains(id)) else { return Ok(None) };
    let Some(display) = request.get("display").and_then(Value::as_object) else {
        if warn_once(path, "kimi_approval_no_display") {
            tracing::warn!(code = "kimi_approval_no_display", "pedido de aprovação sem display; sem botões");
        }
        return Ok(None);
    };
    let tool = request.get("toolName").filter(|value| py::truthy(Some(value))).map(py::py_str).unwrap_or_default();
    let kind = display.get("kind").and_then(Value::as_str).unwrap_or("");
    let mut choices: Vec<(String, bool)> = Vec::new();
    if tool == "ExitPlanMode" || kind == "plan_review" {
        if let Some(options) = display.get("options").and_then(Value::as_array).filter(|options| options.len() >= 2) {
            choices.extend(options.iter().filter(|option| option.is_object() && py::truthy(option.get("label"))).map(|option| (py::py_str(&option["label"]), false)));
        } else { choices.push(("Approve".into(), false)); }
        choices.extend([("Reject".into(), false), ("Revise".into(), true)]);
    } else if kind == "goal_start" {
        choices.push(("Switch to Auto and start".into(), false));
        if display.get("mode").and_then(Value::as_str) == Some("yolo") {
            choices.extend([("Keep YOLO and start".into(), false), ("Do not start".into(), false)]);
        } else {
            choices.extend([("Switch to YOLO and start".into(), false), ("Start in Manual".into(), false), ("Do not start".into(), false)]);
        }
    } else {
        choices.extend([("Approve once".into(), false), ("Approve for this session".into(), false), ("Reject".into(), false), ("Reject with feedback".into(), true)]);
    }
    let title = match tool.as_str() {
        "Bash" => "Run this command?".to_owned(),
        "Write" => "Write this file?".to_owned(),
        "Edit" => "Apply these edits?".to_owned(),
        "TaskStop" => "Stop this task?".to_owned(),
        "ExitPlanMode" => "Ready to build with this plan?".to_owned(),
        "" => "Approve action?".to_owned(),
        tool => format!("Approve {tool}?"),
    };
    let summary_key = match kind { "command" => Some("command"), "file_io" | "diff" => Some("path"), "url_fetch" => Some("url"), "search" => Some("query"), _ => None };
    let summary = if let Some(value) = summary_key.and_then(|key| display.get(key)).filter(|value| py::truthy(Some(value))) {
        format!("`{}`", py::py_str(value))
    } else if kind == "plan_review" { String::new() }
    else { request.get("action").filter(|value| py::truthy(Some(value))).map(py::py_str).unwrap_or_default() };
    Ok(Some(json!({"id":id, "tool_name":tool, "kind_display":display.get("kind"), "titulo":title, "resumo":summary,
        "escolhas":choices.into_iter().map(|(label, feedback)| json!({"label":label,"requires_feedback":feedback})).collect::<Vec<_>>()})))
}

pub fn kimi_response_arrived(path: &Path, id: &str) -> io::Result<bool> {
    Ok(tail_objects(path, "\"context.append_loop_event\"", false)?.iter().any(|object| object["type"] == "context.append_loop_event" && object["event"]["type"] == "tool.result" && object["event"]["toolCallId"].as_str() == Some(id)))
}

pub fn kimi_interaction_resolved(path: &Path, id: &str) -> io::Result<bool> {
    Ok(tail_objects(path, "\"interaction.resolved\"", false)?.iter().any(|object| object["type"] == "interaction.resolved" && object["id"].as_str() == Some(id)))
}
