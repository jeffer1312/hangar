//! Contrato compartilhado de leitura e escrita do arquivo de configuração.
#[test]
fn channel_dotenv_edits_match_python_bytes() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/channel-cases.json"
    ))
    .unwrap();
    for case in fixture["dotenv"].as_array().unwrap() {
        let actual = hangar_server::machine_peers::dotenv::replace(
            case["input"].as_str().unwrap(),
            &[
                ("CP_UPDATE_BRANCH", "test/channel"),
                ("CP_UPDATE_LAST_BRANCH", "test/channel"),
            ],
        );
        assert_eq!(actual, case["expected"], "PEE073 {}", case["name"]);
    }
}

#[test]
fn checkout_dotenv_read_matches_python_settings() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/dotenv-read.json"
    ))
    .unwrap();
    for case in data["cases"].as_array().unwrap() {
        let environment = serde_json::from_value(case["environment"].clone()).unwrap();
        let raw = case["raw"].as_str().unwrap();
        let resolved = hangar_server::machine_peers::dotenv::resolve(raw, &environment);
        assert_eq!(
            serde_json::to_value(resolved).unwrap(),
            case["parsed"],
            "{}",
            case["name"]
        );
        let settings = hangar_server::machine_peers::dotenv::settings(raw, &environment);
        assert_eq!(
            settings
                .get("cp_server_id")
                .cloned()
                .flatten()
                .unwrap_or_default(),
            case["server_id"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
}
#[test]
fn checkout_dotenv_legacy_lookup_expands_previous_values() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/dotenv-read.json"
    ))
    .unwrap();
    let case = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "quoted-interpolation")
        .unwrap();
    assert_eq!(
        hangar_server::machine_peers::dotenv::value(case["raw"].as_str().unwrap(), "CP_SERVER_ID"),
        case["server_id"].as_str().map(str::to_owned)
    );
}
#[test]
fn captured_environment_and_user_file_follow_settings_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let backend = temp.path().join("backend");
    std::fs::create_dir_all(&backend).unwrap();
    let environment = [("CHANNEL".to_owned(), "environment".to_owned())].into();
    std::fs::write(
        backend.join(".env"),
        "CHANNEL=file\ncp_server_id=${CHANNEL}\n",
    )
    .unwrap();
    let read = |environment| {
        hangar_server::machine_peers::config_value_with_environment(
            &home,
            Some(&backend),
            "CP_SERVER_ID",
            environment,
        )
    };
    assert_eq!(read(&environment).unwrap().as_deref(), Some("file"));
    std::fs::write(backend.join(".env"), "CP_SERVER_ID=${CHANNEL}\n").unwrap();
    assert_eq!(read(&environment).unwrap().as_deref(), Some("environment"));
    let mut override_env = environment.clone();
    override_env.insert("CP_SERVER_ID".into(), "override".into());
    assert_eq!(read(&override_env).unwrap().as_deref(), Some("override"));
    std::fs::remove_file(backend.join(".env")).unwrap();
    assert_eq!(read(&environment).unwrap(), None);
    std::fs::create_dir(backend.join(".env")).unwrap();
    assert!(read(&environment).is_err());
}
proptest::proptest! {
    #[test]
    fn arbitrary_dotenv_is_deterministic_and_an_explicit_override_wins(raw in ".{0,1000}") {
        let env=[("CP_SERVER_ID".to_owned(),"override".to_owned())].into();
        let first=hangar_server::machine_peers::dotenv::resolve(&raw,&env);
        proptest::prop_assert_eq!(first,hangar_server::machine_peers::dotenv::resolve(&raw,&env));
        let actual = hangar_server::machine_peers::dotenv::settings(&raw,&env).remove("cp_server_id").flatten();
        proptest::prop_assert_eq!(actual.as_deref(),Some("override"));
    }
}
#[cfg(unix)]
#[test]
#[ignore = "auxiliar de ambiente inválido, chamado em processo isolado"]
fn invalid_environment_helper() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let result = hangar_server::machine_peers::config_value(&home, None, "CP_SERVER_ID");
    assert!(
        result.is_err(),
        "a chave relevante fora de UTF-8 deve ser recusada sem distinção de caixa"
    );
}
#[cfg(unix)]
#[test]
fn invalid_environment_value_is_rejected_for_every_key_case() {
    use std::os::unix::ffi::OsStringExt;
    for key in ["CP_SERVER_ID", "cp_server_id", "Cp_Server_Id"] {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dotenv_config::invalid_environment_helper",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", home.path())
            .env(key, std::ffi::OsString::from_vec(vec![0xff]))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{key}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
