use std::io;
use std::path::Path;

use indexmap::IndexMap;
use serde_json::{Value, json};

use hangar_core::pycompat::truthy;

use crate::artifacts::{
    entries, field, integer, invalid_shape, items, matches_name, read_json, safe_id, session_dir,
    value_or,
};
use crate::live::{live_agents, script_name, total_tokens};

/// Lista conclusões e execuções em andamento nos artefatos irmãos do transcript.
///
/// # Erros
/// Retorna erro quando um artefato JSON válido tem estrutura incompatível com o produtor.
pub fn list_workflows(transcript: &Path) -> io::Result<Value> {
    let directory = session_dir(transcript);
    let mut rows = IndexMap::new();
    for path in entries(&directory.join("workflows")) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !matches_name(&name, "wf_", ".json") {
            continue;
        }
        let Some(value) = read_json(&path) else {
            continue;
        };
        let fallback = path.file_stem().unwrap_or_default().to_string_lossy();
        let id = value_or(&value, "runId", json!(fallback))?;
        let key = id.as_str().ok_or_else(invalid_shape)?.to_owned();
        rows.insert(key, summary(&value, id)?);
    }
    for path in entries(&directory.join("subagents/workflows")) {
        let id = path.file_name().unwrap_or_default().to_string_lossy();
        if !matches_name(&id, "wf_", "") || !path.is_dir() || rows.contains_key(id.as_ref()) {
            continue;
        }
        let agents = live_agents(&path)?;
        rows.insert(id.to_string(), json!({
            "runId": id, "name": script_name(&directory, &id).unwrap_or_else(|| id.to_string()),
            "status": "running", "agentCount": agents.len(), "phaseCount": 0,
            "totalTokens": total_tokens(&agents)?, "durationMs": 0, "startTime": 0, "running": true,
        }));
    }
    let mut sorted = Vec::with_capacity(rows.len());
    for row in rows.into_values() {
        let start = integer(&row["startTime"])?;
        sorted.push((start, row));
    }
    sorted.sort_by(|left, right| {
        left.1["running"]
            .as_bool()
            .cmp(&right.1["running"].as_bool())
            .reverse()
            .then_with(|| right.0.cmp(&left.0))
    });
    Ok(Value::Array(
        sorted.into_iter().map(|(_, row)| row).collect(),
    ))
}

/// Lê o detalhe da execução.
///
/// # Erros
/// Retorna erro quando um artefato tem estrutura incompatível com o produtor.
pub fn get_workflow(transcript: &Path, run_id: &str) -> io::Result<Option<Value>> {
    if !safe_id(run_id) {
        return Ok(None);
    }
    let directory = session_dir(transcript);
    let completion = directory.join("workflows").join(format!("{run_id}.json"));
    if completion.is_file() {
        let Some(value) = read_json(&completion) else {
            return Ok(None);
        };
        let mut phases = Vec::new();
        for phase in items(field(&value, "phases")?)? {
            phases
                .push(json!({"title": field(phase, "title")?, "detail": field(phase, "detail")?}));
        }
        let mut agents = Vec::new();
        for row in items(field(&value, "workflowProgress")?)? {
            if field(row, "type")? != "workflow_agent" {
                continue;
            }
            agents.push(json!({
                "agentId": field(row, "agentId")?, "label": field(row, "label")?,
                "phaseTitle": field(row, "phaseTitle")?, "state": field(row, "state")?, "model": field(row, "model")?,
                "tokens": value_or(row, "tokens", json!(0))?, "durationMs": value_or(row, "durationMs", json!(0))?,
                "toolCalls": value_or(row, "toolCalls", json!(0))?, "lastToolName": field(row, "lastToolName")?,
                "lastToolSummary": field(row, "lastToolSummary")?, "resultPreview": field(row, "resultPreview")?,
            }));
        }
        return Ok(Some(json!({
            "runId": value_or(&value, "runId", json!(run_id))?,
            "name": value_or(&value, "workflowName", json!(run_id))?,
            "status": value_or(&value, "status", json!("completed"))?,
            "totalTokens": value_or(&value, "totalTokens", json!(0))?,
            "durationMs": value_or(&value, "durationMs", json!(0))?,
            "summary": field(&value, "summary")?, "phases": phases, "agents": agents,
        })));
    }
    let path = directory.join("subagents/workflows").join(run_id);
    if !path.is_dir() {
        return Ok(None);
    }
    let agents = live_agents(&path)?;
    Ok(Some(json!({
        "runId": run_id, "name": script_name(&directory, run_id).unwrap_or_else(|| run_id.to_owned()),
        "status": "running", "totalTokens": total_tokens(&agents)?, "durationMs": 0,
        "summary": null, "phases": [], "agents": agents,
    })))
}

fn summary(value: &Value, id: Value) -> io::Result<Value> {
    let phases = field(value, "phases")?;
    let phase_count = match phases {
        Value::Array(phases) => phases.len(),
        Value::Object(phases) => phases.len(),
        Value::String(phases) => phases.chars().count(),
        value if !truthy(value) => 0,
        _ => return Err(invalid_shape()),
    };
    Ok(json!({
        "runId": value_or(value, "runId", id.clone())?,
        "name": value_or(value, "workflowName", id)?,
        "status": value_or(value, "status", json!("completed"))?,
        "agentCount": value_or(value, "agentCount", json!(0))?,
        "phaseCount": phase_count,
        "totalTokens": value_or(value, "totalTokens", json!(0))?,
        "durationMs": value_or(value, "durationMs", json!(0))?,
        "startTime": value_or(value, "startTime", json!(0))?,
        "running": false,
    }))
}
