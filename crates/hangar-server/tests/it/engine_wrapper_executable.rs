//! Disponibilidade do shell segue a mesma busca da sonda de versões.
use hangar_server::accounts::environment::AccountEnvironment;
use serde_json::Value;
#[cfg(unix)]
#[test]
fn engine_wrapper_requires_executable_shell() {
    use std::os::unix::fs::PermissionsExt;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/harness-health/wrapper-executable.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".bashrc"), "").unwrap();
        let mut dirs = Vec::new();
        for (index, mode) in case["modes"].as_array().unwrap().iter().enumerate() {
            let directory = home.path().join(format!("bin-{index}"));
            std::fs::create_dir(&directory).unwrap();
            let file = directory.join("bash");
            std::fs::write(&file, "fixture").unwrap();
            std::fs::set_permissions(
                file,
                std::fs::Permissions::from_mode(mode.as_u64().unwrap() as u32),
            )
            .unwrap();
            dirs.push(directory);
        }
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.path().to_string_lossy().into_owned()),
            (
                "PATH",
                std::env::join_paths(dirs)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]);
        let actual = hangar_server::harness_setup::health::wrapper(&env, "claude", false);
        let program = hangar_server::cli_probe::resolve_program("bash", &env).map(|p| {
            p.to_string_lossy()
                .replace(home.path().to_string_lossy().as_ref(), "$ROOT")
        });
        assert_eq!(
            serde_json::json!(program),
            case["program"],
            "{}",
            case["name"]
        );
        if actual != case["expected"] {
            failures.push(format!(
                "{}: {} != {}",
                case["name"], actual, case["expected"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
