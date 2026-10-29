//! Servidor MCP `hangar` em `/mcp/`: as operações do `hangar-send`/`hangar-preview` como tools.
//! Cada tool chama a mesma rota pública que o CLI chama, por dentro do processo; rota que ainda é do
//! Python segue a ele num sentido, pelo repasse do próprio roteador.
pub mod identity;
mod nav;
mod tools;

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::model::{CacheScope, CallToolRequestParams, CallToolResponse, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use subtle::ConstantTimeEq;

use crate::routes::AppState;

const INSTRUCTIONS: &str = "Hangar: sessões vizinhas e o navegador embutido desta sessão. \
Mostrar algo que você montou ao usuário (desenho, mock de tela, comparação, gráfico, tabela, diagrama): \
html_render, sem esperar pedido; imagem por caminho só para print real de tela. \
Ver ou testar uma página: browser_open e depois browser (um verbo) ou browser_batch (vários já conhecidos); \
tela desconhecida com várias ações: browser_goal, quando a sessão tem o Jev. \
Falar com outra sessão: sessions, send, group, pair; abrir ou fechar: new_session, close_session; parar o turno de uma filha ligada: interrupt; \
ler ou gravar o contrato do grupo: contract.";

static CATALOG: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("catalog.json")).expect("catalog.json é o catálogo do MCP")
});

/// Nomes antigos (português) que sessões abertas antes do rename ainda chamam: chamáveis, fora do
/// catálogo. Ponte, não API: sai quando não houver sessão viva de antes.
const OLD_NAMES: [(&str, &str); 10] = [("quem_sou", "who_am_i"), ("sessoes", "sessions"), ("enviar", "send"),
    ("grupo", "group"), ("parear", "pair"), ("desparear", "unpair"), ("nova_sessao", "new_session"),
    ("nav_abrir", "browser_open"), ("nav", "browser"), ("nav_lote", "browser_batch")];

#[derive(Clone)]
struct Handler { tools: Arc<tools::Tools> }

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(CATALOG.clone()).with_ttl_ms(0).with_cache_scope(CacheScope::Private))
    }

    async fn list_resources(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::default().with_ttl_ms(0).with_cache_scope(CacheScope::Private))
    }

    async fn list_resource_templates(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::default().with_ttl_ms(0).with_cache_scope(CacheScope::Private))
    }

    async fn list_prompts(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::default().with_ttl_ms(0).with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        let headers = context.extensions.get::<axum::http::request::Parts>().map(|p| p.headers.clone()).unwrap_or_default();
        let name = OLD_NAMES.iter().find(|(old, _)| *old == request.name).map_or(&*request.name, |(_, new)| new);
        let args = request.arguments.unwrap_or_default();
        Ok(CallToolResponse::Complete(self.tools.call(name, args, &headers).await))
    }
}

/// `/mcp` e `/mcp/` sobre `api`, o roteador público inteiro: é por ele que as tools chamam as rotas.
pub fn router(st: Arc<AppState>, api: Router) -> Router {
    let service = service(Handler { tools: Arc::new(tools::Tools::new(st.clone(), api)) });
    Router::new().route_service("/mcp", service.clone()).route_service("/mcp/", service)
        .layer(axum::middleware::from_fn_with_state(st, gate))
        .layer(axum::middleware::from_fn(crate::migration_status::count_public))
}

fn service<H: ServerHandler + Clone + Send + Sync + 'static>(handler: H) -> StreamableHttpService<H, LocalSessionManager> {
    // Só esta máquina fala com o /mcp: Host e Origin de fora são recusados (DNS rebinding).
    let config = StreamableHttpServerConfig::default()
        .with_allowed_origins(["http://127.0.0.1:*", "http://localhost:*", "http://[::1]:*"]);
    // A ociosidade do SDK não conta a tool em curso como atividade: com os 5 min dele, o `browser_goal`
    // (até 300 s) perdia a resposta. 30 min parada, como no Python.
    let mut sessions = LocalSessionManager::default();
    sessions.session_config.keep_alive = Some(std::time::Duration::from_secs(1800));
    StreamableHttpService::new(move || Ok(handler.clone()), Arc::new(sessions), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use rmcp::model::{CallToolResult, ContentBlock};
    use serde_json::{Value, json};
    use tower_service::Service;

    /// Mais que o prazo mais longo de uma tool (`browser_goal`, 300 s do Jev).
    const SLOW: std::time::Duration = std::time::Duration::from_secs(400);

    #[derive(Clone)]
    struct Slow;

    impl ServerHandler for Slow {
        fn get_info(&self) -> ServerConfig { ServerConfig::new(ServerCapabilities::builder().enable_tools().build()) }

        async fn call_tool(&self, _: CallToolRequestParams, _: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
            tokio::time::sleep(SLOW).await;
            Ok(CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text("pronto")])))
        }
    }

    async fn post(svc: &mut StreamableHttpService<Slow, LocalSessionManager>, session: Option<&str>, body: Value) -> (Option<String>, String) {
        let mut req = axum::http::Request::post("/").header("host", "127.0.0.1").header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream").header("mcp-protocol-version", "2025-06-18");
        if let Some(session) = session { req = req.header("mcp-session-id", session); }
        let Ok(resp) = svc.call(req.body(Body::from(body.to_string())).unwrap()).await;
        let session = resp.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_owned());
        (session, String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).into_owned())
    }

    /// A tool que demora mais que a ociosidade padrão do SDK (5 min) ainda responde na mesma sessão:
    /// a sessão só fecha depois de 30 min parada, como no Python.
    #[tokio::test(start_paused = true)]
    async fn a_tool_longer_than_five_minutes_keeps_its_session() {
        let mut svc = service(Slow);
        let (session, _) = post(&mut svc, None, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}})).await;
        let session = session.expect("sessão aberta");
        post(&mut svc, Some(&session), json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        let mut caller = svc.clone();
        let (session_for_call, call) = (session.clone(), json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "lenta", "arguments": {}}}));
        let call = tokio::spawn(async move { post(&mut caller, Some(&session_for_call), call).await });
        while !call.is_finished() {
            tokio::time::advance(std::time::Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
        }
        let (_, body) = call.await.unwrap();
        assert!(body.contains("pronto"), "a resposta da tool lenta se perdeu: {body}");
        let (_, again) = post(&mut svc, Some(&session), json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).await;
        assert!(again.contains("\"id\":3"), "a sessão fechou depois da tool lenta: {again}");
    }
}

/// O bearer do dono, conferido antes do servidor MCP, e só de quem está nesta máquina (o celular e os
/// peers usam a API normal).
async fn gate(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let (client_ip, _) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    if !client_ip.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return (StatusCode::FORBIDDEN, [("content-type", "application/json")], r#"{"detail":"so na maquina do backend"}"#).into_response();
    }
    let token = req.headers().get("authorization").and_then(|v| v.as_bytes().strip_prefix(b"Bearer ")).unwrap_or_default();
    let owner = st.cfg.auth_token.as_bytes();
    if token.is_empty() || owner.is_empty() || !bool::from(token.ct_eq(owner)) {
        return (StatusCode::UNAUTHORIZED, [("content-type", "application/json")], r#"{"detail":"unauthorized"}"#).into_response();
    }
    next.run(req).await
}
