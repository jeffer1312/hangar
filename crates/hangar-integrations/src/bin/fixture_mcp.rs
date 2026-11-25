//! Servidor MCP `stdio` mínimo usado nos testes de integração do cliente.

use std::sync::Arc;
use std::time::Duration;

use hangar_core::terminal_process::PRIVATE_ENV_KEYS;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

#[derive(Clone)]
struct FixtureMcpServer;

fn object_schema(value: Value) -> Arc<Map<String, Value>> {
    Arc::new(value.as_object().cloned().unwrap_or_default())
}

fn all_tools() -> Vec<Tool> {
    let read_echo = Tool::new(
        "read_echo",
        "Repete o texto recebido",
        object_schema(json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })),
    )
    .with_annotations(ToolAnnotations::new().read_only(true));

    let write_action = Tool::new(
        "write_action",
        "Executa ação de escrita simulada",
        object_schema(json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })),
    )
    .with_annotations(ToolAnnotations::new().read_only(false));

    let unannotated_tool = Tool::new(
        "unannotated_tool",
        "Ferramenta sem annotations (fail-closed)",
        object_schema(json!({ "type": "object", "properties": {} })),
    );

    let slow_tool = Tool::new(
        "slow_tool",
        "Ferramenta propositalmente lenta para testar timeout",
        object_schema(json!({
            "type": "object",
            "properties": { "delay_ms": { "type": "integer" } }
        })),
    )
    .with_annotations(ToolAnnotations::new().read_only(true));

    let dump_env = Tool::new(
        "dump_env",
        "Inspeciona variáveis recebidas e PID do processo",
        object_schema(json!({ "type": "object", "properties": {} })),
    )
    .with_annotations(ToolAnnotations::new().read_only(true));

    vec![
        read_echo,
        write_action,
        unannotated_tool,
        slow_tool,
        dump_env,
    ]
}

impl ServerHandler for FixtureMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = all_tools();
        if std::env::var("FIXTURE_PAGINATE_LOOP").as_deref() == Ok("1") {
            let cursor = request.and_then(|p| p.cursor);
            return match cursor.as_deref() {
                None => {
                    let mut page = ListToolsResult::with_all_items(tools[..2].to_vec());
                    page.next_cursor = Some("loop-cursor".to_owned());
                    Ok(page)
                }
                Some("loop-cursor") => {
                    let mut page = ListToolsResult::with_all_items(tools[2..3].to_vec());
                    page.next_cursor = Some("loop-cursor".to_owned());
                    Ok(page)
                }
                Some(_) => Err(ErrorData::invalid_params("cursor inválido", None)),
            };
        }
        if std::env::var("FIXTURE_PAGINATE").as_deref() == Ok("1") {
            let cursor = request.and_then(|p| p.cursor);
            return match cursor.as_deref() {
                None => {
                    let mut page = ListToolsResult::with_all_items(tools[..3].to_vec());
                    page.next_cursor = Some("page-2".to_owned());
                    Ok(page)
                }
                Some("page-2") => Ok(ListToolsResult::with_all_items(tools[3..].to_vec())),
                Some(_) => Err(ErrorData::invalid_params("cursor inválido", None)),
            };
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "read_echo" => {
                let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                    ContentBlock::text(text.to_owned()),
                ])))
            }
            "write_action" => {
                let value = args.get("value").and_then(Value::as_str).unwrap_or("");
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                    ContentBlock::text(format!("written:{value}")),
                ])))
            }
            "unannotated_tool" => Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::text("ok-unannotated"),
            ]))),
            "slow_tool" => {
                if let Ok(signal_path) = std::env::var("FIXTURE_SLOW_STARTED_FILE") {
                    let _ = tokio::fs::write(signal_path, b"started").await;
                }
                let delay_ms = args
                    .get("delay_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(30_000);
                // Dorme ignorando cancelamento para comprovar que o gerenciador encerra a árvore presa.
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                    ContentBlock::text("slow-done"),
                ])))
            }
            "dump_env" => {
                let private_present: Vec<String> = std::env::vars()
                    .filter_map(|(env_key, _)| {
                        PRIVATE_ENV_KEYS
                            .iter()
                            .any(|priv_key| priv_key.eq_ignore_ascii_case(&env_key))
                            .then_some(env_key)
                    })
                    .collect();
                let secret_var = std::env::var("FIXTURE_SECRET_VAR").ok();
                let payload = json!({
                    "pid": std::process::id(),
                    "private_present": private_present,
                    "fixture_secret_var": secret_var,
                });
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                    ContentBlock::text(payload.to_string()),
                ])))
            }
            "tool_error_sample" => Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                ContentBlock::text("falha controlada da ferramenta"),
            ]))),
            _ => Err(ErrorData::invalid_params(
                format!("tool not found: {}", request.name),
                None,
            )),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(pid_file) = std::env::var("FIXTURE_PID_FILE") {
        std::fs::write(pid_file, std::process::id().to_string())?;
    }
    if std::env::var("FIXTURE_HANG_INIT").as_deref() == Ok("1") {
        // Bloqueia antes de responder ao initialize para testar timeout/cancelamento de abertura.
        std::future::pending::<()>().await;
    }
    let service = FixtureMcpServer.serve(rmcp::transport::io::stdio()).await?;
    let _ = service.waiting().await;
    Ok(())
}
