//! `codex` falso para o lançador (`hangar-server codex-tui`): app-server WebSocket, TUI, `--version` e
//! a conferência dos marketplaces. O comportamento vem de `$HANGAR_FAKE_CODEX/config.json`, e cada
//! processo grava argv, ambiente e o que recebeu em `$HANGAR_FAKE_CODEX/spawn.jsonl`. Nunca chama rede
//! fora do loopback nem lê credenciais.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

fn dir() -> PathBuf { PathBuf::from(std::env::var_os("HANGAR_FAKE_CODEX").expect("HANGAR_FAKE_CODEX")) }

fn config() -> Value {
    std::fs::read(dir().join("config.json")).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_else(|| json!({}))
}

fn log(line: Value) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(dir().join("spawn.jsonl")).unwrap();
    file.write_all(format!("{line}\n").as_bytes()).unwrap();
}

fn spawned(role: &str, args: &[String]) {
    let env: serde_json::Map<String, Value> = std::env::vars().map(|(k, v)| (k, Value::from(v))).collect();
    log(json!({"role": role, "argv": args, "env": env, "pid": std::process::id(),
        "cwd": std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()}));
}

/// Quantas vezes o app-server já subiu neste caso (a primeira é 1).
fn launch() -> u64 {
    let path = dir().join("launches");
    let count = std::fs::read_to_string(&path).ok().and_then(|text| text.trim().parse::<u64>().ok()).unwrap_or(0) + 1;
    std::fs::write(&path, count.to_string()).unwrap();
    count
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = config();
    let code = match args.first().map(String::as_str) {
        Some("--version") => {
            println!("codex-cli {}", config["version"].as_str().unwrap_or("0.160.0"));
            0
        }
        Some("plugin") => {
            spawned("codex plugin marketplace upgrade", &args);
            println!("{}", config["marketplaces"]["stdout"].as_str().unwrap_or(r#"{"upgradedRoots":[]}"#));
            config["marketplaces"]["rc"].as_i64().unwrap_or(0) as i32
        }
        Some("app-server") => {
            spawned("codex app-server", &args);
            tokio::runtime::Runtime::new().unwrap().block_on(app_server(&args, &config))
        }
        Some("--remote" | "resume") => {
            spawned("codex tui", &args);
            tokio::runtime::Runtime::new().unwrap().block_on(tui(&args, &config))
        }
        _ => {
            spawned("codex unknown", &args);
            78
        }
    };
    std::process::exit(code);
}

fn endpoint_of(args: &[String], flag: &str) -> String {
    args.iter().position(|a| a == flag).and_then(|at| args.get(at + 1)).cloned().expect(flag)
}

async fn app_server(args: &[String], config: &Value) -> i32 {
    let number = launch();
    let server = &config["app_server"];
    let launches = server["launches"].as_array().cloned().unwrap_or_default();
    let this = launches.get(number as usize - 1).cloned().unwrap_or_else(|| json!({}));
    if let Some(rc) = this["die_at_start"].as_i64() {
        eprint!("{}", this["stderr"].as_str().unwrap_or(""));
        return rc as i32;
    }
    if this["never_binds"].as_bool() == Some(true) {
        tokio::time::sleep(Duration::from_secs(120)).await;
        return 0;
    }
    let endpoint = endpoint_of(args, "--listen");
    let listener = tokio::net::TcpListener::bind(endpoint.trim_start_matches("ws://")).await.unwrap();
    let (tui_seen, _) = tokio::sync::broadcast::channel::<()>(4);
    let crash = this["crash_after_tui_ms"].as_u64();
    let threads = server["threads"].clone();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { continue };
        let (sender, mut seen) = (tui_seen.clone(), tui_seen.subscribe());
        let (threads, server, announce) = (threads.clone(), server.clone(), this["announce"].clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    message = ws.next() => {
                        let Some(Ok(message)) = message else { return };
                        if !message.is_text() { continue; }
                        let request: Value = serde_json::from_slice(&message.into_data()).unwrap_or(Value::Null);
                        log(json!({"role": "app-server-rpc", "launch": number, "msg": request}));
                        let reply = match request["method"].as_str() {
                            Some("initialize") => json!({"id": request["id"], "result": {}}),
                            Some("hooks/list") => server.get("hooks_list").cloned().unwrap_or_else(|| json!({"result": {"data": []}})),
                            Some("config/batchWrite") => server.get("batch").cloned().unwrap_or_else(|| json!({"result": {}})),
                            _ => json!({"result": {}}),
                        };
                        let mut reply = reply;
                        reply["id"] = request["id"].clone();
                        if ws.send(Message::Text(reply.to_string().into())).await.is_err() { return; }
                        if request["params"]["clientInfo"]["name"] == "fake-tui" { let _ = sender.send(()); }
                        // Conversa que esta subida anuncia a quem conecta (a `/new` depois de uma queda).
                        if request["method"] == "initialize" {
                            for thread in announce.as_array().into_iter().flatten() {
                                let started = json!({"method": "thread/started", "params": {"thread": thread}});
                                if ws.send(Message::Text(started.to_string().into())).await.is_err() { return; }
                            }
                        }
                    }
                    Ok(()) = seen.recv() => {
                        for thread in threads.as_array().into_iter().flatten() {
                            let started = json!({"method": "thread/started", "params": {"thread": thread}});
                            if ws.send(Message::Text(started.to_string().into())).await.is_err() { return; }
                        }
                    }
                }
            }
        });
        if let Some(ms) = crash {
            let mut seen = tui_seen.subscribe();
            tokio::spawn(async move {
                let _ = seen.recv().await;
                tokio::time::sleep(Duration::from_millis(ms)).await;
                eprintln!("fake app-server crashed");
                std::process::exit(9);
            });
        }
    }
}

/// O arquivo da sessão como a TUI o vê enquanto roda.
fn sidecar() -> Option<Value> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let name = std::env::var("CP_SESSION_NAME").ok()?;
    let raw = std::fs::read(home.join(".hangar/codex-sessions").join(format!("{name}.json"))).ok()?;
    serde_json::from_slice(&raw).ok()
}

async fn tui(args: &[String], config: &Value) -> i32 {
    let endpoint = endpoint_of(args, "--remote");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut socket = None;
    while socket.is_none() && tokio::time::Instant::now() < deadline {
        match tokio_tungstenite::connect_async(endpoint.as_str()).await {
            Ok((ws, _)) => socket = Some(ws),
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let Some(mut ws) = socket else {
        log(json!({"role": "tui-connect-failed"}));
        return 1;
    };
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"clientInfo": {"name": "fake-tui"}}});
    ws.send(Message::Text(hello.to_string().into())).await.unwrap();
    // Espera o lançador gravar o pid da TUI no arquivo da sessão (ou desiste em 3 s) e guarda o que viu.
    let wait = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut seen = sidecar();
    while seen.as_ref().is_none_or(|meta| meta.get("tui_pid").is_none()) && tokio::time::Instant::now() < wait {
        tokio::time::sleep(Duration::from_millis(50)).await;
        seen = sidecar();
    }
    log(json!({"role": "tui-sidecar-snapshot", "sidecar": seen}));
    // A TUI segue viva mesmo com o app-server caído (a de verdade tenta reconectar por ~1 min).
    tokio::spawn(async move { while let Some(Ok(_)) = ws.next().await {} });
    tokio::time::sleep(Duration::from_millis(config["tui"]["hold_ms"].as_u64().unwrap_or(300))).await;
    if Path::new(&dir()).join("tui-hang").exists() {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    log(json!({"role": "tui-sidecar-final", "sidecar": sidecar()}));
    config["tui"]["rc"].as_i64().unwrap_or(0) as i32
}
