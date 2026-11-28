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
fn shared_preferences_copy_worktree_settings_to_the_destination() {
    use hangar_workspace::worktree_settings::{KeepWhen, WorktreeSettings};
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let source_home = source.path().join("worktree-home");
    let target_home = target.path().join("worktree-home");
    let settings = WorktreeSettings {
        location: source_home.join("trees").to_string_lossy().into_owned(),
        inactive_days: Some(5),
        delete_merged: true,
        delete_unchanged: true,
        keep_when: KeepWhen::UncommittedChanges,
    };
    settings.save_at(&source_home.join(".hangar/worktree-settings.json"), &source_home).unwrap();
    let mut context = Context::new(roots(&source.path().join("account-home")));
    context.worktree_home = source_home;
    let bundle = super::export::export(&context, &["hangar_prefs".into()], None, true).unwrap();
    assert!(bundle.manifest()["items"]["hangar_prefs"]["hashes"]["worktrees"].is_string());
    let bundle = Bundle::unpack(&bundle.pack().unwrap()).unwrap();
    let mut target_context = Context::new(roots(&target.path().join("account-home")));
    target_context.worktree_home = target_home.clone();
    let previous = WorktreeSettings { inactive_days: Some(30), ..Default::default() };
    let target_file = target_home.join(".hangar/worktree-settings.json");
    previous.save_at(&target_file, &target_home).unwrap();
    let mut apply = Apply::new(target_context.clone(), bundle, "worktrees", Default::default());
    apply.apply_item("hangar_prefs").unwrap();
    let copied = WorktreeSettings::load_at(&target_file).unwrap();
    assert_eq!(copied.inactive_days, Some(5));
    assert!(copied.delete_merged);
    assert!(copied.delete_unchanged);
    assert_eq!(copied.keep_when, KeepWhen::UncommittedChanges);
    assert_eq!(
        copied.location_path(&target_home).unwrap(),
        target_home.join("trees"),
    );
    assert!(!target.path().join("account-home/.hangar/worktree-settings.json").exists());
    assert!(apply.report["hangar_prefs"]["changed"].as_array().unwrap().contains(&json!("worktrees")));
    assert_eq!(WorktreeSettings::load_at(&apply.backups.join("files/.hangar/worktree-settings.json")).unwrap(), previous);
    let selected = json!({"hangar_prefs": ["dictation_organization_mode"]});
    let filtered = super::export::export(&context, &["hangar_prefs".into()], selected.as_object(), true).unwrap();
    assert!(filtered.items["hangar_prefs"].get("worktrees").is_none());
    let mut filtered_apply = Apply::new(target_context, filtered, "filtered", Default::default());
    filtered_apply.apply_item("hangar_prefs").unwrap();
    assert_eq!(WorktreeSettings::load_at(&target_file).unwrap(), copied);
}
#[test]
fn default_worktree_settings_are_exported_and_a_rejected_copy_only_warns() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let mut context = Context::new(roots(&source.path().join("account-home")));
    context.worktree_home = source.path().join("worktree-home");
    let mut bundle = super::export::export(&context, &["hangar_prefs".into()], None, true).unwrap();
    assert!(bundle.items["hangar_prefs"]["hashes"]["worktrees"].is_string());
    bundle.items["hangar_prefs"]["worktrees"]["future_field"] = json!(true);
    let mut target_context = Context::new(roots(&target.path().join("account-home")));
    target_context.worktree_home = target.path().join("worktree-home");
    let mut apply = Apply::new(target_context, bundle, "rejected", Default::default());
    apply.apply_item("hangar_prefs").unwrap();
    assert!(!target.path().join("worktree-home/.hangar/worktree-settings.json").exists());
    let report = apply.report["hangar_prefs"].to_string();
    assert!(report.contains("config_sync_pref_rejected") && report.contains("worktrees"), "{report}");
}
#[test]
fn shared_preferences_copy_placement_rules_to_the_destination() {
    use hangar_placement::{store, Limits, PlacementConfig, UiMode};
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let source_home = source.path().join("account-home");
    let target_home = target.path().join("account-home");
    let rules = PlacementConfig {
        ui_mode: UiMode::Ask,
        limits: Limits { mem_free_pct: 25, working: 3, open: 7, cpu_pct: 90 },
        // Pasta de outra máquina: vai como está, sem virar caminho do destino.
        clone_dirs: [("vm".to_owned(), source_home.join("Projetos").to_string_lossy().into_owned())].into(),
        ..Default::default()
    };
    store::save(&source_home, &rules).unwrap();
    let bundle = super::export::export(&Context::new(roots(&source_home)), &["hangar_prefs".into()], None, true).unwrap();
    assert!(bundle.manifest()["items"]["hangar_prefs"]["hashes"]["placement"].is_string());
    let bundle = Bundle::unpack(&bundle.pack().unwrap()).unwrap();
    std::fs::create_dir_all(store::path(&target_home).parent().unwrap()).unwrap();
    std::fs::write(store::path(&target_home), b"{quebrado").unwrap();
    let mut apply = Apply::new(Context::new(roots(&target_home)), bundle, "placement", Default::default());
    apply.apply_item("hangar_prefs").unwrap();
    assert_eq!(store::load(&target_home).unwrap(), rules);
    assert!(apply.report["hangar_prefs"]["changed"].as_array().unwrap().contains(&json!("placement")));
    assert_eq!(std::fs::read(apply.backups.join("files/.hangar/placement.json")).unwrap(), b"{quebrado");
}
#[test]
fn a_rejected_placement_copy_only_warns() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let mut bundle = super::export::export(&Context::new(roots(source.path())), &["hangar_prefs".into()], None, true).unwrap();
    bundle.items["hangar_prefs"]["placement"]["schedule_rules"] =
        json!([{"from": "09:00", "to": "18:00", "tz": "Lua/Base", "avoid": "nb"}]);
    let mut apply = Apply::new(Context::new(roots(target.path())), bundle, "rejected", Default::default());
    apply.apply_item("hangar_prefs").unwrap();
    assert!(!hangar_placement::store::path(target.path()).exists());
    let report = apply.report["hangar_prefs"].to_string();
    assert!(report.contains("config_sync_pref_rejected") && report.contains("schedule_rules[0].tz"), "{report}");
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
