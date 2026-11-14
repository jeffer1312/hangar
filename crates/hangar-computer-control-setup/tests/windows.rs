//! Os ramos do servidor Windows: alvos, entrada do MCP e a troca do exe em uso. Esperado do Python
//! (ponta 7ef9011ba): `test_package_entry_on_windows_uses_windows_binary` e as regras de
//! `create_target`/`_targets` com `os.name == "nt"`; a troca é a D3.4 (exe em uso vai a `.anterior`).
#![cfg(windows)]
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use hangar_computer_control_setup::{
    Aux, Facts, Github, Paths, Policy, REPO, SaveRequest, Swapped, TargetRequest, Transport, create_target, save, targets,
    update_tick,
};

fn paths() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    (root, Paths::new(home, Vec::new()))
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// ROT-25: num servidor Windows todo alvo é Windows, também o local.
#[test]
fn rot25_every_target_is_windows_on_a_windows_server() {
    let (_root, paths) = paths();
    std::fs::create_dir_all(paths.targets_dir()).unwrap();
    std::fs::write(paths.targets_dir().join("pc-agent.json"), r#"{"transport": "local"}"#).unwrap();
    std::fs::write(paths.targets_dir().join("srv-agent.json"), r#"{"transport": "ssh", "host": "h"}"#).unwrap();
    assert_eq!(targets(&paths), [
        json!({"name": "pc", "path": text(&paths.targets_dir().join("pc-agent.json")), "transport": "local", "host": "", "os": "windows"}),
        json!({"name": "srv", "path": text(&paths.targets_dir().join("srv-agent.json")), "transport": "ssh", "host": "h", "os": "windows"}),
    ]);
}

/// ROT-33: alvo local no Windows roda o `windows-agent.exe` do pacote, sem argumento nem prazo.
#[test]
fn rot33_local_target_runs_the_windows_agent() {
    let (_root, paths) = paths();
    let request = TargetRequest {
        project_dir: String::new(),
        name: "este-pc".into(),
        transport: Transport::Local,
        host: String::new(),
        proxy_command: String::new(),
        request_timeout: None,
    };
    create_target(&paths, &request).unwrap();
    let written = std::fs::read_to_string(paths.targets_dir().join("este-pc-agent.json")).unwrap();
    assert!(written.ends_with('\n'), "{written:?}");
    let written: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(written, json!({"transport": "local", "command": [text(&paths.exe())]}));
}

/// HIS-33 (`test_package_entry_on_windows_uses_windows_binary`): ligar no Windows grava o
/// `windows-agent.exe` como comando do MCP, sem argumentos.
#[test]
fn his33_package_entry_is_the_windows_agent_without_args() {
    let (_root, paths) = paths();
    std::fs::create_dir_all(paths.targets_dir()).unwrap();
    std::fs::write(paths.exe(), "MZ-v1.0.0").unwrap();
    std::fs::write(paths.install_json(), "{\n  \"tag\": \"v1.0.0\"\n}").unwrap();
    let agent = paths.targets_dir().join("pc-agent.json");
    std::fs::write(&agent, r#"{"transport": "local"}"#).unwrap();
    let body: SaveRequest = serde_json::from_value(json!({"enabled": true, "mode": "package", "agent_config": text(&agent)})).unwrap();
    save(&paths, &Facts::default(), &Aux::default(), &body, || Ok(Map::new())).unwrap();
    let main: Value = serde_json::from_slice(&std::fs::read(paths.main_file()).unwrap()).unwrap();
    let entry = &main["mcpServers"]["hangar-computer-control"];
    assert_eq!(entry["command"], json!(text(&paths.home.join(".hangar").join("computer-control").join("windows-agent.exe"))));
    assert_eq!(entry["args"], json!([]));
}

/// O GitHub de mentira: a release e os assets por caminho.
#[derive(Default)]
struct Fake {
    release: Mutex<Option<Value>>,
    assets: Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

async fn serve(State(fake): State<Arc<Fake>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if path == format!("/repos/{REPO}/releases/latest") {
        return match fake.release.lock().unwrap().clone() {
            Some(release) => ([(axum::http::header::CONTENT_TYPE, "application/json")], release.to_string()).into_response(),
            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    match fake.assets.lock().unwrap().get(&path) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// ROT-86, B2, D3.4: a troca com o `windows-agent.exe` rodando não derruba o processo; o novo toma
/// o lugar e o que estava em uso fica como `windows-agent.exe.anterior`.
#[tokio::test(flavor = "multi_thread")]
async fn rot86_swaps_the_exe_in_use() {
    let (root, paths) = paths();
    let fake = Arc::new(Fake::default());
    let app = axum::Router::new().fallback(serve).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let github = Github { api: base.clone(), download: base };
    let http = hangar_core::net::http_client().build().unwrap();

    // Instalado v0.6.0 com um exe de verdade no lugar do agente, rodando.
    std::fs::create_dir_all(paths.targets_dir()).unwrap();
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    std::fs::copy(Path::new(&system).join("System32").join("PING.EXE"), paths.exe()).unwrap();
    std::fs::write(paths.install_json(), "{\n  \"tag\": \"v0.6.0\"\n}").unwrap();
    let old = std::fs::read(paths.exe()).unwrap();
    let mut live = std::process::Command::new(paths.exe())
        .args(["-n", "300", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();

    *fake.release.lock().unwrap() = Some(json!({"tag_name": "v0.7.0", "body": "", "assets": [
        {"name": "windows-agent.exe"}, {"name": "hangar-computer-control-linux-x86_64"}
    ]}));
    {
        let mut assets = fake.assets.lock().unwrap();
        assets.insert(format!("/{REPO}/releases/download/v0.7.0/windows-agent.exe"), b"MZ-v0.7.0".to_vec());
        assets.insert(format!("/{REPO}/releases/download/v0.7.0/hangar-computer-control-linux-x86_64"), b"\x7fELF-v0.7.0".to_vec());
    }
    let policy = Policy { automations: true, package_channel: Some("latest".into()) };
    let swapped = update_tick(&paths, &http, &github, &policy, Some(&root.path().join("diario"))).await;
    let still_running = live.try_wait().unwrap().is_none();
    let _ = live.kill();
    let _ = live.wait();
    server.abort();
    assert_eq!(swapped.unwrap(), Swapped::Yes { tag: "v0.7.0".into() });
    assert!(still_running, "o processo do agente caiu na troca");
    assert_eq!(std::fs::read(paths.exe()).unwrap(), b"MZ-v0.7.0");
    assert_eq!(std::fs::read(paths.install_dir().join("windows-agent.exe.anterior")).unwrap(), old);
}
