//! Contexto de teste das rotas de Git/arquivos, com diário Rust em arquivo isolado.
#![allow(dead_code)]
use crate::fake::config;
use serde_json::{Value, json};
use std::sync::Arc;

pub type Diags = Arc<tempfile::TempDir>;

pub struct Fixture {
    pub addr: std::net::SocketAddr,
    pub list: Arc<hangar_server::list::bridge::ListBridge>,
    pub runtime: Arc<hangar_server::runtime::gateway::RuntimeRegistry>,
    pub policy: Option<Policy>,
    /// Pasta do diário local do Rust.
    pub diags: Diags,
    pub workspace_slots: Arc<tokio::sync::Semaphore>,
    _home: Option<tempfile::TempDir>,
}

pub struct Policy {
    pub guest: std::net::SocketAddr,
    pub login: Arc<hangar_server::login_guests::Service>,
    pub share: Arc<hangar_server::share::ShareState>,
}

impl Fixture {
    /// O diário sai numa tarefa à parte: espera até 5 s pelo evento.
    pub async fn diag(&self, event: &str) -> Value {
        crate::fake::wait_diary(self.diags.path(), |row| row["evento"] == event).await.into_iter().find(|row| row["evento"] == event).unwrap()
    }
}

pub async fn fixture(root: &std::path::Path) -> Fixture {
    fixture_with_home(root,None).await
}

pub async fn fixture_with_home(root: &std::path::Path, home: Option<&std::path::Path>) -> Fixture {
    fixture_with_mux(root,home,None).await
}

pub async fn session_fixture(root: &std::path::Path) -> Fixture {
    let fixture = fixture_with_cwd(root, None, None, root).await;
    fixture.list.seed("fixture", root.join("fixture.jsonl").to_str().unwrap());
    fixture
}

pub async fn fixture_with_mux(root: &std::path::Path, home: Option<&std::path::Path>, mux: Option<&std::path::Path>) -> Fixture {
    fixture_with_cwd(root, home, mux, &root.join("repo-wt")).await
}

pub async fn fixture_with_account_env(root: &std::path::Path, home: &std::path::Path, mux: &std::path::Path, accounts: hangar_server::accounts::environment::AccountEnvironment, router_only: bool) -> Fixture {
    build(root, Some(home), Some(mux), root, router_only, Some(accounts)).await
}

pub async fn policy_fixture(root: &std::path::Path, home: &std::path::Path, mux: &std::path::Path) -> Fixture {
    build(root, Some(home), Some(mux), root, true, None).await
}

async fn fixture_with_cwd(root: &std::path::Path, home: Option<&std::path::Path>, mux: Option<&std::path::Path>, session_cwd: &std::path::Path) -> Fixture {
    build(root, home, mux, session_cwd, false, None).await
}

async fn build(root: &std::path::Path, home: Option<&std::path::Path>, mux: Option<&std::path::Path>, session_cwd: &std::path::Path, policy: bool, account_env: Option<hangar_server::accounts::environment::AccountEnvironment>) -> Fixture {
    let diags: Diags = Arc::new(tempfile::tempdir().unwrap());
    let isolated_home=home.is_none().then(||tempfile::tempdir().unwrap());
    let home=home.unwrap_or_else(||isolated_home.as_ref().unwrap().path());
    let mut state=hangar_server::routes::AppState::new(config("127.0.0.1"));
    state.diag = state.diag.clone().with_diary(Some(diags.path().to_owned()));
    state.origins_home=home.into();
    state.accounts=hangar_server::accounts::AccountService::new(
        account_env.unwrap_or_else(||hangar_server::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(),home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(),home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(),root.to_string_lossy().into_owned()),
        ].into())));
    let mut env=hangar_server::list::bridge::ListEnv::from_env();
    env.dirs=hangar_server::list::bridge::parse_dirs(&json!({
        "home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),
        "omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")
    }).to_string());
    // Inventário vazio conhecido; a prova não depende do multiplexador instalado na máquina.
    #[cfg(unix)]
    let default_mux={
        let script=home.join("fixture-mux");
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&script,format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\nprintf 'fixture\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\n'\n",session_cwd.display())).unwrap();
        std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    };
    #[cfg(windows)]
    let default_mux={
        let script=home.join("fixture-mux.cmd");
        std::fs::write(&script,format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\necho fixture\t1\t\t{}\t%%1\t\t\t\t0\t0\r\n",session_cwd.display())).unwrap();
        script
    };
    env.mux=hangar_server::list::mux::Mux::with_program(mux.unwrap_or(&default_mux),std::time::Duration::from_secs(5));
    state.list=Arc::new(hangar_server::list::bridge::ListBridge::new(env,hangar_server::list::facts::FactsClient::new()));
    let workspace_slots=state.workspace_slots.clone();
    let list=state.list.clone();
    let runtime=Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new("workspace-fixture".into())
        .with_codex_environment(state.accounts.env.clone()));
    assert!(state.state.runtime.set(runtime.clone()).is_ok());
    let (addr, policy) = if policy {
        let (addr, policy) = serve_policy(state, home, mux.unwrap_or(&default_mux)).await;
        (addr, Some(policy))
    } else { (crate::fake::spawn_state(state).await, None) };
    Fixture { addr, list, runtime, policy, diags,workspace_slots,_home:isolated_home }
}

async fn serve_policy(mut state: hangar_server::routes::AppState, home: &std::path::Path, mux: &std::path::Path) -> (std::net::SocketAddr, Policy) {
    hangar_server::accounts::quotas::disable_open_network_read();
    hangar_server::diag::disable_diary();
    let login = Arc::new(hangar_server::login_guests::Service::new(hangar_server::login_guests::Settings {
        guests:home.join(".claude/guests.json"), vault:home.join("vault.json"),
        runtime_config:home.join(".claude/runtime-config.json"), sync_enabled:false,
        bootstrap:String::new(), secret:b"fixture".to_vec(), secure:false, rate_max:100.into(), rate_window:300.into(),
    }));
    let share = hangar_server::share::from_env(Some(home.join(".claude/shares.json")),
        state.list.env().dirs.as_ref(), mux.as_os_str().to_owned(), None).unwrap();
    state.login_guests = login.clone();
    state.share = Some(share.clone());
    state.side.locate = Arc::new(hangar_server::locate::ListLocate { list:state.list.clone() });
    let state = Arc::new(state);
    let owner = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner_addr = owner.local_addr().unwrap();
    let guest_addr = guest.local_addr().unwrap();
    let owner_app = hangar_server::routes::router(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>();
    let guest_app = hangar_server::routes::guest_router(state).into_make_service_with_connect_info::<hangar_server::share::gate::GuestConn>();
    tokio::spawn(async move { axum::serve(owner, owner_app).await.unwrap(); });
    tokio::spawn(async move { axum::serve(guest, guest_app).await.unwrap(); });
    (owner_addr, Policy { guest:guest_addr, login, share })
}

// A prova com tmux real usa metadados fixos e mantém o transporte dentro do Rust.
#[cfg(target_os = "linux")]
pub async fn real_terminal_fixture(
    home: &std::path::Path,
    metadata_mux: &std::path::Path,
    terminal_mux: &std::path::Path,
    environment: std::collections::BTreeMap<String, String>,
) -> (Fixture, Arc<hangar_server::routes::AppState>, tokio::task::JoinHandle<()>) {
    let mut state = hangar_server::routes::AppState::new(config("127.0.0.1"));
    state.accounts = hangar_server::accounts::AccountService::new(
        hangar_server::accounts::environment::AccountEnvironment::from_map(environment));
    state.origins_home = home.into();
    let mut env = hangar_server::list::bridge::ListEnv::from_env();
    env.dirs = hangar_server::list::bridge::parse_dirs(&json!({
        "home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),
        "omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")
    }).to_string());
    env.mux = hangar_server::list::mux::Mux::with_program(metadata_mux, std::time::Duration::from_secs(5));
    env.capture_program = terminal_mux.as_os_str().into();
    state.list = Arc::new(hangar_server::list::bridge::ListBridge::new(env,
        hangar_server::list::facts::FactsClient::new()));
    let runtime = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new("s01-real-terminal".into()).with_codex_environment(state.accounts.env.clone()));
    assert!(state.state.runtime.set(runtime.clone()).is_ok());
    state.side.locate = Arc::new(hangar_server::locate::ListLocate { list:state.list.clone() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let fixture = Fixture {
        addr, list:state.list.clone(),
        runtime, policy:None, diags:Arc::new(tempfile::tempdir().unwrap()), workspace_slots:state.workspace_slots.clone(), _home:None,
    };
    let state = Arc::new(state);
    let app = hangar_server::routes::router(state.clone())
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    (fixture, state, task)
}

/// Corpo do 503 de Git/arquivos: mesmo formato das outras rotas do Rust, com o motivo em `params`.
pub async fn refusal(response: reqwest::Response, code: &str) -> Value {
    assert_eq!(response.status(), 503);
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], code);
    assert_eq!(body["detail"]["code"], code);
    assert!(body["detail"]["msg"].as_str().is_some_and(|m| !m.is_empty()), "{body}");
    body
}
