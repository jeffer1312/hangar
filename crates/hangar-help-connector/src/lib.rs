//! Conector do Help: leitura de tickets e perfil do fluxo do plugin. Fala JSON-RPC no stdin/stdout;
//! nada além do protocolo vai ao stdout.
//!
//! Argumentos: `--id <id>` (padrão `help-test`, igual ao do manifesto), `--help-url <url>` (padrão
//! `PMEDICO_HELP_URL` ou o Help de produção), `--config-dir <pasta>` (padrão `PMEDICO_CONFIG_DIR` ou
//! as pastas do plugin Promedico) e `--claude-config-dir <pasta>` (padrão `CLAUDE_CONFIG_DIR`).
mod credential;
mod help;
pub mod provided;

use hangar_connectors::protocol::{self, ConnectorError, ErrorCode};
use hangar_connectors::rpc;
use hangar_connectors::work_tracking::{self, GetParams, ListParams};
use hangar_connectors::work_actions;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::BufReader;

struct Options { id: String, url: String, credentials: credential::Sources }

fn options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        id: "help-test".into(),
        url: std::env::var("PMEDICO_HELP_URL").ok().filter(|url| !url.is_empty()).unwrap_or_else(|| help::DEFAULT_URL.into()),
        credentials: credential::Sources::default(),
    };
    let mut args = args.iter().cloned();
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

pub fn cli(args: &[String]) -> std::process::ExitCode {
    match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime)=>runtime.block_on(serve(args)),
        Err(_)=>{eprintln!("hangar-help-connector: não foi possível iniciar o runtime");1.into()}
    }
}

pub async fn serve(args: &[String]) -> std::process::ExitCode {
    let options = match options(args) {
        Ok(options) => options,
        Err(message) => { eprintln!("hangar-help-test-connector: {message}"); return 2.into(); }
    };
    // Endereço inválido não derruba o processo: cada consulta responde o erro tipado.
    let mut help = help::Help::new(&options.url, options.credentials.clone());
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut actions_enabled = false;
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
                actions_enabled = params["capabilities"][work_actions::CAPABILITY] == work_actions::VERSION;
                if actions_enabled { capabilities.insert(work_actions::CAPABILITY.into(), json!(work_actions::VERSION)); }
                Ok(json!({"protocol_version": protocol::PROTOCOL_VERSION, "id": options.id, "capabilities": capabilities}))
            }
            work_tracking::LIST => match (help.as_mut().map_err(|error| error.clone()), ListParams::from_value(params)) {
                (Ok(help), Ok(params)) => help.list(params).await,
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
            work_tracking::GET => match (help.as_mut().map_err(|error| error.clone()), GetParams::from_value(params)) {
                (Ok(help), Ok(params)) => help.get_ticket(params).await,
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
            work_actions::LIST if actions_enabled => GetParams::from_value(params).map(|_| help_actions()),
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

fn help_actions() -> Value {
    json!({"actions": [
        {"id": "start-pm", "label": "Iniciar PM e planejar", "description": "Preparar a PM e iniciar as perguntas do planejamento.",
            "prompt_template": "/pmedico:iniciar-pm {key}", "stage": "planejamento"},
        {"id": "execute-plan", "label": "Executar o plano aprovado", "description": "Continuar o trabalho com as decisões e a modalidade de execução já escolhidas.",
            "prompt_template": "Execute o plano aprovado da {key}, preservando as decisões e a modalidade de execução escolhida.", "stage": "execucao"},
        {"id": "send-review", "label": "Enviar para revisão de código", "description": "Conferir, compilar, revisar, publicar e transicionar pelo plugin.",
            "prompt_template": "/pmedico:iniciar-revisao-pm {key}", "stage": "revisao"},
        {"id": "follow-review", "label": "Acompanhar revisão do MR", "description": "Tratar as threads e acompanhar o MR que já existe.",
            "prompt_template": "/pmedico:iniciar-review-auto", "stage": "revisao"},
        {"id": "show-validation", "label": "Apresentar validação", "description": "Reunir evidências para a aprovação do dono.",
            "prompt_template": "Apresente as evidências de funcionamento da {key} no Hangar e aguarde a aprovação do dono antes de seguir.", "stage": "validacao"},
        {"id": "finish-pm", "label": "Finalizar PM depois do merge", "description": "Conferir o merge e finalizar a PM pelo processo do plugin.",
            "prompt_template": "/pmedico:finalizar-pm {key}", "stage": "entrega"}
    ]})
}
