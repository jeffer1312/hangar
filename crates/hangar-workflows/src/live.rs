use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

use num_bigint::BigInt;
use regex::Regex;
use serde_json::{Value, json};

use hangar_core::pycompat::{py, truthy};

use crate::artifacts::{
    entries, field, integer, invalid_shape, mapping_or_empty, matches_name, number, read_lines,
};

static SCRIPT_NAME: LazyLock<Regex> = LazyLock::new(|| py::py_re(r#"name:\s*['"]([^'"]+)['"]"#));

pub(crate) fn script_name(directory: &Path, run_id: &str) -> Option<String> {
    for path in entries(&directory.join("workflows/scripts")) {
        let name = path.file_name()?.to_string_lossy();
        if !matches_name(&name, "", &format!("{run_id}.js")) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        if let Some(found) = SCRIPT_NAME.captures(&String::from_utf8_lossy(&bytes)) {
            return Some(found[1].to_owned());
        }
        if let Some(cut) = name.find("-wf_").filter(|cut| *cut > 0) {
            return Some(name[..cut].to_owned());
        }
    }
    None
}

pub(crate) fn live_agents(directory: &Path) -> io::Result<Vec<Value>> {
    let mut started = Vec::new();
    let mut resulted = HashSet::new();
    for event in read_lines(&directory.join("journal.jsonl")) {
        let id = field(&event, "agentId")?;
        if !truthy(id) {
            continue;
        }
        let id = id.as_str().ok_or_else(invalid_shape)?;
        match event["type"].as_str() {
            Some("started") if !started.iter().any(|started| started == id) => {
                started.push(id.to_owned())
            }
            Some("result") => {
                resulted.insert(id.to_owned());
            }
            _ => {}
        }
    }
    let mut agents = Vec::with_capacity(started.len());
    for id in started {
        let stats = agent_stats(directory, &id)?;
        agents.push(json!({
            "agentId": id,
            "label": id.chars().take(8).collect::<String>(),
            "phaseTitle": null,
            "state": if resulted.contains(&id) { "done" } else { "progress" },
            "model": stats["model"],
            "tokens": stats["tokens"],
            "durationMs": 0,
            "toolCalls": stats["toolCalls"],
            "lastToolName": stats["lastToolName"],
            "lastToolSummary": null,
            "resultPreview": stats["resultPreview"],
        }));
    }
    Ok(agents)
}

pub(crate) fn total_tokens(agents: &[Value]) -> io::Result<Value> {
    let mut total = BigInt::from(0);
    for agent in agents {
        total += integer(&agent["tokens"])?;
    }
    number(total)
}

fn agent_stats(directory: &Path, id: &str) -> io::Result<Value> {
    let mut tokens = BigInt::from(0);
    let mut output =
        json!({"model": null, "toolCalls": 0, "lastToolName": null, "resultPreview": null});
    let mut tool_calls = 0_u64;
    for row in read_lines(&directory.join(format!("agent-{id}.jsonl"))) {
        let message = mapping_or_empty(field(&row, "message")?)?;
        let usage = mapping_or_empty(field(message, "usage")?)?;
        for key in [
            "input_tokens",
            "output_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
        ] {
            let value = field(usage, key)?;
            if value.is_boolean()
                || value
                    .as_number()
                    .is_some_and(|n| !n.to_string().contains(['.', 'e', 'E']))
            {
                tokens += integer(value)?;
            }
        }
        if truthy(field(message, "model")?) {
            output["model"] = message["model"].clone();
        }
        if message["role"] != "assistant" {
            continue;
        }
        let content = field(message, "content")?;
        let Some(content) = content.as_array() else {
            if truthy(content) && !content.is_string() && !content.is_object() {
                return Err(invalid_shape());
            }
            continue;
        };
        for block in content.iter().filter(|block| block.is_object()) {
            match block["type"].as_str() {
                Some("tool_use") => {
                    tool_calls += 1;
                    output["lastToolName"] = field(block, "name")?.clone();
                }
                Some("text") => {
                    let value = field(block, "text")?;
                    if !truthy(value) {
                        continue;
                    }
                    let text = py::strip(value.as_str().ok_or_else(invalid_shape)?);
                    if !text.is_empty() {
                        output["resultPreview"] = json!(text.chars().take(200).collect::<String>());
                    }
                }
                _ => {}
            }
        }
    }
    output["tokens"] = number(tokens)?;
    output["toolCalls"] = json!(tool_calls);
    Ok(output)
}
