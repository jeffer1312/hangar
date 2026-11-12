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
#[tokio::test]
async fn failed_spawn_keeps_the_command_to_let_the_person_retry() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    let cwd = root.path().to_str().unwrap();
    let error = service.start_run(cwd, "npm run dev").await.unwrap_err();
    assert_eq!(error.status, 502);
    assert_eq!(service.remembered(cwd).as_deref(), Some("npm run dev"));
}
