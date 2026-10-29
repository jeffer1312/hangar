use hangar_server::accounts::environment::AccountEnvironment;

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_matches_python_and_shares_capture() {
    use hangar_server::cli_probe::Probe;
    use serde_json::Value;
    use std::os::unix::fs::PermissionsExt;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/cli-probe.json"
    ))
    .unwrap();
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let whitespace: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/cli-probe-whitespace.json"
    ))
    .unwrap();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(whitespace["cases"].as_array().unwrap())
    {
        let home = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("PATH".into(), home.path().to_string_lossy().into_owned()),
            ]
            .into(),
        );
        let calls = home.path().join("calls");
        let script = home.path().join("claude");
        if case["present"] == true {
            let output = if case["failure"] == true {
                "/bin/sleep 1".into()
            } else {
                format!(
                    "if [ \"$1\" = --help ]; then printf %s {}; else printf %s {}; fi\nprintf %s {} >&2",
                    quote(if case["supported"] == true {
                        "usage --plugin-dir path"
                    } else {
                        "usage"
                    }),
                    quote(case["stdout"].as_str().unwrap()),
                    quote(case["stderr"].as_str().unwrap())
                )
            };
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\n{output}\n",
                    quote(calls.to_str().unwrap())
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut probe = Probe::default();
        probe.timeout = std::time::Duration::from_millis(50);
        let version = probe
            .version(&env)
            .await
            .map(|v| v.iter().map(|n| n.to_string()).collect::<Vec<_>>());
        let expected = case["version"]
            .as_array()
            .map(|v| v.iter().map(|n| n.to_string()).collect::<Vec<_>>());
        assert_eq!(version, expected, "{}", case["name"]);
        assert_eq!(
            serde_json::json!(probe.version_text(&env, "claude").await),
            case["health"],
            "{}",
            case["name"]
        );
        assert_eq!(
            probe.supports_plugin_dir(&env).await,
            case["help"].as_bool().unwrap()
        );
        let count = std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(
            count,
            if case["present"] == true { 2 } else { 0 },
            "captura única {}",
            case["name"]
        );
        probe.invalidate_cli("claude");
        probe.version_text(&env, "claude").await;
        assert_eq!(
            std::fs::read_to_string(&calls)
                .unwrap_or_default()
                .lines()
                .count(),
            if case["present"] == true { 3 } else { 0 }
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_non_executable_is_absent() {
    use hangar_server::cli_probe::Probe;
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("claude"), "#!/bin/sh\nexit 0\n").unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("PATH".into(), home.path().to_string_lossy().into_owned()),
        ]
        .into(),
    );
    assert_eq!(Probe::default().version_text(&env, "claude").await, None);
}

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_cache_isolates_environment_and_invalidates_all_contexts() {
    use hangar_server::cli_probe::Probe;
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("claude");
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s' "$PROBE_VERSION"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let make = |version: &str| {
        AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("PATH".into(), home.path().to_string_lossy().into_owned()),
                ("PROBE_VERSION".into(), version.into()),
            ]
            .into(),
        )
    };
    let probe = Probe::default();
    for version in ["1.2.3", "4.5.6", "1.2.3"] {
        assert_eq!(
            probe
                .version_text(&make(version), "claude")
                .await
                .as_deref(),
            Some(version)
        );
    }
    std::fs::write(&script, "#!/bin/sh\nprintf changed\n").unwrap();
    probe.invalidate_cli("claude");
    for version in ["1.2.3", "4.5.6"] {
        assert_eq!(
            probe
                .version_text(&make(version), "claude")
                .await
                .as_deref(),
            Some("changed")
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_account_capture_preserves_account_and_filters_private_keys() {
    use hangar_server::command_capture::{capture, capture_with_account_env};
    let env = AccountEnvironment::from_map(
        [
            ("CLAUDE_CONFIG_DIR".into(), "/fixture/account".into()),
            ("CP_AUTH_TOKEN".into(), "synthetic-private-value".into()),
        ]
        .into(),
    );
    let args = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s|%s' \"$CLAUDE_CONFIG_DIR\" \"$CP_AUTH_TOKEN\"".into(),
    ];
    let budget = std::time::Duration::from_secs(2);
    assert_eq!(
        capture_with_account_env(args.clone(), env.clone(), budget)
            .await
            .unwrap()
            .stdout,
        "/fixture/account|"
    );
    assert_eq!(capture(args, env, budget).await.unwrap().stdout, "|");
}
