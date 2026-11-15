//! `hangar-server connectors`: o mesmo host do servidor pelas rotas do dono, ou um host isolado
//! (`--registry <arquivo>`) para provar o mecanismo sem servidor. Saída em JSON; erro com código
//! de saída diferente de zero.
use crate::host::Host;
use crate::manifest::Manifest;
use crate::work_tracking::{GetParams, ListParams, with_fetched_at};
use crate::{AdminError, CallError};
use serde_json::{Value, json};
use std::sync::Arc;

pub const USAGE: &str = "uso: hangar-server connectors [--registry <arquivo>] <comando>
  add <manifesto>            cadastra (caminho do manifesto nesta máquina)
  list                       lista os cadastrados, sem iniciar processos
  enable <id> | disable <id> liga ou desliga (desligar encerra o processo)
  remove <id>                encerra e remove
  tickets <id> [--query T] [--assignee me|ID] [--state open|done] [--limit N] [--cursor C]
  ticket <id> <ticket_id>
Sem --registry, fala com o servidor desta máquina e usa o host dele.";

/// Onde os comandos rodam.
pub enum Target {
    /// O servidor em execução, pelas rotas do dono.
    Server { base: String, token: String },
    /// Host próprio do comando, com catálogo à parte: a prova isolada.
    Local(Arc<Host>),
}

enum Command { Add(String), List, Enable(String, bool), Remove(String), Tickets(String, ListParams), Ticket(String, String) }

/// Falha do comando: código estável, mensagem e o código de saída.
struct Failure { code: String, message: String, exit: u8 }

impl Failure {
    fn usage(message: impl Into<String>) -> Self { Self { code: "usage".into(), message: message.into(), exit: 2 } }
}

impl From<AdminError> for Failure { fn from(error: AdminError) -> Self { Self { code: error.code().into(), message: error.message, exit: 1 } } }
impl From<CallError> for Failure { fn from(error: CallError) -> Self { Self { code: error.code().into(), message: error.message(), exit: 1 } } }

fn parse(args: &[String]) -> Result<Command, Failure> {
    let command = match args {
        [verb, path] if verb == "add" => Command::Add(path.clone()),
        [verb] if verb == "list" => Command::List,
        [verb, id] if verb == "enable" => Command::Enable(id.clone(), true),
        [verb, id] if verb == "disable" => Command::Enable(id.clone(), false),
        [verb, id] if verb == "remove" => Command::Remove(id.clone()),
        [verb, id, ticket_id] if verb == "ticket" => Command::Ticket(id.clone(), ticket_id.clone()),
        [verb, id, options @ ..] if verb == "tickets" => {
            if let [.., last] = options && options.len() % 2 != 0 { return Err(Failure::usage(format!("{last} sem valor"))); }
            let pairs = options.chunks(2).map(|pair| match pair[0].strip_prefix("--") {
                Some(name @ ("query" | "assignee" | "state" | "limit" | "cursor")) => Ok((name.to_owned(), pair[1].clone())),
                _ => Err(Failure::usage(format!("opção desconhecida: {}", pair[0]))),
            }).collect::<Result<Vec<_>, _>>()?;
            Command::Tickets(id.clone(), ListParams::from_pairs(pairs).map_err(invalid)?)
        }
        _ => return Err(Failure::usage(USAGE)),
    };
    Ok(command)
}

fn invalid(error: crate::protocol::ConnectorError) -> Failure { Failure { code: error.code.as_str().into(), message: error.message, exit: 2 } }

/// Executa e devolve o código de saída; JSON no stdout, erro em JSON no stderr.
pub async fn run(target: &Target, args: &[String]) -> u8 {
    let outcome = match parse(args) {
        Ok(command) => match target {
            Target::Server { base, token } => remote(base, token, command).await,
            Target::Local(host) => local(host, command).await,
        },
        Err(failure) => Err(failure),
    };
    match outcome {
        Ok(value) => { println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default()); 0 }
        Err(failure) => {
            eprintln!("{}", serde_json::to_string_pretty(&json!({"error": {"code": failure.code, "message": failure.message}})).unwrap_or_default());
            failure.exit
        }
    }
}

async fn local(host: &Arc<Host>, command: Command) -> Result<Value, Failure> {
    Ok(match command {
        Command::Add(path) => json!({"connector": host.add(Manifest::load(std::path::Path::new(&path))?)?}),
        Command::List => json!({"connectors": host.list()?}),
        Command::Enable(id, enabled) => json!({"connector": host.set_enabled(&id, enabled).await?}),
        Command::Remove(id) => json!({"connector": host.remove(&id).await?}),
        Command::Tickets(id, params) => with_fetched_at(json!(host.list_tickets(&id, params).await?)),
        Command::Ticket(id, ticket_id) => {
            let params = GetParams::new(ticket_id).map_err(CallError::from)?;
            with_fetched_at(json!(host.get_ticket(&id, params).await?))
        }
    })
}

fn with_json(request: reqwest::RequestBuilder, body: Value) -> reqwest::RequestBuilder {
    request.header(reqwest::header::CONTENT_TYPE, "application/json").body(body.to_string())
}

fn segment(text: &str) -> String { percent_encoding::utf8_percent_encode(text, percent_encoding::NON_ALPHANUMERIC).to_string() }

async fn remote(base: &str, token: &str, command: Command) -> Result<Value, Failure> {
    let base = base.trim_end_matches('/');
    let client = hangar_core::net::http_client().redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(120)).build()
        .map_err(|_| Failure { code: "client_unavailable".into(), message: "cliente HTTP não iniciou".into(), exit: 3 })?;
    let request = match command {
        Command::Add(path) => {
            let path = std::path::absolute(&path).map_err(|_| Failure::usage("caminho do manifesto inválido"))?;
            with_json(client.post(format!("{base}/api/connectors")), json!({"manifest": path}))
        }
        Command::List => client.get(format!("{base}/api/connectors")),
        Command::Enable(id, enabled) => with_json(client.patch(format!("{base}/api/connectors/{}", segment(&id))), json!({"enabled": enabled})),
        Command::Remove(id) => client.delete(format!("{base}/api/connectors/{}", segment(&id))),
        Command::Tickets(id, params) => {
            let mut query = form_urlencoded::Serializer::new(String::new());
            query.append_pair("limit", &params.limit.to_string());
            if let Some(value) = &params.filter.query { query.append_pair("query", value); }
            if let Some(value) = &params.filter.assignee { query.append_pair("assignee", value); }
            if let Some(value) = &params.filter.state { query.append_pair("state", value); }
            if let Some(value) = &params.cursor { query.append_pair("cursor", value); }
            client.get(format!("{base}/api/connectors/{}/tickets?{}", segment(&id), query.finish()))
        }
        Command::Ticket(id, ticket_id) => client.get(format!("{base}/api/connectors/{}/tickets/{}", segment(&id), segment(&ticket_id))),
    };
    let response = request.bearer_auth(token).send().await.map_err(|error| Failure {
        code: "server_unreachable".into(), message: format!("servidor do Hangar fora do ar em {base} ({})", if error.is_timeout() { "prazo" } else { "conexão" }), exit: 3,
    })?;
    let status = response.status();
    let body: Value = response.bytes().await.ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null);
    if status.is_success() { return Ok(body); }
    let code = body.pointer("/detail/code").and_then(Value::as_str).unwrap_or("http_error").to_owned();
    let message = body.pointer("/detail/msg").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("HTTP {status}"));
    Err(Failure { code, message, exit: 1 })
}
