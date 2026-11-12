use super::*;
use std::sync::Mutex;

#[derive(Default, Clone)]
struct State {
    plugins: Vec<Value>,
    markets: Vec<Value>,
}
struct Fake {
    source: PathBuf,
    target: PathBuf,
    states: Mutex<BTreeMap<PathBuf, State>>,
    installs: Mutex<Vec<String>>,
    orphan: Mutex<bool>,
    fail_install: bool,
    failed_plugin_reads: Mutex<std::collections::VecDeque<bool>>,
    fail_config: bool,
    trust: Option<bool>,
}
impl Fake {
    fn new(source: &Path, target: &Path) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            states: Mutex::new(BTreeMap::from([
                (source.into(), State::default()),
                (target.into(), State::default()),
            ])),
            installs: Mutex::new(vec![]),
            orphan: Mutex::new(false),
            fail_install: false,
            failed_plugin_reads: Mutex::new(std::collections::VecDeque::new()),
            fail_config: false,
            trust: Some(false),
        }
    }
    fn set(&self, path: &Path, plugins: Vec<Value>, markets: Vec<Value>) {
        self.states
            .lock()
            .unwrap()
            .insert(path.into(), State { plugins, markets });
    }
    fn target(&self) -> State {
        self.states.lock().unwrap()[&self.target].clone()
    }
}
impl PluginsNative for Fake {
    fn inventory<'a>(&'a self, account: &'a Path) -> BoxFuture<'a, Result<Inventory, NativeError>> {
        Box::pin(async move {
            let s = self.states.lock().unwrap()[account].clone();
            Ok(Inventory {
                plugins: s.plugins,
                marketplaces: s.markets,
            })
        })
    }
    fn plugins<'a>(&'a self, account: &'a Path) -> BoxFuture<'a, Result<Vec<Value>, NativeError>> {
        Box::pin(async move {
            if self
                .failed_plugin_reads
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(false)
            {
                return Err(NativeError {
                    kind: "CodexNativoErro".into(),
                    stderr: String::new(),
                });
            }
            Ok(self.states.lock().unwrap()[account].plugins.clone())
        })
    }
    fn marketplaces<'a>(
        &'a self,
        account: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<Value>, NativeError>> {
        Box::pin(async move { Ok(self.states.lock().unwrap()[account].markets.clone()) })
    }
    fn cli<'a>(
        &'a self,
        account: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, Result<Value, NativeError>> {
        Box::pin(async move {
            if args[2] == "remove" {
                *self.orphan.lock().unwrap() = false;
                return Ok(json!({}));
            }
            if *self.orphan.lock().unwrap() {
                return Err(NativeError {
                    kind: "CodexNativoErro".into(),
                    stderr: "is already added from a different source".into(),
                });
            }
            let mut states = self.states.lock().unwrap();
            let source = states[&self.source]
                .markets
                .iter()
                .find(|m| m["marketplaceSource"]["source"] == args[3])
                .unwrap()
                .clone();
            states.get_mut(account).unwrap().markets.push(source);
            Ok(json!({}))
        })
    }
    fn install<'a>(
        &'a self,
        account: &'a Path,
        id: &'a str,
    ) -> BoxFuture<'a, Result<Value, NativeError>> {
        Box::pin(async move {
            if self.fail_install {
                return Err(NativeError {
                    kind: "RuntimeError".into(),
                    stderr: "".into(),
                });
            }
            self.installs.lock().unwrap().push(id.into());
            let mut states = self.states.lock().unwrap();
            let mut entry = states[&self.source]
                .plugins
                .iter()
                .find(|p| p["pluginId"] == id)
                .unwrap()
                .clone();
            if let Some(market) = states[account]
                .markets
                .iter()
                .find(|m| m["name"] == market_name(id, &entry))
            {
                entry["marketplaceSource"] = market["marketplaceSource"].clone();
            }
            let target = states.get_mut(account).unwrap();
            target.plugins.retain(|p| p["pluginId"] != id);
            target.plugins.push(entry);
            Ok(json!({"installedPath":account.join("plugins/cache/market/sample/1")}))
        })
    }
    fn parse_toml(&self, text: &str) -> Result<Value, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
    fn edit_config<'a>(
        &'a self,
        path: &'a Path,
        _backups: &'a Path,
        _work: &'a Path,
        plan: &'a mut ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if self.fail_config {
                return Err("Escrita recusada pelo Codex".into());
            }
            let mut config = std::fs::read_to_string(path)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(json!({}));
            let edits = plan(&config)?;
            for edit in edits {
                let key: Vec<String> = serde_json::from_str(&format!(
                    "[{}]",
                    edit["keyPath"].as_str().unwrap().replace("\".\"", "\",\"")
                ))
                .unwrap();
                config[&key[0]][&key[1]][&key[2]] = edit["value"].clone();
                let mut states = self.states.lock().unwrap();
                let state = states.get_mut(path.parent().unwrap()).unwrap();
                if key[0] == "plugins" {
                    if let Some(p) = state.plugins.iter_mut().find(|p| p["pluginId"] == key[1]) {
                        p["enabled"] = edit["value"].clone();
                    }
                }
                if key[0] == "marketplaces" {
                    let item = json!({"name":key[1],"root":config["marketplaces"][&key[1]]["source"],"marketplaceSource":{"sourceType":config["marketplaces"][&key[1]]["source_type"],"source":config["marketplaces"][&key[1]]["source"]}});
                    state.markets.retain(|m| m["name"] != key[1]);
                    state.markets.push(item);
                }
            }
            std::fs::write(path, serde_json::to_vec(&config).unwrap())
                .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
    fn adapt_hooks<'a>(
        &'a self,
        _account: &'a Path,
        _root: &'a Path,
        _backups: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn trust<'a>(
        &'a self,
        _account: &'a Path,
        approve: bool,
    ) -> BoxFuture<'a, Result<Option<bool>, String>> {
        Box::pin(async move {
            assert!(approve);
            Ok(self.trust)
        })
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    source: PathBuf,
    target: PathBuf,
    market: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // O manifesto real guarda caminhos resolvidos, mesmo com TEMP em um alias Windows.
        let root = hangar_core::fs::without_verbatim(std::fs::canonicalize(dir.path()).unwrap());
        let source = root.join("source");
        let target = root.join("target");
        let market = root.join("market");
        for p in [&source, &target, &market] {
            std::fs::create_dir_all(p).unwrap();
        }
        Self {
            dir,
            source,
            target,
            market,
        }
    }
    fn paths(&self) -> Paths<'_> {
        Paths {
            home: self.dir.path(),
            source: &self.source,
            destination: &self.target,
        }
    }
    fn fake(&self) -> Fake {
        Fake::new(&self.source, &self.target)
    }
    fn plugin(&self) -> Value {
        json!({"pluginId":"sample@market","name":"sample","marketplaceName":"market","version":"1.0.0","enabled":true,"marketplaceSource":{"sourceType":"local","source":self.market}})
    }
    fn marketplace(&self) -> Value {
        json!({"name":"market","root":self.market,"marketplaceSource":{"sourceType":"local","source":self.market}})
    }
    fn prepare(&self) -> Fake {
        let fake = self.fake();
        fake.set(&self.source, vec![self.plugin()], vec![self.marketplace()]);
        fake
    }
}
fn has(result: &Value, code: &str) -> bool {
    result["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == code)
}

#[tokio::test]
async fn installs_only_source_plugins_and_preserves_target_data() {
    let f = Fixture::new();
    let fake = f.prepare();
    let mut plugin = f.plugin();
    plugin["enabled"] = json!(false);
    fake.set(&f.source, vec![plugin], vec![f.marketplace()]);
    let mut exclusive = f.plugin();
    exclusive["pluginId"] = json!("exclusive@other");
    fake.set(&f.target, vec![exclusive], vec![]);
    std::fs::create_dir_all(f.target.join("plugins/data")).unwrap();
    std::fs::write(f.target.join("plugins/data/sentinel"), "destino").unwrap();
    let first = sync(f.paths(), &json!({}), &fake).await;
    assert_eq!(first["issues"], json!([]));
    assert!(
        fake.target()
            .plugins
            .iter()
            .any(|p| p["pluginId"] == "exclusive@other" && p["enabled"] == true)
    );
    assert!(
        fake.target()
            .plugins
            .iter()
            .any(|p| p["pluginId"] == "sample@market" && p["enabled"] == false)
    );
    let second = sync(f.paths(), &first["manifest"], &fake).await;
    assert_eq!(second["issues"], json!([]));
    assert_eq!(*fake.installs.lock().unwrap(), vec!["sample@market"]);
    assert_eq!(
        std::fs::read_to_string(f.target.join("plugins/data/sentinel")).unwrap(),
        "destino"
    );
}
#[tokio::test]
async fn repairs_only_an_undeclared_orphan_marketplace() {
    let f = Fixture::new();
    let fake = f.prepare();
    *fake.orphan.lock().unwrap() = true;
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert_eq!(result["issues"], json!([]));
    assert!(!*fake.orphan.lock().unwrap());
}
#[tokio::test]
async fn declared_marketplace_is_never_removed_even_when_inventory_omits_it() {
    let f = Fixture::new();
    let fake = f.prepare();
    *fake.orphan.lock().unwrap() = true;
    std::fs::write(f.target.join("config.toml"),r#"{"marketplaces":{"market":{"source_type":"git","source":"https://example.test/other"}}}"#).unwrap();
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert!(has(&result, "codex_account_plugin_marketplace_add_failed"));
    assert!(*fake.orphan.lock().unwrap());
    assert!(fake.installs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn remote_plugin_does_not_require_a_marketplace() {
    let f = Fixture::new();
    let fake = f.fake();
    let p = json!({"pluginId":"management@remote","version":"0.1.0","enabled":true,"source":{"source":"remote","id":"connector-example"}});
    fake.set(&f.source, vec![p.clone()], vec![]);
    fake.set(&f.target, vec![p], vec![]);
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert_eq!(result["issues"], json!([]));
    assert_eq!(
        result["manifest"]["plugins"]["management@remote"]["origem"],
        json!({"type":"remote","source":"connector-example"})
    );
    assert!(fake.installs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn same_name_with_different_origin_keeps_target() {
    let f = Fixture::new();
    let fake = f.prepare();
    let mut p = f.plugin();
    p["version"] = json!("9.0.0");
    p["marketplaceSource"]["source"] = json!(f.dir.path().join("other"));
    let mut m = f.marketplace();
    m["marketplaceSource"] = p["marketplaceSource"].clone();
    fake.set(&f.target, vec![p], vec![m]);
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert!(has(&result, "codex_account_plugin_origin_conflict"));
    assert_eq!(fake.target().plugins[0]["version"], "9.0.0");
    assert!(fake.installs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn removed_source_disables_only_same_origin_managed_plugin() {
    let f = Fixture::new();
    let fake = f.fake();
    fake.set(&f.target, vec![f.plugin()], vec![f.marketplace()]);
    let path = f.market.to_string_lossy().into_owned();
    let path = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    };
    let old = json!({"plugins":{"sample@market":{"origem":{"type":"local","source":path}}}});
    let result = sync(f.paths(), &old, &fake).await;
    assert_eq!(result["issues"], json!([]));
    assert_eq!(fake.target().plugins[0]["enabled"], false);
}
#[tokio::test]
async fn removed_plugin_with_replaced_origin_is_preserved() {
    let f = Fixture::new();
    let fake = f.fake();
    fake.set(&f.target, vec![f.plugin()], vec![]);
    let old =
        json!({"plugins":{"sample@market":{"origem":{"type":"git","source":"example.test/old"}}}});
    let result = sync(f.paths(), &old, &fake).await;
    assert!(has(&result, "codex_account_plugin_origin_conflict"));
    assert_eq!(fake.target().plugins[0]["enabled"], true);
}

async fn removal_retries_after_failure(
    failed_reads: &[bool],
    fail_config: bool,
    remains_enabled: bool,
) {
    let f = Fixture::new();
    let mut fake = f.fake();
    let plugin = f.plugin();
    let mut personal = plugin.clone();
    personal["pluginId"] = json!("personal@market");
    personal["name"] = json!("personal");
    fake.set(&f.target, vec![plugin, personal], vec![f.marketplace()]);
    let path = f.market.to_string_lossy().into_owned();
    let path = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    };
    let old = json!({"plugins":{"sample@market":{"version":"1.0.0","enabled":true,"origem":{"type":"local","source":path}}}});
    *fake.failed_plugin_reads.lock().unwrap() = failed_reads.iter().copied().collect();
    fake.fail_config = fail_config;
    let failed = sync(f.paths(), &old, &fake).await;
    assert_eq!(
        failed["manifest"]["plugins"]["sample@market"]["origem"],
        old["plugins"]["sample@market"]["origem"],
        "A remoção ainda não confirmada precisa conservar a identidade gerenciada"
    );
    assert_eq!(
        fake.target()
            .plugins
            .iter()
            .find(|p| p["pluginId"] == "sample@market")
            .unwrap()["enabled"],
        remains_enabled
    );
    if fail_config {
        assert!(has(&failed, "codex_account_plugin_config_failed"));
    } else if failed_reads.first() == Some(&true) {
        assert!(has(&failed, "codex_account_plugin_inventory_failed"));
    }
    fake.fail_config = false;
    let retried = sync(f.paths(), &failed["manifest"], &fake).await;
    assert_eq!(retried["issues"], json!([]));
    assert_eq!(
        fake.target()
            .plugins
            .iter()
            .find(|p| p["pluginId"] == "sample@market")
            .unwrap()["enabled"],
        false
    );
    assert!(
        retried["manifest"]["plugins"]
            .get("sample@market")
            .is_none()
    );
    assert_eq!(
        fake.target()
            .plugins
            .iter()
            .find(|p| p["pluginId"] == "personal@market")
            .unwrap()["enabled"],
        true
    );
    assert!(
        retried["manifest"]["plugins"]
            .get("personal@market")
            .is_none()
    );
}

#[tokio::test]
async fn removed_plugin_inventory_failure_preserves_manifest_until_retry() {
    removal_retries_after_failure(&[true, false], false, true).await;
}

#[tokio::test]
async fn removed_plugin_disable_failure_preserves_manifest_until_retry() {
    removal_retries_after_failure(&[], true, true).await;
}

#[tokio::test]
async fn removed_plugin_confirmation_failure_preserves_manifest_until_retry() {
    removal_retries_after_failure(&[false, true], false, false).await;
}
#[tokio::test]
async fn failed_install_preserves_previous_manifest() {
    let f = Fixture::new();
    let mut fake = f.prepare();
    fake.fail_install = true;
    let old = json!({"plugins":{"sample@market":{"version":"0.9.0","enabled":true}}});
    let result = sync(f.paths(), &old, &fake).await;
    assert!(has(&result, "codex_account_plugin_install_failed"));
    assert_eq!(
        result["manifest"]["plugins"]["sample@market"]["version"],
        "0.9.0"
    );
}
#[tokio::test]
async fn malformed_source_keeps_previous_manifest_and_target() {
    let f = Fixture::new();
    let fake = f.prepare();
    let mut p = f.plugin();
    p["marketplaceSource"] = json!({"sourceType":"local"});
    fake.set(&f.source, vec![p], vec![]);
    fake.set(&f.target, vec![f.plugin()], vec![]);
    let old = json!({"plugins":{"sample@market":{"version":"0.9.0"}}});
    let result = sync(f.paths(), &old, &fake).await;
    assert!(has(&result, "codex_account_plugin_origin_conflict"));
    assert_eq!(
        result["manifest"]["plugins"]["sample@market"]["version"],
        "0.9.0"
    );
    assert!(fake.installs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn absent_enabled_refuses_inventory_without_reactivating_plugin() {
    let f = Fixture::new();
    let fake = f.prepare();
    let mut p = f.plugin();
    p.as_object_mut().unwrap().remove("enabled");
    fake.set(&f.source, vec![p], vec![]);
    let mut p = f.plugin();
    p["enabled"] = json!(false);
    fake.set(&f.target, vec![p], vec![]);
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert!(has(&result, "codex_account_plugin_inventory_failed"));
    assert_eq!(fake.target().plugins[0]["enabled"], false);
    assert!(fake.installs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn divergent_version_is_installed_and_verified_again() {
    let f = Fixture::new();
    let fake = f.prepare();
    let mut p = f.plugin();
    p["version"] = json!("0.9.0");
    fake.set(&f.target, vec![p], vec![f.marketplace()]);
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert_eq!(result["issues"], json!([]));
    assert_eq!(fake.target().plugins[0]["version"], "1.0.0");
    assert_eq!(*fake.installs.lock().unwrap(), vec!["sample@market"]);
}
#[tokio::test]
async fn trust_result_is_current_and_unknown_is_not_success() {
    let f = Fixture::new();
    let mut fake = f.fake();
    fake.trust = Some(true);
    assert_eq!(
        sync(f.paths(), &json!({}), &fake).await["trust_pending"],
        true
    );
    fake.trust = Some(false);
    assert_eq!(
        sync(f.paths(), &json!({}), &fake).await["trust_pending"],
        false
    );
    fake.trust = None;
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert!(result["trust_pending"].is_null());
    assert!(has(&result, "codex_account_plugin_trust_unavailable"));
}
#[tokio::test]
async fn bundled_marketplace_materializes_only_installed_plugin() {
    let f = Fixture::new();
    let fake = f.fake();
    let root = f.source.join(".tmp/bundled-marketplaces/market");
    std::fs::create_dir_all(root.join(".agents/plugins")).unwrap();
    std::fs::write(root.join(".agents/plugins/marketplace.json"), "{}").unwrap();
    for name in ["sample", "other"] {
        std::fs::create_dir_all(root.join(format!("plugins/{name}/.codex-plugin"))).unwrap();
        std::fs::write(
            root.join(format!("plugins/{name}/.codex-plugin/plugin.json")),
            r#"{"version":"1.0.0"}"#,
        )
        .unwrap();
    }
    let mut p = f.plugin();
    p["marketplaceSource"]["source"] = json!(root);
    let mut m = f.marketplace();
    m["root"] = json!(root);
    m["marketplaceSource"] = p["marketplaceSource"].clone();
    fake.set(&f.source, vec![p], vec![m]);
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert_eq!(result["issues"], json!([]));
    let target = f.target.join(".tmp/bundled-marketplaces/market");
    assert!(
        target
            .join("plugins/sample/.codex-plugin/plugin.json")
            .is_file()
    );
    assert!(!target.join("plugins/other").exists());
    assert_eq!(
        result["manifest"]["plugins"]["sample@market"]["origem"],
        json!({"type":"bundled","source":"market"})
    );
}
#[tokio::test]
async fn bundled_identity_and_legacy_manifest_survive_checkout_changes() {
    let f = Fixture::new();
    let fake = f.fake();
    let source = f.source.join(".tmp/bundled-marketplaces/market");
    let target = f.target.join(".tmp/bundled-marketplaces/market");
    let mut p = f.plugin();
    p["marketplaceSource"]["source"] = json!(source);
    let mut m = f.marketplace();
    m["root"] = json!(source);
    m["marketplaceSource"] = p["marketplaceSource"].clone();
    fake.set(&f.source, vec![p.clone()], vec![m.clone()]);
    p["marketplaceSource"]["source"] = json!(target);
    m["root"] = json!(target);
    m["marketplaceSource"] = p["marketplaceSource"].clone();
    fake.set(&f.target, vec![p.clone()], vec![m]);
    let result = sync(f.paths(), &json!({}), &fake).await;
    assert_eq!(result["issues"], json!([]));
    assert_eq!(
        result["manifest"]["marketplaces"]["market"]["marketplaceSource"],
        json!({"sourceType":"bundled","name":"market"})
    );
    fake.set(&f.source, vec![], vec![]);
    let old = json!({"plugins":{"sample@market":{"marketplace":"market","origem":{"type":"local","source":source}}}});
    let result = sync(f.paths(), &old, &fake).await;
    assert_eq!(result["issues"], json!([]));
    assert_eq!(fake.target().plugins[0]["enabled"], false);
}

fn external_json() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,48}".prop_map(Value::String)
    ]
    .prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::btree_map(".{0,24}", inner, 0..6)
                .prop_map(|map| Value::Object(map.into_iter().collect()))
        ]
    })
}
proptest::proptest! {
    #[test]
    fn external_plugin_inventory_and_manifest_never_panic(entry in external_json(), previous in external_json()) {
        let f=Fixture::new();let fake=f.fake();fake.set(&f.source,vec![entry],vec![]);
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result=runtime.block_on(sync(f.paths(),&previous,&fake));
        proptest::prop_assert!(result["manifest"].is_object());
        proptest::prop_assert!(result["issues"].is_array());
    }
}

#[cfg(windows)]
#[tokio::test]
async fn removed_plugin_from_python_manifest_matches_windows_native_local_origin() {
    for legacy in [true, false] {
        let f = Fixture::new();
        let native_path = std::fs::canonicalize(&f.market).unwrap();
        let persisted = if legacy {
            hangar_core::fs::without_verbatim(native_path.clone())
                .to_string_lossy()
                .to_lowercase()
        } else {
            native_path.to_string_lossy().to_uppercase()
        };
        let fake = f.fake();
        let mut plugin = f.plugin();
        plugin["marketplaceSource"]["source"] = json!(native_path);
        fake.set(&f.target, vec![plugin], vec![f.marketplace()]);
        let previous =
            json!({"plugins":{"sample@market":{"origem":{"type":"local","source":persisted}}}});

        let result = sync(f.paths(), &previous, &fake).await;

        assert!(!has(&result, "codex_account_plugin_origin_conflict"));
        assert_eq!(
            fake.target().plugins[0]["enabled"],
            false,
            "legado={legacy}"
        );
        assert!(fake.installs.lock().unwrap().is_empty());
    }
}
