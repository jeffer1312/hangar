//! `hangar-server connectors`: o mesmo host do servidor pelas rotas do dono, ou um host isolado
//! (`--registry <arquivo>`) para provar o mecanismo sem servidor. Saída em JSON; erro com código
//! de saída diferente de zero.
use crate::host::Host;
use crate::manifest::Manifest;
use crate::work_review::{AssetParams, DecideParams};
use crate::work_tracking::{GetParams, ListParams, with_fetched_at};
use crate::{AdminError, CallError};
use serde_json::{Value, json};
use std::io::Write as _;
use std::sync::Arc;

pub const USAGE: &str = "uso: hangar-server connectors [--registry <arquivo>] <comando>
  add <manifesto>            cadastra (caminho do manifesto nesta máquina)
  list                       lista os cadastrados, sem iniciar processos
  enable <id> | disable <id> liga ou desliga (desligar encerra o processo)
  remove <id>                encerra e remove
  tickets <id> [--query T] [--assignee me|ID] [--state open|done] [--limit N] [--cursor C]
  ticket <id> <ticket_id>
  review <id> <ticket_id>                         revisão do ticket (work_review), em JSON
  review-asset <id> <ticket_id> <review_id> <asset> bytes da imagem no stdout
  review-decide <id> <ticket_id> '{\"review_id\": \"...\", \"decisions\": [{\"id\": \"...\", \"choice\": \"...\", \"reason\": \"...\"}]}'
Sem --registry, fala com o servidor desta máquina e usa o host dele. Os comandos review* só
funcionam com --registry: o servidor não tem rota de revisão por conector.";

/// Onde os comandos rodam.
pub enum Target {
    /// O servidor em execução, pelas rotas do dono.
    Server { base: String, token: String },
    /// Host próprio do comando, com catálogo à parte: a prova isolada.
    Local(Arc<Host>),
}

enum Command {
    Add(String), List, Enable(String, bool), Remove(String), Tickets(String, ListParams), Ticket(String, String),
    Review(String, GetParams), ReviewAsset(String, AssetParams), ReviewDecide(String, DecideParams),
}

/// JSON para o stdout, ou os bytes de uma imagem como vieram.
enum Output { Json(Value), Bytes(Vec<u8>) }

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
        [verb, id, ticket_id] if verb == "review" => Command::Review(id.clone(), GetParams::new(ticket_id.clone()).map_err(invalid)?),
        [verb, id, ticket_id, review_id, asset] if verb == "review-asset" => {
            Command::ReviewAsset(id.clone(), AssetParams::new(ticket_id.clone(), review_id.clone(), asset.clone()).map_err(invalid)?)
        }
        [verb, id, ticket_id, body] if verb == "review-decide" => {
            let mut body: Value = serde_json::from_str(body).ok().filter(Value::is_object)
                .ok_or_else(|| Failure::usage("review-decide espera um objeto JSON com review_id e decisions"))?;
            body["ticket_id"] = json!(ticket_id);
            Command::ReviewDecide(id.clone(), DecideParams::from_value(body).map_err(invalid)?)
        }
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
            Target::Server { base, token } => remote(base, token, command).await.map(Output::Json),
            Target::Local(host) => local(host, command).await,
        },
        Err(failure) => Err(failure),
    };
    match outcome {
        Ok(Output::Json(value)) => { println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default()); 0 }
        Ok(Output::Bytes(bytes)) => {
            let mut stdout = std::io::stdout().lock();
            if stdout.write_all(&bytes).and_then(|()| stdout.flush()).is_err() { return 1; }
            0
        }
        Err(failure) => {
            eprintln!("{}", serde_json::to_string_pretty(&json!({"error": {"code": failure.code, "message": failure.message}})).unwrap_or_default());
            failure.exit
        }
    }
}

async fn local(host: &Arc<Host>, command: Command) -> Result<Output, Failure> {
    Ok(Output::Json(match command {
        Command::Add(path) => json!({"connector": host.add(Manifest::load(std::path::Path::new(&path))?)?}),
        Command::List => json!({"connectors": host.list()?, "capabilities": crate::protocol::capabilities_json()}),
        Command::Enable(id, enabled) => json!({"connector": host.set_enabled(&id, enabled).await?}),
        Command::Remove(id) => json!({"connector": host.remove(&id).await?}),
        Command::Tickets(id, params) => with_fetched_at(json!(host.list_tickets(&id, params).await?)),
        Command::Ticket(id, ticket_id) => {
            let params = GetParams::new(ticket_id).map_err(CallError::from)?;
            with_fetched_at(json!(host.get_ticket(&id, params).await?))
        }
        Command::Review(id, params) => with_fetched_at(json!({"review": host.get_review(&id, params).await?})),
        Command::ReviewAsset(id, params) => return Ok(Output::Bytes(host.review_asset(&id, params).await?.bytes)),
        Command::ReviewDecide(id, params) => { host.decide_review(&id, params).await?; json!({"ok": true}) }
    }))
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
        Command::Review(..) | Command::ReviewAsset(..) | Command::ReviewDecide(..) => return Err(Failure {
            code: "registry_required".into(), message: "os comandos review* só funcionam com --registry: o servidor não tem rota de revisão por conector".into(), exit: 2,
        }),
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
