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
#[cfg(windows)]
const PYTHON_CONTRACT: &str = include_str!("contract-windows.json");
#[cfg(not(windows))]
const PYTHON_CONTRACT: &str = include_str!("contract.json");

#[cfg(windows)]
#[tokio::test]
async fn op091_native_runner_resolves_mixed_case_path_and_pathext() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::copy(std::env::current_exe().unwrap(), bin.join("omp.exe")).unwrap();
    let env = BTreeMap::from([
        ("Path".into(), bin.to_string_lossy().into_owned()),
        ("PathExt".into(), ".COM;.EXE;.BAT;.CMD".into()),
        ("SYSTEMROOT".into(), std::env::var("SYSTEMROOT").unwrap()),
    ]);
    let output = NativeRunner
        .run(
            vec!["omp".into(), "config".into(), "--list".into()],
            temp.path(),
            &env,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(output.stdout.contains("test"), "{}", output.stdout);
}
fn expand(v: &Value, home: &Path) -> Value {
    let template = home
        .parent()
        .unwrap()
        .join("templates")
        .join("carregar-claude-md-do-projeto.md");
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
            if fs::read_link(&p)
                .ok()
                .map(crate::list::links::without_verbatim)
                .as_deref()
                != Some(Path::new(s))
            {
                let is_dir = Path::new(s).is_dir()
                    || Path::new(s).strip_prefix(home).ok().is_some_and(|p| {
                        entries[p.to_string_lossy().replace('\\', "/")]["dir"] == true
                    });
                if is_dir {
                    std::os::windows::fs::symlink_dir(s, p).unwrap();
                } else {
                    std::os::windows::fs::symlink_file(s, p).unwrap();
                }
            }
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
                let target = fs::read_link(&p).unwrap();
                #[cfg(windows)]
                let target = crate::list::links::without_verbatim(target)
                    .components()
                    .collect::<PathBuf>();
                map.insert(rel, json!({"link":target.to_string_lossy()}));
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
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let git = [
                ("GIT_CONFIG_NOSYSTEM", "1"),
                (
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "nul" } else { "/dev/null" },
                ),
                ("GIT_TERMINAL_PROMPT", "0"),
                ("GIT_OPTIONAL_LOCKS", "0"),
            ];
            assert!(
                env.keys()
                    .filter(|key| key.starts_with("GIT_"))
                    .all(|key| git.iter().any(|(expected, _)| key == expected)),
                "o replay recebeu variável Git herdada do host"
            );
            for (key, value) in git {
                assert!(
                    env.get(key).is_some_and(|actual| actual == value),
                    "o replay recebeu configuração Git divergente em {key}"
                );
            }
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
    let data: Value = serde_json::from_str(PYTHON_CONTRACT).unwrap();
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
        let expected = c["final"].clone();
        #[cfg(windows)]
        let expected = {
            let mut expected = expected;
            for entry in expected.as_object_mut().unwrap().values_mut() {
                if let Some(target) = entry["link"].as_str() {
                    entry["link"] = json!(
                        crate::list::links::without_verbatim(PathBuf::from(target))
                            .components()
                            .collect::<PathBuf>()
                            .to_string_lossy()
                    );
                }
            }
            expected
        };
        assert_eq!(tree(&home), expected, "efeito em disco: {name}");
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
    let service = Service::new(sync, false, 300.0, Arc::new(|| Ok(true))).unwrap();
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
    let service = Service::new(sync, true, 300.0, Arc::new(|| Ok(true))).unwrap();
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
            move || Ok(allowed.load(std::sync::atomic::Ordering::SeqCst))
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
    let data: Value = serde_json::from_str(PYTHON_CONTRACT).unwrap();
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

struct StopReplay {
    replay: Replay,
    stops: Mutex<VecDeque<bool>>,
    stopped: std::sync::atomic::AtomicBool,
}
impl Runner for StopReplay {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let stop = self.stops.lock().unwrap().pop_front().unwrap();
            let result = self.replay.run(args, cwd, env, timeout).await;
            if stop {
                self.stopped
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            result
        })
    }
}
async fn original_assertions(id: &str) {
    let data: Value = serde_json::from_str(include_str!("assertion-contract.json")).unwrap();
    let mut steps = 0;
    for case in data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["id"] == id)
    {
        assert_eq!(case["assertions_passed"], true);
        for step in case["steps"].as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            fs::create_dir(&home).unwrap();
            let c = expand(step, &home);
            install(&home, &c["initial"]);
            let calls = c["calls"].as_array().unwrap();
            let runner = Arc::new(StopReplay {
                replay: Replay {
                    home: home.clone(),
                    calls: Mutex::new(calls.iter().cloned().collect()),
                },
                stops: Mutex::new(
                    calls
                        .iter()
                        .map(|c| c["stop_after"].as_bool().unwrap())
                        .collect(),
                ),
                stopped: std::sync::atomic::AtomicBool::new(c["stop"].as_bool().unwrap()),
            });
            let env = c["env"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
                .collect();
            let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
            let report = sync
                .reconcile(c["dry_run"].as_bool().unwrap(), &|| {
                    runner.stopped.load(std::sync::atomic::Ordering::SeqCst)
                })
                .await;
            assert_eq!(
                report, c["report"],
                "{id}: {} {}, etapa {steps}",
                case["test"], case["parameters"]
            );
            assert!(
                runner.replay.calls.lock().unwrap().is_empty(),
                "{id}: comando esperado ausente"
            );
            assert_eq!(tree(&home), c["final"], "{id}: disco, etapa {steps}");
            steps += 1;
        }
    }
    assert!(steps > 0, "asserções Python ausentes para {id}");
}
macro_rules! original_python_assertions {
    ($test:ident,$id:literal) => {
        #[cfg(not(windows))]
        #[tokio::test]
        async fn $test() {
            original_assertions($id).await;
        }
    };
}
original_python_assertions!(op001_original_assertions, "OP001");
original_python_assertions!(op002_original_assertions, "OP002");
original_python_assertions!(op003_original_assertions, "OP003");
original_python_assertions!(op004_original_assertions, "OP004");
original_python_assertions!(op005_original_assertions, "OP005");
original_python_assertions!(op006_original_assertions, "OP006");
original_python_assertions!(op007_original_assertions, "OP007");
original_python_assertions!(op008_original_assertions, "OP008");
original_python_assertions!(op009_original_assertions, "OP009");
original_python_assertions!(op010_original_assertions, "OP010");
original_python_assertions!(op011_original_assertions, "OP011");
original_python_assertions!(op012_original_assertions, "OP012");
original_python_assertions!(op013_original_assertions, "OP013");
original_python_assertions!(op014_original_assertions, "OP014");
original_python_assertions!(op015_original_assertions, "OP015");
original_python_assertions!(op016_original_assertions, "OP016");
original_python_assertions!(op017_original_assertions, "OP017");
original_python_assertions!(op020_original_assertions, "OP020");
original_python_assertions!(op021_original_assertions, "OP021");
original_python_assertions!(op022_original_assertions, "OP022");
original_python_assertions!(op023_original_assertions, "OP023");
original_python_assertions!(op025_original_assertions, "OP025");
original_python_assertions!(op026_original_assertions, "OP026");
original_python_assertions!(op027_original_assertions, "OP027");
original_python_assertions!(op028_original_assertions, "OP028");
original_python_assertions!(op029_original_assertions, "OP029");
original_python_assertions!(op030_original_assertions, "OP030");
original_python_assertions!(op031_original_assertions, "OP031");
original_python_assertions!(op032_original_assertions, "OP032");
original_python_assertions!(op033_original_assertions, "OP033");
original_python_assertions!(op034_original_assertions, "OP034");
original_python_assertions!(op035_original_assertions, "OP035");
original_python_assertions!(op036_original_assertions, "OP036");

#[test]
fn op045_original_assertions_interval_matches_pydantic() {
    let cases: Value = serde_json::from_str(include_str!("settings-contract.json")).unwrap();
    for case in cases
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == "omp_plugin_sync_interval")
    {
        let env = BTreeMap::from([(
            "CP_OMP_PLUGIN_SYNC_INTERVAL".into(),
            case["input"].as_str().unwrap().into(),
        )]);
        let result = service::interval(&env);
        if let Some(expected) = case["result"].as_f64() {
            assert_eq!(result.unwrap(), expected, "{}", case["input"]);
        } else {
            assert!(result.is_err(), "{}", case["input"]);
        }
    }
}

original_python_assertions!(op043_original_assertions, "OP043");
original_python_assertions!(op048_original_assertions, "OP048");

#[test]
fn op019_digest_reuses_bytes_until_metadata_changes() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("a"), b"a").unwrap();
    fs::write(temp.path().join("sub/b"), b"b").unwrap();
    model::DIGEST_READS.set(0);
    let first = model::digest(temp.path()).unwrap();
    assert_eq!(model::DIGEST_READS.get(), 2);
    assert_eq!(model::digest(temp.path()).unwrap(), first);
    assert_eq!(model::DIGEST_READS.get(), 2);
    fs::write(temp.path().join("a"), b"aa").unwrap();
    assert_ne!(model::digest(temp.path()).unwrap(), first);
    assert_eq!(model::DIGEST_READS.get(), 4);
}

struct FailedRollback {
    replay: Replay,
    listing: std::sync::atomic::AtomicUsize,
}
impl Runner for FailedRollback {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let listing = args.get(2).is_some_and(|s| s == "list");
            let result = self.replay.run(args, cwd, env, timeout).await;
            if listing
                && self
                    .listing
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 2
            {
                fs::write(self.replay.home.join(".omp/plugins/package.json"), "{").unwrap();
            }
            result
        })
    }
}
#[tokio::test]
async fn op018_rollback_failure_preserves_original_native_failure() {
    let data: Value = serde_json::from_str(PYTHON_CONTRACT).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = expand(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "fail-install:no-effect")
            .unwrap(),
        &home,
    );
    install(&home, &c["initial"]);
    let runner = Arc::new(FailedRollback {
        replay: Replay {
            home: home.clone(),
            calls: Mutex::new(c["calls"].as_array().unwrap().iter().cloned().collect()),
        },
        listing: 0.into(),
    });
    let env = c["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let report = sync.reconcile(false, &|| false).await;
    assert_eq!(
        report["errors"],
        json!(["Efeito nativo não corresponde à ação solicitada"])
    );
    assert!(runner.replay.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn op057_invalid_profile_never_touches_marketplaces() {
    let temp = tempfile::tempdir().unwrap();
    let runner = Arc::new(Replay {
        home: temp.path().into(),
        calls: Mutex::new(VecDeque::new()),
    });
    let sync = Synchronizer::new(
        temp.path().into(),
        temp.path().join(".claude"),
        BTreeMap::from([("OMP_PROFILE".into(), "../invalid".into())]),
        runner,
    );
    let before = tree(temp.path());
    let report = sync.import_marketplaces(false, &|| false).await;
    assert_eq!(report["errors"], json!(["Nome de perfil OMP inválido"]));
    assert_eq!(tree(temp.path()), before);
}

fn loop_plan(home: &Path, c: &Value, register: bool) -> (BTreeMap<String, String>, Value) {
    install(home, &c["initial"]);
    let env: BTreeMap<String, String> = c["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    let known: Value = serde_json::from_str(
        &fs::read_to_string(home.join(".claude/plugins/known_marketplaces.json")).unwrap(),
    )
    .unwrap();
    let uri = known["catalog"]["installLocation"].as_str().unwrap();
    let catalog: Value = serde_json::from_str(
        &fs::read_to_string(Path::new(uri).join(".claude-plugin/marketplace.json")).unwrap(),
    )
    .unwrap();
    let registry = json!({"version":1,"marketplaces":[{"name":catalog["name"],"sourceType":"local","sourceUri":uri}]});
    if register {
        io::write(&home.join(".omp/marketplaces.json"), &registry).unwrap();
    }
    let call = json!({"args":["omp","plugin","marketplace","add",uri],"cwd":home.to_string_lossy(),"timeout":120,"output":{"code":0,"stdout":""},"set":{".omp/marketplaces.json":{"text":serde_json::to_string_pretty(&registry).unwrap()+if cfg!(windows){"\r\n"}else{"\n"}}},"remove":[]});
    (env, call)
}
async fn wait_state(service: &Service, expected: &str) {
    for _ in 0..2000 {
        if service.status()["state"] == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("estado esperado {expected}, recebido {}", service.status());
}
#[tokio::test(start_paused = true)]
async fn op038_first_cycle_and_next_interval_are_ordered_and_do_not_overlap() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    let (env, _) = loop_plan(&home, &c, true);
    let mut calls = c["calls"].as_array().unwrap().clone();
    calls.push(calls[0].clone());
    calls.push(calls[4].clone());
    let runner = Arc::new(Replay {
        home: home.clone(),
        calls: Mutex::new(calls.into()),
    });
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let service = Service::new(sync, true, 2.0, Arc::new(|| Ok(true))).unwrap();
    service.start_loop().await;
    wait_state(&service, "updated").await;
    assert_eq!(runner.calls.lock().unwrap().len(), 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(runner.calls.lock().unwrap().len(), 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    wait_state(&service, "unchanged").await;
    assert!(runner.calls.lock().unwrap().is_empty());
    service.close().await;
}
#[tokio::test(start_paused = true)]
async fn op039_failed_cycle_recovers_without_restarting_service() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    let (env, _) = loop_plan(&home, &c, true);
    let mut calls = vec![c["calls"][0].clone(), c["calls"][1].clone()];
    calls[1]["output"]["stdout"] = json!("{");
    calls.extend(c["calls"].as_array().unwrap().clone());
    let runner = Arc::new(Replay {
        home: home.clone(),
        calls: Mutex::new(calls.into()),
    });
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let service = Service::new(sync, true, 2.0, Arc::new(|| Ok(true))).unwrap();
    service.start_loop().await;
    wait_state(&service, "error").await;
    assert_eq!(runner.calls.lock().unwrap().len(), 5);
    tokio::time::advance(Duration::from_secs(2)).await;
    wait_state(&service, "updated").await;
    assert!(runner.calls.lock().unwrap().is_empty());
    service.close().await;
}
struct PulseReplay {
    replay: Replay,
    pulse: Arc<std::sync::atomic::AtomicBool>,
}
impl Runner for PulseReplay {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let import = args.get(2).is_some_and(|s| s == "marketplace");
            let out = self.replay.run(args, cwd, env, timeout).await;
            if import {
                self.pulse.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            out
        })
    }
}
#[tokio::test(start_paused = true)]
async fn op044_observed_stop_stays_latched_even_when_master_switch_returns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    let (env, import) = loop_plan(&home, &c, false);
    let mut calls = vec![import];
    calls.extend(c["calls"].as_array().unwrap().clone());
    let pulse = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runner = Arc::new(PulseReplay {
        replay: Replay {
            home: home.clone(),
            calls: Mutex::new(calls.into()),
        },
        pulse: pulse.clone(),
    });
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let service = Service::new(
        sync,
        true,
        2.0,
        Arc::new(move || Ok(!pulse.swap(false, std::sync::atomic::Ordering::SeqCst))),
    )
    .unwrap();
    service.start_loop().await;
    wait_state(&service, "paused").await;
    assert_eq!(runner.replay.calls.lock().unwrap().len(), 5);
    assert!(service.status()["last_report"]["plugins"].is_null());
    tokio::time::advance(Duration::from_secs(2)).await;
    wait_state(&service, "updated").await;
    assert!(runner.replay.calls.lock().unwrap().is_empty());
    service.close().await;
}
#[tokio::test(start_paused = true)]
async fn op047_suspension_wins_over_import_in_the_same_cycle() {
    let data: Value = serde_json::from_str(PYTHON_CONTRACT).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = expand(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "manual")
            .unwrap(),
        &home,
    );
    let (env, import) = loop_plan(&home, &c, false);
    let mut calls = vec![import];
    calls.extend(c["calls"].as_array().unwrap().clone());
    let runner = Arc::new(Replay {
        home: home.clone(),
        calls: Mutex::new(calls.into()),
    });
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let service = Service::new(sync, true, 300.0, Arc::new(|| Ok(true))).unwrap();
    service.start_loop().await;
    wait_state(&service, "suspended").await;
    assert_eq!(
        service.status()["last_report"]["marketplaces"]["items"][0]["action"],
        "import"
    );
    assert_eq!(
        service.status()["last_report"]["plugins"]["items"][0]["action"],
        "suspended"
    );
    assert!(runner.calls.lock().unwrap().is_empty());
    service.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn op042_http_status_responds_while_native_worker_is_waiting() {
    use axum::{Router, http::StatusCode, routing::get};
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
    let service = Service::new(sync, true, 300.0, Arc::new(|| Ok(true))).unwrap();
    service.start_loop().await;
    runner.entered.notified().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let published = service.clone();
    let router = Router::new().route(
        "/api/omp/plugin-sync",
        get(move || {
            let service = published.clone();
            async move { crate::session_write::json_response(StatusCode::OK, service.status()) }
        }),
    );
    let (stop, finish) = tokio::sync::oneshot::channel::<()>();
    let http = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = finish.await;
            })
            .await
            .unwrap();
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .get(format!("http://{addr}/api/omp/plugin-sync"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let status = response.json::<Value>().await.unwrap();
    assert_eq!(status["state"], "running");
    assert!(status["completed_at"].is_null());
    runner.release.notify_one();
    service.close().await;
    assert_eq!(service.status()["state"], "stopped");
    assert_eq!(runner.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let _ = stop.send(());
    http.await.unwrap();
}

#[test]
fn op079_flags_match_original_pydantic_and_reject_invalid_configuration() {
    let cases: Value = serde_json::from_str(include_str!("settings-contract.json")).unwrap();
    for case in cases
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == "omp_plugin_sync_enabled")
    {
        for key in [
            "CP_OMP_PLUGIN_SYNC_ENABLED",
            "CP_OMP_CLAUDE_CONTEXT_ENABLED",
        ] {
            let env = BTreeMap::from([(key.into(), case["input"].as_str().unwrap().into())]);
            let result = service::flag(&env, key);
            if let Some(expected) = case["result"].as_bool() {
                assert_eq!(result.unwrap(), expected);
            } else {
                assert!(result.is_err());
                assert!(validate_environment(&env).is_err());
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn op079_windows_environment_names_ignore_case() {
    let env = BTreeMap::from([
        ("Cp_Omp_Plugin_Sync_Enabled".into(), "yes".into()),
        ("cp_omp_plugin_sync_interval".into(), "1_0".into()),
    ]);
    assert_eq!(service::flag(&env, "CP_OMP_PLUGIN_SYNC_ENABLED"), Ok(true));
    assert_eq!(service::interval(&env), Ok(10.0));
}

#[tokio::test]
async fn op080_context_rule_targets_current_without_resolving_version() {
    let data: Value = serde_json::from_str(PYTHON_CONTRACT).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = expand(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "context-normal")
            .unwrap(),
        &home,
    );
    install(&home, &c["initial"]);
    let real = home.join(".hangar/versions/v1");
    let rule = real.join("scripts/omp/carregar-claude-md-do-projeto.md");
    fs::create_dir_all(rule.parent().unwrap()).unwrap();
    fs::write(&rule, c["template"].as_str().unwrap()).unwrap();
    let current = home.join(".hangar/current");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &current).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real, &current).unwrap();
    let runner = Arc::new(Replay {
        home: home.clone(),
        calls: Mutex::new(c["calls"].as_array().unwrap().iter().cloned().collect()),
    });
    let env = c["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let template = service::context_template(&home);
    assert_eq!(
        template,
        current
            .join("scripts")
            .join("omp")
            .join("carregar-claude-md-do-projeto.md")
    );
    let report = configure_claude_context(&sync, &template, true).await;
    assert_eq!(report["errors"], json!([]));
    let stored =
        fs::read_link(home.join(".omp/agent/rules/carregar-claude-md-do-projeto.md")).unwrap();
    assert_eq!(crate::list::links::without_verbatim(stored), template);
    assert!(runner.calls.lock().unwrap().is_empty());
}

fn measurement_usage() -> (f64, u64) {
    #[cfg(unix)]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        let cpu = usage.ru_utime.tv_sec as f64
            + usage.ru_utime.tv_usec as f64 / 1e6
            + usage.ru_stime.tv_sec as f64
            + usage.ru_stime.tv_usec as f64 / 1e6;
        let rss = if cfg!(target_os = "linux") {
            fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find_map(|line| {
                    line.strip_prefix("VmHWM:")
                        .and_then(|v| v.split_whitespace().next())
                        .and_then(|v| v.parse().ok())
                })
                .unwrap()
        } else {
            usage.ru_maxrss as u64 / 1024
        };
        (cpu, rss)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::FILETIME,
            System::Threading::{GetCurrentProcess, GetProcessTimes},
        };
        #[repr(C)]
        struct Memory {
            size: u32,
            faults: u32,
            peak: usize,
            working: usize,
            paged_peak: usize,
            paged: usize,
            nonpaged_peak: usize,
            nonpaged: usize,
            pagefile: usize,
            pagefile_peak: usize,
        }
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                memory: *mut Memory,
                size: u32,
            ) -> i32;
        }
        let mut times: [FILETIME; 4] = unsafe { std::mem::zeroed() };
        let process = unsafe { GetCurrentProcess() };
        assert_ne!(
            unsafe {
                GetProcessTimes(
                    process,
                    &mut times[0],
                    &mut times[1],
                    &mut times[2],
                    &mut times[3],
                )
            },
            0
        );
        let ticks = |v: FILETIME| (u64::from(v.dwHighDateTime) << 32) | u64::from(v.dwLowDateTime);
        let mut memory: Memory = unsafe { std::mem::zeroed() };
        memory.size = std::mem::size_of::<Memory>() as u32;
        let size = memory.size;
        assert_ne!(
            unsafe { GetProcessMemoryInfo(process, &mut memory, size) },
            0
        );
        (
            (ticks(times[2]) + ticks(times[3])) as f64 / 1e7,
            memory.peak as u64 / 1024,
        )
    }
}
#[tokio::test]
#[ignore = "Medição comparável do dry-run público; executar explicitamente sem outros testes."]
async fn measure_omp_dry_reconciliation() {
    let data: Value = serde_json::from_str(PYTHON_CONTRACT).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = expand(
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "unchanged")
            .unwrap(),
        &home,
    );
    install(&home, &c["initial"]);
    let runner = Arc::new(Replay {
        home: home.clone(),
        calls: Mutex::new(VecDeque::new()),
    });
    let env = c["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect();
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner);
    for _ in 0..5 {
        assert_eq!(sync.reconcile(true, &|| false).await["errors"], json!([]));
    }
    let before = measurement_usage().0;
    let mut samples = vec![];
    for _ in 0..50 {
        let start = std::time::Instant::now();
        let report = sync.reconcile(true, &|| false).await;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(report["errors"], json!([]));
        assert_eq!(report["items"][0]["action"], "unchanged");
    }
    let (cpu, rss) = measurement_usage();
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"language":"rust","platform":std::env::consts::OS,"scenario":"unchanged-dry-reconcile","samples":50,"warmups":5,"p50_ms":samples[25],"p95_ms":samples[47],"cpu_ms":(cpu-before)*1000.0,"peak_rss_kib":rss})
    );
}

#[path = "mechanism_tests.rs"]
mod mechanisms;

#[tokio::test(start_paused = true)]
async fn guest_omp_invalid_configuration_is_error_without_running_a_cycle() {
    let home = tempfile::tempdir().unwrap();
    let runner = Arc::new(Replay {home:home.path().into(),calls:Mutex::new([json!({"sentinel":true})].into())});
    let sync = Synchronizer::new(home.path().into(),home.path().join(".claude"),BTreeMap::new(),runner.clone());
    let service = Service::new(sync,true,300.0,Arc::new(||Err("loop_config_invalid"))).unwrap();
    let preserved = home.path().join("existing.json");
    fs::write(&preserved,"existing").unwrap();
    service.start_loop().await;
    wait_state(&service,"error").await;
    assert_eq!(service.status()["last_report"]["errors"],json!(["loop_config_invalid"]));
    assert!(service.status()["started_at"].is_null());
    assert_eq!(runner.calls.lock().unwrap().len(),1);
    assert_eq!(fs::read_to_string(preserved).unwrap(),"existing");
    service.close().await;
}

#[tokio::test(start_paused = true)]
async fn guest_omp_configuration_error_between_operations_preserves_reports_and_work() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let captured = fresh_contract(&home);
    let (env,import) = loop_plan(&home,&captured,false);
    let mut calls = vec![import];
    calls.extend(captured["calls"].as_array().unwrap().clone());
    let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runner = Arc::new(PulseReplay {replay:Replay {home:home.clone(),calls:Mutex::new(calls.into())},pulse:failed.clone()});
    let sync = Synchronizer::new(home.clone(),home.join(".claude"),env,runner.clone());
    let service = Service::new(sync,true,2.0,Arc::new({let failed=failed.clone();move || {
        if failed.load(std::sync::atomic::Ordering::SeqCst) {Err("loop_config_invalid")} else {Ok(true)}
    }})).unwrap();
    service.start_loop().await;
    wait_state(&service,"error").await;
    assert_eq!(service.status()["last_report"]["configuration"]["errors"],json!(["loop_config_invalid"]));
    assert!(!service.status()["last_report"]["marketplaces"].is_null());
    assert!(service.status()["last_report"]["plugins"].is_null());
    assert_eq!(runner.replay.calls.lock().unwrap().len(),5);
    assert!(home.join(".omp/marketplaces.json").is_file());
    failed.store(false,std::sync::atomic::Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(2)).await;
    wait_state(&service,"updated").await;
    assert!(runner.replay.calls.lock().unwrap().is_empty());
    service.close().await;
}
