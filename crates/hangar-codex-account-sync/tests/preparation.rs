use futures_util::future::BoxFuture;
use hangar_codex_account_sync::{ConfigPlan, Native, Paths, preparation_status, prepare};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Cli {
    now: f64,
    version: String,
    plugins: Arc<AtomicUsize>,
    pending: bool,
    fail_plugins: bool,
    change_source: Option<PathBuf>,
}
impl Default for Cli {
    fn default() -> Self {
        Self {
            now: 1000.0,
            version: "test".into(),
            plugins: Arc::new(AtomicUsize::new(0)),
            pending: false,
            fail_plugins: false,
            change_source: None,
        }
    }
}
impl Native for Cli {
    fn parse_toml(&self, raw: &str) -> Result<Value, String> {
        toml::from_str(raw).map_err(|e| e.to_string())
    }
    fn edit_config<'a>(
        &'a mut self,
        path: &'a Path,
        _backups: &'a Path,
        _work: &'a Path,
        plan: &'a mut ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut value: Value = if path.exists() {
                self.parse_toml(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)?
            } else {
                json!({})
            };
            let edits = plan(&value)?;
            if !edits.is_empty() {
                for edit in edits {
                    let key: String = serde_json::from_str(edit["keyPath"].as_str().ok_or("key")?)
                        .map_err(|e| e.to_string())?;
                    if edit["value"].is_null() {
                        value.as_object_mut().ok_or("object")?.remove(&key);
                    } else {
                        value[&key] = edit["value"].clone();
                    }
                }
                std::fs::create_dir_all(path.parent().ok_or("parent")?)
                    .map_err(|e| e.to_string())?;
                std::fs::write(path, toml::to_string(&value).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            }
            if let Some(source) = self.change_source.take() {
                std::fs::write(source, "model = 'changed'\n").map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }
    fn sync_plugins<'a>(
        &'a mut self,
        _paths: Paths<'a>,
        previous: &'a Value,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            self.plugins.fetch_add(1, Ordering::SeqCst);
            Ok(
                json!({"manifest":previous,"issues":if self.fail_plugins{vec![json!({"code":"codex_account_plugin_inventory_failed","params":{}})]}else{vec![]},"trust_pending":if self.fail_plugins{None}else{Some(self.pending)}}),
            )
        })
    }
    fn check_trust(&mut self) -> BoxFuture<'_, Result<Option<bool>, String>> {
        Box::pin(async move { Ok(Some(self.pending)) })
    }
    fn cli_version(&mut self, _force: bool) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move { Ok(self.version.clone()) })
    }
    fn now(&self) -> f64 {
        self.now
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    source: PathBuf,
    destination: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join(".codex");
        let destination = directory.path().join(".codex-work");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        Self {
            directory,
            source,
            destination,
        }
    }
    fn paths(&self) -> Paths<'_> {
        Paths {
            home: self.directory.path(),
            source: &self.source,
            destination: &self.destination,
        }
    }
    fn source(&self, name: &str, data: &str) {
        let path = self.source.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
    fn target(&self, name: &str, data: &str) {
        let path = self.destination.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
    fn config(&self, name: &str) -> Value {
        toml::from_str(&std::fs::read_to_string(self.destination.join(name)).unwrap()).unwrap()
    }
}
fn has(value: &Value, code: &str) -> bool {
    value["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == code)
}

#[tokio::test]
async fn inherits_preferences_without_copying_identity_or_runtime() {
    let f = Fixture::new();
    f.source("config.toml", "model='gpt-model'\nservice_tier='fast'\n");
    f.source("auth.json", "identity");
    f.target("auth.json", "personal");
    f.target("config.toml", "[projects.local]\ntrust_level='trusted'\n");
    let result = prepare(f.paths(), false, false, &mut Cli::default()).await;
    assert_eq!(result["status"], "ready");
    assert_eq!(f.config("config.toml")["model"], "gpt-model");
    assert_eq!(
        f.config("config.toml")["projects"]["local"]["trust_level"],
        "trusted"
    );
    assert_eq!(
        std::fs::read_to_string(f.destination.join("auth.json")).unwrap(),
        "personal"
    );
}
#[tokio::test]
async fn default_account_is_noop_and_missing_source_config_is_empty() {
    let f = Fixture::new();
    let mut cli = Cli::default();
    assert_eq!(
        prepare(f.paths(), true, false, &mut cli).await["status"],
        "ready"
    );
    assert!(!f.destination.join("config.toml").exists());
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    assert_eq!(
        f.config("config.toml")["cli_auth_credentials_store"],
        "file"
    );
}
#[tokio::test]
async fn removed_preferences_and_profiles_preserve_personal_edits() {
    let f = Fixture::new();
    f.source("config.toml", "model='first'\n");
    f.source("work.config.toml", "model='first'\n");
    let mut cli = Cli::default();
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    f.target(
        "config.toml",
        "model='personal'\ncli_auth_credentials_store='file'\n",
    );
    f.target(
        "work.config.toml",
        "model='personal-profile'\ncli_auth_credentials_store='file'\n",
    );
    f.source("config.toml", "");
    std::fs::remove_file(f.source.join("work.config.toml")).unwrap();
    let result = prepare(f.paths(), false, true, &mut cli).await;
    assert!(has(&result, "codex_account_local_change"));
    assert_eq!(f.config("config.toml")["model"], "personal");
    assert_eq!(f.config("work.config.toml")["model"], "personal-profile");
}
#[tokio::test]
async fn invalid_source_leaves_managed_destination_unchanged() {
    let f = Fixture::new();
    f.source("config.toml", "model='first'\n");
    let mut cli = Cli::default();
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    let before = std::fs::read(f.destination.join("config.toml")).unwrap();
    f.source("config.toml", "[broken");
    let result = prepare(f.paths(), false, true, &mut cli).await;
    assert_eq!(result["status"], "error");
    assert!(has(&result, "codex_account_source_invalid"));
    assert_eq!(
        std::fs::read(f.destination.join("config.toml")).unwrap(),
        before
    );
}
#[tokio::test]
async fn restriction_conflict_stays_local_after_source_removal() {
    let f = Fixture::new();
    f.source("config.toml", "forced_login_method='chatgpt'\n");
    f.target("config.toml", "forced_login_method='api'\n");
    let mut cli = Cli::default();
    let result = prepare(f.paths(), false, false, &mut cli).await;
    assert!(has(&result, "codex_account_restriction_conflict"));
    assert_eq!(f.config("config.toml")["forced_login_method"], "api");
    f.source("config.toml", "");
    prepare(f.paths(), false, true, &mut cli).await;
    assert_eq!(f.config("config.toml")["forced_login_method"], "api");
}
#[tokio::test]
async fn resources_removed_from_source_only_delete_unchanged_managed_files() {
    let f = Fixture::new();
    f.source("skills/a/SKILL.md", "managed");
    f.source("skills/b/SKILL.md", "managed");
    let mut cli = Cli::default();
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    f.target("skills/a/SKILL.md", "personal");
    std::fs::remove_file(f.source.join("skills/a/SKILL.md")).unwrap();
    std::fs::remove_file(f.source.join("skills/b/SKILL.md")).unwrap();
    let result = prepare(f.paths(), false, true, &mut cli).await;
    assert!(has(&result, "codex_account_resource_local_change"));
    assert_eq!(
        std::fs::read_to_string(f.destination.join("skills/a/SKILL.md")).unwrap(),
        "personal"
    );
    assert!(!f.destination.join("skills/b/SKILL.md").exists());
}
#[tokio::test]
async fn cache_invalidates_on_force_source_destination_version_expiry_and_clock() {
    for change in [
        "force",
        "source",
        "destination",
        "version",
        "expiry",
        "clock",
    ] {
        let f = Fixture::new();
        f.source("config.toml", "[plugins.sample]\nenabled=true\n");
        let mut cli = Cli::default();
        assert_eq!(
            prepare(f.paths(), false, false, &mut cli).await["status"],
            "ready"
        );
        cli.now += 1.0;
        assert_eq!(
            prepare(f.paths(), false, false, &mut cli).await["status"],
            "ready"
        );
        assert_eq!(cli.plugins.load(Ordering::SeqCst), 1);
        match change {
            "source" => f.source("AGENTS.md", "new"),
            "destination" => f.target("config.toml", "model='personal'\n"),
            "version" => cli.version = "new".into(),
            "expiry" => cli.now += 301.0,
            "clock" => cli.now = 0.0,
            _ => {}
        }
        prepare(f.paths(), false, change == "force", &mut cli).await;
        assert_eq!(cli.plugins.load(Ordering::SeqCst), 2, "{change}");
    }
}
#[tokio::test]
async fn pending_hook_approval_is_rechecked_without_reinstalling_plugins() {
    let f = Fixture::new();
    f.source("config.toml", "[plugins.sample]\nenabled=true\n");
    let mut cli = Cli {
        pending: true,
        ..Default::default()
    };
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["trust_pending"],
        true
    );
    cli.pending = false;
    let result = prepare(f.paths(), false, false, &mut cli).await;
    assert_eq!(result["trust_pending"], false);
    assert_eq!(cli.plugins.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn source_change_during_native_write_does_not_confirm_signature() {
    let f = Fixture::new();
    f.source("config.toml", "model='first'\n");
    let mut cli = Cli {
        change_source: Some(f.source.join("config.toml")),
        ..Default::default()
    };
    let result = prepare(f.paths(), false, false, &mut cli).await;
    assert!(has(&result, "codex_account_changed_during_prepare"));
    assert_eq!(result["status"], "error");
}
#[tokio::test]
async fn status_is_read_only_and_invalid_manifest_blocks_writes() {
    let f = Fixture::new();
    assert_eq!(preparation_status(f.paths(), false)["status"], "idle");
    assert!(!f.directory.path().join(".hangar").exists());
    let digest: String = ring::digest::digest(
        &ring::digest::SHA256,
        f.destination.to_string_lossy().as_bytes(),
    )
    .as_ref()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
    let state = f.directory.path().join(".hangar/codex-contas").join(digest);
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("estado.json"), "[]").unwrap();
    let result = prepare(f.paths(), false, false, &mut Cli::default()).await;
    assert!(has(&result, "codex_account_state_invalid"));
    assert!(!f.destination.join("config.toml").exists());
}
#[cfg(unix)]
#[tokio::test]
async fn symlink_boundaries_and_external_skill_are_preserved() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.directory.path().join("external");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("SKILL.md"), "skill").unwrap();
    std::fs::create_dir(f.source.join("skills")).unwrap();
    symlink(&outside, f.source.join("skills/external")).unwrap();
    f.source("auth.json", "secret");
    std::fs::create_dir(f.source.join("agents")).unwrap();
    symlink(
        f.source.join("auth.json"),
        f.source.join("agents/secret.md"),
    )
    .unwrap();
    symlink(&outside, f.destination.join("agents")).unwrap();
    let result = prepare(f.paths(), false, false, &mut Cli::default()).await;
    assert!(has(&result, "codex_account_source_forbidden"));
    assert_eq!(
        std::fs::read_to_string(f.destination.join("skills/external/SKILL.md")).unwrap(),
        "skill"
    );
    assert!(!outside.join("secret.md").exists());
}
#[cfg(unix)]
#[tokio::test]
async fn external_hook_keeps_relative_helpers_and_executable_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let external = f.directory.path().join("external");
    std::fs::create_dir(&external).unwrap();
    let script = external.join("hook.sh");
    std::fs::write(&script, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir(f.source.join("hooks")).unwrap();
    symlink(&script, f.source.join("hooks/hook.sh")).unwrap();
    let result = prepare(f.paths(), false, false, &mut Cli::default()).await;
    assert_eq!(result["status"], "ready");
    assert_eq!(
        std::fs::read_link(f.destination.join("hooks/hook.sh")).unwrap(),
        script
    );
}

#[tokio::test]
async fn removed_hook_definition_keeps_destination_trust_state() {
    let f = Fixture::new();
    f.source("config.toml", "[hooks]\ncommand='echo old'\n");
    let mut cli = Cli::default();
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    f.target("config.toml","cli_auth_credentials_store='file'\n[hooks]\ncommand='echo old'\n[hooks.state]\ntrusted=true\n");
    f.source("config.toml", "");
    let result = prepare(f.paths(), false, true, &mut cli).await;
    assert_eq!(result["status"], "ready");
    assert_eq!(
        f.config("config.toml")["hooks"],
        json!({"state":{"trusted":true}})
    );
}
#[tokio::test]
async fn agent_and_hook_paths_are_remapped_but_personal_collision_is_untouched() {
    let f = Fixture::new();
    f.source("agents/a.md", "instructions");
    f.source(
        "agents/a.toml",
        &format!(
            "model_instructions_file = {}\n",
            serde_json::to_string(&f.source.join("agents/a.md")).unwrap()
        ),
    );
    f.source("hooks/run.sh", "echo hook");
    f.source(
        "hooks.json",
        &json!({"command":format!("sh '{}'",f.source.join("hooks/run.sh").display())}).to_string(),
    );
    f.source(
        "config.toml",
        &format!(
            "[agents.a]\nconfig_file = {}\n",
            serde_json::to_string(&f.source.join("agents/a.toml")).unwrap()
        ),
    );
    let mut cli = Cli::default();
    let result = prepare(f.paths(), false, false, &mut cli).await;
    assert_eq!(result["status"], "ready");
    assert_eq!(
        f.config("config.toml")["agents"]["a"]["config_file"],
        f.destination
            .join("agents/a.toml")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        f.config("agents/a.toml")["model_instructions_file"],
        f.destination.join("agents/a.md").to_string_lossy().as_ref()
    );
    let hooks: Value =
        serde_json::from_slice(&std::fs::read(f.destination.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(
        hooks["command"],
        format!("sh '{}'", f.destination.join("hooks/run.sh").display())
    );
    let collision = Fixture::new();
    collision.source("agents/a.md", "source");
    collision.source("agents/a.toml", "model_instructions_file='agents/a.md'\n");
    collision.target("agents/a.toml", "model='personal'\n");
    let result = prepare(collision.paths(), false, false, &mut Cli::default()).await;
    assert!(has(&result, "codex_account_resource_conflict"));
    assert_eq!(collision.config("agents/a.toml")["model"], "personal");
}
#[tokio::test]
async fn unmapped_references_keep_prior_managed_values_and_resources() {
    let f = Fixture::new();
    f.source("agents/a.md", "instructions");
    f.source("config.toml", "model_instructions_file='agents/a.md'\n");
    f.source("hooks/run.sh", "echo hook");
    f.source(
        "hooks.json",
        &json!({"command":format!("sh '{}'",f.source.join("hooks/run.sh").display())}).to_string(),
    );
    let mut cli = Cli::default();
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    let before = std::fs::read(f.destination.join("hooks.json")).unwrap();
    f.source("private.txt", "private");
    f.source("config.toml", "model_instructions_file='private.txt'\n");
    f.source(
        "hooks.json",
        &json!({"command":format!("sh '{}'",f.source.join("private.txt").display())}).to_string(),
    );
    let result = prepare(f.paths(), false, true, &mut cli).await;
    assert!(has(&result, "codex_account_unmapped_reference"));
    assert_eq!(
        f.config("config.toml")["model_instructions_file"],
        f.destination.join("agents/a.md").to_string_lossy().as_ref()
    );
    assert_eq!(
        std::fs::read(f.destination.join("hooks.json")).unwrap(),
        before
    );
}
#[tokio::test]
async fn plugin_failure_preserves_pending_trust_until_a_valid_read() {
    let f = Fixture::new();
    f.source("config.toml", "[plugins.sample]\nenabled=true\n");
    let mut cli = Cli {
        pending: true,
        ..Default::default()
    };
    assert_eq!(
        prepare(f.paths(), false, false, &mut cli).await["status"],
        "ready"
    );
    cli.fail_plugins = true;
    let result = prepare(f.paths(), false, true, &mut cli).await;
    assert_eq!(result["trust_pending"], true);
    assert_eq!(result["status"], "partial");
    cli.fail_plugins = false;
    cli.pending = false;
    assert_eq!(
        prepare(f.paths(), false, true, &mut cli).await["trust_pending"],
        false
    );
}
#[tokio::test]
async fn concurrent_preparations_reuse_published_cache() {
    let f = Fixture::new();
    f.source("config.toml", "[plugins.sample]\nenabled=true\n");
    let shared = Arc::new(AtomicUsize::new(0));
    let mut first = Cli {
        plugins: shared.clone(),
        ..Default::default()
    };
    let mut second = Cli {
        plugins: shared.clone(),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        prepare(f.paths(), false, false, &mut first),
        prepare(f.paths(), false, false, &mut second)
    );
    assert_eq!(a["status"], "ready");
    assert_eq!(b["status"], "ready");
    assert_eq!(shared.load(Ordering::SeqCst), 1);
}
#[cfg(unix)]
#[tokio::test]
async fn broad_source_links_and_destination_config_links_are_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    symlink(f.directory.path(), f.source.join("agents")).unwrap();
    let outside = f.directory.path().join("outside.toml");
    std::fs::write(&outside, "model='personal'\n").unwrap();
    symlink(&outside, f.destination.join("config.toml")).unwrap();
    let result = prepare(f.paths(), false, false, &mut Cli::default()).await;
    assert!(has(&result, "codex_account_source_root_link"));
    assert!(has(&result, "codex_account_path_conflict"));
    assert_eq!(
        std::fs::read_to_string(outside).unwrap(),
        "model='personal'\n"
    );
    assert!(f.destination.join("config.toml").is_symlink());
}
#[tokio::test]
async fn legacy_uploads_inside_skills_are_ignored() {
    let f = Fixture::new();
    f.source("skills/test/SKILL.md", "instructions");
    f.source("skills/test/.hangar-uploads/private.txt", "private");
    let result = prepare(f.paths(), false, false, &mut Cli::default()).await;
    assert_eq!(result["status"], "ready");
    assert!(!f.destination.join("skills/test/.hangar-uploads").exists());
}
