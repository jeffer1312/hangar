//! `hangar-server mcp-stdio`: o MCP `hangar` para agente que só fala stdio com cabeçalho fixo (o `agy`). Cada
//! mensagem JSON-RPC de uma linha vai ao `/mcp/` local; a resposta (JSON ou SSE) volta uma mensagem por linha.
//! A identidade da sessão sai do ambiente deste processo, herdado do agente, como no `env_http_headers` do
//! Codex; o token sai do `.env`, nunca do ambiente da sessão.
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// Só o conectar tem prazo: a tool do cofre espera o dono por até 10 min.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Id próprio da reabertura da sessão MCP: a resposta dela não vai ao agente.
const REOPEN_ID: &str = "hangar-mcp-stdio-reopen";

pub fn cli(args: &[String]) -> ExitCode {
    let mut err = std::io::stderr();
    let invocation = match hangar_registration::from_process(args, "mcp-stdio", false, &mut err) {
        Ok(invocation) => invocation,
        Err(code) => return ExitCode::from(code),
    };
    let config = match Config::load(&invocation) {
        Ok(config) => config,
        Err(reason) => {
            // Sem servidor o agente só vê o MCP falhar: a frase vai ao stderr, que ele guarda no log.
            eprintln!("hangar-mcp-stdio: {reason}");
            return ExitCode::from(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("hangar-mcp-stdio: {error}");
            return ExitCode::from(1);
        }
    };
    runtime.block_on(run(config, tokio::io::stdin(), tokio::io::stdout()));
    ExitCode::SUCCESS
}

/// Para onde e como falar: o `/mcp/` da porta do `.env`, o `.env` (relido quando o token muda) e a identidade.
#[derive(Clone, Debug)]
pub struct Config {
    pub url: String,
    pub env_file: PathBuf,
    pub identity: Vec<(&'static str, String)>,
}

impl Config {
    pub fn load(invocation: &hangar_registration::Invocation) -> Result<Self, String> {
        let env_file = hangar_registration::env_file(invocation);
        let env = hangar_registration::read_env(&env_file).map_err(|error| format!("não consegui ler {}: {error}", env_file.display()))?;
        if env.get("cp_auth_token").is_none_or(String::is_empty) {
            return Err(format!("CP_AUTH_TOKEN ausente em {}", env_file.display()));
        }
        let port = env.get("cp_port").filter(|port| !port.is_empty()).map_or("8765", String::as_str);
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/mcp/"),
            env_file,
            identity: hangar_registration::mcp_headers::identity(&invocation.vars),
        })
    }
}

/// Lê linhas de `input` até o agente fechar a entrada; cada resposta sai inteira numa linha de `output`.
pub async fn run<R, W>(config: Config, input: R, output: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(write_lines(rx, output));
    let bridge = Arc::new(Bridge::new(config, tx));
    let mut lines = BufReader::new(input).lines();
    let mut tasks = JoinSet::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                bridge.emit(&json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("JSON inválido: {error}")}}));
                continue;
            }
        };
        // O `initialize` abre a sessão MCP: o resto espera o id dela.
        if method(&message) == Some("initialize") {
            *bridge.init.lock().unwrap() = Some(message.clone());
            bridge.exchange(message).await;
            continue;
        }
        let bridge = bridge.clone();
        tasks.spawn(async move { bridge.exchange(message).await });
        while tasks.try_join_next().is_some() {}
    }
    // Entrada fechada: o agente saiu e ninguém lê mais as respostas.
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    bridge.close().await;
    drop(bridge);
    let _ = writer.await;
}

async fn write_lines<W: AsyncWrite + Unpin>(mut rx: mpsc::UnboundedReceiver<String>, mut output: W) {
    while let Some(line) = rx.recv().await {
        if output.write_all(line.as_bytes()).await.is_err() || output.write_all(b"\n").await.is_err() || output.flush().await.is_err() {
            return;
        }
    }
}

fn method(message: &Value) -> Option<&str> { message.get("method").and_then(Value::as_str) }

/// O id de um pedido (mensagem com `method` e `id`): é ele que espera resposta.
fn request_id(message: &Value) -> Option<&Value> {
    method(message).and(message.get("id")).filter(|id| !id.is_null())
}

/// Para onde vai a resposta de um envio.
#[derive(Clone, Copy, PartialEq)]
enum Sink { Agent, Discard }

struct Bridge {
    client: reqwest::Client,
    config: Config,
    token: Mutex<String>,
    session: Mutex<Option<String>>,
    protocol: Mutex<Option<String>>,
    init: Mutex<Option<Value>>,
    /// Reabertura que não terminou: sem sessão o servidor recusa o pedido comum, então o próximo refaz o handshake.
    stale: Mutex<bool>,
    out: mpsc::UnboundedSender<String>,
}

impl Bridge {
    fn new(config: Config, out: mpsc::UnboundedSender<String>) -> Self {
        // Loopback nunca passa por proxy do ambiente.
        let client = hangar_core::net::http_client().no_proxy().connect_timeout(CONNECT_TIMEOUT).build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let token = read_token(&config.env_file).unwrap_or_default();
        Self { client, config, token: Mutex::new(token), session: Mutex::new(None), protocol: Mutex::new(None), init: Mutex::new(None), stale: Mutex::new(false), out }
    }

    fn emit(&self, message: &Value) {
        let _ = self.out.send(message.to_string());
    }

    /// Um envio do agente; pedido que não chega ao servidor volta ao agente como erro, nunca em silêncio.
    async fn exchange(&self, message: Value) {
        let result = async {
            if method(&message) != Some("initialize") && *self.stale.lock().unwrap() {
                self.reopen().await?;
            }
            self.forward(&message, Sink::Agent).await
        }.await;
        if let Err(reason) = result
            && let Some(id) = request_id(&message)
        {
            self.emit(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("hangar mcp-stdio: {reason}")}}));
        }
    }

    async fn forward(&self, message: &Value, sink: Sink) -> Result<(), String> {
        // Cada recuperação uma vez só: token relido e sessão reaberta não gastam a vez uma da outra.
        let (mut token_reread, mut reopened) = (false, false);
        loop {
            let response = self.post(message).await.map_err(|error| format!("o hangar-server não respondeu em {}: {error}", self.config.url))?;
            let status = response.status();
            if !token_reread && status == reqwest::StatusCode::UNAUTHORIZED {
                // O token pode ter mudado com o servidor de pé.
                token_reread = true;
                *self.token.lock().unwrap() = read_token(&self.config.env_file)?;
                continue;
            }
            // Só pedido do agente reabre: o `notifications/initialized` da própria reabertura não entra em laço.
            if !reopened && sink == Sink::Agent && status == reqwest::StatusCode::NOT_FOUND && method(message) != Some("initialize")
                && self.session.lock().unwrap().is_some()
            {
                // Servidor reiniciado esquece a sessão MCP: abre outra com o mesmo `initialize` e repete.
                reopened = true;
                self.reopen().await?;
                continue;
            }
            if let Some(session) = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                *self.session.lock().unwrap() = Some(session.to_owned());
            }
            if status == reqwest::StatusCode::ACCEPTED {
                return Ok(());
            }
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(format!("HTTP {}: {}", status.as_u16(), body.chars().take(300).collect::<String>()));
            }
            return self.relay(response, request_id(message), sink).await;
        }
    }

    async fn post(&self, message: &Value) -> reqwest::Result<reqwest::Response> {
        let mut request = self.client.post(&self.config.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("authorization", format!("Bearer {}", self.token.lock().unwrap()));
        if let Some(session) = self.session.lock().unwrap().clone() {
            request = request.header("mcp-session-id", session);
        }
        if let Some(protocol) = self.protocol.lock().unwrap().clone() {
            request = request.header("mcp-protocol-version", protocol);
        }
        for (name, value) in &self.config.identity {
            // Valor fora do que cabe num cabeçalho fica de fora, como o pane que não resolve.
            if reqwest::header::HeaderValue::from_str(value).is_ok() {
                request = request.header(*name, value);
            }
        }
        request.body(message.to_string()).send().await
    }

    /// Repassa o corpo, JSON ou SSE, mensagem a mensagem; pedido sem resposta no corpo é erro.
    async fn relay(&self, response: reqwest::Response, id: Option<&Value>, sink: Sink) -> Result<(), String> {
        let event_stream = response.headers().get("content-type").and_then(|v| v.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        let mut answered = false;
        // `true` quando é a resposta do pedido.
        let deliver = |message: Value| -> bool {
            if let Some(version) = message.pointer("/result/protocolVersion").and_then(Value::as_str) {
                *self.protocol.lock().unwrap() = Some(version.to_owned());
            }
            let answer = id.is_some_and(|id| message.get("id") == Some(id) && method(&message).is_none());
            if sink == Sink::Agent {
                self.emit(&message);
            }
            answer
        };
        if event_stream {
            let mut events = response.bytes_stream().eventsource();
            while let Some(event) = events.next().await {
                let event = match event {
                    Ok(event) => event,
                    // Já respondido, o fim torto do fluxo não muda nada.
                    Err(_) if answered => break,
                    Err(error) => return Err(format!("o fluxo SSE caiu: {error}")),
                };
                // O evento inicial do SDK (só `id`, sem dados) não é mensagem.
                if let Ok(message) = serde_json::from_str::<Value>(&event.data) {
                    answered |= deliver(message);
                }
            }
        } else {
            let body = response.bytes().await.map_err(|error| format!("o corpo da resposta caiu: {error}"))?;
            if !body.iter().all(u8::is_ascii_whitespace) {
                match serde_json::from_slice::<Value>(&body).map_err(|error| format!("resposta não é JSON: {error}"))? {
                    Value::Array(batch) => for message in batch { answered |= deliver(message) },
                    message => answered |= deliver(message),
                }
            }
        }
        if id.is_some() && !answered {
            return Err("o servidor encerrou a resposta sem responder o pedido".into());
        }
        Ok(())
    }

    /// Abre outra sessão MCP com o `initialize` que o agente mandou; a resposta fica aqui.
    async fn reopen(&self) -> Result<(), String> {
        let Some(mut init) = self.init.lock().unwrap().clone() else { return Err("sessão MCP perdida antes do initialize".into()) };
        *self.stale.lock().unwrap() = true;
        *self.session.lock().unwrap() = None;
        init["id"] = json!(REOPEN_ID);
        Box::pin(self.forward(&init, Sink::Discard)).await?;
        Box::pin(self.forward(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), Sink::Discard)).await?;
        *self.stale.lock().unwrap() = false;
        Ok(())
    }

    /// Fecha a sessão MCP no servidor; sem resposta em pouco tempo, o servidor a esquece sozinho.
    async fn close(&self) {
        let Some(session) = self.session.lock().unwrap().clone() else { return };
        let request = self.client.delete(&self.config.url)
            .header("authorization", format!("Bearer {}", self.token.lock().unwrap()))
            .header("mcp-session-id", session);
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, request.send()).await;
    }
}

fn read_token(env_file: &std::path::Path) -> Result<String, String> {
    let env = hangar_registration::read_env(env_file).map_err(|error| format!("não consegui ler {}: {error}", env_file.display()))?;
    env.get("cp_auth_token").filter(|token| !token.is_empty()).cloned()
        .ok_or_else(|| format!("CP_AUTH_TOKEN ausente em {}", env_file.display()))
}
