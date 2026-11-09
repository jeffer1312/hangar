//! Raízes das conversas resolvidas pelo ambiente efetivo do servidor.
use crate::account_env::AccountEnvironment;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SessionDirectories {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub codex_home: PathBuf,
    pub pi_sessions: PathBuf,
    pub omp_config: PathBuf,
    pub omp_agent: PathBuf,
    pub kimi_home: PathBuf,
}

impl SessionDirectories {
    pub fn resolve(
        env: &AccountEnvironment,
        projects: Option<&Path>,
    ) -> Result<Self, &'static str> {
        let value = |name: &str| {
            let found = if cfg!(windows) {
                env.base
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value)
            } else {
                env.base.get(name)
            };
            found.filter(|value| !value.is_empty()).map(String::as_str)
        };
        let omp = env.omp_directories()?;
        Ok(Self {
            home: env.home.clone(),
            claude: projects
                .map(|path| {
                    path.parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .unwrap_or_else(|| {
                            if path.has_root() {
                                path
                            } else {
                                Path::new(".")
                            }
                        })
                        .to_path_buf()
                })
                .unwrap_or_else(|| {
                    value("CLAUDE_CONFIG_DIR")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| env.claude_base.clone())
                }),
            codex_home: std::path::absolute(&env.codex_default)
                .map_err(|_| "codex_directory_invalid")?,
            pi_sessions: value("PI_CODING_AGENT_SESSION_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| env.home.join(".pi/agent/sessions")),
            omp_config: env.home.join(value("PI_CONFIG_DIR").unwrap_or(".omp")),
            omp_agent: omp.agent_dir,
            kimi_home: value("KIMI_CODE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| env.home.join(".kimi-code")),
        })
    }

    pub fn default_claude(&self) -> PathBuf {
        self.home.join(".claude")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_codex_home_is_anchored_at_the_server_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_pairs([
            ("HOME", dir.path().to_string_lossy().into_owned()),
            ("USERPROFILE", dir.path().to_string_lossy().into_owned()),
            ("CODEX_HOME", "relative-codex".to_owned()),
        ]);
        let roots = SessionDirectories::resolve(&env, None).unwrap();
        assert!(roots.codex_home.is_absolute());
        assert_eq!(
            roots.codex_home.parent(),
            Some(std::env::current_dir().unwrap().as_path())
        );
        assert_eq!(
            roots.codex_home.file_name(),
            Some(std::ffi::OsStr::new("relative-codex"))
        );
    }

    #[test]
    fn the_server_claude_directory_keeps_the_configured_literal_path() {
        let dir = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_pairs([
            ("HOME", dir.path().to_string_lossy().into_owned()),
            ("USERPROFILE", dir.path().to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR", "~/claude-relative".to_owned()),
        ]);
        let roots = SessionDirectories::resolve(&env, None).unwrap();
        assert_eq!(roots.claude, Path::new("~/claude-relative"));
    }

    #[test]
    fn a_project_directory_at_the_filesystem_root_keeps_its_root_parent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let root = home.ancestors().last().unwrap();
        let roots = SessionDirectories::resolve(&env, Some(root)).unwrap();
        assert_eq!(roots.claude, root);
    }

    #[test]
    fn agent_roots_follow_their_effective_environment() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let path = |name: &str| home.join(name).to_string_lossy().into_owned();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR", path("claude-account")),
            ("CODEX_HOME", path("codex-account")),
            ("PI_CODING_AGENT_SESSION_DIR", path("pi-sessions")),
            ("KIMI_CODE_HOME", path("kimi-account")),
            ("PI_CONFIG_DIR", ".omp-personal".to_owned()),
            ("PI_CODING_AGENT_DIR", path("omp-agent")),
        ]);
        let roots = SessionDirectories::resolve(&env, None).unwrap();
        assert_eq!(roots.claude, home.join("claude-account"));
        assert_eq!(roots.codex_home, home.join("codex-account"));
        assert_eq!(roots.pi_sessions, home.join("pi-sessions"));
        assert_eq!(roots.omp_config, home.join(".omp-personal"));
        assert_eq!(roots.omp_agent, home.join("omp-agent"));
        assert_eq!(roots.kimi_home, home.join("kimi-account"));
    }

    #[test]
    fn custom_projects_selects_the_registry_base_without_changing_default_account() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let projects = home.join("projects-custom/projects");
        let roots = SessionDirectories::resolve(&env, Some(&projects)).unwrap();
        assert_eq!(roots.claude, home.join("projects-custom"));
        assert_eq!(roots.default_claude(), home.join(".claude"));
        assert_eq!(roots.pi_sessions, home.join(".pi/agent/sessions"));
    }

    #[test]
    fn default_roots_need_no_python_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let roots = SessionDirectories::resolve(&env, None).unwrap();
        assert_eq!(roots.claude, home.join(".claude"));
        assert_eq!(roots.codex_home, home.join(".codex"));
        assert_eq!(roots.pi_sessions, home.join(".pi/agent/sessions"));
        assert_eq!(roots.omp_config, home.join(".omp"));
        assert_eq!(roots.omp_agent, home.join(".omp/agent"));
        assert_eq!(roots.kimi_home, home.join(".kimi-code"));
    }
}
