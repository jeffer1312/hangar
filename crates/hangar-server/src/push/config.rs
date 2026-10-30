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
