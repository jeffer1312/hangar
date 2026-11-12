//! Edições nativas de TOML com manifesto de propriedade por preferência.
use crate::{Error, Native, Paths, backup, issue, object, project_preferences, read, resources};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

const RESTRICTIONS: &[&str] = &["forced_login_method", "forced_chatgpt_workspace_id"];

fn managed_restrictions(previous: &Value) -> serde_json::Map<String, Value> {
    object(&previous["managed_restrictions"])
        .into_iter()
        .filter(|(key, _)| RESTRICTIONS.contains(&key.as_str()))
        .collect()
}

pub(crate) fn merge(current: &Value, desired: &Value, previous: &Value) -> Value {
    let Some(wanted) = desired.as_object() else {
        return desired.clone();
    };
    let mut result = object(current);
    for (key, old) in object(previous) {
        if !wanted.contains_key(&key) && result.get(&key) == Some(&old) {
            result.remove(&key);
        }
    }
    for (key, value) in wanted {
        let next = merge(
            result.get(key).unwrap_or(&Value::Null),
            value,
            &previous[key],
        );
        result.insert(key.clone(), next);
    }
    Value::Object(result)
}
fn edits(current: &Value, target: &Value, managed: &BTreeSet<String>) -> Vec<Value> {
    managed.iter().filter_map(|key|{let wanted=target.get(key);if wanted.is_some()&&current.get(key)!=wanted || wanted.is_none()&&current.get(key).is_some(){Some(json!({"keyPath":serde_json::to_string(key).unwrap_or_default(),"value":wanted,"mergeStrategy":"replace"}))}else{None}}).collect()
}
pub(crate) async fn edit(
    native: &mut impl Native,
    path: &Path,
    source: &Value,
    paths: Paths<'_>,
    previous: &Value,
    available: &BTreeSet<String>,
    backups: &Path,
    private: &Path,
    issues: &mut Vec<Value>,
) -> Result<Value, Error> {
    let (projection, projection_issues) =
        project_preferences(source).map_err(|_| Error::Invalid)?;
    issues.extend(projection_issues);
    let start = issues.len();
    let projection = resources::map_tree(&projection, paths, available, issues, "", false)
        .ok_or(Error::Invalid)?;
    let blocked = issues[start..]
        .iter()
        .any(|v| v["code"] == "codex_account_unmapped_reference");
    let old = object(&previous["values"]);
    let authored = managed_restrictions(previous);
    let issue_start = issues.len();
    let mut result = json!({});
    let mut plan = |current: &Value| -> Result<Vec<Value>, String> {
        if !current.is_object() {
            return Err("config.toml não é um objeto".into());
        }
        // Uma tentativa descartada pelo escritor não transfere posse nem autoriza a próxima.
        issues.truncate(issue_start);
        let mut managed: BTreeSet<String> = object(&projection).keys().cloned().collect();
        result = json!({"values":projection,"restrictions":{},"managed_restrictions":authored});
        let mut target = serde_json::Map::new();
        for (key, value) in object(&projection) {
            target.insert(
                key.clone(),
                merge(
                    &current[&key],
                    &value,
                    old.get(&key).unwrap_or(&Value::Null),
                ),
            );
        }
        for (key, value) in &old {
            if projection.get(key).is_some() {
                continue;
            }
            if blocked {
                result["values"][key] = value.clone();
                continue;
            }
            if let Some(actual) = current.get(key) {
                let comparable = if key == "hooks" && actual.is_object() {
                    let mut values = object(actual);
                    values.remove("state");
                    Value::Object(values)
                } else {
                    actual.clone()
                };
                if comparable != *value {
                    issues.push(issue("codex_account_local_change", json!({"key":key})));
                } else {
                    managed.insert(key.clone());
                    if key == "hooks" && actual.is_object() {
                        target.insert(
                            key.clone(),
                            actual
                                .get("state")
                                .map(|v| json!({"state":v}))
                                .unwrap_or(json!({})),
                        );
                    }
                }
            }
        }
        for &key in RESTRICTIONS {
            let actual = current.get(key);
            let owned = authored.get(key);
            let locally_changed = actual.is_some() && owned.is_some() && actual != owned;
            if let Some(actual) = actual {
                result["restrictions"][key] = actual.clone();
                if locally_changed {
                    issues.push(issue("codex_account_local_change", json!({"key":key})));
                }
            } else {
                result["managed_restrictions"]
                    .as_object_mut()
                    .map(|values| values.remove(key));
            }
            let Some(wanted) = source.get(key) else {
                continue;
            };
            if actual == Some(wanted) {
                // Igualdade confirma a observação, mas não prova uma escrita do Hangar.
                continue;
            }
            if actual.is_none() || (owned.is_some() && actual == owned) {
                target.insert(key.into(), wanted.clone());
                result["restrictions"][key] = wanted.clone();
                result["managed_restrictions"][key] = wanted.clone();
                managed.insert(key.into());
            } else {
                issues.push(issue(
                    "codex_account_restriction_conflict",
                    json!({"key":key}),
                ));
            }
        }
        Ok(edits(current, &Value::Object(target), &managed))
    };
    native
        .edit_config(path, backups, private, &mut plan)
        .await
        .map_err(|_| Error::Native)?;
    Ok(result)
}
pub(crate) async fn remove(
    native: &mut impl Native,
    path: &Path,
    previous: &Value,
    backups: &Path,
    private: &Path,
    issues: &mut Vec<Value>,
) -> Result<Value, Error> {
    let authored = managed_restrictions(previous);
    let issue_start = issues.len();
    let mut remaining = serde_json::Map::new();
    let mut remaining_restrictions = serde_json::Map::new();
    let mut observations = serde_json::Map::new();
    let mut plan = |current: &Value| -> Result<Vec<Value>, String> {
        if !current.is_object() {
            return Err("perfil inválido".into());
        }
        issues.truncate(issue_start);
        remaining.clear();
        remaining_restrictions.clear();
        observations.clear();
        let mut result = Vec::new();
        for (key, old) in object(&previous["values"]) {
            if let Some(actual) = current.get(&key) {
                if actual == &old {
                    result.push(json!({"keyPath":serde_json::to_string(&key).unwrap_or_default(),"value":null,"mergeStrategy":"replace"}));
                } else {
                    issues.push(issue("codex_account_local_change", json!({"key":key})));
                    remaining.insert(key, old);
                }
            }
        }
        for &key in RESTRICTIONS {
            let Some(actual) = current.get(key) else {
                continue;
            };
            match authored.get(key) {
                Some(old) if actual == old => {
                    result.push(json!({"keyPath":serde_json::to_string(key).unwrap_or_default(),"value":null,"mergeStrategy":"replace"}));
                }
                Some(old) => {
                    issues.push(issue("codex_account_local_change", json!({"key":key})));
                    remaining_restrictions.insert(key.into(), old.clone());
                    observations.insert(key.into(), actual.clone());
                }
                None => {
                    observations.insert(key.into(), actual.clone());
                }
            }
        }
        Ok(result)
    };
    native
        .edit_config(path, backups, private, &mut plan)
        .await
        .map_err(|_| Error::Native)?;
    if let Some(raw) = read(path)?
        && let Ok(text) = std::str::from_utf8(&raw)
        && native
            .parse_toml(text)
            .is_ok_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
    {
        backup(path, &raw, backups)?;
        if read(path)?.as_deref() != Some(&raw) {
            return Err(Error::Changed);
        }
        std::fs::remove_file(path)?;
    }
    Ok(
        json!({"values":remaining,"restrictions":observations,"managed_restrictions":remaining_restrictions}),
    )
}
pub(crate) fn contains(current: &Value, desired: &Value) -> bool {
    if let Some(values) = desired.as_object() {
        current.is_object()
            && values
                .iter()
                .all(|(k, v)| current.get(k).is_some_and(|actual| contains(actual, v)))
    } else {
        current == desired
    }
}
pub(crate) fn verify(native: &impl Native, path: &Path, manifest: &Value) -> Result<(), Error> {
    let raw = read(path)?.ok_or(Error::Changed)?;
    let current = native
        .parse_toml(std::str::from_utf8(&raw).map_err(|_| Error::Changed)?)
        .map_err(|_| Error::Changed)?;
    if contains(&current, &Value::Object(object(&manifest["values"])))
        && contains(&current, &Value::Object(object(&manifest["restrictions"])))
    {
        Ok(())
    } else {
        Err(Error::Changed)
    }
}
