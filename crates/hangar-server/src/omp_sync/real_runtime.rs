//! Provas opt-in com o gerenciador nativo, sem credenciais nem chamada a modelo.
use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::any,
};
use std::{fs, process::Stdio};
use tokio::io::AsyncWriteExt;

struct DiagnosticNative;
impl Runner for DiagnosticNative {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let out = NativeRunner.run(args, cwd, env, timeout).await?;
            if out.code != 0 {
                eprintln!("CLI de prova falhou: {}\n{}", out.stdout, out.stderr);
            }
            Ok(out)
        })
    }
}

#[derive(Clone)]
struct GitHttp {
    root: PathBuf,
    env: BTreeMap<String, String>,
}
async fn git_http(State(state): State<GitHttp>, req: Request) -> Result<Response, StatusCode> {
    let (parts, body) = req.into_parts();
    let body = to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut command = tokio::process::Command::new("git");
    command
        .args(["http-backend"])
        .env_clear()
        .envs(&state.env)
        .env("GIT_PROJECT_ROOT", state.root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", parts.uri.path())
        .env("QUERY_STRING", parts.uri.query().unwrap_or(""))
        .env("REQUEST_METHOD", parts.method.as_str())
        .env(
            "CONTENT_TYPE",
            parts
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(""),
        )
        .env("CONTENT_LENGTH", body.len().to_string())
        .env(
            "HTTP_GIT_PROTOCOL",
            parts
                .headers
                .get("git-protocol")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(""),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(mut input) = child.stdin.take() {
        input
            .write_all(&body)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let out = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !out.status.success() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let split = out
        .stdout
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let headers =
        std::str::from_utf8(&out.stdout[..split]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut response = Response::builder();
    for line in headers.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("status") {
                if let Some(code) = v
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<u16>().ok())
                {
                    response = response.status(code)
                }
            } else {
                response = response.header(k.trim(), v.trim())
            }
        }
    }
    response
        .body(Body::from(out.stdout[split + 4..].to_vec()))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn git(env: &BTreeMap<String, String>, cwd: &Path, args: &[&str]) -> String {
    let mut command = vec![
        "git".into(),
        "-c".into(),
        format!(
            "core.hooksPath={}",
            if cfg!(windows) { "nul" } else { "/dev/null" }
        ),
        "-c".into(),
        "user.name=Fixture".into(),
        "-c".into(),
        "user.email=fixture@example.invalid".into(),
    ];
    command.extend(args.iter().map(|a| a.to_string()));
    let out = NativeRunner
        .run(command, cwd, env, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(out.code, 0, "git {args:?}");
    out.stdout.trim().into()
}
fn json_file(path: &Path, v: Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    io::write(path, &v).unwrap()
}
fn inventory(home: &Path, source: &Path, url: &str, revision: &str, enabled: bool) {
    json_file(
        &home.join(".claude/plugins/installed_plugins.json"),
        json!({"version":2,"plugins":{"comet@catalog":[{"scope":"user","installPath":source,"gitCommitSha":revision}]}}),
    );
    json_file(
        &home.join(".claude/plugins/known_marketplaces.json"),
        json!({"catalog":{"source":{"source":"git","url":url},"installLocation":source}}),
    );
    json_file(
        &home.join(".claude/settings.json"),
        json!({"enabledPlugins":{"comet@catalog":enabled}}),
    );
    json_file(
        &source.join(".claude-plugin/marketplace.json"),
        json!({"name":"catalog","owner":{"name":"Fixture"},"plugins":[{"name":"comet","source":{"source":"url","url":url,"sha":revision}}]}),
    );
}
/// O Git HTTP é real e só atende loopback; o teste executa o próprio CLI omp instalado.
#[tokio::test]
#[ignore = "Requer omp, Bun e Git instalados; executar explicitamente no sandbox."]
async fn native_plugin_install_adopt_update_disable_and_remove() {
    for (adopt, xdg) in [(false, false), (true, false), (false, true)] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let mut env = BTreeMap::new();
        for key in ["PATH", "SYSTEMROOT", "WINDIR", "LANG"] {
            if let Ok(v) = std::env::var(key) {
                env.insert(key.into(), v);
            }
        }
        for (key, path) in [
            ("HOME", home.clone()),
            ("USERPROFILE", home.clone()),
            ("PI_CODING_AGENT_DIR", home.join(".omp/agent")),
            ("XDG_CONFIG_HOME", home.join(".config")),
            ("XDG_DATA_HOME", home.join(".local/share")),
            ("XDG_CACHE_HOME", home.join(".cache")),
        ] {
            env.insert(key.into(), path.to_string_lossy().into_owned());
        }
        if xdg {
            fs::create_dir_all(home.join(".local/share/omp")).unwrap();
        }
        env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            if cfg!(windows) { "nul" } else { "/dev/null" }.into(),
        );
        if let Some(real) = std::env::home_dir()
            .map(|h| h.join(".omp/natives"))
            .filter(|p| p.is_dir())
        {
            #[cfg(unix)]
            for path in [home.join(".omp/natives"), home.join(".cache/omp/natives")] {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&real, path).unwrap();
            }
        }
        let repos = home.join("repositories");
        fs::create_dir_all(&repos).unwrap();
        let bare = repos.join("org/repo.git");
        fs::create_dir_all(bare.parent().unwrap()).unwrap();
        git(
            &env,
            &repos,
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                bare.to_str().unwrap(),
            ],
        )
        .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/org/repo.git");
        let app = Router::new()
            .route("/{*path}", any(git_http))
            .with_state(GitHttp {
                root: repos,
                env: env.clone(),
            });
        let mut server = Server(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let source = home.join("source");
        fs::create_dir_all(source.join("commands")).unwrap();
        json_file(
            &source.join("package.json"),
            json!({"name":"hangar-comet-runtime-fixture","version":"1.0.0","omp":{"commands":["commands/base.md"],"features":{"optional":{"default":false,"commands":["commands/optional.md"]}},"settings":{"message":{"type":"string","default":""}}}}),
        );
        fs::write(source.join("commands/base.md"), "Primeira revisão.\n").unwrap();
        fs::write(source.join("commands/optional.md"), "Comando opcional.\n").unwrap();
        json_file(
            &source.join(".claude-plugin/marketplace.json"),
            json!({"name":"catalog","owner":{"name":"Fixture"},"plugins":[]}),
        );
        git(&env, &source, &["init", "--initial-branch=main"]).await;
        git(&env, &source, &["remote", "add", "origin", &url]).await;
        git(
            &env,
            &source,
            &["add", "package.json", "commands", ".claude-plugin"],
        )
        .await;
        git(&env, &source, &["commit", "-m", "Primeira revisão"]).await;
        let first = git(&env, &source, &["rev-parse", "HEAD"]).await;
        git(
            &env,
            &source,
            &["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        )
        .await;
        inventory(&home, &source, &url, &first, true);
        let sync = Synchronizer::new(
            home.clone(),
            home.join(".claude"),
            env.clone(),
            Arc::new(DiagnosticNative),
        );
        let root = sync.directories().unwrap().data_root.join("plugins");
        let imported = sync.import_marketplaces(false, &|| false).await;
        assert_eq!(imported["errors"], json!([]), "{imported}");
        assert_eq!(imported["items"][0]["action"], "import");
        let catalog = io::read(&root.parent().unwrap().join("marketplaces.json"), None).unwrap();
        assert_eq!(catalog["marketplaces"][0]["sourceUri"], url);
        assert_eq!(
            sync.import_marketplaces(false, &|| false).await["items"][0]["action"],
            "unchanged"
        );
        let lock = root.join("omp-plugins.lock.json");
        let package = root.join("node_modules/hangar-comet-runtime-fixture");
        if adopt {
            let out = NativeRunner
                .run(
                    vec![
                        "omp".into(),
                        "plugin".into(),
                        "install".into(),
                        format!("git+{url}#{first}[optional]"),
                        "--json".into(),
                    ],
                    &home,
                    &sync.env,
                    Duration::from_secs(120),
                )
                .await
                .unwrap();
            assert_eq!(out.code, 0);
            let mut settings = io::read(&lock, None).unwrap();
            settings["settings"]["hangar-comet-runtime-fixture"] =
                json!({"message":"Preferência pessoal"});
            io::write(&lock, &settings).unwrap();
        }
        let report = sync.reconcile(false, &|| false).await;
        assert_eq!(report["errors"], json!([]), "{report}");
        assert_eq!(
            report["items"][0]["action"],
            if adopt { "adopt" } else { "install" }
        );
        let snapshot = fs::read(&lock).unwrap();
        assert_eq!(
            sync.reconcile(false, &|| false).await["items"][0]["action"],
            "unchanged"
        );
        assert_eq!(fs::read(&lock).unwrap(), snapshot);
        fs::write(
            source.join("commands/base.md"),
            "Segunda revisão, mesma versão.\n",
        )
        .unwrap();
        git(&env, &source, &["add", "commands/base.md"]).await;
        git(&env, &source, &["commit", "-m", "Segunda revisão"]).await;
        let next = git(&env, &source, &["rev-parse", "HEAD"]).await;
        assert_ne!(first, next);
        git(
            &env,
            &source,
            &["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        )
        .await;
        inventory(&home, &source, &url, &next, true);
        let report = sync.reconcile(false, &|| false).await;
        assert_eq!(report["errors"], json!([]), "{report}");
        assert_eq!(
            fs::read_to_string(package.join("commands/base.md")).unwrap(),
            "Segunda revisão, mesma versão.\n"
        );
        let settings = io::read(&lock, None).unwrap();
        assert_eq!(
            settings["plugins"]["hangar-comet-runtime-fixture"]["enabledFeatures"],
            if adopt {
                json!(["optional"])
            } else {
                Value::Null
            }
        );
        if adopt {
            assert_eq!(
                settings["settings"]["hangar-comet-runtime-fixture"],
                json!({"message":"Preferência pessoal"})
            );
        }
        let manifest = io::read(&root.join("package.json"), None).unwrap();
        let dependency = manifest["dependencies"]["hangar-comet-runtime-fixture"]
            .as_str()
            .unwrap();
        assert_eq!(
            dependency.strip_prefix("git+").unwrap_or(dependency),
            format!("{url}#{next}")
        );
        assert!(!truth(manifest.get("trustedDependencies")));
        inventory(&home, &source, &url, &next, false);
        let report = sync.reconcile(false, &|| false).await;
        assert_eq!(report["errors"], json!([]), "{report}");
        assert_eq!(
            io::read(&lock, None).unwrap()["plugins"]["hangar-comet-runtime-fixture"]["enabled"],
            false
        );
        json_file(
            &home.join(".claude/plugins/installed_plugins.json"),
            json!({"version":2,"plugins":{}}),
        );
        let report = sync.reconcile(false, &|| false).await;
        assert_eq!(report["errors"], json!([]), "{report}");
        assert!(!package.exists());
        assert!(
            io::read(&lock, None).unwrap()["plugins"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        server.0.abort();
        let _ = (&mut server.0).await;
    }
}

#[tokio::test]
#[ignore = "Requer CLI omp instalado; roda isolado, sem credenciais nem chamada a modelo."]
async fn native_marketplace_layouts_match_the_cli() {
    for layout in [
        "custom-agent",
        "xdg-absent",
        "xdg-default",
        "xdg-custom",
        "config-root",
        "profile-legacy",
        "profile-xdg",
        "canonical-default",
        "xdg-double-slash",
    ] {
        if cfg!(windows) && layout == "xdg-double-slash" {
            continue;
        }
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let mut env = BTreeMap::new();
        for key in ["PATH", "SYSTEMROOT", "WINDIR", "LANG"] {
            if let Ok(value) = std::env::var(key) {
                env.insert(key.into(), value);
            }
        }
        let data = home.join(".local/share");
        env.insert("XDG_DATA_HOME".into(), data.to_string_lossy().into_owned());
        env.insert(
            "XDG_CONFIG_HOME".into(),
            home.join(".config").to_string_lossy().into_owned(),
        );
        env.insert(
            "XDG_CACHE_HOME".into(),
            home.join(".cache").to_string_lossy().into_owned(),
        );
        let mut expected = home.join(".omp");
        if ["custom-agent", "xdg-custom"].contains(&layout) {
            env.insert(
                "PI_CODING_AGENT_DIR".into(),
                home.join("custom-agent").to_string_lossy().into_owned(),
            );
        }
        if [
            "xdg-default",
            "xdg-custom",
            "profile-legacy",
            "canonical-default",
            "xdg-double-slash",
        ]
        .contains(&layout)
        {
            fs::create_dir_all(data.join("omp")).unwrap();
        }
        match layout {
            "xdg-default" => {
                env.insert(
                    "PI_CODING_AGENT_DIR".into(),
                    home.join(".omp/agent").to_string_lossy().into_owned(),
                );
                if cfg!(any(target_os = "linux", target_os = "macos")) {
                    expected = data.join("omp");
                }
            }
            "config-root" => {
                env.insert("PI_CONFIG_DIR".into(), ".omp-alt".into());
                expected = home.join(".omp-alt");
            }
            "profile-legacy" | "profile-xdg" => {
                env.insert("OMP_PROFILE".into(), "review".into());
                env.insert(
                    "PI_CODING_AGENT_DIR".into(),
                    home.join("ignored-agent").to_string_lossy().into_owned(),
                );
                expected = home.join(".omp/profiles/review");
                if layout == "profile-xdg" {
                    fs::create_dir_all(data.join("omp/profiles/review")).unwrap();
                    if cfg!(any(target_os = "linux", target_os = "macos")) {
                        expected = data.join("omp/profiles/review");
                    }
                }
            }
            "canonical-default" => {
                env.insert("OMP_PROFILE".into(), String::new());
                env.insert("PI_PROFILE".into(), "review".into());
                env.insert(
                    "PI_CODING_AGENT_DIR".into(),
                    home.join(".omp/profiles/review/agent")
                        .to_string_lossy()
                        .into_owned(),
                );
                if cfg!(any(target_os = "linux", target_os = "macos")) {
                    expected = data.join("omp");
                }
            }
            "xdg-double-slash" => {
                env.insert(
                    "XDG_DATA_HOME".into(),
                    format!("//{}", data.to_string_lossy().trim_start_matches('/')),
                );
                expected = data.join("omp");
            }
            _ => {}
        }
        let catalog = home.join("catalog");
        json_file(
            &catalog.join(".claude-plugin/marketplace.json"),
            json!({"name":"layout-fixture","owner":{"name":"Fixture"},"plugins":[]}),
        );
        json_file(
            &home.join(".claude/plugins/known_marketplaces.json"),
            json!({"fixture":{"source":{"source":"directory","path":catalog},"installLocation":catalog}}),
        );
        let sync = Synchronizer::new(
            home.clone(),
            home.join(".claude"),
            env,
            Arc::new(DiagnosticNative),
        );
        assert_eq!(sync.directories().unwrap().data_root, expected, "{layout}");
        let report = sync.import_marketplaces(false, &|| false).await;
        assert_eq!(report["errors"], json!([]), "{layout}: {report}");
        let registry = expected.join("marketplaces.json");
        let contents = fs::read(&registry).unwrap();
        assert_eq!(
            io::read(&registry, None).unwrap()["marketplaces"][0]["name"],
            "layout-fixture",
            "{layout}"
        );
        assert_eq!(
            sync.import_marketplaces(false, &|| false).await["errors"],
            json!([]),
            "{layout}"
        );
        assert_eq!(fs::read(&registry).unwrap(), contents, "{layout}");
        assert!(!expected.join("plugins/package.json").exists(), "{layout}");
    }
}
