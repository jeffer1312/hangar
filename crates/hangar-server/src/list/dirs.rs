//! Diretórios efetivos da lista, resolvidos sem consultar o processo Python.
use super::discover_other::Dirs;
use crate::accounts::environment::AccountEnvironment;
use crate::machine_peers::dotenv::SettingsSnapshot;
use std::path::PathBuf;

pub fn from_snapshot(
    env: &AccountEnvironment,
    settings: &SettingsSnapshot,
) -> Result<Dirs, &'static str> {
    let projects = settings
        .value("CP_PROJECTS_DIR")
        .map_err(|_| "list_settings_unavailable")?
        .map(PathBuf::from);
    Dirs::resolve(env, projects.as_deref())
}

pub fn system() -> Result<Dirs, &'static str> {
    if let Some(raw) = std::env::var_os("HANGAR_LIST_DIRS") {
        let raw = raw.into_string().map_err(|_| "list_dirs_invalid")?;
        return super::bridge::parse_dirs(&raw).ok_or("list_dirs_invalid");
    }
    let env = AccountEnvironment::capture();
    let backend = std::env::current_dir().map_err(|_| "list_cwd_unavailable")?;
    let settings = SettingsSnapshot::capture(
        &env.home,
        Some(&backend),
        crate::machine_peers::dotenv::SettingsEnvironment::capture(),
    )
    .map_err(|_| "list_settings_unavailable")?;
    from_snapshot(&env, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine_peers::dotenv::SettingsEnvironment;

    #[test]
    fn projects_directory_from_the_settings_file_selects_the_session_base() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let projects = home.join("configured/projects");
        let settings = SettingsSnapshot::from_parts(
            SettingsEnvironment::default(),
            format!("CP_PROJECTS_DIR='{}'\n", projects.display()),
        );
        let roots = from_snapshot(&env, &settings).unwrap();
        assert_eq!(roots.claude, home.join("configured"));
        assert_eq!(roots.default_claude(), home.join(".claude"));
    }
}
