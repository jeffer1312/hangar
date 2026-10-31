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
    let enabled = crate::orchestration::truthy(&value);
    crate::orchestration::codec::discard(value);
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
