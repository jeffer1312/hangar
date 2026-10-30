fn git(home: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn project_instructions_match_original_scopes_records_git_and_disable() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/instruction-projects.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for row in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let codex = home.path().join(".codex");
        let project = home.path().join("project");
        let name = row["name"].as_str().unwrap();
        for (relative, hex) in row["before"].as_object().unwrap() {
            let path = home.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut raw = unhex(hex.as_str().unwrap());
            if relative.ends_with("config.toml") {
                raw = String::from_utf8(raw)
                    .unwrap()
                    .replace("$ROOT", home.path().to_str().unwrap())
                    .into_bytes();
            }
            fs::write(path, raw).unwrap();
        }
        fs::create_dir_all(&codex).unwrap();
        let has_git = row["git"].as_bool().unwrap();
        if has_git {
            git(home.path(), &["init", "-q", project.to_str().unwrap()]);
            fs::write(
                project.join(".git/info/exclude"),
                row["initial_exclude"].as_str().unwrap(),
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
                (
                    "CP_CODEX_SYNC_ENABLED".into(),
                    if name == "disabled" { "0" } else { "1" }.into(),
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
        let mut cwd = if name == "registered" {
            None
        } else {
            Some(project.join("src"))
        };
        let mut results = Vec::new();
        for round in 0..row["results"].as_array().unwrap().len() {
            if round > 0 {
                if name == "registered-record" {
                    cwd = None;
                    fs::write(project.join("src/CLAUDE.md"), "ALTERADO\n").unwrap();
                }
                if name == "remove-source" {
                    fs::remove_file(project.join("src/CLAUDE.md")).unwrap();
                }
            }
            let result = if name == "project-only" {
                prepare_project(&config, &codex, cwd.as_deref().unwrap(), true).await
            } else {
                prepare_with(
                    &config,
                    &codex,
                    cwd.as_deref(),
                    name != "disabled",
                    &NativeLinks,
                )
                .await
            };
            results.push(match result {
                Ok(()) => json!({"ok":true}),
                Err(error) => json!({"error":match error {Error::Invalid(_)=>"ValueError",Error::Changed(_)=>"AlteradoExternamente",Error::Io(_)=>"OSError"},"message":error.to_string()}),
            });
        }
        let mut files = serde_json::Map::new();
        let mut records = Vec::new();
        let mut backups = Vec::new();
        snapshot(
            home.path(),
            home.path(),
            &mut files,
            &mut records,
            &mut backups,
        );
        records.sort_by_key(|value| value["alvo"].as_str().unwrap().to_owned());
        if let Some(data) = files.get_mut(".codex/config.toml") {
            let raw = String::from_utf8(unhex(data["hex"].as_str().unwrap()))
                .unwrap()
                .replace(home.path().to_str().unwrap(), "$ROOT");
            data["hex"] = json!(
                raw.as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
        }
        let exclude =
            has_git.then(|| fs::read_to_string(project.join(".git/info/exclude")).unwrap());
        let status = has_git.then(|| {
            git(
                home.path(),
                &[
                    "-C",
                    project.to_str().unwrap(),
                    "status",
                    "--porcelain",
                    "--untracked-files=all",
                ],
            )
        });
        let actual = json!({"results":results,"files":files,"records":records,"backups":backups,"exclude":exclude,"git_status":status});
        let actual: Value = serde_json::from_str(
            &actual
                .to_string()
                .replace(home.path().to_str().unwrap(), "$ROOT"),
        )
        .unwrap();
        let expected = json!({"results":row["results"],"files":row["files"],"records":row["records"],"backups":row["backups"],"exclude":row["exclude"],"git_status":row["git_status"]});
        if actual != expected {
            differences.push((name.to_owned(), actual, expected));
        }
    }
    assert!(
        differences.is_empty(),
        "projetos divergentes: {differences:?}"
    );
}
