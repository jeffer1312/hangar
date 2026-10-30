use super::{
    apply::Apply,
    bundle::{Blob, Bundle},
    export::Context,
    files,
    paths::{self, Roots},
};
use serde_json::json;
fn roots(home: &std::path::Path) -> Roots {
    Roots {
        home: home.display().to_string(),
        claude: home.join(".claude").display().to_string(),
        codex: home.join(".codex").display().to_string(),
        hangar: home.join("checkout").display().to_string(),
    }
}
#[test]
fn malformed_warnings_are_discarded_before_application() {
    let mut source = Bundle::default();
    source.warnings.insert("engines".into(), json!("invalid"));
    source
        .warnings
        .insert("codex".into(), json!([{"code":"kept"}]));
    let mut decoded = Bundle::unpack(&source.pack().unwrap()).unwrap();
    assert!(!decoded.warnings.contains_key("engines"));
    decoded.warn("engines", "added", json!({}));
    assert_eq!(decoded.warnings["codex"], source.warnings["codex"]);
    assert_eq!(decoded.warnings["engines"][0]["code"], "added");
}
#[test]
fn declared_background_without_blob_fails_without_writing() {
    let home = tempfile::tempdir().unwrap();
    let context = Context::new(roots(home.path()));
    let native = context.native.clone();
    let mut bundle = Bundle::default();
    bundle.items.insert(
        "hangar_appearance".into(),
        json!({"background":{"name":"image.png"},"native":{"theme":"dark"}}),
    );
    let mut apply = Apply::new(context, bundle, "review", Default::default());
    let error = apply.apply_item("hangar_appearance").unwrap_err();
    assert_eq!(error.code, "config_sync_invalid_bundle");
    assert!(!native.exists());
}
#[test]
fn canonical_paths_require_both_boundaries() {
    let roots = roots(std::path::Path::new("/users/example"));
    for tail in ["/notes", "/.hangar/versions/v1/scripts/tool"] {
        let path = format!("{}{tail}", roots.home);
        assert_eq!(
            paths::canonicalize(&format!("prefix{path}"), &roots),
            format!("prefix{path}")
        );
        for prefix in ["", "--file=", "\"", "file://"] {
            let value = paths::canonicalize(&format!("{prefix}{path}"), &roots);
            assert!(value.starts_with(&format!("{prefix}⟦")), "{value}");
        }
    }
}
#[cfg(unix)]
#[test]
fn unchanged_bytes_still_tighten_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("private.json");
    std::fs::write(&path, b"secret").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let incoming = [(
        String::new(),
        Blob {
            data: b"secret".to_vec(),
            mode: 0o600,
        },
    )]
    .into();
    let (changed, _) = files::write_entry(
        &path,
        "file",
        &incoming,
        &roots(home.path()),
        &home.path().join("backups"),
    )
    .unwrap();
    assert!(changed);
    assert_eq!(files::mode(&path).unwrap(), 0o600);
    assert_eq!(std::fs::read(&path).unwrap(), b"secret");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_application_and_model_restore_share_the_lock() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".claude");
    std::fs::create_dir_all(&config).unwrap();
    let path = config.join("settings.json");
    std::fs::write(&path, br#"{"model":"temporary"}"#).unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let backup = home.path().join("backups");
    let writer = tokio::task::spawn_blocking(move || {
        let mut entered = Some(entered);
        files::edit_json(&path, &backup, |data| {
            if let Some(entered) = entered.take() {
                let _ = entered.send(());
                released.recv().unwrap();
            }
            data["kept"] = json!(true);
            Ok(())
        })
        .unwrap()
    });
    observed.await.unwrap();
    let restore = tokio::spawn(async move {
        crate::model_picker::default_model::restore_after_write(
            &config,
            &crate::model_picker::default_model::Snapshot::Model(json!("original")),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!restore.is_finished());
    release.send(()).unwrap();
    assert!(writer.await.unwrap());
    assert!(restore.await.unwrap().unwrap());
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(data, json!({"model":"original","kept":true}));
}

#[test]
fn jev_reads_after_the_participating_writer_releases_its_lock() {
    use crate::accounts::environment::AccountEnvironment;
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    std::fs::write(&path, br#"{"mcpServers":{"hangar-computer-control":{"command":"tool","env":{"LOCAL":"original"}}}}"#).unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().display().to_string()),
            ("USERPROFILE".into(), home.path().display().to_string()),
            ("TYPESAFE_API_KEY".into(), "fixture-key".into()),
        ]
        .into(),
    );
    let config = super::runtime::RuntimeConfig::new(env).unwrap();
    std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
    std::fs::write(&config.path, br#"{"jev_api_key":"fixture-key"}"#).unwrap();
    let guard = crate::session_open::claude::lock_config(&path).unwrap();
    let (started, observed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        config.sync_jev()
    });
    observed.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!worker.is_finished());
    std::fs::write(&path, br#"{"kept":true,"mcpServers":{"other":{"command":"keep"},"hangar-computer-control":{"command":"tool","env":{"LOCAL":"changed"}}}}"#).unwrap();
    drop(guard);
    worker.join().unwrap().unwrap();
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(data["kept"], true);
    assert_eq!(data["mcpServers"]["other"]["command"], "keep");
    assert_eq!(
        data["mcpServers"]["hangar-computer-control"]["env"]["LOCAL"],
        "changed"
    );
    assert_eq!(
        data["mcpServers"]["hangar-computer-control"]["env"]["TYPESAFE_API_KEY"],
        "fixture-key"
    );
}

#[test]
fn failed_skill_listing_is_excluded_instead_of_exported_empty() {
    let home = tempfile::tempdir().unwrap();
    let context = Context::new(roots(home.path()));
    std::fs::create_dir_all(&context.roots.claude).unwrap();
    std::fs::write(
        std::path::Path::new(&context.roots.claude).join("skills"),
        b"not a directory",
    )
    .unwrap();
    let bundle = super::export::export(&context, &["claude_skills".into()], None, false).unwrap();
    assert!(!bundle.items.contains_key("claude_skills"));
    assert_eq!(
        bundle.warnings["claude_skills"][0]["code"],
        "config_sync_item_failed"
    );
}

fn retained_version(folder: &std::path::Path, error: &str, expected: &[u8]) -> std::path::PathBuf {
    let saved = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hangar-recovery-")
        })
        .expect("versão deslocada retida");
    assert!(error.contains(saved.to_str().unwrap()), "{error}");
    assert_eq!(std::fs::read(&saved).unwrap(), expected);
    saved
}

#[test]
fn json_application_reports_and_retains_the_external_last_window_write() {
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("settings.json");
    std::fs::write(&target, br#"{"original":true}"#).unwrap();
    let outside = target.clone();
    let error = files::with_publication_race(
        &target,
        move || {
            std::fs::write(&outside, br#"{"external":true}"#).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o640)).unwrap();
            }
        },
        || {
            files::edit_json(&target, &home.path().join("backups"), |value| {
                value["generated"] = json!(true);
                Ok(())
            })
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "config_sync_changed");
    let saved = retained_version(home.path(), &error.message, br#"{"external":true}"#);
    #[cfg(unix)]
    assert_eq!(files::mode(&saved).unwrap(), 0o640);
    std::fs::write(&target, b"third writer").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"third writer");
    assert_eq!(std::fs::read(saved).unwrap(), br#"{"external":true}"#);
}

#[test]
fn json_application_does_not_replace_a_concurrently_created_file() {
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("settings.json");
    let outside = target.clone();
    let error = files::with_publication_race(
        &target,
        move || std::fs::write(outside, b"external creation").unwrap(),
        || {
            files::edit_json(&target, &home.path().join("backups"), |value| {
                value["generated"] = json!(true);
                Ok(())
            })
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "config_sync_changed");
    assert_eq!(std::fs::read(target).unwrap(), b"external creation");
}

#[test]
fn jev_validates_before_publication_and_retains_external_replacement() {
    use crate::accounts::environment::AccountEnvironment;
    let home = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().display().to_string()),
            ("USERPROFILE".into(), home.path().display().to_string()),
        ]
        .into(),
    );
    let config = super::runtime::RuntimeConfig::new(env).unwrap();
    std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
    std::fs::write(&config.path, br#"{"jev_api_key":"fixture-key"}"#).unwrap();
    let target = home.path().join(".claude.json");
    let original =
        br#"{"mcpServers":{"hangar-computer-control":{"command":"tool","env":{"LOCAL":"kept"}}}}"#;
    std::fs::write(&target, original).unwrap();
    let parked = home.path().join(".hangar/computer-control.json");
    std::fs::create_dir_all(parked.parent().unwrap()).unwrap();
    std::fs::write(&parked, br#"{"env":"invalid"}"#).unwrap();
    assert!(config.sync_jev().is_err());
    assert_eq!(std::fs::read(&target).unwrap(), original);
    std::fs::remove_file(parked).unwrap();
    let outside = target.clone();
    let error = files::with_publication_race(
        &target,
        move || std::fs::write(outside, br#"{"external":"preserved"}"#).unwrap(),
        || config.sync_jev(),
    )
    .unwrap_err();
    let saved = retained_version(home.path(), &error, br#"{"external":"preserved"}"#);
    std::fs::write(&target, b"third writer").unwrap();
    assert_eq!(std::fs::read(target).unwrap(), b"third writer");
    assert_eq!(
        std::fs::read(saved).unwrap(),
        br#"{"external":"preserved"}"#
    );
}
