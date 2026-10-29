//! Decisão de confiança do preparo, compartilhada pela integração e pelo reparo.
use crate::{engines::probe::truthy, orchestration::codec};
use serde_json::{Value, json};
use std::path::Path;

pub fn edits(
    data: &Value,
    home: &Path,
    account: &Path,
    reload: bool,
) -> Result<(Vec<Value>, usize), String> {
    let entries = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Lista de hooks inválida")?;
    let prefixes = [account.join("hooks.json"), home.join(".codex/hooks.json")]
        .map(|path| format!("{}:", path.to_string_lossy().replace('\\', "/")));
    let mut edits = Vec::new();
    let mut manual = 0;
    for entry in entries {
        let entry = entry
            .as_object()
            .ok_or("O Codex não conseguiu carregar todos os hooks")?;
        if entry.get("errors").is_some_and(truthy) || entry.get("warnings").is_some_and(truthy) {
            return Err("O Codex não conseguiu carregar todos os hooks".into());
        }
        let hooks = match entry.get("hooks") {
            None => continue,
            Some(value) => value.as_array().ok_or("Lista de hooks inválida")?,
        };
        for hook in hooks {
            let hook = hook.as_object().ok_or("Hook inválido")?;
            let status = hook.get("trustStatus").and_then(Value::as_str);
            let pending = matches!(status, Some("untrusted" | "modified"));
            if hook.get("enabled").and_then(Value::as_bool) != Some(true)
                || !(pending || reload && status == Some("trusted"))
            {
                continue;
            }
            let text = |field| {
                hook.get(field)
                    .filter(|value| !codec::contains_nonfinite(value))
                    .and_then(Value::as_str)
            };
            let key = text("key").ok_or("Identidade de hook inválida")?;
            let current = text("currentHash").ok_or("Identidade de hook inválida")?;
            let normalized = key.replace('\\', "/");
            let synced = matches!(
                hook.get("source").and_then(Value::as_str),
                Some("user" | "plugin")
            ) || prefixes.iter().any(|prefix| normalized.starts_with(prefix));
            if synced {
                let path = ["hooks", "state", key, "trusted_hash"]
                    .map(|part| codec::dumps(&Value::String(part.into()), false))
                    .join(".");
                edits.push(json!({"keyPath":path,"value":current,"mergeStrategy":"replace"}));
            } else if pending {
                manual += 1;
            }
        }
    }
    Ok((edits, manual))
}
