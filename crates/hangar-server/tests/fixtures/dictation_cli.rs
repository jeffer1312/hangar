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
    if args.iter().any(|arg| arg == "--exit-empty") {
        return;
    }
    if args.iter().any(|arg| arg == "--exit-failure") {
        println!(
            "{}",
            json!({"type":"result","result":"parcial","is_error":false})
        );
        std::process::exit(3);
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
        let overrides: Vec<&str> = args
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].as_str())
            .collect();
        let system_prompt = if args.iter().any(|a| a == "exec") {
            overrides
                .iter()
                .find_map(|o| o.strip_prefix("model_instructions_file="))
                .and_then(|path| serde_json::from_str::<String>(path).ok())
        } else {
            value_of(&args, "--system-prompt-file").cloned()
        }
        .and_then(|path| std::fs::read_to_string(path).ok());
        // O teste confere o que o CLI recebeu de fato, não só o resultado.
        std::fs::write(
            std::path::Path::new(&root).join("fixture-record.json"),
            json!({
                "args": args,
                "anthropic_api_key": std::env::var_os("ANTHROPIC_API_KEY").is_some(),
                "config_dir": std::env::var("CLAUDE_CONFIG_DIR").ok(),
                "credentials_present": std::path::Path::new(&root).join(".credentials.json").exists(),
                "system_prompt": system_prompt,
                "stdin": text,
            })
            .to_string(),
        )
        .unwrap();
        if system_prompt.is_none() {
            eprintln!("prompt de sistema ausente");
            std::process::exit(8);
        }
        if args.iter().any(|a| a == "exec") {
            for flag in ["--ignore-user-config", "--ephemeral", "--sandbox"] {
                if !args.iter().any(|a| a == flag) {
                    std::process::exit(8);
                }
            }
            for prefix in [
                "history.persistence=\"none\"",
                "cli_auth_credentials_store=",
                "log_dir=",
                "sqlite_home=",
            ] {
                if !overrides.iter().any(|o| o.starts_with(prefix)) {
                    eprintln!("override ausente: {prefix}");
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
            if !claude_isolated(&args) || args.iter().any(|a| a == "--system-prompt") {
                eprintln!("isolamento ausente ou prompt no argv");
                std::process::exit(8);
            }
            println!(
                "{}",
                json!({"type":"result","result":text,"is_error":false})
            );
        }
        return;
    }
    if args.iter().any(|a| a == "--input-format") && !claude_isolated(&args) {
        eprintln!("catálogo sem isolamento");
        std::process::exit(8);
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
                    std::fs::write(
                        std::path::Path::new(&root).join("fixture-catalog-stalled"),
                        "ready",
                    )
                    .unwrap();
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

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
}

/// Sem fontes de configuração e sem MCP além do vazio declarado.
fn claude_isolated(args: &[String]) -> bool {
    args.iter().any(|a| a == "--strict-mcp-config")
        && value_of(args, "--setting-sources").is_some_and(|v| v.is_empty())
}
