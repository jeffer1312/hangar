//! Configuração congelada do vigia e preferências vivas do runtime compartilhado.
use crate::{machine_peers::dotenv::SettingsSnapshot, value_coercion};
use serde_json::Value;
use std::time::Duration;

pub(super) struct Frozen {
    pub auto_resume: bool,
    pub poll: Poll,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Poll { Seconds(f64), Overflow }

impl Frozen {
    pub fn load(snapshot: &SettingsSnapshot) -> Result<Self, &'static str> {
        let auto_resume = match snapshot.value("CP_AUTO_RESUME").map_err(|_| "push_config_unreadable")? {
            None => false,
            Some(raw) => value_coercion::boolean(&Value::String(raw)).ok_or("push_auto_resume_invalid")?,
        };
        let seconds = match snapshot.value("CP_STALL_POLL_SECONDS").map_err(|_| "push_config_unreadable")? {
            None => serde_json::json!(30),
            Some(raw) => value_coercion::integer(&Value::String(raw)).ok_or("push_poll_invalid")?,
        };
        let poll = if seconds.to_string().starts_with('-') { Poll::Seconds(0.0) }
        else { match seconds.as_f64().filter(|seconds| seconds.is_finite()) {
            Some(seconds) => Poll::Seconds(seconds), None => Poll::Overflow,
        }};
        Ok(Self { auto_resume, poll })
    }
}
impl Poll {
    pub async fn wait(self) -> Result<(), &'static str> {
        let Self::Seconds(mut remaining) = self else { return Err("push_poll_overflow"); };
        if remaining <= 0.0 { tokio::task::yield_now().await; return Ok(()); }
        // Intervalos enormes aceitos pelo Python não cabem num Instant do sistema.
        while remaining > 0.0 {
            let slice = remaining.min(86_400.0);
            tokio::time::sleep(Duration::from_secs_f64(slice)).await;
            remaining -= slice;
        }
        Ok(())
    }
}

pub(super) fn flag(config: &crate::config_sync::runtime::RuntimeConfig, key: &str) -> bool {
    let value = config.get(key);
    let enabled = crate::config_sync::apply::truthy(&value);
    crate::orchestration::codec::discard(value);
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine_peers::dotenv::SettingsEnvironment;
    fn snapshot(field: &str, value: &str) -> SettingsSnapshot {
        SettingsSnapshot::from_parts(SettingsEnvironment::from_pairs([(format!("CP_{}", field.to_ascii_uppercase()), value.to_owned())]), String::new())
    }
    fn state(home: &std::path::Path, pairs: &[(&str, &str)]) -> crate::routes::AppState {
        let mut state = crate::routes::AppState::new(crate::config::Config {
            listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
            internal_secret: "fixture-secret".into(), auth_token: "fixture-owner".into(),
            log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
        });
        let mut env = std::collections::BTreeMap::from([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".to_owned(), home.to_string_lossy().into_owned()),
        ]);
        env.extend(pairs.iter().map(|(key,value)| (key.to_string(),value.to_string())));
        let mut list_env = crate::list::bridge::ListEnv::from_env();
        list_env.dirs = None;
        state.list = std::sync::Arc::new(crate::list::bridge::ListBridge::new(list_env,
            crate::list::facts::FactsClient::new("127.0.0.1:1".parse().unwrap(), "fixture-secret".into())));
        state.accounts.env = crate::accounts::environment::AccountEnvironment::from_map(env);
        state
    }
    #[test]
    fn guest_watch_flags_match_original_live_file() {
        let fixture = crate::orchestration::codec::loads(include_str!("../../../../../backend/tests/fixtures/contract/guests/automation-flags.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let home = tempfile::tempdir().unwrap();
            let state = state(home.path(), &[]);
            let config = crate::config_sync::runtime::RuntimeConfig::from_state(&state).unwrap();
            std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
            let key = row["key"].as_str().unwrap();
            let body = Value::Object([(key.to_owned(), crate::orchestration::codec::clone_value(&row["value"]))].into_iter().collect());
            std::fs::write(&config.path, crate::orchestration::codec::dumps(&body, false)).unwrap();
            assert_eq!(flag(&config, key), row["enabled"].as_bool().unwrap(), "GUE-082 {row}");
            crate::orchestration::codec::discard(body);
        }
        crate::orchestration::codec::discard(fixture);
    }
    #[test]
    fn guest_watch_snapshot_is_frozen_and_overrides_remain_live() {
        let home = tempfile::tempdir().unwrap();
        let mut state = state(home.path(), &[("CP_AUTO_RESUME", "true"), ("CP_NOTIFY_DEAD", "false")]);
        let config = crate::config_sync::runtime::RuntimeConfig::from_state(&state).unwrap();
        assert!(Frozen::load(&config.settings).unwrap().auto_resume);
        assert!(!flag(&config, "notify_dead"));
        state.accounts.env = super::tests::state(home.path(), &[("CP_AUTO_RESUME", "false"), ("CP_NOTIFY_DEAD", "true")]).accounts.env;
        let again = crate::config_sync::runtime::RuntimeConfig::from_state(&state).unwrap();
        assert!(Frozen::load(&again.settings).unwrap().auto_resume);
        assert!(!flag(&again, "notify_dead"));
        std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
        std::fs::write(&config.path, r#"{"notify_dead":"false"}"#).unwrap();
        assert!(flag(&config, "notify_dead"));
        std::fs::write(&config.path, r#"{"notify_dead":false}"#).unwrap();
        assert!(!flag(&config, "notify_dead"));
    }
    #[test]
    fn guest_loop_automations_preserves_python_string_truthiness() {
        let home = tempfile::tempdir().unwrap();
        let state = state(home.path(), &[]);
        let config = crate::config_sync::runtime::RuntimeConfig::from_state(&state).unwrap();
        std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
        std::fs::write(&config.path, r#"{"automations":"false"}"#).unwrap();
        assert!(crate::loops::automations(&state).unwrap(), "GUE-082: string persistida não é booleano do ambiente");
    }
    #[tokio::test]
    async fn guest_watch_settings_match_original_types_and_sleep() {
        let fixture: Value = serde_json::from_str(include_str!("../../../../../backend/tests/fixtures/contract/guests/watch-settings.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let field = row["field"].as_str().unwrap();
            let parsed = Frozen::load(&snapshot(field, row["input"].as_str().unwrap()));
            assert_eq!(parsed.is_ok(), row["accepted"].as_bool().unwrap(), "GUE-082 {row}");
            let Ok(parsed) = parsed else {continue;};
            if field == "auto_resume" {assert_eq!(parsed.auto_resume, row["value"].as_bool().unwrap());}
            else {
                match row["sleep"].as_str().unwrap() {
                    "completed" => {assert_eq!(parsed.poll, Poll::Seconds(0.0)); parsed.poll.wait().await.unwrap();}
                    "OverflowError" => {assert_eq!(parsed.poll, Poll::Overflow); assert_eq!(parsed.poll.wait().await, Err("push_poll_overflow"));}
                    "waiting" => {let Poll::Seconds(seconds) = parsed.poll else {panic!("{row}")}; assert!(seconds > 0.0);}
                    _ => panic!("{row}"),
                }
            }
        }
    }
}
