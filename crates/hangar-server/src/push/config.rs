//! O push usa o mesmo snapshot de Settings do servidor, inclusive o diretório de projetos.
use std::{collections::BTreeMap, path::PathBuf};

pub(super) struct Frozen {
    pub dir: PathBuf,
    pub env: BTreeMap<String, String>,
}
fn storage(projects: &str) -> PathBuf {
    let projects = crate::value_coercion::path(projects);
    let parent = projects.parent().unwrap_or(&projects);
    parent.join(".hangar-push")
}
impl Frozen {
    pub fn from_state(st: &crate::routes::AppState) -> Result<Self, &'static str> {
        let snapshot = st.settings_snapshot().map_err(|_| "push_config_invalid")?;
        let projects = snapshot
            .value("CP_PROJECTS_DIR")
            .map_err(|_| "push_config_invalid")?;
        let dir = projects
            .as_deref()
            .map(storage)
            .unwrap_or_else(|| st.accounts.env.claude_base.join(".hangar-push"));
        let mut env = BTreeMap::new();
        for key in ["CP_VAPID_PUBLIC", "CP_VAPID_PRIVATE", "CP_VAPID_SUBJECT"] {
            let value = snapshot
                .value(key)
                .map_err(|_| "push_config_invalid")?
                .unwrap_or_else(|| {
                    if key == "CP_VAPID_SUBJECT" {
                        "mailto:push@hangar.dev.br".into()
                    } else {
                        String::new()
                    }
                });
            env.insert(key.to_owned(), value);
        }
        Ok(Self { dir, env })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn guest_push_configuration_paths_match_python() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/guests/push-config.json"
        ))
        .unwrap();
        for case in fixture["rows"].as_array().unwrap() {
            let actual = storage(case["raw"].as_str().unwrap()).join("subs.json");
            assert_eq!(
                actual,
                Path::new(case["file"].as_str().unwrap()),
                "{case:?}"
            );
        }
    }
    #[test]
    fn guest_push_vapid_uses_frozen_settings_when_loaded_again() {
        use crate::{
            accounts::{AccountService, environment::AccountEnvironment},
            auth::TrustedHosts,
            config::Config,
            routes::AppState,
        };
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/guests/push-config.json"
        ))
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file,"CP_VAPID_PUBLIC=original-public\nCP_VAPID_PRIVATE=original-private\nCP_VAPID_SUBJECT=mailto:fixture@example.invalid\n").unwrap();
        let mut st = AppState::new(Config {
            listen: "127.0.0.1:1".parse().unwrap(),
            upstream: "127.0.0.1:2".parse().unwrap(),
            internal_secret: "fixture".into(),
            auth_token: "owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse(""),
        });
        st.accounts = AccountService::new(AccountEnvironment::from_pairs([
            ("HOME", home.path().to_str().unwrap()),
            ("USERPROFILE", home.path().to_str().unwrap()),
        ]));
        let first = Frozen::from_state(&st).unwrap();
        std::fs::write(
            &file,
            "CP_VAPID_PUBLIC=changed-public\nCP_VAPID_PRIVATE=changed-private\n",
        )
        .unwrap();
        let later = Frozen::from_state(&st).unwrap();
        for (key, field) in [
            ("CP_VAPID_PUBLIC", "vapid_public"),
            ("CP_VAPID_PRIVATE", "vapid_private"),
            ("CP_VAPID_SUBJECT", "vapid_subject"),
        ] {
            assert_eq!(first.env[key], fixture["dotenv_snapshot"][field]);
            assert_eq!(later.env[key], first.env[key]);
        }
    }
}
