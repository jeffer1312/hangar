use hangar_projects::Service;
use serde_json::json;
use std::collections::BTreeMap;
fn service(root: &std::path::Path) -> Service {
    Service {
        preferences: root.join("runner.json"),
        program: vec![root.join("missing-mux").to_string_lossy().into()],
        scope: Vec::new(),
        env: BTreeMap::new(),
        windows: false,
    }
}
#[test]
fn runners_keep_detection_order_and_choose_only_the_highest_rank() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    std::fs::write(
        root.path().join("package.json"),
        r#"{"scripts":{"start":"x","dev":"x","other":"x"}}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("pnpm-lock.yaml"), "").unwrap();
    std::fs::write(
        root.path().join("Makefile"),
        "dev:\n\tx\nserve:\n\ty\ndev:\n",
    )
    .unwrap();
    let runners = service.detect_runners(root.path().to_str().unwrap());
    assert_eq!(
        runners
            .iter()
            .map(|r| r.command.as_str())
            .collect::<Vec<_>>(),
        [
            "pnpm run start",
            "pnpm run dev",
            "pnpm run other",
            "make dev",
            "make serve"
        ]
    );
    assert_eq!(
        runners
            .iter()
            .filter(|r| r.is_dev_guess)
            .map(|r| r.command.as_str())
            .collect::<Vec<_>>(),
        ["pnpm run dev"]
    );
}
#[test]
fn legacy_remembered_and_custom_commands_share_the_same_preference_entry() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    std::fs::write(&service.preferences, r#"{"cwd":"npm run dev"}"#).unwrap();
    assert_eq!(service.remembered("cwd").as_deref(), Some("npm run dev"));
    let commands = service
        .set_custom_commands("cwd", json!([{"label":"dev","command":"run","extra":true}]))
        .unwrap();
    assert_eq!(commands[0].source, "custom");
    assert_eq!(service.remembered("cwd").as_deref(), Some("npm run dev"));
    assert_eq!(
        service
            .set_custom_commands("cwd", json!([{"label":" ","command":"run"}]))
            .unwrap_err()
            .status,
        400
    );
}
#[cfg(unix)]
#[tokio::test]
async fn failed_spawn_keeps_the_command_to_let_the_person_retry() {
    let root = tempfile::tempdir().unwrap();
    let mut service = service(root.path());
    let script = r#"case "$*" in *has-session*) echo "can't find session: x" >&2; exit 1;; *new-session*) echo nope >&2; exit 1;; esac"#;
    service.program = vec!["/bin/sh".into(), "-c".into(), script.into()];
    let cwd = root.path().to_str().unwrap();
    let error = service.start_run(cwd, "npm run dev").await.unwrap_err();
    assert_eq!(error.status, 502);
    assert_eq!(service.remembered(cwd).as_deref(), Some("npm run dev"));
}
#[test]
fn custom_commands_keep_the_old_body_caps_and_error_codes() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    let one = json!({"label":"dev","command":"run"});
    let too_many = json!(vec![one; 501]);
    assert_eq!(service.set_custom_commands("cwd", too_many).unwrap_err().status, 422);
    let long_label = json!([{"label":"é".repeat(201),"command":"run"}]);
    assert_eq!(service.set_custom_commands("cwd", long_label).unwrap_err().status, 422);
    let long_command = json!([{"label":"dev","command":"x".repeat(200_001)}]);
    assert_eq!(service.set_custom_commands("cwd", long_command).unwrap_err().status, 422);
    let empty = service
        .set_custom_commands("cwd", json!([{"label":"","command":"run"}]))
        .unwrap_err();
    assert_eq!(empty.code, Some("erro_runner_custom"));
    assert!(service
        .set_custom_commands("cwd", json!([{"label":"é".repeat(200),"command":"run"}]))
        .is_ok());
}
#[test]
fn makefile_with_invalid_utf8_still_lists_its_targets() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    std::fs::write(root.path().join("Makefile"), b"# configura\xe7\xe3o\ndev:\n\tx\n").unwrap();
    let runners = service.detect_runners(root.path().to_str().unwrap());
    assert_eq!(runners.iter().map(|r| r.command.as_str()).collect::<Vec<_>>(), ["make dev"]);
}
#[cfg(unix)]
#[tokio::test]
async fn multiplexer_gets_only_the_given_environment() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("env.txt");
    let mut service = service(root.path());
    service.program = vec!["/bin/sh".into(), "-c".into(), format!("env > '{}'", out.display())];
    service.env = BTreeMap::from([("ONLY_THIS".into(), "1".into())]);
    service.run_pane(root.path().to_str().unwrap()).await;
    let env = std::fs::read_to_string(&out).unwrap();
    assert!(env.contains("ONLY_THIS=1"));
    assert!(!env.lines().any(|l| l.starts_with("HOME=")), "{env}");
}
#[test]
fn unreadable_preferences_are_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    std::fs::write(&service.preferences, "{corrompido").unwrap();
    let error = service
        .set_custom_commands("cwd", json!([{"label":"dev","command":"run"}]))
        .unwrap_err();
    assert_eq!(error.status, 500);
    assert_eq!(std::fs::read_to_string(&service.preferences).unwrap(), "{corrompido");
}
#[cfg(unix)]
#[tokio::test]
async fn stop_without_an_answer_from_the_multiplexer_is_not_success() {
    let root = tempfile::tempdir().unwrap();
    let mut service = service(root.path());
    service.program = vec!["/bin/sh".into(), "-c".into(), "echo boom >&2; exit 1".into()];
    let error = service.stop_run(root.path().to_str().unwrap()).await.unwrap_err();
    assert_eq!(error.code, Some("runner_mux_unavailable"));
}
#[cfg(unix)]
#[tokio::test]
async fn unreadable_preferences_do_not_block_the_play() {
    let root = tempfile::tempdir().unwrap();
    let mut service = service(root.path());
    let script = r#"case "$*" in *has-session*) echo "can't find session: x" >&2; exit 1;; *new-session*) echo nope >&2; exit 1;; esac"#;
    service.program = vec!["/bin/sh".into(), "-c".into(), script.into()];
    std::fs::write(&service.preferences, "").unwrap();
    let error = service.start_run(root.path().to_str().unwrap(), "npm run dev").await.unwrap_err();
    assert_eq!(error.status, 502);
}
