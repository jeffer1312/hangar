//! O filho falso dos testes de contrato do cano: `claude` fala stream-json (responde initialize e,
//! a cada prompt, pede uma permissão e só fecha o turno quando ela é respondida); `app-server` fala o
//! JSON-RPC do Codex com dois pedidos de aprovação por turno.
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn out(value: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

fn bye() {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(b"tchau\n");
    let _ = stderr.flush();
}

fn claude(event: &Value) {
    match event["type"].as_str() {
        Some("control_request") => {
            match event["request"]["subtype"].as_str() {
                Some("initialize") => out(json!({"type": "system", "subtype": "init", "session_id": "sid-1", "model": "haiku", "permissionMode": "default"})),
                Some("emit_peer") => out(json!({"type": "command_lifecycle", "command_uuid": "peer-1", "state": "started"})),
                Some("finish_peer") => out(json!({"type": "result", "subtype": "success", "usage": {"input_tokens": 1}})),
                _ => {}
            }
            out(json!({"type": "control_response", "response": {"subtype": "success", "request_id": event["request_id"], "response": {}}}));
        }
        Some("user") => {
            let text = event["message"]["content"][0]["text"].as_str().unwrap_or_default();
            if text == "sair" {
                bye();
                std::process::exit(3);
            }
            // `sair quando <arquivo>`: o teste decide o instante da saída (ex.: depois de o cliente cair).
            if let Some(gate) = text.strip_prefix("sair quando ") {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !std::path::Path::new(gate).exists() && std::time::Instant::now() < deadline {
                    // estavel: intervalo de consulta do arquivo que solta a saída, com teto de 30 s.
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                bye();
                std::process::exit(3);
            }
            out(json!({"type": "control_request", "request_id": "perm-1",
                "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}}}));
        }
        Some("control_response") => {
            out(json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "feito"}]}}));
            out(json!({"type": "result", "subtype": "success", "usage": {"input_tokens": 1}}));
        }
        _ => {}
    }
}

fn app_server(event: &Value) {
    match event["method"].as_str() {
        Some("turn/start") => {
            out(json!({"jsonrpc": "2.0", "id": event["id"], "result": {"turn": {"id": "t1"}}}));
            out(json!({"jsonrpc": "2.0", "id": 0, "method": "item/commandExecution/requestApproval",
                "params": {"threadId": "th", "itemId": "i1", "command": "touch x"}}));
            out(json!({"jsonrpc": "2.0", "id": 1, "method": "item/fileChange/requestApproval",
                "params": {"threadId": "th", "itemId": "i2"}}));
        }
        Some("turn/interrupt") => {
            out(json!({"jsonrpc": "2.0", "id": event["id"], "result": {}}));
            out(json!({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}}));
        }
        Some(_) => {}
        None if !event["id"].is_null() => {
            out(json!({"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "th", "requestId": event["id"]}}));
            if event["id"] == 1 {
                out(json!({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}}));
            }
        }
        None => {}
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        if mode == "app-server" { app_server(&event) } else { claude(&event) }
    }
    if mode != "app-server" { bye(); }
}
