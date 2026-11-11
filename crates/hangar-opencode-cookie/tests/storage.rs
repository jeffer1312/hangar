use hangar_opencode_cookie::{path, read, write};
use serde_json::json;

#[test]
fn saved_cookie_is_private_trimmed_and_removed_when_either_value_is_empty() {
    let root = tempfile::tempdir().unwrap();
    assert!(read(root.path()).unwrap().is_empty());
    assert!(
        write(
            root.path(),
            "engine:oc",
            "\u{1c}ws-9\u{1f}",
            " cookie-exemplo "
        )
        .unwrap()
    );
    let entries = read(root.path()).unwrap();
    assert_eq!(entries["engine:oc"]["workspace_id"], "ws-9");
    assert_eq!(entries["engine:oc"]["auth_cookie"], "cookie-exemplo");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path(root.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!write(root.path(), "engine:oc", "", "cookie-exemplo").unwrap());
    assert!(!read(root.path()).unwrap().contains_key("engine:oc"));
    assert!(!write(root.path(), "absent", "", "").unwrap());
}

#[test]
fn old_file_is_read_only_until_the_new_file_exists() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".claude")).unwrap();
    let old = root.path().join(".claude/.claude-pocket-opencode.json");
    std::fs::write(
        &old,
        json!({"old":{"workspace_id":"w", "auth_cookie":"c", "extra":1},
        "invalid":{"workspace_id":"w"}})
        .to_string(),
    )
    .unwrap();
    assert_eq!(read(root.path()).unwrap()["old"]["workspace_id"], "w");
    write(root.path(), "new", "w2", "c2").unwrap();
    let entries = read(root.path()).unwrap();
    assert_eq!(entries["old"]["auth_cookie"], "c");
    assert!(entries["old"].get("extra").is_none());
    assert!(!entries.contains_key("invalid"));
    std::fs::write(path(root.path()), "[]").unwrap();
    assert!(
        read(root.path()).is_err(),
        "arquivo novo inválido não reabre o antigo"
    );
    write(root.path(), "recovered", "w3", "c3").unwrap();
    let recovered = read(root.path()).unwrap();
    assert!(recovered.contains_key("recovered"));
    assert!(!recovered.contains_key("old"));
}

#[test]
fn concurrent_writes_preserve_other_credentials() {
    let root = tempfile::tempdir().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = ["one", "two", "three"]
        .into_iter()
        .map(|id| {
            let (home, barrier) = (root.path().to_path_buf(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                write(&home, id, "workspace", "cookie-exemplo")
            })
        })
        .collect();
    let outcomes: Vec<_> = workers.into_iter().map(|worker| worker.join()).collect();
    for outcome in outcomes {
        assert!(outcome.unwrap().unwrap());
    }
    let entries = read(root.path()).unwrap();
    for id in ["one", "two", "three"] {
        assert_eq!(entries[id]["workspace_id"], "workspace");
    }
}

#[test]
fn discarded_python_json_values_do_not_erase_other_credentials() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".claude")).unwrap();
    for raw in [
        r#"{"A":{"workspace_id":"w","auth_cookie":"c"},"discarded":NaN}"#,
        r#"{"A":{"workspace_id":"w","auth_cookie":"c","extra":"\ud800"}}"#,
    ] {
        std::fs::write(path(root.path()), raw).unwrap();
        write(root.path(), "B", "w2", "c2").unwrap();
        let entries = read(root.path()).unwrap();
        assert_eq!(
            entries
                .get("A")
                .and_then(|entry| entry["auth_cookie"].as_str()),
            Some("c")
        );
        assert_eq!(entries["B"]["workspace_id"], "w2");
    }
}

#[test]
#[cfg(unix)]
fn io_failure_never_replaces_the_existing_cookie_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".claude")).unwrap();
    let target = path(root.path());
    std::fs::write(
        root.path().join(".claude/.claude-pocket-opencode.json"),
        r#"{"old":{"workspace_id":"w","auth_cookie":"c"}}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink(&target, &target).unwrap();
    assert!(write(root.path(), "engine:new", "w", "cookie-exemplo").is_err());
    assert_eq!(std::fs::read_link(&target).unwrap(), target);
}

#[test]
fn write_failure_is_reported_and_does_not_claim_a_saved_cookie() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".claude"), "não é diretório").unwrap();
    assert!(write(root.path(), "engine:oc", "ws", "cookie-exemplo").is_err());
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 64, rng_seed: proptest::test_runner::RngSeed::Fixed(1), ..Default::default()
    })]
    #[test]
    fn external_file_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048)) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".claude")).unwrap();
        std::fs::write(path(root.path()), bytes).unwrap();
        if let Ok(entries) = read(root.path()) {
            for entry in entries.values() {
                proptest::prop_assert!(entry["workspace_id"].as_str().is_some_and(|text| !text.is_empty()));
                proptest::prop_assert!(entry["auth_cookie"].as_str().is_some_and(|text| !text.is_empty()));
            }
        }
    }
}
