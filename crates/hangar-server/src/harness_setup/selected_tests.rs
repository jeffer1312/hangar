use super::*;
use serde_json::{Value, json};

#[test]
fn selected_hook_installers_match_original_files_and_idempotence() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/selected-hooks.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir_in(crate::list::links::without_verbatim(std::env::temp_dir().canonicalize().unwrap())).unwrap();
        let repo = home.path().join("repo");
        let account = home.path().join(".claude");
        std::fs::create_dir_all(&account).unwrap();
        let path = account.join("settings.json");
        std::fs::write(&path, case["initial"].to_string()).unwrap();
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
        );
        let service = AccountService::new(env);
        let name = case["installer"].as_str().unwrap();
        let _ = hooks::sync_installer(&service, &repo, name);
        let data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let normalized = crate::transcript::pyjson::dumps(&data, false)
            .replace(
                &crate::install_root::python_exe(&repo, cfg!(windows))
                    .to_string_lossy()
                    .to_string(),
                "/fixture/python",
            )
            .replace(&repo.to_string_lossy().to_string(), "/fixture/repo")
            .replace(
                &std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
                "/fixture/hangar-server",
            )
            .replace(&home.path().to_string_lossy().to_string(), "/fixture/home");
        let actual: Value = serde_json::from_str(&normalized).unwrap();
        let before = std::fs::read(&path).unwrap();
        let _ = hooks::sync_installer(&service, &repo, name);
        if actual != case["after"]
            || before != std::fs::read(&path).unwrap()
            || json!(account.join("hooks/guard_tmux.py").is_symlink()) != case["guard_link"]
        {
            differences.push((index, actual, case["after"].clone()));
        }
    }
    assert!(
        differences.is_empty(),
        "instaladores divergentes: {differences:?}"
    );
}
