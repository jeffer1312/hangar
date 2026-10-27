use super::*;
#[test]
fn op081_rejects_duplicates_and_non_objects() {
    assert_eq!(
        parse_record("{\"a\":1,\"a\":2}").unwrap_err(),
        "JSON contém chaves repetidas"
    );
    assert_eq!(
        parse_record("[]").unwrap_err(),
        "Registro deve ser um objeto"
    );
    assert_eq!(parse_record("{").unwrap_err(), "Registro JSON inválido");
}

use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
fn expand(v: &Value, home: &Path) -> Value {
    let template = home
        .parent()
        .unwrap()
        .join("templates/carregar-claude-md-do-projeto.md");
    fn convert(v: &Value, home: &str, template: &str) -> Value {
        match v {
            Value::String(s) => {
                let home_json = serde_json::to_string(home).unwrap();
                let template_json = serde_json::to_string(template).unwrap();
                json!(
                    s.replace("<HOME_JSON>", &home_json[1..home_json.len() - 1])
                        .replace(
                            "<TEMPLATE_JSON>",
                            &template_json[1..template_json.len() - 1]
                        )
                        .replace("<HOME>", home)
                        .replace("<TEMPLATE>", template)
                )
            }
            Value::Array(a) => Value::Array(a.iter().map(|v| convert(v, home, template)).collect()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), convert(v, home, template)))
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    convert(v, &home.to_string_lossy(), &template.to_string_lossy())
}
fn install(home: &Path, entries: &Value) {
    for (rel, v) in entries.as_object().unwrap() {
        let p = home.join(rel);
        if v["dir"] == true {
            fs::create_dir_all(&p).unwrap();
            continue;
        }
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        if let Some(s) = v["text"].as_str() {
            fs::write(p, s).unwrap()
        } else if let Some(s) = v["base64"].as_str() {
            fs::write(p, STANDARD.decode(s).unwrap()).unwrap()
        } else if let Some(s) = v["link"].as_str() {
            #[cfg(unix)]
            if fs::read_link(&p).ok().as_deref() != Some(Path::new(s)) {
                std::os::unix::fs::symlink(s, p).unwrap();
            }
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(s, p).unwrap();
        }
    }
}
fn tree(home: &Path) -> Value {
    fn visit(root: &Path, p: &Path, map: &mut Map<String, Value>) {
        let mut paths = fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        for p in paths {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.contains("/objects/") || rel.contains("/logs/") || rel.contains("/hooks/") {
                continue;
            }
            let m = fs::symlink_metadata(&p).unwrap();
            if m.file_type().is_symlink() {
                map.insert(
                    rel,
                    json!({"link":fs::read_link(&p).unwrap().to_string_lossy()}),
                );
            } else if m.is_dir() {
                map.insert(rel, json!({"dir":true}));
                visit(root, &p, map);
            } else {
                let bytes = fs::read(p).unwrap();
                map.insert(
                    rel,
                    match String::from_utf8(bytes.clone()) {
                        Ok(s) => json!({"text":s}),
                        Err(_) => json!({"base64":STANDARD.encode(bytes)}),
                    },
                );
            }
        }
    }
    let mut map = Map::new();
    visit(home, home, &mut map);
    Value::Object(map)
}
struct Replay {
    home: PathBuf,
    calls: Mutex<VecDeque<Value>>,
}
impl Runner for Replay {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        _env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let call = {
                let mut calls = self.calls.lock().unwrap();
                let config_get = |v: &Value| v["args"][1] == "config" && v["args"][2] == "get";
                if args.get(1).map(String::as_str) == Some("config")
                    && args.get(2).map(String::as_str) == Some("get")
                {
                    // As três leituras independentes são paralelas nos dois lados.
                    let position = calls
                        .iter()
                        .take_while(|v| config_get(v))
                        .position(|v| v["args"][3].as_str() == args.get(3).map(String::as_str))
                        .expect("consulta de configuração sem par Python no lote");
                    calls.remove(position).unwrap()
                } else {
                    calls
                        .pop_front()
                        .expect("comando extra sem equivalente Python")
                }
            };
            let mut expected = call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            if args.get(1).map(String::as_str) == Some("config")
                && args.get(2).map(String::as_str) == Some("set")
            {
                assert_eq!(
                    serde_json::from_str::<Value>(&args[4]).unwrap(),
                    serde_json::from_str::<Value>(&expected[4]).unwrap()
                );
                expected[4] = args[4].clone();
            }
            assert_eq!(args, expected);
            assert_eq!(cwd.to_string_lossy(), call["cwd"].as_str().unwrap());
            assert_eq!(timeout.as_secs_f64(), call["timeout"].as_f64().unwrap());
            for rel in call["remove"].as_array().unwrap() {
                let p = self.home.join(rel.as_str().unwrap());
                if p.is_dir() {
                    let _ = fs::remove_dir_all(p);
                } else {
                    let _ = fs::remove_file(p);
                }
            }
            install(&self.home, &call["set"]);
            if let Some(s) = call["output"]["error"].as_str() {
                return Err(s.into());
            }
            Ok(Output {
                code: call["output"]["code"].as_i64().unwrap(),
                stdout: call["output"]["stdout"].as_str().unwrap().to_owned(),
                stderr: String::new(),
            })
        })
    }
}
async fn contract_case(selected: &str) {
    let data: Value = serde_json::from_str(include_str!("contract.json")).unwrap();
    assert!(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == selected),
        "caso Python ausente: {selected}"
    );
    for case in data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] == selected)
    {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let c = expand(case, &home);
        install(&home, &c["initial"]);
        let replay = Arc::new(Replay {
            home: home.clone(),
            calls: Mutex::new(c["calls"].as_array().unwrap().iter().cloned().collect()),
        });
        let env = c["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
            .collect();
        let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, replay.clone());
        let name = c["name"].as_str().unwrap();
        let report = match c["operation"].as_str() {
            Some("context") => {
                let template = home
                    .parent()
                    .unwrap()
                    .join("templates/carregar-claude-md-do-projeto.md");
                fs::create_dir_all(template.parent().unwrap()).unwrap();
                fs::write(&template, c["template"].as_str().unwrap()).unwrap();
                context::configure(&sync, &template, c["enabled"].as_bool().unwrap()).await
            }
            Some("marketplaces") => {
                sync.import_marketplaces(name == "marketplace-dry-run", &|| false)
                    .await
            }
            _ => {
                sync.reconcile(name == "dry-run" || name == "dry-run-absent", &|| false)
                    .await
            }
        };
        assert_eq!(report, c["report"], "relatório: {name}");
        assert!(
            replay.calls.lock().unwrap().is_empty(),
            "comando Python não executado: {name}"
        );
        assert_eq!(tree(&home), c["final"], "efeito em disco: {name}");
    }
}
proptest::proptest! {
 #[test]
 fn arbitrary_json_and_origins_never_panic(s in ".{0,1024}") {let _=parse_record(&s);let _=model::identity(&s);let _=model::spec(&s);}
}

#[test]
fn op075_op076_readers_fallback_but_writers_fail() {
    let home = Path::new("/isolated");
    let env = BTreeMap::from([
        ("OMP_PROFILE".into(), "../invalid".into()),
        ("PI_CODING_AGENT_DIR".into(), "raw/../agent".into()),
    ]);
    let (p, warning) = agent_directory(home, &env, false).unwrap();
    assert_eq!(p, PathBuf::from("raw/../agent"));
    assert_eq!(
        warning.as_deref(),
        Some("perfil do omp ignorado: Nome de perfil OMP inválido")
    );
    assert_eq!(
        agent_directory(home, &env, true).unwrap_err(),
        "perfil do omp inválido: Nome de perfil OMP inválido"
    );
}
#[test]
fn op045_op046_finite_interval_and_empty_default() {
    assert_eq!(service::interval(&BTreeMap::new()).unwrap(), 300.0);
    assert_eq!(
        service::interval(&BTreeMap::from([(
            "CP_OMP_PLUGIN_SYNC_INTERVAL".into(),
            " ".into()
        )]))
        .unwrap(),
        300.0
    );
    for value in ["0", "-1", "inf", "NaN", "invalid"] {
        assert!(
            service::interval(&BTreeMap::from([(
                "CP_OMP_PLUGIN_SYNC_INTERVAL".into(),
                value.into()
            )]))
            .is_err()
        )
    }
}

#[tokio::test]
async fn op037_disabled_service_never_runs_processes() {
    let temp = tempfile::tempdir().unwrap();
    let runner = Arc::new(Replay {
        home: temp.path().into(),
        calls: Mutex::new(VecDeque::new()),
    });
    let sync = Synchronizer::new(
        temp.path().into(),
        temp.path().join(".claude"),
        BTreeMap::new(),
        runner,
    );
    let service = Service::new(sync, false, 300.0, Arc::new(|| true)).unwrap();
    service.start_loop().await;
    service.close().await;
    assert_eq!(service.status()["state"], "disabled");
    assert!(service.status()["last_report"].is_null());
}
struct BlockingRunner {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: std::sync::atomic::AtomicUsize,
}
impl Runner for BlockingRunner {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        _cwd: &'a Path,
        _env: &'a BTreeMap<String, String>,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            assert_eq!(args[1..], ["plugin", "list", "--json"]);
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(Output {
                code: 0,
                stdout: "{\"npm\":[],\"marketplace\":[]}".into(),
                stderr: String::new(),
            })
        })
    }
}
#[tokio::test]
async fn op040_close_awaits_native_operation_and_does_not_repeat() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    fs::create_dir_all(home.join(".claude/plugins")).unwrap();
    fs::write(
        home.join(".claude/plugins/installed_plugins.json"),
        "{\"version\":2,\"plugins\":{}}",
    )
    .unwrap();
    let runner = Arc::new(BlockingRunner {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        calls: 0.into(),
    });
    let sync = Synchronizer::new(
        home.into(),
        home.join(".claude"),
        BTreeMap::new(),
        runner.clone(),
    );
    let service = Service::new(sync, true, 300.0, Arc::new(|| true)).unwrap();
    service.start_loop().await;
    runner.entered.notified().await;
    assert_eq!(service.status()["state"], "running");
    let closing = tokio::spawn({
        let service = service.clone();
        async move { service.close().await }
    });
    tokio::task::yield_now().await;
    assert!(!closing.is_finished());
    runner.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runner.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(service.status()["state"], "stopped");
}
#[tokio::test(start_paused = true)]
async fn op041_master_switch_pauses_and_resumes_at_the_next_interval() {
    let temp = tempfile::tempdir().unwrap();
    let runner = Arc::new(Replay {
        home: temp.path().into(),
        calls: Mutex::new(VecDeque::new()),
    });
    let sync = Synchronizer::new(
        temp.path().into(),
        temp.path().join(".claude"),
        BTreeMap::new(),
        runner,
    );
    let allowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let service = Service::new(
        sync,
        true,
        3.0,
        Arc::new({
            let allowed = allowed.clone();
            move || allowed.load(std::sync::atomic::Ordering::SeqCst)
        }),
    )
    .unwrap();
    service.start_loop().await;
    tokio::task::yield_now().await;
    assert_eq!(service.status()["state"], "paused");
    assert!(service.status()["last_report"].is_null());
    allowed.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(3)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(service.status()["state"], "error");
    assert_eq!(
        service.status()["last_report"]["plugins"]["errors"][0],
        "Registro obrigatório ausente"
    );
    service.close().await;
}

macro_rules! python_contract {
    ($test:ident,$case:literal) => {
        #[tokio::test]
        async fn $test() {
            contract_case($case).await;
        }
    };
}
python_contract!(python_contract_fresh, "fresh");
python_contract!(python_contract_unchanged, "unchanged");
python_contract!(python_contract_adopt, "adopt");
python_contract!(python_contract_update, "update");
python_contract!(python_contract_disable, "disable");
python_contract!(python_contract_remove, "remove");
python_contract!(python_contract_manual, "manual");
python_contract!(python_contract_corrupt_ledger, "corrupt-ledger");
python_contract!(python_contract_ambiguous, "ambiguous");
python_contract!(python_contract_scope, "scope");
python_contract!(python_contract_inactive, "inactive");
python_contract!(python_contract_inactive_manual, "inactive-manual");
python_contract!(python_contract_missing_sha, "missing-sha");
python_contract!(python_contract_invalid_registry, "invalid-registry");
python_contract!(python_contract_manifest, "manifest");
python_contract!(python_contract_dry_run, "dry-run");
python_contract!(python_contract_update_fails, "update-fails");
python_contract!(python_contract_features_null, "features-null");
python_contract!(python_contract_features_empty, "features-empty");
python_contract!(python_contract_features_extra, "features-extra");
python_contract!(python_contract_native_disabled, "native-disabled");
python_contract!(python_contract_native_no_proof, "native-no-proof");
python_contract!(python_contract_collision_scheme, "collision-scheme");
python_contract!(python_contract_collision_host, "collision-host");
python_contract!(python_contract_collision_port, "collision-port");
python_contract!(python_contract_collision_path, "collision-path");
python_contract!(python_contract_collision_case, "collision-case");
python_contract!(
    python_contract_bad_native_package_json,
    "bad-native-package.json"
);
python_contract!(
    python_contract_bad_native_omp_plugins_lock_json,
    "bad-native-omp-plugins.lock.json"
);
python_contract!(
    python_contract_bad_native_installed_plugins_json,
    "bad-native-installed_plugins.json"
);
python_contract!(python_contract_bad_ledger_source_id, "bad-ledger-source.id");
python_contract!(
    python_contract_bad_ledger_source_name,
    "bad-ledger-source.name"
);
python_contract!(
    python_contract_bad_ledger_source_origin,
    "bad-ledger-source.origin"
);
python_contract!(
    python_contract_bad_ledger_source_url,
    "bad-ledger-source.url"
);
python_contract!(
    python_contract_bad_ledger_source_revision,
    "bad-ledger-source.revision"
);
python_contract!(
    python_contract_bad_ledger_source_enabled,
    "bad-ledger-source.enabled"
);
python_contract!(python_contract_bad_ledger_status, "bad-ledger-status");
python_contract!(python_contract_bad_ledger_native, "bad-ledger-native");
python_contract!(
    python_contract_bad_ledger_native_proof,
    "bad-ledger-native.proof"
);
python_contract!(
    python_contract_bad_ledger_native_enabled,
    "bad-ledger-native.enabled"
);
python_contract!(
    python_contract_bad_ledger_native_features,
    "bad-ledger-native.features"
);
python_contract!(
    python_contract_bad_ledger_native_settings,
    "bad-ledger-native.settings"
);
python_contract!(
    python_contract_bad_ledger_native_digest,
    "bad-ledger-native.digest"
);
python_contract!(
    python_contract_bad_ledger_native_path,
    "bad-ledger-native.path"
);
python_contract!(
    python_contract_bad_ledger_native_spec,
    "bad-ledger-native.spec"
);
python_contract!(python_contract_fail_list_error, "fail-list:error");
python_contract!(python_contract_fail_list_timeout, "fail-list:timeout");
python_contract!(python_contract_fail_install_error, "fail-install:error");
python_contract!(python_contract_fail_install_timeout, "fail-install:timeout");
python_contract!(
    python_contract_fail_install_no_effect,
    "fail-install:no-effect"
);
python_contract!(python_contract_context_normal, "context-normal");
python_contract!(python_contract_context_disabled, "context-disabled");
python_contract!(python_contract_context_append_file, "context-append-file");
python_contract!(python_contract_context_append_link, "context-append-link");
python_contract!(python_contract_context_equivalent, "context-equivalent");
python_contract!(
    python_contract_context_canonical_custom,
    "context-canonical-custom"
);
python_contract!(
    python_contract_context_global_missing,
    "context-global-missing"
);
python_contract!(
    python_contract_context_restricted_enabled,
    "context-restricted-enabled"
);
python_contract!(
    python_contract_context_restricted_agents,
    "context-restricted-agents"
);
python_contract!(
    python_contract_context_restricted_condition,
    "context-restricted-condition"
);
python_contract!(
    python_contract_context_restricted_always,
    "context-restricted-always"
);
python_contract!(python_contract_context_typed_bool, "context-typed-bool");
python_contract!(python_contract_context_typed_key, "context-typed-key");
python_contract!(
    python_contract_context_duplicate_key,
    "context-duplicate-key"
);
python_contract!(python_contract_context_nested, "context-nested");
python_contract!(python_contract_context_mdc, "context-mdc");
python_contract!(python_contract_context_block_id, "context-block-id");
python_contract!(
    python_contract_context_block_provider,
    "context-block-provider"
);
python_contract!(python_contract_context_block_ttsr, "context-block-ttsr");
python_contract!(python_contract_context_swap_append, "context-swap-append");
python_contract!(python_contract_context_swap_dir, "context-swap-dir");
python_contract!(
    python_contract_context_rules_symlink,
    "context-rules-symlink"
);
python_contract!(python_contract_marketplace_fresh, "marketplace-fresh");
python_contract!(
    python_contract_marketplace_unchanged,
    "marketplace-unchanged"
);
python_contract!(python_contract_marketplace_conflict, "marketplace-conflict");
python_contract!(python_contract_marketplace_dry_run, "marketplace-dry-run");
python_contract!(
    python_contract_marketplace_no_effect,
    "marketplace-no-effect"
);
python_contract!(python_contract_marketplace_secret, "marketplace-secret");

#[cfg(unix)]
#[tokio::test]
async fn op091_native_runner_deadline_reaps_the_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
    let result = NativeRunner
        .run(
            vec!["sh".into(), "-c".into(), "sleep 30 & wait".into()],
            temp.path(),
            &env,
            Duration::from_millis(100),
        )
        .await;
    assert_eq!(result.unwrap_err(), "TimeoutExpired");
    let output = NativeRunner
        .run(
            vec!["sh".into(), "-c".into(), "printf 'ação'".into()],
            temp.path(),
            &env,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "ação");
    let terminated = NativeRunner
        .run(
            vec!["sh".into(), "-c".into(), "kill -TERM $$".into()],
            temp.path(),
            &env,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(terminated.code, -15);
}
#[tokio::test]
#[ignore = "Prova com o CLI omp instalado, em HOME isolada; rode explicitamente no sandbox."]
async fn native_omp_context_and_marketplaces_in_isolated_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/CLAUDE.md"), "GLOBAL_CLAUDE_SENTINEL\n").unwrap();
    let mut env = BTreeMap::new();
    for k in ["PATH", "SYSTEMROOT", "WINDIR", "LANG"] {
        if let Ok(v) = std::env::var(k) {
            env.insert(k.into(), v);
        }
    }
    env.insert(
        "PI_CODING_AGENT_DIR".into(),
        home.join(".omp/agent").to_string_lossy().into_owned(),
    );
    for (k, p) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        env.insert(k.into(), home.join(p).to_string_lossy().into_owned());
    }
    let sync = Synchronizer::new(
        home.clone(),
        home.join(".claude"),
        env,
        Arc::new(NativeRunner),
    );
    let template = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/omp/carregar-claude-md-do-projeto.md");
    let result = configure_claude_context(&sync, &template, true).await;
    assert_eq!(result["errors"], json!([]), "{result}");
    assert_eq!(
        fs::canonicalize(home.join(".omp/agent/APPEND_SYSTEM.md")).unwrap(),
        home.join(".claude/CLAUDE.md")
    );
    let before = fs::read(home.join(".omp/agent/config.yml")).unwrap();
    assert_eq!(
        configure_claude_context(&sync, &template, true).await["errors"],
        json!([])
    );
    assert_eq!(
        fs::read(home.join(".omp/agent/config.yml")).unwrap(),
        before
    );
    let project = home.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("CLAUDE.md"), "PROJECT_CLAUDE_SENTINEL\n").unwrap();
    for file in [project.join("AGENTS.md"), home.join(".omp/agent/AGENTS.md")] {
        fs::write(file, "FORBIDDEN_AGENTS_SENTINEL\n").unwrap();
    }
    #[cfg(unix)]
    if let Some(real) = std::env::home_dir()
        .map(|h| h.join(".omp/natives"))
        .filter(|p| p.is_dir())
    {
        for path in [home.join(".omp/natives"), home.join(".cache/omp/natives")] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if !path.exists() {
                std::os::unix::fs::symlink(&real, path).unwrap();
            }
        }
    }
    let result_path = home.join("context-proof.json");
    let mut driver_env = sync.env.clone();
    driver_env.insert(
        "OMP_DRIVER_RESULT".into(),
        result_path.to_string_lossy().into_owned(),
    );
    driver_env.insert("OMP_DRIVER_RUN".into(), "omp-context-proof".into());
    driver_env.insert("EXPECT_PROJECT".into(), "1".into());
    let driver =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/tests/omp-context-driver.ts");
    let output = NativeRunner
        .run(
            vec![
                "omp".into(),
                "--no-extensions".into(),
                "--no-skills".into(),
                "--no-session".into(),
                "--print".into(),
                "--model".into(),
                "openai-codex/gpt-6-astra".into(),
                "--api-key".into(),
                "fixture-sem-credencial".into(),
                "--extension".into(),
                driver.to_string_lossy().into_owned(),
            ],
            &project,
            &driver_env,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(output.code, 0);
    let proof = io::read(&result_path, None).unwrap();
    assert_eq!(proof["run"], "omp-context-proof");
    assert_eq!(proof["ok"], true, "{proof}");
    let catalog = home.join("catalog");
    fs::create_dir_all(catalog.join(".claude-plugin")).unwrap();
    fs::write(
        catalog.join(".claude-plugin/marketplace.json"),
        json!({"name":"catalog-fixture","owner":{"name":"Fixture"},"plugins":[]}).to_string(),
    )
    .unwrap();
    fs::create_dir_all(home.join(".claude/plugins")).unwrap();
    fs::write(home.join(".claude/plugins/known_marketplaces.json"),json!({"fixture":{"source":{"source":"directory","path":catalog},"installLocation":catalog}}).to_string()).unwrap();
    let report = sync.import_marketplaces(false, &|| false).await;
    assert_eq!(report["errors"], json!([]), "{report}");
    let registry = io::read(&home.join(".omp/marketplaces.json"), None).unwrap();
    assert_eq!(registry["marketplaces"][0]["name"], "catalog-fixture");
    assert!(!home.join(".omp/plugins/package.json").exists());
    assert_eq!(
        sync.import_marketplaces(false, &|| false).await["items"][0]["action"],
        "unchanged"
    );
}

struct GateReplay {
    replay: Replay,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    installs: std::sync::atomic::AtomicUsize,
    stop_after: Option<Arc<std::sync::atomic::AtomicBool>>,
}
impl Runner for GateReplay {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let installing = args.get(2).is_some_and(|s| s == "install");
            if installing {
                self.installs
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if self.stop_after.is_none() {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
            }
            let out = self.replay.run(args, cwd, env, timeout).await?;
            if installing {
                if let Some(stop) = &self.stop_after {
                    stop.store(true, std::sync::atomic::Ordering::SeqCst)
                }
            }
            Ok(out)
        })
    }
}
fn fresh_contract(home: &Path) -> Value {
    let data: Value = serde_json::from_str(include_str!("contract.json")).unwrap();
    expand(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "fresh")
            .unwrap(),
        home,
    )
}
#[tokio::test]
async fn op024_two_concurrent_writers_install_once() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    install(&home, &c["initial"]);
    let mut calls = c["calls"].as_array().unwrap().clone();
    let last = calls.last().unwrap().clone();
    calls.push(calls[0].clone());
    calls.push(last);
    let runner = Arc::new(GateReplay {
        replay: Replay {
            home: home.clone(),
            calls: Mutex::new(calls.into()),
        },
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        installs: 0.into(),
        stop_after: None,
    });
    let env = c["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let first = tokio::spawn({
        let sync = sync.clone();
        async move { sync.reconcile(false, &|| false).await }
    });
    runner.entered.notified().await;
    let second = tokio::spawn({
        let sync = sync.clone();
        async move { sync.reconcile(false, &|| false).await }
    });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    runner.release.notify_one();
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    assert_eq!(a["errors"], json!([]));
    assert_eq!(b["errors"], json!([]));
    assert_eq!(runner.installs.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(runner.replay.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn op021_interrupted_install_never_authorizes_removal() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    install(&home, &c["initial"]);
    let mut calls = c["calls"].as_array().unwrap().clone();
    let final_listing = calls.pop().unwrap();
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runner = Arc::new(GateReplay {
        replay: Replay {
            home: home.clone(),
            calls: Mutex::new(calls.into()),
        },
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        installs: 0.into(),
        stop_after: Some(stopped.clone()),
    });
    let env = c["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let report = sync
        .reconcile(false, &|| stopped.load(std::sync::atomic::Ordering::SeqCst))
        .await;
    assert_eq!(report["errors"], json!([]));
    let ledger = io::read(&sync.ledger(), None).unwrap();
    assert_eq!(ledger["items"]["@fixture/comet"]["status"], "suspended");
    assert!(runner.replay.calls.lock().unwrap().is_empty());
    fs::write(
        home.join(".claude/plugins/installed_plugins.json"),
        "{\"version\":2,\"plugins\":{}}",
    )
    .unwrap();
    runner.replay.calls.lock().unwrap().push_back(final_listing);
    let report = sync.reconcile(false, &|| false).await;
    assert_eq!(report["errors"], json!([]));
    assert_eq!(report["items"][0]["action"], "suspended");
    assert!(
        home.join(".omp/plugins/node_modules/@fixture/comet")
            .is_dir()
    );
    assert!(runner.replay.calls.lock().unwrap().is_empty());
}

python_contract!(python_contract_equivalent, "equivalent");

python_contract!(python_contract_subdir, "subdir");

python_contract!(python_contract_dry_run_absent, "dry-run-absent");

python_contract!(python_contract_manual_enabled, "manual-enabled");

python_contract!(python_contract_manual_features, "manual-features");

python_contract!(python_contract_manual_removed, "manual-removed");
