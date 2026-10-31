#![allow(dead_code)]
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use hangar_server::auth::TrustedHosts;
use hangar_server::config::Config;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::locate::ListLocate;
use hangar_server::routes::{AppState, guest_router, router};
use hangar_server::share::gate::GuestConn;
use hangar_server::share::store::Kind;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const OWNER: &str = "remote-browser-contract-owner";
pub const SECRET: &str = "remote-browser-test-internal";

static ELECTRON_PORT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A pré-condição do ensaio de taxa (`transport_gaps`), conferida antes de medir: a rajada não coube no
/// tempo real porque a máquina estava lenta. As conferências depois dela medem o servidor e não se repetem.
pub const SLOW_RUN: &str = "rajada saiu da janela real de observação";
const SLOW_RUN_ATTEMPTS: u32 = 5;

#[cfg(unix)]
pub async fn isolated(test: &str, inspect: impl FnOnce(&Path)) -> bool {
    isolated_with_timeout(test, Duration::from_secs(20), inspect).await
}

#[cfg(unix)]
pub async fn isolated_with_timeout(test: &str, deadline: Duration, inspect: impl FnOnce(&Path)) -> bool {
    if std::env::var("HANGAR_REMOTE_BROWSER_TEST").as_deref() == Ok(test) { return false; }
    let needs_electron_port = test == "remote_browser::transport::electron_target_precedes_native_and_queries_live_url"
        || test == "remote_browser::revisions::evaluate_timeout_keeps_original_debug_and_sidecar_url"
        || test == "remote_browser::url_gaps::ws_electron_precedes_usable_native"
        || test.starts_with("remote_browser::url_gaps::ws_evaluate_");
    // Os filhos são isolados, mas compartilham a porta fixa do Electron.
    let _electron_port = if needs_electron_port { Some(ELECTRON_PORT.lock().await) } else { None };
    // Ensaio de taxa que a máquina não executou a tempo não prova nada: só ele é refeito, numa pasta nova.
    let mut attempts = 0;
    let (root, output) = loop {
    attempts += 1;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    let bin = git_probe(root.path());
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", test, "--nocapture"]).env_clear().current_dir(root.path()).kill_on_drop(true)
        .env("HANGAR_REMOTE_BROWSER_TEST", test).env("HANGAR_REMOTE_BROWSER_ROOT", root.path())
        .env("HOME", &home).env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join(".config")).env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("TMPDIR", root.path().join("tmp"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude")).env("CODEX_HOME", home.join(".codex"))
        .env("CP_CLAUDE_CONFIG_DIRS", home.join(".claude")).env("HANGAR_LIST_DIRS", dirs_json(&home))
        .env("HANGAR_SHARES_FILE", home.join("shares.json"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    let output = tokio::time::timeout(deadline, command.output()).await
        .expect("processo de ensaio não encerrou").unwrap();
    let slow = !output.status.success() && [&output.stdout, &output.stderr].iter()
        .any(|bytes| String::from_utf8_lossy(bytes).contains(SLOW_RUN));
    if !slow || attempts == SLOW_RUN_ATTEMPTS { break (root, output); }
    eprintln!("{test}: ensaio lento demais para medir (tentativa {attempts} de {SLOW_RUN_ATTEMPTS}), refazendo: {}",
        String::from_utf8_lossy(&output.stderr).lines().find(|l| l.contains(SLOW_RUN)).unwrap_or_default());
    };
    crate::assert_ran_one(&output.stdout);
    assert!(output.status.success(), "ensaio isolado falhou: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    // O filho já saiu: nenhuma tarefa dele pode escrever depois desta conferência.
    inspect(root.path());
    true
}

fn dirs_json(home: &Path) -> String {
    json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),
        "omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string()
}

#[cfg(unix)]
pub fn git_probe(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::write_executable(&bin.join("git"), format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n", root.join("git.calls").display()));
    bin
}

static CHROMIUM: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_slice(&std::fs::read(crate::common::contract().join("remote-browser/chromium.json")).unwrap()).unwrap()
});

/// Uma tabela por vez: cada teste isolado roda em processo próprio e só paga pela que usa.
pub fn contract(name: &str) -> Value {
    use super::{transport_cases as transport, url_cases as url};
    fn expanded(table: &str) -> Value {
        let mut value: Value = serde_json::from_str(table).unwrap();
        expand(&mut value, &CHROMIUM);
        value
    }
    static TRANSPORT: LazyLock<Value> = LazyLock::new(|| expanded(transport::CASES));
    static SUPPLEMENT: LazyLock<Value> = LazyLock::new(|| expanded(transport::SUPPLEMENT));
    static REVIEW: LazyLock<Value> = LazyLock::new(|| expanded(transport::REVIEW));
    static GAPS: LazyLock<Value> = LazyLock::new(|| expanded(super::transport_gaps_cases::CASES));
    static URL_COMMANDS: LazyLock<Value> = LazyLock::new(|| expanded(url::COMMANDS));
    static URL_GAPS: LazyLock<Value> = LazyLock::new(|| expanded(url::GAPS));
    let table: &Value = match name {
        "chromium" => &CHROMIUM,
        "transport" => &TRANSPORT,
        "transport-supplement" => &SUPPLEMENT,
        "transport-review-supplement" => &REVIEW,
        "transport-gaps" => &GAPS,
        "url-commands" => &URL_COMMANDS,
        "url-gaps" => &URL_GAPS,
        other => panic!("tabela desconhecida: {other}"),
    };
    table.clone()
}

/// As tabelas abreviam o quadro medido e as repetições longas: `⟦FRAME⟧`, `⟦SCREENSHOT⟧`, `⟦n×trecho⟧`.
fn expand(value: &mut Value, chromium: &Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|item| expand(item, chromium)),
        Value::Object(fields) => fields.values_mut().for_each(|item| expand(item, chromium)),
        Value::String(text) => *text = match text.as_str() {
            "⟦FRAME⟧" => chromium["frame"]["data"].as_str().unwrap().to_owned(),
            "⟦SCREENSHOT⟧" => chromium["screenshot"]["data"].as_str().unwrap().to_owned(),
            other => expand_runs(other),
        },
        _ => {}
    }
}

fn expand_runs(mut rest: &str) -> String {
    let mut text = String::new();
    while let Some((before, tail)) = rest.split_once('⟦') {
        let (run, after) = tail.split_once('⟧').unwrap();
        let (times, chunk) = run.split_once('×').unwrap();
        text.push_str(before);
        text.push_str(&chunk.repeat(times.parse().unwrap()));
        rest = after;
    }
    text.push_str(rest);
    text
}

/// O JPEG veio do Chromium medido, nunca de um gerador substituto do protocolo.
pub fn frozen_jpeg() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(contract("chromium")["frame"]["data"].as_str().unwrap()).unwrap()
}

pub struct Server {
    pub address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Server {
    pub async fn owner(app: Router) -> Self {
        Self::at(app, "127.0.0.1:0".parse().unwrap()).await
    }

    pub async fn at(app: Router, address: SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind(address).await.expect("porta exclusiva do ensaio indisponível");
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async { let _ = stopped.await; }).await.unwrap();
        });
        Self { address, stop: Some(stop), task: Some(task) }
    }

    pub async fn guest(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<GuestConn>())
                .with_graceful_shutdown(async { let _ = stopped.await; }).await.unwrap();
        });
        Self { address, stop: Some(stop), task: Some(task) }
    }

    pub async fn close(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let mut task = self.task.take().unwrap();
        match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("servidor de ensaio não encerrou em {}", self.address);
            }
        }
        crate::assert_closed(self.address, "servidor do navegador remoto").await;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() { task.abort(); }
    }
}

#[derive(Clone, Debug)]
pub struct Attempt { pub path: String, pub headers: HeaderMap, pub body: Vec<u8> }

/// A borda externa anota e recusa; nenhum `/info` de mentira pode satisfazer a descoberta.
pub struct RefusingServer { pub server: Server, pub attempts: Arc<Mutex<Vec<Attempt>>> }

impl RefusingServer {
    pub async fn new() -> Self {
        async fn refuse(State(attempts): State<Arc<Mutex<Vec<Attempt>>>>, req: Request) -> Response {
            let (parts, body) = req.into_parts();
            let body = to_bytes(body, 1024 * 1024).await.unwrap();
            let head = parts.method == axum::http::Method::HEAD
                && parts.uri.path().ends_with("/navegador");
            attempts.lock().unwrap().push(Attempt { path: parts.uri.to_string(), headers: parts.headers, body: body.to_vec() });
            if head {
                return (StatusCode::METHOD_NOT_ALLOWED, [("allow","GET"),("content-type","application/json")],
                    r#"{"detail":"Method Not Allowed"}"#).into_response();
            }
            (StatusCode::SERVICE_UNAVAILABLE, "remote_browser_test_upstream_refused").into_response()
        }
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let server = Server::owner(Router::new().fallback(refuse).with_state(attempts.clone())).await;
        Self { server, attempts }
    }

    pub fn paths(&self) -> Vec<String> { self.attempts.lock().unwrap().iter().map(|a| a.path.clone()).collect() }

    pub fn assert_no_browser_or_discovery_calls(&self) {
        for path in self.paths() {
            let path = path.split('?').next().unwrap();
            assert!(!path.contains("/nav-remoto") && !path.ends_with("/navegador")
                && !path.ends_with("/info") && path != "/internal/list/facts", "consulta indevida ao Python: {path}");
        }
    }
}

#[cfg(unix)]
pub struct Scene {
    pub root: tempfile::TempDir,
    pub home: PathBuf,
    pub state: Arc<AppState>,
    pub owner: Server,
    pub guest: Server,
    pub upstream: RefusingServer,
}

#[cfg(unix)]
impl Scene {
    pub async fn new() -> Self {
        let isolated = PathBuf::from(std::env::var_os("HANGAR_REMOTE_BROWSER_ROOT").expect("Scene exige processo isolado"));
        let home = isolated.join("home");
        assert_eq!(std::env::var_os("HOME"), Some(home.clone().into_os_string()), "HOME fora do ensaio");
        let root = tempfile::tempdir_in(&isolated).unwrap();
        let discovery = crate::list_support::sessions(root.path(), 1);
        let script = root.path().join("remote-browser-mux");
        // A lista e o porteiro consultam formatos diferentes do mesmo terminal de ensaio.
        crate::write_executable(&script, format!(r#"#!/bin/sh
if [ "$1" = display-message ] && [ "$5" = '#{{session_created}}' ]; then
    printf 'display-message\n' >> '{log}'
    [ "$2" = -p ] && [ "$3" = -t ] && [ "$4" = '=s0:' ] || exit 1
    printf '1\n'
    exit 0
fi
if [ "$1" = has-session ]; then
    printf 'has-session\n' >> '{log}'
    [ "$2" = -t ] && [ "$3" = '=s0' ]
    exit $?
fi
exec '{discovery}' "$@"
"#, log = root.path().join("calls.log").display(), discovery = discovery.display()));
        // O multiplexador de ensaio e o construtor leem as mesmas fontes temporárias.
        let source = root.path().join("home/.claude");
        std::fs::rename(source.join("projects"), home.join(".claude/projects")).unwrap();
        std::fs::rename(source.join(".hangar-state"), home.join(".claude/.hangar-state")).unwrap();
        let dirs = parse_dirs(&dirs_json(&home));
        let upstream = RefusingServer::new().await;
        let cfg = Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: upstream.server.address,
            internal_secret: SECRET.into(), auth_token: OWNER.into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") };
        let mut state = AppState::new(cfg.clone());
        let list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(2)),
            capture_program: script.clone().into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
            FactsClient::new(cfg.upstream, SECRET.into())));
        state.hangar_dir = home.join(".hangar");
        std::fs::create_dir_all(state.hangar_dir.join("nav")).unwrap();
        Self::write_headless(&home, "headless", "headless-key", root.path());
        state.share = hangar_server::share::from_env(Some(home.join("shares.json")), list.env().dirs.as_ref(), script.into_os_string());
        list.set_shares(state.share.as_ref().unwrap().clone());
        state.list = list.clone();
        state.side.locate = Arc::new(ListLocate { list: list.clone() });
        state.side.queue_dir = Some(home.join(".claude/.hangar-queue"));
        state.state = Arc::new(hangar_server::state::live::StateEnv::new(state.terminal.clone(), list, state.diag.clone()));
        state.side.monitors = Some(hangar_server::state::live::spawner(state.state.clone()));
        let state = Arc::new(state);
        let owner = Server::owner(router(state.clone())).await;
        let guest = Server::guest(guest_router(state.clone())).await;
        Self { root, home, state, owner, guest, upstream }
    }

    fn write_headless(home: &Path, name: &str, key: &str, cwd: &Path) {
        let dir = home.join(".hangar/claude-headless");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), json!({"name":name, "key":key, "session_id":"headless-transcript", "cwd":cwd}).to_string()).unwrap();
    }

    pub fn nav_dir(&self) -> PathBuf { self.state.hangar_dir.join("nav") }

    pub async fn guest_token(&self, name: &str, kind: Kind) -> String {
        let share = self.state.share.as_ref().unwrap();
        let life = share.lives.life(name, true).await.expect("sessão de ensaio com identidade real");
        share.store.create_redeemed(name, Some(life), kind).unwrap().1
    }

    pub async fn close(self) {
        self.owner.close().await;
        self.guest.close().await;
        self.upstream.server.close().await;
    }
}
