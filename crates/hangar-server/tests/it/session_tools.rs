//! Rotas de abertura usam contexto, autorização e efeitos em processo no Rust.
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use hangar_core::guest::{Caller, GuestGrant, Kind};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use hangar_server::runtime::gateway::RuntimeRegistry;

use crate::fake::{OWNER, SECRET, config};

struct Scene {
    _root: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    state: Arc<AppState>,
    app: axum::Router,
    hits: Arc<AtomicUsize>,
    upstream: tokio::task::JoinHandle<()>,
}
impl Drop for Scene {
    fn drop(&mut self) {
        reap_fake(&self.home.join("editor-pid"), false);
        reap_fake(&self.home.join("terminal-pid"), true);
        self.upstream.abort();
    }
}
fn reap_fake(path: &Path, group: bool) {
    let Some(pid) = std::fs::read_to_string(path)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
    else {
        return;
    };
    let mut status = 0;
    // O waitpid impede sinalizar um número que já deixou de ser nosso filho.
    let mut observed = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    while observed < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
        observed = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    }
    if observed == 0 {
        unsafe {
            let killed = libc::kill(if group { -pid } else { pid }, libc::SIGKILL);
            let signal_error = std::io::Error::last_os_error().raw_os_error();
            let mut waited = libc::waitpid(pid, &mut status, 0);
            while waited < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
            {
                waited = libc::waitpid(pid, &mut status, 0);
            }
            let reaped = waited == pid
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
            if !std::thread::panicking() {
                assert!(
                    killed == 0 || signal_error == Some(libc::ESRCH),
                    "o processo falso não foi encerrado"
                );
                assert!(reaped, "o processo falso não foi recolhido");
            }
        }
    }
}
impl Scene {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("repo with spaces");
        let bin = home.join("bin");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let mux = bin.join("mux");
        crate::write_executable(
            &mux,
            format!(
                r#"#!/bin/sh
base='{}'
case "$1" in
list-panes) printf 'fixture\t1\t\t{}\t%%1\t\tclaude\t1700000000\t0\t0\n' ;;
has-session)
  case "$3" in '=fixture') exit 0 ;; '=term-fixture') test -f "$base/shell-alive" ;; *) exit 1 ;; esac ;;
show-options) test -f "$base/shell-marked" && printf '1\n' ;;
display) printf '%s\n' '{}' ;;
new-session) printf '%s\n' "$@" > "$base/create-args"; : > "$base/shell-alive" ;;
set-option) : > "$base/shell-marked" ;;
kill-session) /bin/rm -f "$base/shell-alive" "$base/shell-marked" ;;
esac
"#,
                home.display(),
                cwd.display(),
                cwd.display()
            ),
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        let upstream = axum::Router::new().fallback(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::Relaxed);
                (StatusCode::BAD_GATEWAY, "unexpected-python")
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let mut state = AppState::new(config(upstream_addr, ""));
        state.origins_home = home.clone();
        state.accounts = hangar_server::accounts::AccountService::new(
            hangar_server::accounts::environment::AccountEnvironment::from_map(BTreeMap::from([
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
                (
                    "CLAUDE_CONFIG_DIR".into(),
                    home.join(".claude").to_string_lossy().into_owned(),
                ),
                ("PATH".into(), bin.to_string_lossy().into_owned()),
                (
                    "RESULT".into(),
                    home.join("result").to_string_lossy().into_owned(),
                ),
                ("CP_TERMINAL".into(), "kitty".into()),
                ("HANGAR_INTERNAL_SECRET".into(), "must-not-leak".into()),
                ("CP_AUTH_TOKEN".into(), "must-not-leak".into()),
                ("HANGAR_RUNTIME_INSTANCE".into(), "must-not-leak".into()),
            ])),
        );
        let list=Arc::new(ListBridge::new(ListEnv {
            mux:Mux::with_program(&mux,Duration::from_secs(10)),capture_program:mux.into_os_string(),
            procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),
            dirs:parse_dirs(&json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
                "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),
                "omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string()),
        },FactsClient::new(upstream_addr,SECRET.into())));
        state.list = list.clone();
        state.side.locate = Arc::new(hangar_server::locate::ListLocate { list });
        let runtime = Arc::new(
            RuntimeRegistry::new(upstream_addr, SECRET.into(), "session-tools-test".into())
                .with_codex_environment(state.accounts.env.clone()),
        );
        assert!(state.state.runtime.set(runtime).is_ok());
        let state = Arc::new(state);
        let app = router(state.clone());
        Self {
            _root: root,
            home,
            cwd,
            state,
            app,
            hits,
            upstream,
        }
    }
    async fn request(
        &self,
        name: &str,
        operation: &str,
        owner: bool,
        guest: Option<Kind>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/api/sessions/{name}/{operation}"));
        if owner {
            builder = builder.header("authorization", format!("Bearer {OWNER}"));
        }
        let mut request = builder.body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
        if let Some(kind) = guest {
            request
                .extensions_mut()
                .insert(Caller::Guest(Arc::new(GuestGrant {
                    token: "test-guest".into(),
                    token_hash: "test-hash".into(),
                    sessions: HashMap::from([("fixture".into(), kind)]),
                    pair_roots: vec![],
                    invite: true,
                })));
        }
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }
    fn no_python(&self) {
        assert_eq!(self.hits.load(Ordering::Relaxed), 0);
    }
    async fn finish(&self) {
        self.state
            .state
            .runtime
            .get()
            .unwrap()
            .shutdown()
            .await
            .unwrap();
        self.no_python();
    }
}

// Cada caso nasce sem o ambiente e a sonda systemd da máquina do programador.
fn isolated(test: &str) -> bool {
    if std::env::var("SESSION_TOOLS_CHILD").as_deref() == Ok(test) {
        return false;
    }
    let root = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .env("SESSION_TOOLS_CHILD", test)
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .env("TMPDIR", root.path())
        .env("PATH", "/usr/bin:/bin");
    if let Some(value) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", value);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[tokio::test]
async fn session_tools_auth_and_missing_targets_never_reach_python() {
    if isolated("session_tools::session_tools_auth_and_missing_targets_never_reach_python") {
        return;
    }
    let scene = Scene::new().await;
    for operation in ["shell", "open-editor", "open-terminal"] {
        assert_eq!(
            scene.request("fixture", operation, false, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, body) = scene.request("missing", operation, true, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{operation}: {body}");
        assert_eq!(body["detail"]["code"], "erro_sessao_inexistente");
        assert_eq!(
            scene
                .request("fixture", operation, false, Some(Kind::Pair))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for operation in ["open-editor", "open-terminal"] {
        assert_eq!(
            scene
                .request("fixture", operation, false, Some(Kind::Share))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    scene.finish().await;
}

#[tokio::test]
async fn session_tools_shell_is_hidden_reused_and_cannot_seize_an_existing_name() {
    if isolated(
        "session_tools::session_tools_shell_is_hidden_reused_and_cannot_seize_an_existing_name",
    ) {
        return;
    }
    let scene = Scene::new().await;
    std::fs::write(scene.home.join("shell-alive"), "").unwrap();
    let (status, body) = scene.request("fixture", "shell", true, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["detail"]["code"], "erro_sessao_tmux_em_uso");
    assert!(!scene.home.join("create-args").exists());
    std::fs::remove_file(scene.home.join("shell-alive")).unwrap();
    let (status, body) = scene.request("fixture", "shell", true, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"ok":true,"shell":"term-fixture"}));
    assert!(scene.home.join("shell-marked").exists());
    let args = std::fs::read_to_string(scene.home.join("create-args")).unwrap();
    assert!(args.contains("\n-s\nterm-fixture\n"));
    assert!(args.ends_with(&format!("-c\n{}\n", scene.cwd.display())));
    std::fs::remove_file(scene.home.join("create-args")).unwrap();
    assert_eq!(
        scene.request("fixture", "shell", true, None).await.0,
        StatusCode::OK
    );
    assert!(!scene.home.join("create-args").exists());
    assert_eq!(
        scene
            .request("fixture", "shell", false, Some(Kind::Share))
            .await
            .0,
        StatusCode::OK
    );
    scene.finish().await;
}

#[tokio::test]
async fn session_tools_ingress_hold_blocks_all_effects() {
    if isolated("session_tools::session_tools_ingress_hold_blocks_all_effects") {
        return;
    }
    let scene = Scene::new().await;
    let runtime = scene.state.state.runtime.get().unwrap();
    runtime
        .ingress()
        .hold("fixture", Duration::from_secs(10))
        .await
        .unwrap();
    for operation in ["shell", "open-editor", "open-terminal"] {
        let (status, body) = scene.request("fixture", operation, true, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{operation}: {body}");
        assert_eq!(body["detail"]["code"], "session_transfer_busy");
    }
    runtime.ingress().release("fixture");
    assert!(!scene.home.join("create-args").exists());
    assert!(!scene.home.join("result").exists());
    scene.finish().await;
}

#[tokio::test]
async fn session_tools_terminal_accepts_the_hidden_shell_without_a_list_row() {
    if isolated("session_tools::session_tools_terminal_accepts_the_hidden_shell_without_a_list_row")
    {
        return;
    }
    let scene = Scene::new().await;
    let signal = Signal::new(&scene.home.join("terminal-done"));
    crate::write_executable(
        &scene.home.join("bin/kitty"),
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$HOME/terminal-pid\"\nprintf '%s\\n' \"$@\" > \"$RESULT\"\nprintf 'secret=%s\\ntoken=%s\\ninstance=%s\\n' \"${HANGAR_INTERNAL_SECRET+x}\" \"${CP_AUTH_TOKEN+x}\" \"${HANGAR_RUNTIME_INSTANCE+x}\" >> \"$RESULT\"\nprintf done > \"$HOME/terminal-done\"\n",
    );
    std::fs::write(scene.home.join("shell-alive"), "").unwrap();
    let (status, body) = scene
        .request("term-fixture", "open-terminal", true, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"ok":true}));
    signal.wait().await;
    assert_eq!(
        std::fs::read_to_string(scene.home.join("result")).unwrap(),
        "tmux\nattach\n-t\n=term-fixture:\nsecret=\ntoken=\ninstance=\n"
    );
    scene.finish().await;
}

struct Signal(tokio::io::unix::AsyncFd<std::fs::File>);
impl Signal {
    fn new(path: &Path) -> Self {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .unwrap();
        Self(tokio::io::unix::AsyncFd::new(file).unwrap())
    }
    async fn wait(&self) {
        use std::io::Read;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut ready = self.0.readable().await.unwrap();
                match ready.try_io(|file| {
                    let mut bytes = [0; 16];
                    file.get_ref().read(&mut bytes)
                }) {
                    Ok(Ok(count)) if count > 0 => return,
                    Ok(Err(error)) => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("sinal do editor");
    }
}

#[tokio::test]
async fn session_tools_editor_uses_live_configuration_and_the_session_directory() {
    if isolated(
        "session_tools::session_tools_editor_uses_live_configuration_and_the_session_directory",
    ) {
        return;
    }
    let scene = Scene::new().await;
    let signal = Signal::new(&scene.home.join("editor-done"));
    let editor = scene.home.join("bin/editor");
    crate::write_executable(
        &editor,
        format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$HOME/editor-pid\"\nprintf '%s\\n' \"$#\" \"$1\" \"$HOME\" > \"$RESULT\"\nprintf 'secret=%s\\ntoken=%s\\ninstance=%s\\n' \"${{HANGAR_INTERNAL_SECRET+x}}\" \"${{CP_AUTH_TOKEN+x}}\" \"${{HANGAR_RUNTIME_INSTANCE+x}}\" >> \"$RESULT\"\nprintf done > '{}'\n",
            scene.home.join("editor-done").display()
        ),
    );
    std::fs::write(
        scene.home.join(".claude/runtime-config.json"),
        json!({"editor":editor}).to_string(),
    )
    .unwrap();
    let (status, body) = scene.request("fixture", "open-editor", true, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"ok":true}));
    signal.wait().await;
    assert_eq!(
        std::fs::read_to_string(scene.home.join("result")).unwrap(),
        format!(
            "1\n{}\n{}\nsecret=\ntoken=\ninstance=\n",
            scene.cwd.display(),
            scene.home.display()
        )
    );
    std::fs::write(
        scene.home.join(".claude/runtime-config.json"),
        json!({"editor":scene.home.join("missing-editor")}).to_string(),
    )
    .unwrap();
    let (status, body) = scene.request("fixture", "open-editor", true, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["detail"]["code"], "erro_editor_falhou");
    scene.finish().await;
}

#[tokio::test]
async fn session_tools_trailing_slash_redirects_preserve_the_query() {
    if isolated("session_tools::session_tools_trailing_slash_redirects_preserve_the_query") {
        return;
    }
    let scene = Scene::new().await;
    for operation in ["shell", "open-editor", "open-terminal"] {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/api/sessions/fixture/{operation}/?keep=1"))
            .header("authorization", format!("Bearer {OWNER}"))
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
        let response = scene.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert!(
            response.headers()["location"]
                .to_str()
                .unwrap()
                .ends_with(&format!("/api/sessions/fixture/{operation}?keep=1"))
        );
    }
    assert!(!scene.home.join("create-args").exists());
    assert!(!scene.home.join("result").exists());
    scene.finish().await;
}
