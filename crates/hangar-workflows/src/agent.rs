use std::io;
use std::path::Path;

use indexmap::IndexMap;
use serde_json::{Value, json};

use hangar_core::pycompat::{py, pyjson, truthy};

use crate::artifacts::{
    field, invalid_shape, items, mapping_or_empty, read_json, read_lines, session_dir, value_or,
};

/// Lê estatísticas persistidas, prompt, resultado integral e ferramentas do agente.
///
/// # Erros
/// Retorna erro quando um artefato JSON válido tem estrutura incompatível com o produtor.
pub fn get_agent(transcript: &Path, run_id: &str, agent_id: &str) -> io::Result<Option<Value>> {
    let directory = session_dir(transcript);
    let run = directory.join("subagents/workflows").join(run_id);
    let mut meta = json!({});
    if let Some(completion) = read_json(&directory.join("workflows").join(format!("{run_id}.json")))
    {
        for row in items(field(&completion, "workflowProgress")?)? {
            if field(row, "type")? == "workflow_agent" && field(row, "agentId")? == agent_id {
                meta = row.clone();
                break;
            }
        }
    }
    let journal = run.join("journal.jsonl");
    let mut result = Value::Null;
    for event in read_lines(&journal) {
        if event["type"] == "result" && event["agentId"] == agent_id {
            let value = field(&event, "result")?;
            result = match value {
                Value::String(_) => value.clone(),
                _ => json!(pyjson::dumps_indent(value, 2, false)),
            };
        }
    }
    let agent = run.join(format!("agent-{agent_id}.jsonl"));
    let has_transcript = agent.is_file();
    let mut prompt: Option<String> = None;
    let mut tools = IndexMap::<String, u64>::new();
    let mut last_name = Value::Null;
    let mut last_target = Value::Null;
    for row in read_lines(&agent) {
        let message = mapping_or_empty(field(&row, "message")?)?;
        let content = field(message, "content")?;
        if row["type"] == "user" && prompt.is_none() {
            match content {
                Value::String(text) => prompt = Some(text.clone()),
                Value::Array(blocks) => {
                    let mut texts = Vec::new();
                    for block in blocks
                        .iter()
                        .filter(|block| block.is_object() && block["type"] == "text")
                    {
                        let text = match block.get("text") {
                            Some(text) => text.as_str().ok_or_else(invalid_shape)?,
                            None => "",
                        };
                        texts.push(text);
                    }
                    prompt = Some(texts.join(" "));
                }
                _ => {}
            }
        }
        if row["type"] != "assistant" {
            continue;
        }
        let Some(blocks) = content.as_array() else {
            continue;
        };
        for block in blocks
            .iter()
            .filter(|block| block.is_object() && block["type"] == "tool_use")
        {
            let name = value_or(block, "name", json!("?"))?;
            let name = name.as_str().ok_or_else(invalid_shape)?;
            *tools.entry(name.to_owned()).or_default() += 1;
            last_name = json!(name);
            let input = field(block, "input")?;
            if let Some(target) = tool_target(input) {
                last_target = json!(target);
            }
        }
    }
    if !has_transcript && !truthy(&meta) {
        return Ok(None);
    }

    // Resultado e estado leem o journal separadamente, como no caminho original.
    let mut started = false;
    let mut resulted = false;
    for event in read_lines(&journal) {
        if event["agentId"] != agent_id {
            continue;
        }
        match event["type"].as_str() {
            Some("started") => started = true,
            Some("result") => resulted = true,
            _ => {}
        }
    }
    let state = if resulted {
        json!("done")
    } else if started {
        json!("progress")
    } else {
        Value::Null
    };
    let prompt = prompt
        .map(Value::String)
        .filter(truthy)
        .unwrap_or(field(&meta, "promptPreview")?.clone());
    let result = if truthy(&result) {
        result
    } else {
        field(&meta, "resultPreview")?.clone()
    };
    Ok(Some(json!({
        "agentId": agent_id,
        "label": value_or(&meta, "label", json!(agent_id.chars().take(8).collect::<String>()))?,
        "phaseTitle": field(&meta, "phaseTitle")?,
        "state": value_or(&meta, "state", state)?,
        "model": field(&meta, "model")?,
        "tokens": value_or(&meta, "tokens", json!(0))?,
        "durationMs": value_or(&meta, "durationMs", json!(0))?,
        "toolCalls": value_or(&meta, "toolCalls", json!(tools.values().sum::<u64>()))?,
        "prompt": prompt, "result": result,
        "tools": tools.into_iter().map(|(name, count)| json!({"name": name, "count": count})).collect::<Vec<_>>(),
        "lastToolName": last_name, "lastToolTarget": last_target,
    })))
}

fn tool_target(input: &Value) -> Option<String> {
    let input = input.as_object()?;
    let preferred = ["file_path", "pattern", "command", "path", "query", "code"]
        .into_iter()
        .find_map(|key| input.get(key).filter(|value| truthy(value)));
    preferred
        .and_then(target_text)
        .or_else(|| input.values().find_map(target_text))
}

fn target_text(value: &Value) -> Option<String> {
    let text = py::strip(value.as_str()?);
    (!text.is_empty()).then(|| text.chars().take(120).collect())
}
