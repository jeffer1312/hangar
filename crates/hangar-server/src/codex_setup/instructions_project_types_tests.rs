#[tokio::test]
async fn instruction_project_types_match_original_selection_and_refusal() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/instruction-project-types.json")).unwrap();
    let mut differences = Vec::new();
    for row in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let codex = home.path().join(".codex");
        let project = home.path().join("project");
        let claude = home.path().join(".claude");
        for path in [&codex, &project, &claude] {
            fs::create_dir(path).unwrap();
        }
        fs::write(claude.join("CLAUDE.md"), "GLOBAL\n").unwrap();
        fs::write(project.join("CLAUDE.md"), "PROJETO\n").unwrap();
        if let Some(config) = row["config"].as_str() {
            fs::write(
                codex.join("config.toml"),
                config.replace("$ROOT", home.path().to_str().unwrap()),
            )
            .unwrap();
        }
        if row["record"].is_object() {
            fs::create_dir(codex.join(".hangar-instrucoes")).unwrap();
            fs::write(
                codex.join(".hangar-instrucoes/seed.json"),
                serde_json::to_vec(&row["record"]).unwrap(),
            )
            .unwrap();
        }
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
                ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ]
            .into(),
        );
        let config = RuntimeConfig::from_snapshot(
            env,
            Arc::new(SettingsSnapshot::from_parts(
                SettingsEnvironment::default(),
                String::new(),
            )),
        )
        .unwrap();
        let result = match prepare(&config, &codex, None, true).await {
            Ok(()) => json!({"ok":true}),
            Err(error) => {
                json!({"ok":false,"message":error.to_string().replace(home.path().to_str().unwrap(),"$ROOT")})
            }
        };
        let actual = json!({"result":result,"global_alias":codex.join("AGENTS.override.md").is_symlink(),"project_alias":project.join("AGENTS.override.md").is_symlink(),"source":fs::read_to_string(project.join("CLAUDE.md")).unwrap()});
        let expected = json!({"result":row["result"],"global_alias":row["global_alias"],"project_alias":row["project_alias"],"source":row["source"]});
        if actual != expected {
            differences.push((row["name"].clone(), actual, expected));
        }
        if row["record"].is_object() {
            let record: Value = serde_json::from_slice(
                &fs::read(codex.join(".hangar-instrucoes/seed.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                record, row["record"],
                "registro pessoal alterado: {}",
                row["name"]
            );
        }
    }
    assert!(
        differences.is_empty(),
        "tipos de projeto divergentes: {differences:?}"
    );
}
