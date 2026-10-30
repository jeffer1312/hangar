//! Adaptação dos bytes de hooks para o protocolo do Codex.
use crate::{config_sync::runtime_values::ValueGuard, orchestration::{body, codec}};
use serde_json::Value;

pub fn complete_allow(raw: &[u8]) -> Vec<u8> {
    let Ok(value) = body::parse(raw) else { return raw.to_vec(); };
    let mut value = ValueGuard(value);
    let Some(specific) = value.as_object_mut().and_then(|object| object.get_mut("hookSpecificOutput")).and_then(Value::as_object_mut) else {
        return raw.to_vec();
    };
    if specific.get("hookEventName").and_then(Value::as_str) == Some("PreToolUse")
        && specific.contains_key("updatedInput") && !specific.contains_key("permissionDecision")
    {
        specific.insert("permissionDecision".into(), Value::String("allow".into()));
        codec::dumps(&value, false).into_bytes()
    } else { raw.to_vec() }
}

pub fn normalize_json(raw: &[u8]) -> Vec<u8> {
    let mut values = ValueGuard(Value::Array(Vec::new()));
    let items = values.as_array_mut().unwrap();
    if let Ok(value) = body::parse(raw) { items.push(value); }
    else {
        for line in raw.split(|byte| matches!(*byte, b'\r' | b'\n')) {
            if line.iter().all(|byte| matches!(*byte, 9..=13 | 32)) { continue; }
            let Ok(value) = body::parse(line) else { return raw.to_vec(); };
            items.push(value);
        }
    }
    if items.is_empty() || items.iter().any(|value| !value.is_object()) { return raw.to_vec(); }
    let asynchronous = items.len() == 2 && items[0].get("async") == Some(&Value::Bool(true))
        && items[0].as_object().unwrap().keys().all(|key| matches!(key.as_str(), "async" | "asyncTimeout"));
    if asynchronous { codec::discard(items.remove(0)); }
    if items.len() != 1 { return raw.to_vec(); }
    let output = items[0].as_object_mut().unwrap();
    let metrics = output.remove("metrics");
    let summary = output.remove("rewakeSummary");
    let changed = asynchronous || metrics.is_some() || summary.is_some();
    if let Some(metrics) = metrics { codec::discard(metrics); }
    let summary = ValueGuard(summary.unwrap_or(Value::Null));
    if let Some(summary) = summary.as_str().filter(|summary| !summary.is_empty())
        && !output.get("systemMessage").is_some_and(crate::orchestration::truthy)
    {
        if let Some(previous) = output.insert("systemMessage".into(), Value::String(summary.into())) {
            codec::discard(previous);
        }
    }
    if !changed { return raw.to_vec(); }
    let mut result = codec::dumps(&items[0], false).into_bytes();
    result.push(b'\n');
    result
}
