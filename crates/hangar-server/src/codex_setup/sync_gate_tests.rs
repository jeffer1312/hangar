use super::*;
use crate::{
    accounts::environment::AccountEnvironment,
    config_sync::runtime::RuntimeConfig,
    machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot},
};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

#[test]
fn sync_gate_matches_original_environment_and_runtime_truthiness() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/sync-gate.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let mut values = std::collections::BTreeMap::from([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                home.path().to_string_lossy().into_owned(),
            ),
        ]);
        if let Some(value) = case["env"].as_str() {
            values.insert("CP_CODEX_SYNC_ENABLED".into(), value.into());
        }
        let config = RuntimeConfig::from_snapshot(
            AccountEnvironment::from_map(values),
            Arc::new(SettingsSnapshot::from_parts(
                SettingsEnvironment::default(),
                String::new(),
            )),
        )
        .unwrap();
        fs::create_dir_all(config.path.parent().unwrap()).unwrap();
        fs::write(
            &config.path,
            serde_json::to_vec(
                &json!({"codex_sync":case["sync"],"automations":case["automations"]}),
            )
            .unwrap(),
        )
        .unwrap();
        let actual = sync_enabled(&config);
        if json!(actual) != case["enabled"] {
            differences.push((case.clone(), actual));
        }
    }
    assert!(differences.is_empty(), "gate divergente: {differences:?}");
}
