use futures_util::future::BoxFuture;
use hangar_codex_account_sync::{ConfigPlan, Native, Paths, prepare};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Admin {
    change_source_once: bool,
    fail_plugins_once: bool,
    fail_config_once: Option<String>,
    installed: BTreeMap<String, bool>,
}
impl Native for Admin {
    fn parse_toml(&self, raw: &str) -> Result<Value, String> {
        toml::from_str(raw).map_err(|error| error.to_string())
    }
    fn edit_config<'a>(
        &'a mut self,
        path: &'a Path,
        _backups: &'a Path,
        _work: &'a Path,
        plan: &'a mut ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if self
                .fail_config_once
                .as_ref()
                .is_some_and(|name| path.file_name().is_some_and(|file| file == name.as_str()))
            {
                self.fail_config_once.take();
                return Err("escritor nativo indisponível".into());
            }
            let mut current = match fs::read_to_string(path) {
                Ok(raw) => self.parse_toml(&raw)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
                Err(error) => return Err(error.to_string()),
            };
            for edit in plan(&current)? {
                let key: String =
                    serde_json::from_str(edit["keyPath"].as_str().ok_or("chave ausente")?)
                        .map_err(|error| error.to_string())?;
                if edit["value"].is_null() {
                    current
                        .as_object_mut()
                        .ok_or("objeto esperado")?
                        .remove(&key);
                } else {
                    current[&key] = edit["value"].clone();
                }
            }
            fs::create_dir_all(path.parent().ok_or("pasta ausente")?)
                .map_err(|error| error.to_string())?;
            fs::write(
                path,
                toml::to_string(&current).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())
        })
    }
    fn sync_plugins<'a>(
        &'a mut self,
        paths: Paths<'a>,
        previous: &'a Value,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            if self.fail_plugins_once {
                self.fail_plugins_once = false;
                return Err("gerenciador nativo indisponível".into());
            }
            let source = self.parse_toml(
                &fs::read_to_string(paths.source.join("config.toml"))
                    .map_err(|error| error.to_string())?,
            )?;
            let desired = source["plugins"].as_object().cloned().unwrap_or_default();
            let mut managed = serde_json::Map::new();
            for (id, entry) in &desired {
                let enabled = entry["enabled"]
                    .as_bool()
                    .ok_or("estado de plugin inválido")?;
                self.installed.insert(id.clone(), enabled);
                managed.insert(id.clone(),json!({"enabled":enabled,"version":"1","origem":{"type":"remote","source":"fixture"}}));
            }
            for id in previous["plugins"]
                .as_object()
                .into_iter()
                .flat_map(|items| items.keys())
            {
                if !desired.contains_key(id) {
                    self.installed.insert(id.clone(), false);
                }
            }
            if self.change_source_once {
                self.change_source_once = false;
                fs::write(paths.source.join("config.toml"), "model='second'\n")
                    .map_err(|error| error.to_string())?;
            }
            Ok(
                json!({"manifest":{"plugins":managed,"marketplaces":{}},"issues":[],"trust_pending":false}),
            )
        })
    }
    fn check_trust(&mut self) -> BoxFuture<'_, Result<Option<bool>, String>> {
        Box::pin(async { Ok(Some(false)) })
    }
    fn cli_version(&mut self, _force: bool) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Ok("fixture".into()) })
    }
    fn now(&self) -> f64 {
        1000.0
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    target: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join(".codex");
        let target = root.path().join(".codex-account");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        Self {
            _root: root,
            source,
            target,
        }
    }
    fn paths(&self) -> Paths<'_> {
        Paths {
            home: self._root.path(),
            source: &self.source,
            destination: &self.target,
        }
    }
    fn source(&self, name: &str, content: &str) {
        let path = self.source.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    fn target(&self, name: &str, content: &str) {
        let path = self.target.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    fn config(&self) -> Value {
        toml::from_str(&fs::read_to_string(self.target.join("config.toml")).unwrap()).unwrap()
    }
}
fn has(result: &Value, code: &str) -> bool {
    result["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == code)
}

#[tokio::test]
async fn changed_source_keeps_ownership_of_written_config_profiles_resources_and_plugins() {
    let f = Fixture::new();
    f.source(
        "config.toml",
        "model='first'\npersonality='shared'\n[plugins.sample]\nenabled=true\n",
    );
    f.source("work.config.toml", "model='first'\n");
    f.source("skills/shared/SKILL.md", "first");
    f.source("skills/personal/SKILL.md", "source");
    f.target("skills/personal/SKILL.md", "personal");
    let mut admin = Admin {
        change_source_once: true,
        ..Default::default()
    };
    let first = prepare(f.paths(), false, false, &mut admin).await;
    assert_eq!(first["status"], "error");
    assert!(has(&first, "codex_account_changed_during_prepare"));
    assert_eq!(admin.installed.get("sample"), Some(&true));
    assert_eq!(
        fs::read_to_string(f.target.join("skills/shared/SKILL.md")).unwrap(),
        "first"
    );

    fs::remove_file(f.source.join("work.config.toml")).unwrap();
    fs::remove_file(f.source.join("skills/personal/SKILL.md")).unwrap();
    f.source("skills/shared/SKILL.md", "second");
    let second = prepare(f.paths(), false, false, &mut admin).await;

    assert_eq!(second["status"], "ready", "{second}");
    assert!(!has(&second, "codex_account_resource_conflict"));
    assert_eq!(f.config()["model"], "second");
    assert!(f.config().get("personality").is_none());
    assert!(!f.target.join("work.config.toml").exists());
    assert_eq!(
        fs::read_to_string(f.target.join("skills/shared/SKILL.md")).unwrap(),
        "second"
    );
    assert_eq!(
        fs::read_to_string(f.target.join("skills/personal/SKILL.md")).unwrap(),
        "personal"
    );
    assert_eq!(admin.installed.get("sample"), Some(&false));
}

#[tokio::test]
async fn native_failure_after_resource_copy_keeps_ownership_for_the_next_removal() {
    let f = Fixture::new();
    f.source(
        "config.toml",
        "personality='shared'\n[plugins.sample]\nenabled=true\n",
    );
    f.source("work.config.toml", "model='first'\n");
    f.source("skills/shared/SKILL.md", "first");
    let mut admin = Admin {
        fail_plugins_once: true,
        ..Default::default()
    };
    let first = prepare(f.paths(), false, false, &mut admin).await;
    assert_eq!(first["status"], "error");
    assert!(has(&first, "codex_account_prepare_failed"));
    assert!(f.target.join("work.config.toml").is_file());
    assert!(f.target.join("skills/shared/SKILL.md").is_file());

    f.source("config.toml", "");
    fs::remove_file(f.source.join("work.config.toml")).unwrap();
    fs::remove_file(f.source.join("skills/shared/SKILL.md")).unwrap();
    let second = prepare(f.paths(), false, false, &mut admin).await;

    assert_eq!(second["status"], "ready", "{second}");
    assert!(f.config().get("personality").is_none());
    assert!(!f.target.join("work.config.toml").exists());
    assert!(!f.target.join("skills/shared/SKILL.md").exists());
}

#[tokio::test]
async fn later_profile_failure_keeps_the_manifest_of_the_earlier_written_profile() {
    let f = Fixture::new();
    f.source("config.toml", "personality='shared'\n");
    f.source("a.config.toml", "model='first'\n");
    f.source("z.config.toml", "model='last'\n");
    let mut admin = Admin {
        fail_config_once: Some("z.config.toml".into()),
        ..Default::default()
    };
    let first = prepare(f.paths(), false, false, &mut admin).await;
    assert_eq!(first["status"], "error");
    assert!(f.target.join("a.config.toml").is_file());
    assert!(!f.target.join("z.config.toml").exists());

    f.source("config.toml", "");
    fs::remove_file(f.source.join("a.config.toml")).unwrap();
    fs::remove_file(f.source.join("z.config.toml")).unwrap();
    let second = prepare(f.paths(), false, false, &mut admin).await;

    assert_eq!(second["status"], "ready", "{second}");
    assert!(f.config().get("personality").is_none());
    assert!(!f.target.join("a.config.toml").exists());
}
