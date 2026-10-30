use hangar_server::accounts::environment::AccountEnvironment;

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

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_retries_failed_capture_and_nonzero_exit_without_invalidation() {
    use hangar_server::cli_probe::Probe;
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("claude");
    let env = AccountEnvironment::from_pairs([
        ("HOME", home.path().to_string_lossy().into_owned()),
        ("PATH", home.path().to_string_lossy().into_owned()),
    ]);
    let mut probe = Probe::default();
    probe.timeout = std::time::Duration::from_millis(100);
    std::fs::write(&script, "#!/bin/sh\n/bin/sleep 2\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        probe.version_text(&env, "claude").await.as_deref(),
        Some("")
    );
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '2.1.999 --plugin-dir'\nprintf 'falha de configuração' >&2\nexit 1\n",
    )
    .unwrap();
    assert_eq!(
        probe.version_text(&env, "claude").await.as_deref(),
        Some("")
    );
    std::fs::write(&script, "#!/bin/sh\nprintf '2.1.999 --plugin-dir'\n").unwrap();
    assert_eq!(
        probe.version_text(&env, "claude").await.as_deref(),
        Some("2.1.999 --plugin-dir")
    );
}
