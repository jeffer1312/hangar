use hangar_server::accounts::environment::AccountEnvironment;

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_non_executable_is_absent() {
    use hangar_server::cli_probe::Probe;
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("claude"), "#!/bin/sh\nexit 0\n").unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("PATH".into(), home.path().to_string_lossy().into_owned()),
        ]
        .into(),
    );
    assert_eq!(Probe::default().version_text(&env, "claude").await, None);
}

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_cache_isolates_environment_and_invalidates_all_contexts() {
    use hangar_server::cli_probe::Probe;
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("claude");
    std::fs::write(
        &script,
        r#"#!/bin/sh
printf '%s' "$PROBE_VERSION"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let make = |version: &str| {
        AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("PATH".into(), home.path().to_string_lossy().into_owned()),
                ("PROBE_VERSION".into(), version.into()),
            ]
            .into(),
        )
    };
    let probe = Probe::default();
    for version in ["1.2.3", "4.5.6", "1.2.3"] {
        assert_eq!(
            probe
                .version_text(&make(version), "claude")
                .await
                .as_deref(),
            Some(version)
        );
    }
    std::fs::write(&script, "#!/bin/sh\nprintf changed\n").unwrap();
    probe.invalidate_cli("claude");
    for version in ["1.2.3", "4.5.6"] {
        assert_eq!(
            probe
                .version_text(&make(version), "claude")
                .await
                .as_deref(),
            Some("changed")
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_account_capture_preserves_account_and_filters_private_keys() {
    use hangar_server::command_capture::{capture, capture_with_account_env};
    let env = AccountEnvironment::from_map(
        [
            ("CLAUDE_CONFIG_DIR".into(), "/fixture/account".into()),
            ("CP_AUTH_TOKEN".into(), "synthetic-private-value".into()),
        ]
        .into(),
    );
    let args = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s|%s' \"$CLAUDE_CONFIG_DIR\" \"$CP_AUTH_TOKEN\"".into(),
    ];
    let budget = std::time::Duration::from_secs(2);
    assert_eq!(
        capture_with_account_env(args.clone(), env.clone(), budget)
            .await
            .unwrap()
            .stdout,
        "/fixture/account|"
    );
    assert_eq!(capture(args, env, budget).await.unwrap().stdout, "|");
}

#[tokio::test]
#[cfg(unix)]
async fn cli_probe_retries_failed_capture_and_nonzero_exit_without_invalidation() {
    use hangar_server::cli_probe::Probe;
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("claude");
    let env = AccountEnvironment::from_pairs([
        ("HOME", home.path().to_string_lossy().into_owned()),
        ("PATH", home.path().to_string_lossy().into_owned()),
    ]);
    let mut probe = Probe::default();
    probe.timeout = std::time::Duration::from_millis(100);
    std::fs::write(&script, "#!/bin/sh\n/bin/sleep 2\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        probe.version_text(&env, "claude").await.as_deref(),
        Some("")
    );
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '2.1.999 --plugin-dir'\nprintf 'falha de configuração' >&2\nexit 1\n",
    )
    .unwrap();
    assert_eq!(
        probe.version_text(&env, "claude").await.as_deref(),
        Some("")
    );
    std::fs::write(&script, "#!/bin/sh\nprintf '2.1.999 --plugin-dir'\n").unwrap();
    assert_eq!(
        probe.version_text(&env, "claude").await.as_deref(),
        Some("2.1.999 --plugin-dir")
    );
}

/// Casa, repositório e `claude` falso de um caso de `contract/r1-plugin-publish`.
#[cfg(unix)]
struct PublishCase {
    home: tempfile::TempDir,
    repo: tempfile::TempDir,
    bin: tempfile::TempDir,
    env: AccountEnvironment,
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct Claude { version: &'static str, help_flag: bool, exit: i32, sleep: u32 }

#[cfg(unix)]
impl PublishCase {
    fn new(claude: Option<Claude>, seeded: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let (home, repo, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for plugin in ["hangar", "github-actions"] {
            std::fs::create_dir_all(repo.path().join("plugins").join(plugin).join(".claude-plugin")).unwrap();
            std::fs::write(repo.path().join("plugins").join(plugin).join(".claude-plugin/plugin.json"), "{}").unwrap();
        }
        std::fs::create_dir_all(repo.path().join("plugins/sem-manifesto")).unwrap();
        if seeded {
            let hangar = home.path().join(".hangar");
            std::fs::create_dir_all(&hangar).unwrap();
            std::fs::write(hangar.join("plugin-dir"), "/stale/plugins/hangar\n").unwrap();
            std::fs::write(hangar.join("plugin.json"), r#"{"url": "http://stale/api/plugin", "chave": "stale"}"#).unwrap();
            for name in ["plugin-dir", "plugin.json"] {
                std::fs::set_permissions(hangar.join(name), std::fs::Permissions::from_mode(0o644)).unwrap();
            }
        }
        let probes = bin.path().join("probes");
        if let Some(c) = claude {
            let help = if c.help_flag { "  --plugin-dir <path>  Load plugins" } else { "  --model <model>" };
            // `pids`: a sonda e o filho que ela espera, para conferir que o prazo não deixa órfão.
            let pids = bin.path().join("pids");
            let script = format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\necho $$ >> '{}'\n/bin/sleep {} &\necho $! >> '{}'\nwait\ncase \"$1\" in --version) printf '%s\\n' '{}';; --help) printf 'Options:\\n%s\\n' '{help}';; esac\nexit {}\n",
                probes.display(), pids.display(), c.sleep, pids.display(), c.version, c.exit);
            std::fs::write(bin.path().join("claude"), script).unwrap();
            std::fs::set_permissions(bin.path().join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.path().to_string_lossy().into_owned()),
            ("PATH", bin.path().to_string_lossy().into_owned()),
        ]);
        Self { home, repo, bin, env }
    }

    fn probes(&self) -> Vec<String> {
        std::fs::read_to_string(self.bin.path().join("probes")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn file(&self, name: &str) -> Option<(u32, String)> {
        use std::os::unix::fs::PermissionsExt;
        let path = self.home.path().join(".hangar").join(name);
        let mode = std::fs::metadata(&path).ok()?.permissions().mode() & 0o777;
        Some((mode, std::fs::read_to_string(path).unwrap()))
    }

    fn pids(&self) -> Vec<u32> {
        std::fs::read_to_string(self.bin.path().join("pids")).unwrap_or_default().lines().filter_map(|line| line.trim().parse().ok()).collect()
    }

    fn machine(&self, probe: hangar_server::cli_probe::Probe) -> hangar_server::config_sync::plugin_publish::Machine {
        hangar_server::config_sync::plugin_publish::Machine { home: self.home.path().to_owned(), repo: Some(self.repo.path().to_owned()),
            port: 8765, secret: "contract-owner-token".into(), probe, env: self.env.clone() }
    }
}

/// O processo terminou (ou virou zumbi esperando quem o recolha), sem ficar rodando.
#[cfg(unix)]
fn gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| stat.rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')))
}

#[cfg(unix)]
#[derive(Clone, Default)]
struct WarnLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(unix)]
impl std::io::Write for WarnLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// R1.5-11 / R1.7: `~/.hangar/plugin.json` sempre (0600) e `plugin-dir` (0644) só com os hooks ligados e
/// o `claude` com mods por padrão (2.1.287+) ou com `--plugin-dir` no `--help`. Casos de
/// `contract/r1-plugin-publish`; o `hooks_default` é o ligado (`claude_function_hooks` nasce ligado).
#[tokio::test]
#[cfg(unix)]
async fn plugin_files_are_published_like_python() {
    use hangar_server::cli_probe::Probe;
    use hangar_server::config_sync::plugin_publish::publish_in_turn;
    let v = |version, help_flag| Some(Claude { version, help_flag, exit: 0, sleep: 0 });
    let both = vec!["--version".to_owned(), "--help".to_owned()];
    let only_version = vec!["--version".to_owned()];
    // (caso, claude, já tinha arquivos, sondas com hooks ligados, plugin-dir publicado)
    let cases: Vec<(&str, Option<Claude>, bool, Vec<String>, bool)> = vec![
        ("v2_1_287", v("2.1.287 (Claude Code)", false), true, only_version.clone(), true),
        ("v2_1_287__no_seed", v("2.1.287 (Claude Code)", false), false, only_version.clone(), true),
        ("v2_2_0", v("2.2.0 (Claude Code)", false), true, only_version, true),
        ("v2_1_286_without_flag", v("2.1.286 (Claude Code)", false), true, both.clone(), false),
        ("v2_1_286_without_flag__no_seed", v("2.1.286 (Claude Code)", false), false, both.clone(), false),
        ("v2_1_200_with_flag", v("2.1.200 (Claude Code)", true), true, both.clone(), true),
        ("garbage_version_with_flag", v("not a version", true), true, both.clone(), true),
        ("garbage_version_without_flag", v("not a version", false), true, both.clone(), false),
        ("probe_exit_1", Some(Claude { version: "", help_flag: false, exit: 1, sleep: 0 }), true, both.clone(), false),
        ("no_claude_on_path", None, true, vec![], false),
    ];
    let plugin_json = r#"{"url": "http://127.0.0.1:8765/api/plugin", "chave": "4db1dc76b92e69cb0591f56830b89d1e"}"#;
    for (name, claude, seeded, probes, published) in cases {
        for hooks in [true, false] {
            let case = PublishCase::new(claude, seeded);
            let log = WarnLog::default();
            let writer = log.clone();
            let subscriber = tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish();
            let _guard = tracing::subscriber::set_default(subscriber);
            publish_in_turn(case.machine(Probe::default()), move || hooks).await.unwrap();
            let context = format!("{name} hooks={hooks}");
            assert_eq!(case.probes(), if hooks { probes.clone() } else { vec![] }, "{context}: hooks desligados não sondam");
            assert_eq!(case.file("plugin.json"), Some((0o600, plugin_json.to_owned())), "{context}");
            let dir = format!("{0}/plugins/hangar\n{0}/plugins/github-actions\n", case.repo.path().display());
            assert_eq!(case.file("plugin-dir"), (hooks && published).then_some((0o644, dir)), "{context}");
            assert!(!String::from_utf8_lossy(&log.0.lock().unwrap()).contains("WARN"), "{context}: só o prazo estourado avisa");
        }
    }
}

/// `probe_hangs`: as duas sondas estouram o prazo, cada uma com um aviso, e o `plugin-dir` sai. A sonda e o
/// filho que ela esperava morrem no prazo: nenhum órfão fica rodando.
#[tokio::test]
#[cfg(unix)]
async fn plugin_dir_is_removed_when_the_claude_probe_hangs() {
    use hangar_server::cli_probe::Probe;
    use hangar_server::config_sync::plugin_publish::publish_in_turn;
    let case = PublishCase::new(Some(Claude { version: "2.1.287 (Claude Code)", help_flag: true, exit: 0, sleep: 30 }), true);
    let log = WarnLog::default();
    let writer = log.clone();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish());
    let mut probe = Probe::default();
    probe.timeout = std::time::Duration::from_millis(300);
    publish_in_turn(case.machine(probe), || true).await.unwrap();
    assert_eq!(case.probes(), ["--version", "--help"]);
    assert_eq!(case.file("plugin-dir"), None);
    assert_eq!(case.file("plugin.json").unwrap().0, 0o600);
    assert_eq!(String::from_utf8_lossy(&log.0.lock().unwrap()).matches("WARN").count(), 2);
    let pids = case.pids();
    assert_eq!(pids.len(), 4, "duas sondas, cada uma com o filho: {pids:?}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !pids.iter().all(|pid| gone(*pid)) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(pids.iter().all(|pid| gone(*pid)), "órfão da sonda ainda rodando: {pids:?}");
}

/// A publicação que começou primeiro (a da subida, sondando o `claude`) termina antes de a seguinte ler
/// `claude_function_hooks`: a troca feita no meio vale no fim, e não é desfeita pela sonda lenta.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(unix)]
async fn plugin_publications_run_in_order_and_read_hooks_in_their_turn() {
    use hangar_server::cli_probe::Probe;
    use hangar_server::config_sync::plugin_publish::publish_in_turn;
    use std::sync::{Arc, Mutex};
    // `claude` antigo com a flag e lento: a subida (hooks ligados) sonda as duas vezes.
    let case = PublishCase::new(Some(Claude { version: "2.1.200 (Claude Code)", help_flag: true, exit: 0, sleep: 1 }), false);
    let order = Arc::new(Mutex::new(Vec::new()));
    let boot = {
        let (machine, order) = (case.machine(Probe::default()), order.clone());
        tokio::spawn(publish_in_turn(machine, move || { order.lock().unwrap().push("subida"); true }))
    };
    while case.probes().is_empty() { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    let change = {
        let (machine, order) = (case.machine(Probe::default()), order.clone());
        tokio::spawn(publish_in_turn(machine, move || { order.lock().unwrap().push("troca"); false }))
    };
    boot.await.unwrap().unwrap();
    change.await.unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), ["subida", "troca"]);
    assert_eq!(case.file("plugin-dir"), None, "a troca (hooks desligados) é a que vale");
}

/// Leitura de `claude_function_hooks` que não termina não vira "desligado": nada é publicado nem apagado.
#[tokio::test]
#[cfg(unix)]
async fn unreadable_hooks_publish_nothing() {
    use hangar_server::cli_probe::Probe;
    use hangar_server::config_sync::plugin_publish::publish_in_turn;
    let case = PublishCase::new(Some(Claude { version: "2.1.287 (Claude Code)", help_flag: false, exit: 0, sleep: 0 }), true);
    let error = publish_in_turn(case.machine(Probe::default()), || panic!("leitura interrompida")).await.unwrap_err();
    assert_eq!(error.code, "plugin_hooks_unreadable");
    assert_eq!(case.file("plugin-dir"), Some((0o644, "/stale/plugins/hangar\n".into())));
    assert_eq!(case.file("plugin.json"), Some((0o644, r#"{"url": "http://stale/api/plugin", "chave": "stale"}"#.into())));
    assert!(case.probes().is_empty());
}


#[test]
fn claude_version_is_read_like_the_python_regex() {
    use hangar_server::config_sync::plugin_publish::parse_version;
    assert_eq!(parse_version("2.1.287 (Claude Code)"), Some((2, 1, 287)));
    assert_eq!(parse_version("  2.2.0\n"), Some((2, 2, 0)));
    assert_eq!(parse_version("2.1.287abc"), Some((2, 1, 287)));
    for text in ["", "not a version", "2.1", "2..1", "2.1.", "v2.1.287", "2.1.x", "+2.1.287"] {
        assert_eq!(parse_version(text), None, "{text}");
    }
}

proptest::proptest! {
    /// O `--version` é texto de outro programa: nada nele derruba a sonda.
    #[test]
    fn any_claude_version_text_is_read_without_panicking(text in "(?s).{0,40}") {
        let _ = hangar_server::config_sync::plugin_publish::parse_version(&text);
    }
}

#[test]
fn default_probe_timeout_is_the_python_ten_seconds() {
    assert_eq!(hangar_server::cli_probe::Probe::default().timeout, std::time::Duration::from_secs(10));
}
