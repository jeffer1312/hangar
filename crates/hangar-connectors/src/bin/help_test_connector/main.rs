//! Conector de teste do Help (`work_tracking/v1`, só leitura). Fala JSON-RPC no stdin/stdout;
//! nada além do protocolo vai ao stdout.
//!
//! Argumentos: `--id <id>` (padrão `help-test`, igual ao do manifesto), `--help-url <url>` (padrão
//! `PMEDICO_HELP_URL` ou o Help de produção), `--config-dir <pasta>` (padrão `PMEDICO_CONFIG_DIR` ou
//! as pastas do plugin Promedico) e `--claude-config-dir <pasta>` (padrão `CLAUDE_CONFIG_DIR`).
mod credential;
mod help;

use hangar_connectors::protocol::{self, ConnectorError, ErrorCode};
use hangar_connectors::rpc;
use hangar_connectors::work_tracking::{self, GetParams, ListParams};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::BufReader;

struct Options { id: String, url: String, credentials: credential::Sources }

fn options() -> Result<Options, String> {
    let mut options = Options {
        id: "help-test".into(),
        url: std::env::var("PMEDICO_HELP_URL").ok().filter(|url| !url.is_empty()).unwrap_or_else(|| help::DEFAULT_URL.into()),
        credentials: credential::Sources::default(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} sem valor"))?;
        match flag.as_str() {
            "--id" => options.id = value,
            "--help-url" => options.url = value,
            "--config-dir" => options.credentials.config_dir = Some(PathBuf::from(value)),
            "--claude-config-dir" => options.credentials.claude_dir = Some(PathBuf::from(value)),
            other => return Err(format!("opção desconhecida: {other}")),
        }
    }
    Ok(options)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(message) => { eprintln!("hangar-help-test-connector: {message}"); return 2.into(); }
    };
    // Endereço inválido não derruba o processo: cada consulta responde o erro tipado.
    let mut help = help::Help::new(&options.url, options.credentials.clone());
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    loop {
        let line = match rpc::read_message(&mut stdin).await {
            Ok(Some(line)) => line,
            Ok(None) => return 0.into(),
            Err(error) => { eprintln!("hangar-help-test-connector: entrada inválida: {}", error.code); return 1.into(); }
        };
        let Ok(request) = serde_json::from_slice::<Value>(&line) else { return 1.into() };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let outcome = match method {
            protocol::INITIALIZE => {
                let mut capabilities = serde_json::Map::new();
                capabilities.insert(work_tracking::CAPABILITY.into(), json!(work_tracking::VERSION));
                Ok(json!({"protocol_version": protocol::PROTOCOL_VERSION, "id": options.id, "capabilities": capabilities}))
            }
            work_tracking::LIST => match &mut help {
                Ok(help) => match ListParams::from_value(params) { Ok(params) => help.list(params).await, Err(error) => Err(error) },
                Err(error) => Err(error.clone()),
            },
            work_tracking::GET => match &mut help {
                Ok(help) => match GetParams::from_value(params) { Ok(params) => help.get_ticket(params).await, Err(error) => Err(error) },
                Err(error) => Err(error.clone()),
            },
            protocol::SHUTDOWN => {
                let _ = rpc::write_message(&mut stdout, &protocol::success(&id, Value::Null)).await;
                return 0.into();
            }
            _ => Err(ConnectorError::new(ErrorCode::UnsupportedCapability, format!("método não oferecido: {method}"))),
        };
        let response = match outcome {
            Ok(result) => protocol::success(&id, result),
            Err(error) => protocol::failure(&id, &error),
        };
        if rpc::write_message(&mut stdout, &response).await.is_err() { return 1.into(); }
    }
}
