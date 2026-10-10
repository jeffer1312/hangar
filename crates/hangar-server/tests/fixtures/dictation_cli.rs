//! CLI de fixture: conversa por stdio e recusa ambiente ou customizações indevidas.
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(index) = args
        .iter()
        .position(|arg| matches!(arg.as_str(), "--pipe-child" | "--hold-connect"))
    {
        let address = &args[index + 1];
        let mut socket = std::net::TcpStream::connect(address).unwrap();
        writeln!(socket, "{}", std::process::id()).unwrap();
        if args[index] == "--pipe-child" {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--hold-connect", address])
                .spawn()
                .unwrap();
            child.wait().unwrap();
        } else {
            loop {
                std::thread::park();
            }
        }
    }
    if args.iter().any(|arg| arg == "list-panes") {
        return;
    }
    if args.iter().any(|arg| arg == "--version") {
        println!("codex-cli 0.162.1");
        return;
    }
    if args.iter().any(|arg| arg == "--hold") {
        loop {
            std::thread::park();
        }
    }
    for key in [
        "CP_AUTH_TOKEN",
        "HANGAR_INTERNAL_SECRET",
        "HANGAR_PLUGIN_TOKEN",
        "TMUX",
        "TMUX_PANE",
    ] {
        if std::env::var_os(key).is_some() {
            eprintln!("ambiente de outra sessão recebido");
            std::process::exit(7);
        }
    }
    let root = std::env::var("CODEX_HOME")
        .ok()
        .filter(|_| args.iter().any(|a| a == "app-server" || a == "exec"))
        .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok())
        .unwrap_or_default();
    let account = std::path::Path::new(&root)
        .file_name()
        .unwrap()
        .to_string_lossy();
    let model = format!("model-{account}");
    if args.iter().any(|a| a == "exec")
        || args.iter().any(|a| a == "--output-format") && args.iter().any(|a| a == "json")
    {
        std::fs::write(
            std::path::Path::new(&root).join("fixture-executed"),
            "executou",
        )
        .unwrap();
        let selected = args
            .iter()
            .position(|a| a == "--model")
            .and_then(|i| args.get(i + 1));
        if selected != Some(&model) {
            eprintln!("modelo de outra conta recebido");
            std::process::exit(9);
        }
        let mut input = String::new();
        use std::io::Read;
        io::stdin().read_to_string(&mut input).unwrap();
        let text = input.trim();
        if args.iter().any(|a| a == "exec") {
            for flag in ["--ignore-user-config", "--ephemeral", "--sandbox"] {
                if !args.iter().any(|a| a == flag) {
                    std::process::exit(8);
                }
            }
            println!(
                "{}",
                json!({"type":"item.completed","item":{"type":"agent_message","text":text}})
            );
            println!(
                "{}",
                json!({"type":"turn.completed","usage":{"input_tokens":20,"output_tokens":8}})
            );
        } else {
            for flag in [
                "--safe-mode",
                "--no-session-persistence",
                "--tools",
                "--disable-slash-commands",
            ] {
                if !args.iter().any(|a| a == flag) {
                    std::process::exit(8);
                }
            }
            println!(
                "{}",
                json!({"type":"result","result":text,"is_error":false})
            );
        }
        return;
    }
    for line in io::stdin().lock().lines() {
        let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if let Some(method) = value["method"].as_str() {
            if method == "initialized" {
                continue;
            }
            let result = if method == "model/list" {
                json!({"data":[{"model":model,"displayName":account,"supportedReasoningEfforts":[{"reasoningEffort":"low"}],"serviceTiers":[]}]})
            } else {
                json!({})
            };
            println!("{}", json!({"id":value["id"],"result":result}));
        } else {
            let response = if value.pointer("/request/subtype").and_then(Value::as_str)
                == Some("list_models")
            {
                let settings: Value =
                    std::fs::read(std::path::Path::new(&root).join("settings.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                        .unwrap_or(Value::Null);
                if settings["fixture_catalog_stall"] == true {
                    loop {
                        std::thread::park();
                    }
                }
                json!({"models":[{"value":model,"displayName":account}]})
            } else {
                json!({})
            };
            println!(
                "{}",
                json!({"type":"control_response","response":{"subtype":"success","request_id":value["request_id"],"response":response}})
            );
        }
        io::stdout().flush().unwrap();
    }
}
