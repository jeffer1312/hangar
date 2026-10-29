//! Mescla só hooks reconhecidos, mantendo índices de confiança dos demais.
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashSet};
fn validate<'a>(
    document: &'a Value,
    label: &str,
) -> Result<Option<&'a Map<String, Value>>, String> {
    let invalid = |reason: &str| format!("hooks.json inválido ({label}): {reason}");
    if !document.is_object() || document.get("hooks").is_some_and(|v| !v.is_object()) {
        return Err(invalid("objeto hooks esperado"));
    }
    let Some(events) = document.get("hooks").and_then(Value::as_object) else {
        return Ok(None);
    };
    for groups in events.values() {
        let groups = groups
            .as_array()
            .ok_or_else(|| invalid("lista de grupos esperada"))?;
        for group in groups {
            let hooks = group
                .get("hooks")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("grupo de hooks esperado"))?;
            if group.get("matcher").is_some_and(|v| !v.is_string()) {
                return Err(invalid("matcher precisa ser texto"));
            }
            for hook in hooks {
                let kind = hook
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| invalid("tipo de hook esperado"))?;
                if kind == "command"
                    && hook
                        .get("command")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s.trim().is_empty())
                {
                    return Err(invalid("comando precisa ser texto"));
                }
                if hook
                    .get("timeout")
                    .is_some_and(|v| v.as_f64().is_none_or(|n| n < 0.))
                {
                    return Err(invalid("timeout precisa ser número não negativo"));
                }
            }
        }
    }
    Ok(Some(events))
}
fn identity(group: &Value, hook: &Value) -> String {
    let matcher = group["matcher"].as_str().unwrap_or("");
    json!([
        matcher,
        hook["type"],
        if hook["type"] == "command" {
            hook["command"].clone()
        } else {
            hook.clone()
        }
    ])
    .to_string()
}
pub fn merge(current: &Value, source: &Value, previous: &Value) -> Result<Value, String> {
    validate(current, "atual")?;
    let desired = validate(source, "fonte")?;
    let old = validate(previous, "manifesto anterior")?;
    let mut out = current.clone();
    if out.get("hooks").is_none() {
        out["hooks"] = json!({});
    }
    let events: BTreeSet<_> = desired
        .into_iter()
        .chain(old)
        .flat_map(|m| m.keys())
        .cloned()
        .collect();
    for event in events {
        let wanted: Vec<Value> = desired
            .and_then(|m| m.get(&event))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let known: Vec<Value> = wanted
            .iter()
            .chain(
                old.and_then(|m| m.get(&event))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .cloned()
            .collect();
        let replaced: HashSet<_> = known
            .iter()
            .flat_map(|group| {
                group["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(move |hook| identity(group, hook))
            })
            .collect();
        let mut pending: Vec<Value> = wanted
            .iter()
            .filter(|g| !g["hooks"].as_array().unwrap().is_empty())
            .cloned()
            .collect();
        let mut empty: Vec<Value> = wanted
            .iter()
            .filter(|g| g["hooks"].as_array().unwrap().is_empty())
            .cloned()
            .collect();
        let mut groups = Vec::new();
        for group in out["hooks"][&event].as_array().into_iter().flatten() {
            let mut rest = Vec::new();
            let mut base = None;
            let mut mixed = false;
            for hook in group["hooks"].as_array().unwrap() {
                let key = identity(group, hook);
                let found = pending.iter().enumerate().find_map(|(gi, g)| {
                    g["hooks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .position(|h| identity(g, h) == key)
                        .map(|hi| (gi, hi))
                });
                if let Some((gi, hi)) = found {
                    if let Some(previous) = base {
                        if previous != gi {
                            mixed = true;
                        }
                    } else {
                        base = Some(gi);
                    }
                    let next = pending[gi]["hooks"].as_array_mut().unwrap().remove(hi);
                    rest.push(next);
                } else if !replaced.contains(&key) {
                    rest.push(hook.clone());
                    mixed = true;
                }
            }
            let no_hooks = group["hooks"].as_array().unwrap().is_empty();
            if no_hooks && empty.contains(group) {
                let index = empty.iter().position(|v| v == group).unwrap();
                empty.remove(index);
                groups.push(group.clone());
            } else if !rest.is_empty() || (no_hooks && !known.contains(group)) {
                let mut result = if mixed || base.is_none() {
                    group.clone()
                } else {
                    pending[base.unwrap()].clone()
                };
                result["hooks"] = json!(rest);
                groups.push(result);
            }
        }
        groups.extend(
            pending
                .into_iter()
                .filter(|g| !g["hooks"].as_array().unwrap().is_empty()),
        );
        groups.extend(empty);
        if groups.is_empty() {
            out["hooks"].as_object_mut().unwrap().remove(&event);
        } else {
            out["hooks"][&event] = json!(groups);
        }
    }
    Ok(out)
}
