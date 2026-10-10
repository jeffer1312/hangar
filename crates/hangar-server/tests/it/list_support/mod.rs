//! Servidor da lista do dono com sessões Claude de mentira, para os testes das rotas e do acordar
//! por arquivo. O multiplexador é um script que conta as chamadas e recusa enquanto existir `fail`.
#![allow(dead_code)]
use crate::fake::{self, OWNER, SECRET, client, config, next_named, sse, Events};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() }

/// `n` sessões Claude paradas (`s0`..), marcador `idle`, pane pronto; `fail` presente = recusa.
pub fn sessions(root: &Path, n: usize) -> std::path::PathBuf {
    let home = root.join("home");
    let mut panes = String::new();
    for i in 0..n {
        let cwd = root.join(format!("w{i}"));
        let sid = format!("00000000-0000-0000-0000-{i:012}");
        let proj = home.join(".claude/projects").join(hangar_workspace::worktrees::sanitize_cwd(cwd.to_str().unwrap()));
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(proj.join(format!("{sid}.jsonl")), fake::claude_line(0)).unwrap();
        std::fs::create_dir_all(home.join(".claude/.hangar-state")).unwrap();
        std::fs::write(home.join(format!(".claude/.hangar-state/{sid}.json")), format!(r#"{{"state":"idle","ts":{}}}"#, now() + 60.0)).unwrap();
        panes.push_str(&format!("s{i}\\t1\\t\\t{}\\t%%{i}\\t\\t\\t\\t0\\t0\\n", cwd.display()));
    }
    let script = root.join("tmux");
    std::fs::write(&script, format!(
        "#!/bin/sh\necho \"$1\" >> '{log}'\n[ -e '{fail}' ] && exit 2\n[ \"$1\" = list-panes ] || {{ printf '● pronto\\n❯\\n'; exit 0; }}\nprintf '{panes}'\n",
        log = root.join("calls.log").display(), fail = root.join("fail").display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

pub fn calls(root: &Path, sub: &str) -> usize {
    std::fs::read_to_string(root.join("calls.log")).unwrap_or_default().lines().filter(|l| *l == sub).count()
}

pub struct Server { pub addr: SocketAddr, pub python: Arc<fake::Fake>, pub list: Arc<ListBridge> }

pub async fn server(root: &Path, n: usize) -> Server {
    let script = sessions(root, n);
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(config(upstream, ""));
    let list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    state.list = list.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, python, list }
}

pub async fn get(addr: SocketAddr, token: &str) -> (u16, Value) {
    let resp = client().get(format!("http://{addr}/api/sessions")).bearer_auth(token).send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

pub async fn open(addr: SocketAddr) -> Events {
    let resp = client().get(format!("http://{addr}/api/sessions/events?token={OWNER}")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["cache-control"], "no-store");
    sse(resp)
}

pub fn names(v: &Value) -> Vec<&str> { v.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect() }

pub fn set_facts(python: &fake::Fake, key: &str, value: Value) {
    python.list_facts.lock().unwrap().0[key] = value;
}


pub fn write_marker(root: &Path, i: usize, state: &str) {
    let sid = format!("00000000-0000-0000-0000-{i:012}");
    std::fs::write(root.join(format!("home/.claude/.hangar-state/{sid}.json")), format!(r#"{{"state":"{state}","ts":{}}}"#, now() + 60.0)).unwrap();
}

/// Próximo `sessions` em que a sessão `i` está em `state`, e quanto demorou desde `since`.
pub async fn until_state(es: &mut Events, i: usize, state: &str, since: std::time::Instant) -> (Value, Duration) {
    loop {
        let rows: Value = serde_json::from_str(&next_named(es, "sessions").await.data).unwrap();
        if rows.as_array().unwrap().iter().any(|r| r["name"] == format!("s{i}") && r["state"] == state) {
            return (rows, since.elapsed());
        }
    }
}

