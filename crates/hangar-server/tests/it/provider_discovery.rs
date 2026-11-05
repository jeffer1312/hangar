use hangar_server::{accounts::environment::AccountEnvironment, provider_discovery::Discovery};

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
