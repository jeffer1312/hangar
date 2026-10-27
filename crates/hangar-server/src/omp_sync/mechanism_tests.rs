//! Provas dos mecanismos compartilhados que a reprodução de CLI não observa sozinha.
use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!("mechanisms-contract.json")).unwrap()
}
fn controlled_environment(home: &Path) -> BTreeMap<String, String> {
    let mut env: BTreeMap<_, _> = std::env::vars()
        .filter(|(key, _)| !key.starts_with("GIT_"))
        .collect();
    for (key, value) in fixture()["environment_input"].as_object().unwrap() {
        env.insert(key.clone(), value.as_str().unwrap().to_owned());
    }
    env.insert(
        "XDG_CONFIG_HOME".into(),
        home.join(".config").to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_DATA_HOME".into(),
        home.join(".local/share").to_string_lossy().into_owned(),
    );
    env
}
struct EnvironmentReplay {
    replay: Replay,
    expected: BTreeMap<String, String>,
    seen: std::sync::atomic::AtomicUsize,
}
impl Runner for EnvironmentReplay {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            // A fixture do Python contém só variáveis sintéticas; o restante do host não vai ao diagnóstico.
            let observed: BTreeMap<_, _> = env
                .iter()
                .filter(|(key, _)| key.starts_with("GIT_") || self.expected.contains_key(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            assert_eq!(
                observed, self.expected,
                "ambiente recebido na fronteira pública de cada subprocesso"
            );
            self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.replay.run(args, cwd, env, timeout).await
        })
    }
}
#[tokio::test]
async fn op087_public_reconciliation_strips_host_git_environment_for_every_subprocess() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    install(&home, &c["initial"]);
    let expected = expand(&fixture()["environment_expected"], &home);
    let mut expected: BTreeMap<String, String> = serde_json::from_value(expected).unwrap();
    if cfg!(windows) {
        expected.insert("GIT_CONFIG_GLOBAL".into(), "nul".into());
        for key in ["HOME", "USERPROFILE", "CLAUDE_CONFIG_DIR"] {
            let value = PathBuf::from(&expected[key])
                .components()
                .collect::<PathBuf>();
            expected.insert(key.into(), value.to_string_lossy().into_owned());
        }
    }
    let runner = Arc::new(EnvironmentReplay {
        replay: Replay {
            home: home.clone(),
            calls: Mutex::new(c["calls"].as_array().unwrap().clone().into()),
        },
        expected,
        seen: 0.into(),
    });
    let sync = Synchronizer::new(
        home.clone(),
        home.join(".claude"),
        controlled_environment(&home),
        runner.clone(),
    );
    let report = sync.reconcile(false, &|| false).await;
    assert_eq!(report, c["report"]);
    assert!(runner.seen.load(std::sync::atomic::Ordering::SeqCst) > 1);
    assert!(runner.replay.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn op087_native_git_ignores_host_repository_and_config_injection() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let host = home.join("host.gitconfig");
    fs::write(&host, "[host]\n poisoned = true\n").unwrap();
    fs::write(home.join(".gitconfig"), "[host]\n personal = true\n").unwrap();
    let mut env = controlled_environment(home);
    env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        host.to_string_lossy().into_owned(),
    );
    env.insert("GIT_CONFIG_KEY_0".into(), "host.injected".into());
    let sync = Synchronizer::new(
        home.into(),
        home.join(".claude"),
        env,
        Arc::new(NativeRunner),
    );
    let output = sync
        .runner
        .run(
            vec![
                "git".into(),
                "config".into(),
                "--list".into(),
                "--show-origin".into(),
            ],
            home,
            &sync.env,
            Duration::from_secs(15),
        )
        .await
        .unwrap();
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(
        !output.stdout.contains("host."),
        "nenhuma configuração do host deve entrar no Git isolado"
    );
    assert!(
        output.stderr.is_empty(),
        "nenhuma GIT_DIR inválida deve afetar o Git isolado"
    );
}
fn symbolic_file(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link).unwrap();
}
struct ForbiddenRunner;
impl Runner for ForbiddenRunner {
    fn run<'a>(
        &'a self,
        _: Vec<String>,
        _: &'a Path,
        _: &'a BTreeMap<String, String>,
        _: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async { panic!("uma trava recusada não pode iniciar CLI nem ignorar a falha") })
    }
}
#[tokio::test]
async fn op089_public_operations_refuse_symbolic_ledger_and_lock_without_touching_target() {
    for case in fixture()["lock_cases"].as_array().unwrap() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let sync = Synchronizer::new(
            home.into(),
            home.join(".claude"),
            BTreeMap::new(),
            Arc::new(ForbiddenRunner),
        );
        let target = home.join("target");
        let text = case["target_text"].as_str().unwrap();
        fs::write(&target, text).unwrap();
        let path = if case["target"] == "ledger" {
            sync.ledger()
        } else {
            sync.ledger().with_extension("lock")
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symbolic_file(&target, &path);
        let actual = if case["operation"] == "reconcile" {
            sync.reconcile(false, &|| false).await
        } else {
            sync.import_marketplaces(false, &|| false).await
        };
        assert_eq!(actual, case["report"], "{case}");
        assert_eq!(fs::read_to_string(&target).unwrap(), text);
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!home.join(".omp").exists());
    }
}
#[tokio::test]
async fn op089_process_lock_holds_public_reconciliation_until_guard_is_released() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    install(&home, &c["initial"]);
    let runner = Arc::new(Replay {
        home: home.clone(),
        calls: Mutex::new(c["calls"].as_array().unwrap().clone().into()),
    });
    let env = serde_json::from_value(c["env"].clone()).unwrap();
    let sync = Synchronizer::new(home.clone(), home.join(".claude"), env, runner.clone());
    let lock_path = sync.ledger().with_extension("lock");
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    held.lock().unwrap();
    let calls = runner.calls.lock().unwrap().len();
    let mut worker = tokio::spawn(async move { sync.reconcile(false, &|| false).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut worker)
            .await
            .is_err(),
        "o escritor deve esperar a trava do sistema"
    );
    assert_eq!(
        runner.calls.lock().unwrap().len(),
        calls,
        "nenhum CLI antes da posse"
    );
    assert!(
        !home.join(".hangar/omp-plugin-sync.json").exists(),
        "nenhum pending antes da posse"
    );
    drop(held);
    let actual = tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual, c["report"]);
    assert!(runner.calls.lock().unwrap().is_empty());
}
#[test]
fn op082_atomic_json_preserves_unicode_newline_old_file_and_cleans_failed_temporary() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("ledger.json");
    let expected = fixture();
    let old = json!({"ação":"migração","itens":[1,2]});
    io::write(&target, &old).unwrap();
    let text = fs::read_to_string(&target).unwrap();
    assert_eq!(
        text.replace("\r\n", "\n"),
        expected["atomic_success"]["bytes"].as_str().unwrap()
    );
    let before = fs::read(&target).unwrap();
    let seen = std::cell::Cell::new(false);
    let failure = crate::runtime::queue::atomic_write_inspected(&target, b"new", |file| {
        seen.set(true);
        assert_eq!(file.metadata()?.len(), 0);
        assert_eq!(
            fs::read(&target)?,
            before,
            "a versão antiga ainda é visível enquanto o temporário está aberto"
        );
        Err(std::io::Error::other("synthetic write failure"))
    });
    assert!(seen.get());
    assert!(failure.is_err());
    assert_eq!(fs::read(&target).unwrap(), before);
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "o temporário da falha foi limpo"
    );
    // A falha na substituição também conserva o alvo e remove o temporário já preenchido e sincronizado.
    let directory = tmp.path().join("directory");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("sentinel"), b"old").unwrap();
    assert!(io::write(&directory, &json!({"ação":"nova"})).is_err());
    assert_eq!(fs::read(directory.join("sentinel")).unwrap(), b"old");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 2);
}

#[test]
fn op085_packed_refs_and_symbolic_git_metadata_match_python() {
    let data = fixture();
    let packed = &data["packed_refs"];
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for (path, text) in packed["files"].as_object().unwrap() {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text.as_str().unwrap()).unwrap();
    }
    assert_eq!(
        serde_json::to_value(model::git_identity(root).unwrap()).unwrap(),
        packed["expected"]
    );
    let outside = root.join("outside");
    fs::write(&outside, "a".repeat(40)).unwrap();
    let reference = root.join(".git/refs/heads/work");
    fs::create_dir_all(reference.parent().unwrap()).unwrap();
    symbolic_file(&outside, &reference);
    assert_eq!(
        model::git_identity(root).unwrap_err(),
        packed["outside_ref_error"]
    );
    fs::remove_file(reference).unwrap();
    fs::rename(root.join(".git"), root.join("real-git")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("real-git"), root.join(".git")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.join("real-git"), root.join(".git")).unwrap();
    assert_eq!(
        serde_json::to_value(model::git_identity(root).unwrap()).unwrap(),
        packed["symbolic_git_expected"]
    );
}
#[test]
fn op086_bun_jsonc_keeps_comments_and_trailing_delimiters_inside_strings() {
    let data = fixture();
    let case = &data["bun_jsonc"];
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("bun.lock"), case["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(
            model::bun_revision(tmp.path(), case["name"].as_str().unwrap()).unwrap()
        )
        .unwrap(),
        case["expected"]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn op091_cancel_before_deadline_reaps_the_created_process_group() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_owned();
    let task = tokio::spawn({
        let home = home.clone();
        async move {
            NativeRunner
                .run(
                    vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        "sleep 30 & echo $! > descendant; echo $$ > leader; wait".into(),
                    ],
                    &home,
                    &BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
                    Duration::from_secs(30),
                )
                .await
        }
    });
    let (pid, leader) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let (Ok(raw), Ok(parent)) = (
                fs::read_to_string(home.join("descendant")),
                fs::read_to_string(home.join("leader")),
            ) {
                if let (Ok(pid), Ok(leader)) =
                    (raw.trim().parse::<i32>(), parent.trim().parse::<i32>())
                {
                    break (pid, leader);
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    struct CreatedProcess(i32);
    impl Drop for CreatedProcess {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
            }
        }
    }
    let cleanup = CreatedProcess(pid);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let stopped = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let active = fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .is_some_and(|s| {
                    s.rsplit_once(')')
                        .is_some_and(|(_, tail)| !tail.trim_start().starts_with('Z'))
                });
            if !active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let reaped = tokio::time::timeout(Duration::from_secs(2), async {
        while Path::new(&format!("/proc/{leader}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    drop(cleanup);
    assert!(
        stopped,
        "cancelamento antes do prazo deixou o descendente criado vivo"
    );
    assert!(
        reaped,
        "o líder criado permaneceu sem reap depois do cancelamento"
    );
}

#[tokio::test]
async fn op087_replay_rejects_subprocesses_with_host_git_environment() {
    use futures_util::FutureExt;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    let c = fresh_contract(&home);
    install(&home, &c["initial"]);
    let call = c["calls"][0].clone();
    let replay = Replay {
        home: home.clone(),
        calls: Mutex::new(VecDeque::from([call.clone()])),
    };
    let args = serde_json::from_value(call["args"].clone()).unwrap();
    let cwd = PathBuf::from(call["cwd"].as_str().unwrap());
    let env = controlled_environment(&home);
    let result = std::panic::AssertUnwindSafe(replay.run(
        args,
        &cwd,
        &env,
        Duration::from_secs_f64(call["timeout"].as_f64().unwrap()),
    ))
    .catch_unwind()
    .await;
    assert!(
        result.is_err(),
        "o replay ignorou o ambiente Git hostil e produziu um falso GREEN"
    );
}

#[tokio::test]
async fn op099_public_status_requires_owner_token_and_returns_the_published_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config = crate::config::Config::from_lookup(|key| match key {
        "HANGAR_SERVER_LISTEN" => Some("127.0.0.1:18765".into()),
        "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
        "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
        "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
        _ => None,
    })
    .unwrap();
    let st = Arc::new(crate::routes::AppState::new(config));
    let sync = Synchronizer::new(
        home.into(),
        home.join(".claude"),
        BTreeMap::new(),
        Arc::new(ForbiddenRunner),
    );
    let service = Service::new(sync, false, 300., Arc::new(|| true)).unwrap();
    let expected = service.status();
    assert!(st.omp_sync.set(service).is_ok());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, done) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            crate::routes::router(st).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = done.await;
        })
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    for token in [None, Some("invalid"), Some("fixture-owner")] {
        let mut request = client
            .get(format!("http://{address}/api/omp/plugin-sync"))
            .header("x-forwarded-for", "198.51.100.96");
        if let Some(token) = token {
            request = request.bearer_auth(token)
        }
        let response = request.send().await.unwrap();
        if token == Some("fixture-owner") {
            assert_eq!(response.status(), 200);
            assert_eq!(response.json::<Value>().await.unwrap(), expected)
        } else {
            assert_eq!(response.status(), 401);
        }
    }
    let _ = stop.send(());
    task.await.unwrap();
}

#[test]
fn op083_git_origins_keep_protocol_user_port_and_refuse_hostile_lexical_forms() {
    for case in fixture()["origins"].as_array().unwrap() {
        let result = model::identity(case["input"].as_str().unwrap());
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err(), error, "{case}");
        } else {
            assert_eq!(result.unwrap(), case["value"], "{case}");
        }
    }
}

#[tokio::test]
async fn op081_public_reconciliation_refuses_invalid_utf8_and_symbolic_inventory() {
    for case in fixture()["records"].as_array().unwrap() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let record = home.join(".claude/plugins/installed_plugins.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let target = home.join("record-target");
        if case["kind"] == "invalid_utf8" {
            fs::write(&record, [255]).unwrap();
        } else {
            fs::write(&target, case["target_text"].as_str().unwrap()).unwrap();
            symbolic_file(&target, &record);
        }
        let sync = Synchronizer::new(
            home.into(),
            home.join(".claude"),
            BTreeMap::new(),
            Arc::new(ForbiddenRunner),
        );
        assert_eq!(sync.reconcile(false, &|| false).await, case["report"]);
        if case["kind"] == "symbolic_record" {
            assert_eq!(
                fs::read_to_string(target).unwrap(),
                case["target_text"].as_str().unwrap()
            );
            assert!(
                fs::symlink_metadata(record)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        assert!(
            !sync.ledger().exists(),
            "a leitura recusada não pode ganhar propriedade"
        );
    }
}
