//! Contexto estendido pela configuração oficial, com restauração dos valores anteriores.
use super::{files, instructions_toml, status};
use crate::{config_sync::runtime::RuntimeConfig, orchestration::codec};
use num_bigint::BigInt;
use serde_json::{Value, json};
const CONTEXT: i64 = 1_000_000;
const COMPACTION: i64 = 900_000;
fn integer(value: &Value) -> Option<BigInt> {
    match value {
        Value::Bool(value) => Some(BigInt::from(u8::from(*value))),
        Value::Number(value) if !value.to_string().contains(['.', 'e', 'E']) => {
            value.to_string().parse().ok()
        }
        _ => None,
    }
}
fn extended(value: &Value) -> bool {
    integer(value).is_some_and(|value| value >= BigInt::from(CONTEXT))
}
pub fn read(config: &RuntimeConfig) -> Result<Value, String> {
    let home = &config.env.codex_default;
    let raw = files::read(&home.join("config.toml")).map_err(|error| error.to_string())?;
    let data = match raw.as_deref().filter(|bytes| !bytes.is_empty()) {
        None => json!({}),
        Some(raw) => {
            instructions_toml::parse(std::str::from_utf8(raw).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?
        }
    };
    let mut models = Vec::new();
    if let Ok(Some(raw)) = files::read(&home.join("models_cache.json")) {
        if let Ok(catalog) = crate::orchestration::body::parse(&raw) {
            if let Some(entries) = catalog.get("models").and_then(Value::as_array) {
                for model in entries {
                    if !model.is_object() {
                        break;
                    }
                    if model.get("visibility").is_some_and(|value| value != "list") {
                        continue;
                    }
                    if let (Some(base), Some(maximum)) = (
                        integer(&model["context_window"]),
                        integer(&model["max_context_window"]),
                    ) {
                        if maximum > base {
                            let Some(slug) = model.get("slug") else {
                                break;
                            };
                            models.push(json!({"model":codec::clone_value(slug),"default":codec::clone_value(&model["context_window"]),"max":codec::clone_value(&model["max_context_window"])}));
                        }
                    }
                }
            }
            codec::discard(catalog);
        }
    }
    let current = &data["model_context_window"];
    let enabled = if integer(current).is_some() {
        extended(current)
    } else {
        current.is_null() && config.get("codex_extended_context") != Value::Bool(false)
    };
    let result = json!({"contexto_estendido":enabled,"contexto_configurado":codec::clone_value(current),"compactacao":codec::clone_value(&data["model_auto_compact_token_limit"]),"modelos":models,"codex_voice_beta":true});
    codec::discard(data);
    Ok(result)
}
fn edits(current: &Value, enabled: bool, record: &mut Value) -> (Vec<Value>, bool) {
    let context = &current["model_context_window"];
    let compact = &current["model_auto_compact_token_limit"];
    let is_extended = extended(context);
    let mut changed = false;
    let (wanted, compact_wanted) = if enabled {
        if is_extended && crate::config_sync::value_equality::equal(compact, &json!(COMPACTION)) {
            return (vec![], false);
        }
        if !is_extended {
            codec::discard(std::mem::replace(
                record,
                json!({"anterior":codec::clone_value(context),"compactacao_anterior":codec::clone_value(compact)}),
            ));
        } else if record.get("compactacao_anterior").is_none() {
            record["compactacao_anterior"] = codec::clone_value(compact);
        }
        changed = true;
        (json!(CONTEXT), json!(COMPACTION))
    } else {
        if !is_extended {
            return (vec![], false);
        }
        let prior = &record["anterior"];
        (
            if extended(prior) {
                Value::Null
            } else {
                codec::clone_value(prior)
            },
            if crate::config_sync::value_equality::equal(compact, &json!(COMPACTION)) {
                codec::clone_value(&record["compactacao_anterior"])
            } else {
                codec::clone_value(compact)
            },
        )
    };
    (
        vec![
            json!({"keyPath":"model_context_window","value":wanted,"mergeStrategy":"replace"}),
            json!({"keyPath":"model_auto_compact_token_limit","value":compact_wanted,"mergeStrategy":"replace"}),
        ],
        changed,
    )
}
pub async fn save(config: RuntimeConfig, enabled: bool) -> Result<Value, String> {
    let home = &config.env.codex_default;
    std::fs::create_dir_all(home).map_err(|error| error.to_string())?;
    let _guard = files::exclusive(&home.join(".hangar-integracao.lock"))
        .await
        .map_err(|error| error.to_string())?;
    let root = status::root(&config.env.home, home).map_err(|error| error.message)?;
    let record_path = root.join("contexto-anterior.json");
    files::edit_config(
        &config.env,
        &home.join("config.toml"),
        &root.join("backups"),
        &root,
        |current| {
            let raw = files::read(&record_path).map_err(|error| error.to_string())?;
            let mut record = files::object(&record_path).map_err(|error| error.to_string())?;
            let (edits, changed) = edits(current, enabled, &mut record);
            if changed {
                files::write(
                    &record_path,
                    &files::bytes(&record).map_err(|error| error.to_string())?,
                    raw.as_deref(),
                    None,
                )
                .map_err(|error| error.to_string())?;
            }
            codec::discard(record);
            Ok(edits)
        },
    )
    .await?;
    let saved = config.clone();
    tokio::task::spawn_blocking(move || {
        saved.apply(
            &[("codex_extended_context".into(), json!(enabled))].into_iter().collect(),
            &Default::default(),
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    read(&config)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enabling_preserves_previous_values_and_disabling_restores_them() {
        for previous in [Value::Null, json!(300000)] {
            let mut record = json!({});
            let (first, changed) = edits(
                &json!({"model_context_window":previous,"model_auto_compact_token_limit":250000}),
                true,
                &mut record,
            );
            assert!(changed);
            assert_eq!(first[0]["value"], CONTEXT);
            assert_eq!(first[1]["value"], COMPACTION);
            let (same, changed) = edits(
                &json!({"model_context_window":CONTEXT,"model_auto_compact_token_limit":COMPACTION}),
                true,
                &mut record,
            );
            assert!(same.is_empty());
            assert!(!changed);
            let (restored, _) = edits(
                &json!({"model_context_window":CONTEXT,"model_auto_compact_token_limit":COMPACTION}),
                false,
                &mut record,
            );
            assert_eq!(restored[0]["value"], previous);
            assert_eq!(restored[1]["value"], 250000);
            let (manual, _) = edits(
                &json!({"model_context_window":CONTEXT,"model_auto_compact_token_limit":777777}),
                false,
                &mut record,
            );
            assert_eq!(manual[1]["value"], 777777);
        }
    }
    #[test]
    fn integral_float_compaction_is_equal_to_the_official_limit() {
        let current =
            json!({"model_context_window":CONTEXT,"model_auto_compact_token_limit":900000.0});
        let mut record = json!({"anterior":300000,"compactacao_anterior":250000});
        let original = record.clone();
        let (unchanged, saved) = edits(&current, true, &mut record);
        assert!(unchanged.is_empty());
        assert!(!saved);
        assert_eq!(record, original);
        let (restored, saved) = edits(&current, false, &mut record);
        assert!(!saved);
        assert_eq!(restored[0]["value"], 300000);
        assert_eq!(restored[1]["value"], 250000);
        let (manual, _) = edits(
            &json!({"model_context_window":CONTEXT,"model_auto_compact_token_limit":900000.5}),
            false,
            &mut record,
        );
        assert_eq!(manual[1]["value"], 900000.5);
    }

    #[tokio::test]
    async fn disabling_implicit_context_preserves_toml_and_publishes_preference() {
        let home = tempfile::tempdir().unwrap();
        let env = crate::accounts::environment::AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().display().to_string()),
                ("USERPROFILE".into(), home.path().display().to_string()),
                ("PATH".into(), String::new()),
            ]
            .into(),
        );
        std::fs::create_dir_all(&env.codex_default).unwrap();
        let path = env.codex_default.join("config.toml");
        std::fs::write(&path, b"# pessoal\nmodel = 'example'\n").unwrap();
        let config = RuntimeConfig::new(env).unwrap();
        assert_eq!(read(&config).unwrap()["contexto_estendido"], true);
        let response = save(config.clone(), false).await.unwrap();
        assert_eq!(response["contexto_estendido"], false);
        assert_eq!(response["codex_voice_beta"], true);
        assert_eq!(config.get("codex_extended_context"), false);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"# pessoal\nmodel = 'example'\n"
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    #[test]
    fn explicit_context_and_real_catalog_limits_match_original() {
        let home = tempfile::tempdir().unwrap();
        let env = crate::accounts::environment::AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().display().to_string()),
                ("USERPROFILE".into(), home.path().display().to_string()),
            ]
            .into(),
        );
        std::fs::create_dir_all(&env.codex_default).unwrap();
        std::fs::write(
            env.codex_default.join("config.toml"),
            "model_context_window = 1000000\n",
        )
        .unwrap();
        std::fs::write(env.codex_default.join("models_cache.json"),br#"{"models":[{"slug":"large","context_window":272000,"max_context_window":872000},{"slug":"small","context_window":128000,"max_context_window":128000},{"slug":"hidden","context_window":1,"max_context_window":2,"visibility":"hide"}]}"#).unwrap();
        let config = RuntimeConfig::new(env).unwrap();
        std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
        std::fs::write(&config.path, br#"{"codex_extended_context":false}"#).unwrap();
        let response = read(&config).unwrap();
        assert_eq!(response["contexto_estendido"], true);
        assert_eq!(
            response["modelos"],
            json!([{"model":"large","default":272000,"max":872000}])
        );
    }
}
