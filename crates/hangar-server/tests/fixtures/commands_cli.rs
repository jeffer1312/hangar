//! CLI de prova do initialize; não lê credenciais nem chama rede.
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn main() {
    if std::env::args().any(|a| a == "--child") {
        loop {
            std::thread::park();
        }
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let Some(Ok(line)) = lines.next() else {
        return;
    };
    let request: Value = serde_json::from_str(&line).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--child")
        .spawn()
        .unwrap();
    let record = json!({
        "pid":std::process::id(), "child":child.id(), "request":request,
        "cwd":std::env::current_dir().unwrap(),
        "config_dir":std::env::var("CLAUDE_CONFIG_DIR").ok(),
        "session_identity": (["TMUX","TMUX_PANE","CP_SESSION_NAME","CP_SESSION_KEY"].iter().any(|k| std::env::var_os(k).is_some())),
        "private_authority": (["HANGAR_INTERNAL_SECRET","CP_AUTH_TOKEN","HANGAR_PLUGIN_TOKEN"].iter().any(|k| std::env::var_os(k).is_some())),
        "args":std::env::args().skip(1).collect::<Vec<_>>()
    });
    if let Some(path) = std::env::var_os("COMMANDS_PROBE_RECORD") {
        std::fs::write(path, record.to_string()).unwrap();
    }
    match std::env::var("COMMANDS_PROBE_MODE").as_deref() {
        Ok("hang") => {}
        Ok("exit") => return,
        _ => {
            println!("não é JSON");
            println!(
                "{}",
                json!({"type":"control_response","response":{"request_id":"outra-sonda","response":{"commands":[{"name":"wrong"}]}}})
            );
            let response = match std::env::var("COMMANDS_PROBE_MODE").as_deref() {
                Ok("reject") => {
                    json!({"request_id":request["request_id"],"subtype":"error","error":"recusado"})
                }
                Ok("empty") => {
                    json!({"request_id":request["request_id"],"subtype":"success","response":{"commands":[]}})
                }
                Ok("context") => {
                    let commands = std::fs::read("commands-response.json")
                        .ok()
                        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                        .unwrap_or_else(|| json!([{"name":"usage"}]));
                    json!({"request_id":request["request_id"],"subtype":"success","response":{"commands":commands}})
                }
                _ => {
                    json!({"request_id":request["request_id"],"subtype":"success","response":{"commands":[{"name":"usage","description":"Consulta o uso"},"lixo",{},null]}})
                }
            };
            println!("{}", json!({"type":"control_response","response":response}));
            std::io::stdout().flush().unwrap();
        }
    }
    for line in lines {
        if line.is_err() {
            break;
        }
    }
}
