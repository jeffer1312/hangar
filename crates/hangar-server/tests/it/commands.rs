use hangar_server::commands::cache::{Cache, Probe, ProbeResult};
use hangar_server::commands::{Catalog, CommandInfo};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[path = "commands_cases.rs"]
mod cases;

#[derive(Deserialize)]
struct Case {
    name: String,
    files: indexmap::IndexMap<String, String>,
    cli: Option<Vec<Value>>,
    project: bool,
    terminal: bool,
    excluded: Vec<String>,
    expected: Vec<CommandInfo>,
}

/// Troca `@base` e os nomes de `expected` pelos comandos de `BASE`, por extenso.
fn table() -> Vec<Case> {
    let base: Vec<Value> = serde_json::from_str(cases::BASE).unwrap();
    let mut rows: Vec<Value> = serde_json::from_str(cases::CASES).unwrap();
    rows.push(serde_json::from_str(cases::PLATFORM_PATH_CASE).unwrap());
    assert_eq!(rows.len(), 22);
    rows.into_iter()
        .map(|mut row| {
            let expected = row["expected"].as_array().unwrap().iter().flat_map(|item| match item.as_str() {
                Some("@base") => base.clone(),
                Some(name) => vec![base.iter().find(|c| c["name"] == name).unwrap().clone()],
                None => vec![item.clone()],
            }).collect();
            row["expected"] = Value::Array(expected);
            serde_json::from_value(row).unwrap()
        })
        .collect()
}

#[test]
fn commands_catalog_per_scenario() {
    for case in table() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        for (path, content) in case.files {
            let target = root.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(
                target,
                content.replace("$ROOT", &root.path().to_string_lossy().replace('\\', "/")),
            )
            .unwrap();
        }
        let cwd = case.project.then(|| root.path().join("project"));
        let actual = Catalog::default().compose(
            &home,
            cwd.as_deref(),
            case.cli.as_deref(),
            &case.excluded,
            case.terminal,
        );
        // `reload-plugins` é provado à parte, abaixo.
        let actual: Vec<_> = actual
            .into_iter()
            .filter(|command| command.name != "reload-plugins")
            .collect();
        assert_eq!(
            actual.len(),
            case.expected.len(),
            "quantidade no cenário {}",
            case.name
        );
        for (actual, expected) in actual.iter().zip(&case.expected) {
            assert_eq!(actual, expected, "cenário {}", case.name);
        }
    }
}

#[test]
fn commands_reload_plugins_is_available_in_the_fallback_of_both_modes() {
    let root = tempfile::tempdir().unwrap();
    for terminal in [true, false] {
        let commands = Catalog::default().compose(root.path(), None, None, &[], terminal);
        let reload = commands
            .iter()
            .find(|command| command.name == "reload-plugins")
            .expect("reload-plugins precisa aparecer antes de a sonda da CLI responder");
        assert_eq!(reload.display, "/reload-plugins");
        assert_eq!(reload.argument_hint.as_deref(), Some("[--force]"));
        assert!(!reload.destructive);
    }
}

struct CliProbe {
    calls: AtomicUsize,
}

impl Probe for CliProbe {
    fn signature(&self, _: Option<&Path>) -> Result<String, &'static str> {
        Ok("fixture-cli-v1".into())
    }
    fn run<'a>(
        &'a self,
        _: Option<&'a Path>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ProbeResult> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                serde_json::json!({"name":"usage","description":"Consulta o uso"}),
            ])
        })
    }
}

#[tokio::test]
async fn commands_probe_is_shared_and_its_cache_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(root.path()));
    let probe = Arc::new(CliProbe {
        calls: AtomicUsize::new(0),
    });
    assert!(cache.get(probe.clone(), None).await.is_none());
    let answer = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(answer) = cache.get(probe.clone(), None).await {
                break answer;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a sonda precisa publicar o catálogo");
    assert_eq!(
        answer,
        vec![serde_json::json!({"name":"usage","description":"Consulta o uso"})]
    );
    let restarted = Arc::new(Cache::new(root.path()));
    assert_eq!(restarted.get(probe.clone(), None).await, Some(answer));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

fn native_probe(root: &Path, mode: &str) -> hangar_server::commands::probe::NativeProbe {
    use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let target = bin.join(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    crate::copy_executable(std::path::Path::new(env!("CARGO_BIN_EXE_hangar-commands-fixture")), &target);
    let mut env = std::collections::BTreeMap::from([
        ("PATH".into(), bin.to_string_lossy().into_owned()),
        ("HOME".into(), root.to_string_lossy().into_owned()),
        ("USERPROFILE".into(), root.to_string_lossy().into_owned()),
        (
            "COMMANDS_PROBE_RECORD".into(),
            root.join("record.json").to_string_lossy().into_owned(),
        ),
        ("COMMANDS_PROBE_MODE".into(), mode.into()),
    ]);
    for name in [
        "TMUX",
        "TMUX_PANE",
        "CP_SESSION_NAME",
        "CP_SESSION_KEY",
        "HANGAR_INTERNAL_SECRET",
        "CP_AUTH_TOKEN",
        "HANGAR_PLUGIN_TOKEN",
    ] {
        env.insert(name.into(), "fixture".into());
    }
    hangar_server::commands::probe::NativeProbe {
        accounts: AccountService::new(AccountEnvironment::from_map(env)),
    }
}

#[tokio::test]
async fn commands_native_probe_correlates_initialize_and_ends_its_process_tree() {
    let root = tempfile::tempdir().unwrap();
    let probe = native_probe(root.path(), "success");
    let result = probe.run(None).await.unwrap();
    assert_eq!(
        result,
        vec![serde_json::json!({"name":"usage","description":"Consulta o uso"})]
    );
    let record: Value =
        serde_json::from_slice(&fs::read(root.path().join("record.json")).unwrap()).unwrap();
    assert_eq!(record["request"]["request"]["subtype"], "initialize");
    assert_eq!(record["session_identity"], false);
    assert_eq!(record["private_authority"], false);
    assert!(record["config_dir"].is_null());
    assert_ne!(record["cwd"].as_str(), Some(root.path().to_str().unwrap()));
    let system = sysinfo::System::new_all();
    for key in ["pid", "child"] {
        assert!(
            system
                .process(sysinfo::Pid::from_u32(record[key].as_u64().unwrap() as u32))
                .is_none_or(|p| matches!(
                    p.status(),
                    sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
                )),
            "processo auxiliar {key} ficou vivo"
        );
    }
}

#[tokio::test]
async fn commands_http_is_owned_by_rust_and_reads_the_headless_project() {
    use hangar_server::{
        accounts::{AccountService, environment::AccountEnvironment},
        auth::TrustedHosts,
        config::Config,
        routes::AppState,
    };
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir_all(project.join(".claude/commands")).unwrap();
    fs::write(
        project.join(".claude/commands/local.md"),
        "---\ndescription: Comando do projeto\nargument-hint: <alvo>\n---\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join(".hangar/claude-headless")).unwrap();
    fs::write(
        root.path().join(".hangar/claude-headless/s.json"),
        serde_json::json!({"name":"s","session_id":"fixture","cwd":project}).to_string(),
    )
    .unwrap();
    let cfg = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: super::refused_address(),
        auth_token: "commands-owner".into(),
        internal_secret: "commands-private".into(),
        trusted: TrustedHosts::parse("127.0.0.1"),
        log_path: None,
    };
    let mut state = AppState::new(cfg);
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        std::collections::BTreeMap::from([
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                root.path().to_string_lossy().into_owned(),
            ),
            ("PATH".into(), String::new()),
        ]),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    state.groups = None;
    state.share = None;
    let task = tokio::spawn(async move {
        hangar_server::routes::serve_with_state(listener, state)
            .await
            .unwrap();
    });
    let result = async {
        let response = super::fake::client()
            .get(format!("http://{address}/api/sessions/s/commands"))
            .bearer_auth("commands-owner")
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let health: Value = serde_json::from_slice(
            &super::fake::client()
                .get(format!("http://{address}/__hangar_server/health"))
                .send()
                .await?
                .bytes()
                .await?,
        )
        .unwrap();
        let private = health["terminal_address"].as_str().unwrap();
        let bridge = format!("http://{private}/__hangar_server/forwarded");
        let denied = super::fake::client()
            .get(&bridge)
            .header("x-hangar-path", "/api/sessions/s/commands")
            .send()
            .await?;
        assert_eq!(denied.status(), 404);
        let forwarded = super::fake::client()
            .get(&bridge)
            .header("x-hangar-path", "/api/sessions/s/commands")
            .header("x-hangar-internal", "commands-private")
            .send()
            .await?;
        assert_eq!(forwarded.status(), 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&forwarded.bytes().await?).unwrap(),
            serde_json::from_slice::<Value>(&bytes).unwrap()
        );
        let outside = super::fake::client()
            .get(&bridge)
            .header("x-hangar-path", "/api/sessions/s/extra/commands")
            .header("x-hangar-internal", "commands-private")
            .send()
            .await?;
        assert_eq!(outside.status(), 404);
        Ok::<_, reqwest::Error>((status, bytes))
    }
    .await;
    task.abort();
    let _ = task.await;
    let (status, bytes) = result.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "a rota Claude deve funcionar sem backend Python"
    );
    let commands: Vec<CommandInfo> = serde_json::from_slice(&bytes).unwrap();
    let local = commands
        .iter()
        .find(|c| c.name == "local")
        .expect("comando do projeto da sessão sem terminal");
    assert_eq!(local.argument_hint.as_deref(), Some("<alvo>"));
    assert!(commands.iter().any(|c| c.name == "reload-plugins"));
}

#[test]
fn commands_frontmatter_updates_and_directory_changes_are_visible() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".claude/commands");
    fs::create_dir_all(&path).unwrap();
    let file = path.join("local.md");
    fs::write(&file, "---\ndescription: antes\n---\n").unwrap();
    let mut catalog = Catalog::default();
    let first = catalog.compose(root.path(), None, None, &[], true);
    assert_eq!(
        first
            .iter()
            .find(|c| c.name == "local")
            .unwrap()
            .description
            .as_deref(),
        Some("antes")
    );
    fs::write(&file, "---\ndescription: agora\n---\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    fs::write(path.join("added.md"), "texto").unwrap();
    let changed = catalog.compose(root.path(), None, None, &[], true);
    assert_eq!(
        changed
            .iter()
            .find(|c| c.name == "local")
            .unwrap()
            .description
            .as_deref(),
        Some("agora")
    );
    assert!(changed.iter().any(|c| c.name == "added"));
    fs::remove_file(file).unwrap();
    assert!(
        !catalog
            .compose(root.path(), None, None, &[], true)
            .iter()
            .any(|c| c.name == "local")
    );
    let dated = path.join("dated.md");
    for (name, seconds) in [("past-one", 1), ("past-two", 2)] {
        fs::write(&dated, format!("---\nname: {name}\n---\n")).unwrap();
        fs::File::options()
            .write(true)
            .open(&dated)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH - Duration::from_secs(seconds))
            .unwrap();
        assert!(
            catalog
                .compose(root.path(), None, None, &[], true)
                .iter()
                .any(|c| c.name == name),
            "mudança de mesmo tamanho também invalida o cache antes de 1970"
        );
    }
    #[cfg(unix)]
    {
        let target = root.path().join("target.md");
        fs::write(
            &target,
            "---\nname: linked\ndescription: Segue o destino\n---\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, path.join("alias.md")).unwrap();
        let linked = catalog.compose(root.path(), None, None, &[], true);
        assert_eq!(
            linked
                .iter()
                .find(|command| command.name == "linked")
                .unwrap()
                .description
                .as_deref(),
            Some("Segue o destino")
        );
    }
}

#[tokio::test]
async fn commands_headless_keeps_reload_despite_cli_tui_flag_and_filters_session_choices() {
    use hangar_server::{
        accounts::{AccountService, environment::AccountEnvironment},
        commands::{Context, Service},
    };
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let account = root.path().join("account");
    fs::create_dir_all(project.join(".claude/commands")).unwrap();
    fs::create_dir_all(&account).unwrap();
    fs::write(project.join(".claude/commands/falar.md"), "texto").unwrap();
    let accounts = AccountService::new(AccountEnvironment::from_map(
        std::collections::BTreeMap::from([
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                root.path().to_string_lossy().into_owned(),
            ),
            ("PATH".into(), String::new()),
        ]),
    ));
    let metadata = serde_json::json!({"cwd":project,"config_dir":account,"claude_settings":{"skillOverrides":{"falar":"off"},"enabledPlugins":{"tools@market":false}}});
    let view = serde_json::json!({"alive":false,"commands":[{"name":"usage"},"lixo",{}, {"name":"reload-plugins"},{"name":"doctor"},{"name":"falar"},{"name":"tools:deploy"}],"terminal_commands":["doctor","color","reload-plugins"]});
    let commands = Arc::new(Service::default())
        .list(accounts.clone(), Context::headless(&metadata, &view))
        .await
        .unwrap();
    assert_eq!(
        commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["usage", "reload-plugins"]
    );
    let mut invalid = metadata;
    invalid["claude_settings"] = serde_json::json!({"skillOverrides": []});
    let commands = Arc::new(Service::default())
        .list(accounts, Context::headless(&invalid, &view))
        .await
        .unwrap();
    assert_eq!(
        commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["usage", "reload-plugins", "falar", "tools:deploy"]
    );
}

#[tokio::test]
async fn commands_native_probe_rejection_empty_and_exited_leader_leave_no_child() {
    for (mode, expected) in [
        ("reject", "claude_commands_probe_rejected"),
        ("empty", "claude_commands_probe_empty"),
        ("exit", "claude_commands_probe_exited"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let probe = native_probe(root.path(), mode);
        assert_eq!(probe.run(None).await, Err(expected));
        let record: Value =
            serde_json::from_slice(&fs::read(root.path().join("record.json")).unwrap()).unwrap();
        let system = sysinfo::System::new_all();
        assert!(
            system
                .process(sysinfo::Pid::from_u32(
                    record["child"].as_u64().unwrap() as u32
                ))
                .is_none_or(|p| matches!(
                    p.status(),
                    sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
                )),
            "filho ficou vivo depois de {mode}"
        );
    }
}

#[tokio::test]
async fn commands_native_probe_timeout_uses_the_controlled_clock() {
    let root = tempfile::tempdir().unwrap();
    let probe = native_probe(root.path(), "hang");
    let task = tokio::spawn(async move { probe.run(None).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !root.path().join("record.json").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::time::resume();
    assert_eq!(task.await.unwrap(), Err("claude_commands_probe_timeout"));
}

#[test]
fn commands_signature_changes_with_account_configuration() {
    let root = tempfile::tempdir().unwrap();
    let probe = native_probe(root.path(), "success");
    let account = root.path().join("account");
    fs::create_dir_all(&account).unwrap();
    let before = probe.signature(Some(&account)).unwrap();
    fs::write(account.join("settings.json"), "{}").unwrap();
    assert_ne!(probe.signature(Some(&account)).unwrap(), before);
    assert_ne!(
        probe.signature(None).unwrap(),
        probe.signature(Some(&account)).unwrap()
    );
}

#[test]
fn commands_external_json_does_not_panic_or_duplicate_names() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngSeed, TestRunner};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(".claude/commands");
    fs::create_dir_all(&directory).unwrap();
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| serde_json::json!(n)),
        ".{0,60}".prop_map(Value::String)
    ];
    let values = leaf.prop_recursive(4, 128, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            proptest::collection::btree_map(".{0,24}", inner, 0..8)
                .prop_map(|map| Value::Object(map.into_iter().collect()))
        ]
    });
    let header = prop_oneof![
        ".{0,512}",
        (".{0,32}", ".{0,80}").prop_map(|(name, description)| format!(
            "name: {}\ndescription: {}",
            serde_json::to_string(&name).unwrap(),
            serde_json::to_string(&description).unwrap()
        ))
    ];
    let input = (
        proptest::collection::vec(values, 0..30),
        header,
        proptest::collection::vec("[a-z][a-z:-]{0,18}", 1..30),
    );
    let config = Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x434f4d4d414e4453),
        failure_persistence: None,
        ..Config::default()
    };
    TestRunner::new(config)
        .run(&input, |(mut raw, header, names)| {
            fs::write(directory.join("fuzz.md"), format!("---\n{header}\n---\n")).unwrap();
            for name in names {
                let command = serde_json::json!({"name":name,"description":"Comando da CLI"});
                raw.extend([command.clone(), command]);
            }
            let cli = hangar_server::commands::cache::valid_commands(&Value::Array(raw));
            let commands =
                Catalog::default().compose(root.path(), None, Some(&cli), &["doctor".into()], true);
            let names: std::collections::HashSet<_> = commands.iter().map(|c| &c.name).collect();
            prop_assert_eq!(names.len(), commands.len());
            prop_assert!(!names.contains(&"doctor".to_owned()));
            for command in commands {
                prop_assert_eq!(command.display, format!("/{}", command.name));
            }
            Ok(())
        })
        .unwrap();
}

struct FailingProbe {
    calls: AtomicUsize,
    version: AtomicUsize,
    finished: tokio::sync::Notify,
}
impl Probe for FailingProbe {
    fn signature(&self, _: Option<&Path>) -> Result<String, &'static str> {
        Ok(format!("fixture-v{}", self.version.load(Ordering::SeqCst)))
    }
    fn run<'a>(
        &'a self,
        _: Option<&'a Path>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ProbeResult> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.finished.notify_one();
            Err("fixture_probe_failed")
        })
    }
}

#[tokio::test]
async fn commands_failure_backoff_is_600_seconds_and_new_signature_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(root.path()));
    let probe = Arc::new(FailingProbe {
        calls: AtomicUsize::new(0),
        version: AtomicUsize::new(0),
        finished: tokio::sync::Notify::new(),
    });
    cache.get(probe.clone(), None).await;
    probe.finished.notified().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(599)).await;
    cache.get(probe.clone(), None).await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    cache.get(probe.clone(), None).await;
    probe.finished.notified().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    probe.version.store(1, Ordering::SeqCst);
    cache.get(probe.clone(), None).await;
    probe.finished.notified().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
    tokio::time::resume();
}

#[tokio::test]
async fn commands_cache_recovers_from_corruption_and_preserves_two_accounts() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".hangar")).unwrap();
    fs::write(root.path().join(".hangar/claude-slash-commands.json"), "{").unwrap();
    let cache = Arc::new(Cache::new(root.path()));
    let probe = Arc::new(CliProbe {
        calls: AtomicUsize::new(0),
    });
    let first = root.path().join("first");
    let second = root.path().join("second");
    let (a, b) = tokio::join!(
        cache.get(probe.clone(), Some(first.clone())),
        cache.get(probe.clone(), Some(second.clone()))
    );
    assert!(a.is_none() && b.is_none());
    let mut queries = tokio::task::JoinSet::new();
    for directory in [first.clone(), second.clone()].into_iter().cycle().take(64) {
        let (cache, probe) = (cache.clone(), probe.clone());
        queries.spawn(async move { cache.get(probe, Some(directory)).await });
    }
    while let Some(result) = queries.join_next().await {
        if let Some(commands) = result.unwrap() {
            assert_eq!(
                commands,
                vec![serde_json::json!({"name":"usage","description":"Consulta o uso"})]
            );
        }
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (a, b) = tokio::join!(
                cache.get(probe.clone(), Some(first.clone())),
                cache.get(probe.clone(), Some(second.clone()))
            );
            if a.is_some() && b.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let saved: Value = serde_json::from_slice(
        &fs::read(root.path().join(".hangar/claude-slash-commands.json")).unwrap(),
    )
    .unwrap();
    assert!(saved[first.to_str().unwrap()]["comandos"].is_array());
    assert!(saved[second.to_str().unwrap()]["comandos"].is_array());
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn commands_cancelled_native_probe_still_ends_the_children() {
    let root = tempfile::tempdir().unwrap();
    let probe = native_probe(root.path(), "hang");
    let task = tokio::spawn(async move { probe.run(None).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !root.path().join("record.json").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let record: Value =
        serde_json::from_slice(&fs::read(root.path().join("record.json")).unwrap()).unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let system = sysinfo::System::new_all();
            if ["pid", "child"].iter().all(|key| {
                system
                    .process(sysinfo::Pid::from_u32(record[*key].as_u64().unwrap() as u32))
                    .is_none_or(|p| {
                        matches!(
                            p.status(),
                            sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
                        )
                    })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelamento deixou auxiliares vivos");
}
