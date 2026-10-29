use hangar_server::{
    accounts::environment::AccountEnvironment,
    provider_discovery::{Discovery, scan},
};
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/cli-probe.json"
    ))
    .unwrap()
}

#[test]
#[cfg(unix)]
fn provider_discovery_matches_python_posix_and_windows_candidates() {
    use std::os::unix::fs::PermissionsExt;
    for case in fixture()["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        for dir in ["one", "two"] {
            std::fs::create_dir(home.path().join(dir)).unwrap();
        }
        for (file, allowed) in case["files"].as_object().unwrap() {
            let parts: Vec<_> = file
                .split(['/', '\\'])
                .filter(|part| !part.is_empty())
                .collect();
            let path = home
                .path()
                .join(parts[parts.len() - 2])
                .join(parts[parts.len() - 1]);
            std::fs::write(&path, "fixture").unwrap();
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if allowed == true { 0o700 } else { 0o600 }),
            )
            .unwrap();
        }
        let windows = case["windows"].as_bool().unwrap();
        let path = [home.path().join("one"), home.path().join("two")]
            .map(|p| p.to_string_lossy().into_owned())
            .join(if windows { ";" } else { ":" });
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("PATHEXT".into(), case["pathext"].as_str().unwrap().into()),
            ]
            .into(),
        );
        assert_eq!(json!(scan(&env, &path, windows)), case["result"], "{case}");
    }
}

#[tokio::test]
#[cfg(unix)]
async fn provider_discovery_login_path_matches_python_and_caches_per_context() {
    for case in fixture()["login"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join("shell");
        let calls = home.path().join("calls");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        if case["scenario"] != "error" {
            crate::write_executable(
                &shell,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nprintf %s {}\nexit {}\n",
                    quote(calls.to_str().unwrap()),
                    quote(case["stdout"].as_str().unwrap()),
                    case["code"]
                ),
            );
        }
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("SHELL".into(), shell.to_string_lossy().into_owned()),
                ("PATH".into(), "/fallback".into()),
            ]
            .into(),
        );
        let discovery = Discovery::default();
        assert_eq!(discovery.login_path(&env).await, case["result"], "{case}");
        if case["scenario"] != "error" {
            std::fs::remove_file(&shell).unwrap();
        }
        assert_eq!(discovery.login_path(&env).await, case["second"]);
        if case["scenario"] != "error" {
            assert_eq!(
                std::fs::read_to_string(&calls).unwrap(),
                "-l -c printenv PATH\n"
            );
        }
        let mut other = env.clone();
        other.base.insert("PATH".into(), "/other-context".into());
        assert_eq!(discovery.login_path(&other).await, "/other-context");
    }
}

#[tokio::test]
#[cfg(unix)]
async fn provider_discovery_coalesces_calls_and_invalidates_only_file_probe() {
    use std::sync::Arc;
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let shell = home.path().join("shell");
    let calls = home.path().join("calls");
    crate::write_executable(
        &shell,
        format!(
            "#!/bin/sh\nprintf x >> '{}'\nprintf %s \"$PATH\"\n",
            calls.display()
        ),
    );
    crate::write_executable(&bin.join("claude"), "#!/bin/sh\nexit 99\n");
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("SHELL".into(), shell.to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
        ]
        .into(),
    );
    let discovery = Arc::new(Discovery::default());
    let mut jobs = Vec::new();
    for _ in 0..16 {
        let (discovery, env) = (discovery.clone(), env.clone());
        jobs.push(tokio::spawn(async move {
            discovery.providers(&env).await.unwrap()
        }));
    }
    for job in jobs {
        assert!(job.await.unwrap()["claude"].available);
    }
    assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x");
    std::fs::remove_file(bin.join("claude")).unwrap();
    std::fs::remove_file(&shell).unwrap();
    assert!(discovery.providers(&env).await.unwrap()["claude"].available);
    discovery.invalidate();
    let probes = discovery.providers(&env).await.unwrap();
    assert!(!probes["claude"].available);
    assert_eq!(probes["claude"].reason, Some("nao_encontrado"));
    assert_eq!(
        std::fs::read_to_string(&calls).unwrap(),
        "x",
        "o PATH de login permanece em cache após invalidar arquivos"
    );
}
