//! A ponte contra um servidor MCP de verdade (o mesmo SDK e transporte do `/mcp/` do Hangar).
use std::sync::Arc;

use hangar_mcp_stdio::{Config, run};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

/// Devolve o que o servidor viu: token, identidade e o nome da tool.
#[derive(Clone)]
struct Echo;

impl ServerHandler for Echo {
    fn get_info(&self) -> ServerConfig { ServerConfig::new(ServerCapabilities::builder().enable_tools().build()) }

    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        let headers = context.extensions.get::<axum::http::request::Parts>().map(|p| p.headers.clone()).unwrap_or_default();
        let seen = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
        let text = format!("{}|{}|{}|{}", seen("authorization"), seen("x-hangar-key"), seen("x-hangar-pane"), request.name);
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text(text)])))
    }
}

async fn serve(listener: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
    let config = StreamableHttpServerConfig::default().with_allowed_origins(["http://127.0.0.1:*"]);
    let service = StreamableHttpService::new(|| Ok(Echo), Arc::new(LocalSessionManager::default()), config);
    let router = axum::Router::new().route_service("/mcp/", service);
    tokio::spawn(async move { let _ = axum::serve(listener, router).await; })
}

struct Agent { input: DuplexStream, output: Lines<BufReader<DuplexStream>>, _env: tempfile::TempDir }

impl Agent {
    async fn send(&mut self, message: Value) {
        self.input.write_all(format!("{message}\n").as_bytes()).await.unwrap();
    }

    async fn read(&mut self) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(20), self.output.next_line()).await
            .expect("a ponte não respondeu").unwrap().expect("a ponte fechou a saída");
        serde_json::from_str(&line).unwrap()
    }
}

fn start(port: u16) -> Agent {
    let env = tempfile::tempdir().unwrap();
    let env_file = env.path().join(".env");
    std::fs::write(&env_file, format!("CP_AUTH_TOKEN=tok\nCP_PORT={port}\n")).unwrap();
    let config = Config {
        url: format!("http://127.0.0.1:{port}/mcp/"),
        env_file,
        identity: vec![("X-Hangar-Key", "chave-1".into()), ("X-Hangar-Pane", "%3".into())],
    };
    let (input, bridge_in) = tokio::io::duplex(1 << 16);
    let (bridge_out, output) = tokio::io::duplex(1 << 16);
    tokio::spawn(run(config, bridge_in, bridge_out));
    Agent { input, output: BufReader::new(output).lines(), _env: env }
}

fn initialize(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "agy", "version": "0"}}})
}

fn call(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "who_am_i", "arguments": {}}})
}

#[tokio::test]
async fn the_session_identity_and_the_token_reach_the_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _server = serve(listener).await;
    let mut agent = start(port);
    agent.send(initialize(1)).await;
    let init = agent.read().await;
    assert_eq!(init["id"], 1);
    assert!(init["result"]["protocolVersion"].is_string(), "{init}");
    // A notificação não tem resposta: a linha seguinte já é a da tool.
    agent.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    agent.send(call(2)).await;
    let answer = agent.read().await;
    assert_eq!(answer["id"], 2, "{answer}");
    assert_eq!(answer["result"]["content"][0]["text"], "Bearer tok|chave-1|%3|who_am_i");
}

#[tokio::test]
async fn a_server_restart_opens_a_new_mcp_session_and_repeats_the_call() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve(listener).await;
    let mut agent = start(addr.port());
    agent.send(initialize(1)).await;
    agent.read().await;
    agent.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    agent.send(call(2)).await;
    assert_eq!(agent.read().await["id"], 2);
    // Outro servidor na mesma porta não conhece a sessão de antes (404).
    server.abort();
    let _ = server.await;
    let _again = serve(tokio::net::TcpListener::bind(addr).await.unwrap()).await;
    agent.send(call(3)).await;
    let answer = agent.read().await;
    assert_eq!(answer["id"], 3, "{answer}");
    assert_eq!(answer["result"]["content"][0]["text"], "Bearer tok|chave-1|%3|who_am_i");
}

#[tokio::test]
async fn a_request_without_a_server_gets_an_error_instead_of_silence() {
    let port = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let mut agent = start(port);
    agent.send(initialize(7)).await;
    let answer = agent.read().await;
    assert_eq!(answer["id"], 7);
    assert_eq!(answer["error"]["code"], -32000);
    assert!(answer["error"]["message"].as_str().unwrap().contains("não respondeu"), "{answer}");
}
