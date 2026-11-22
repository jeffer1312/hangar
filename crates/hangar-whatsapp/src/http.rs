//! Rotas `/api/whatsapp*` (contrato C4), só do dono. Convidado leva 403 antes de qualquer leitura; o
//! que é do binário vai ao C2 assinado, e a resposta só passa com a assinatura dele conferida.
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hangar_core::edge::Edge;
use hangar_core::guest::Caller;
use hangar_core::http::{detail, json_response};
use serde_json::{Map, Value, json};

use crate::client::{CallError, Client, Reply};
use crate::install::{self, Busy, Context, Installer};

const MAX_BODY: usize = 64 * 1024;
const OWNER_ONLY: (&str, &str) = ("whatsapp_owner_only", "só o dono usa o WhatsApp do Hangar");
const OFF_MSG: &str = "O WhatsApp está desligado. Ligue em Configurações > WhatsApp.";
/// Parâmetros que cada leitura repassa ao C2; o resto (o `token` do dono, por exemplo) fica aqui.
const CHATS: &[&str] = &["q", "limit"];
const MESSAGES: &[&str] = &["chat", "since", "until", "before_id", "limit"];
const SEARCH: &[&str] = &["q", "chat", "limit"];

/// O WhatsApp de uma máquina: instalação, aceite e o cliente do serviço.
pub struct WhatsApp {
    ctx: Context,
    installer: Installer,
    client: Client,
    /// O último texto do QR e o desenho dele: o resumo relido com o mesmo QR não o desenha de novo.
    qr: Mutex<Option<(String, Option<String>)>>,
}

struct Http {
    edge: Edge,
    whatsapp: Arc<WhatsApp>,
}

/// Apagamento que parou: o arquivo que falhou e os que já tinham saído.
struct Partial {
    detail: String,
    failed: String,
    removed: Vec<String>,
}

impl WhatsApp {
    pub fn new(home: &Path) -> Arc<Self> {
        Self::with_context(Context::new(home))
    }

    pub fn with_context(ctx: Context) -> Arc<Self> {
        let client = Client::new(ctx.paths.data.clone());
        Arc::new(Self { ctx, installer: Installer::default(), client, qr: Mutex::default() })
    }

    /// O QR do pareamento em SVG, recodificado só quando o texto muda.
    fn qr_svg(&self, text: &str) -> Option<String> {
        let mut cached = self.qr.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((last, svg)) = cached.as_ref() && last == text {
            return svg.clone();
        }
        let svg = hangar_core::qr::svg(text);
        *cached = Some((text.to_owned(), svg.clone()));
        svg
    }
}

/// As rotas do dono. A porta do convidado não monta este roteador.
pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, whatsapp: Arc<WhatsApp>) -> Router<S> {
    Router::new()
        .route("/api/whatsapp", axum::routing::any(public))
        .route("/api/whatsapp/{*rest}", axum::routing::any(public))
        .with_state(Arc::new(Http { edge, whatsapp }))
}

/// Caminho de uma leitura, para quem chama a rota em processo (as tools do MCP): `route` é `chats`,
/// `messages` ou `search`; valores vazios ficam de fora.
pub fn api_path(route: &str, params: &[(&str, &str)]) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter().filter(|(_, v)| !v.is_empty()))
        .finish();
    if query.is_empty() { format!("/api/whatsapp/{route}") } else { format!("/api/whatsapp/{route}?{query}") }
}

async fn public(
    State(state): State<Arc<Http>>,
    peer: Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>,
    request: Request,
) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    // O CORS só olha se o pedido veio com `Origin`.
    let mut origin = HeaderMap::new();
    if let Some(value) = request.headers().get(header::ORIGIN) {
        origin.insert(header::ORIGIN, value.clone());
    }
    let mut response = reply(&state, peer.ok().map(|peer| peer.0), request).await;
    hangar_core::http_cors::apply(&origin, &mut response);
    response
}

async fn reply(state: &Http, peer: Option<SocketAddr>, request: Request) -> Response {
    // Convidado (convite, par ou login) nunca chega ao WhatsApp, nem com o porteiro deixando passar.
    if hangar_core::edge::on_guest_port(&request) || matches!(hangar_core::guest::caller(&request), Some(Caller::Guest(_))) {
        return refused(StatusCode::FORBIDDEN, OWNER_ONLY.0, OWNER_ONLY.1);
    }
    let socket = peer.unwrap_or_else(|| "0.0.0.0:0".parse().expect("endereço estático válido"));
    if let Err(response) = state.edge.owner_only(socket, &request, OWNER_ONLY).await {
        return response;
    }
    let path = hangar_core::http::uvicorn_path(request.uri().path());
    let query = request.uri().query().unwrap_or_default().to_owned();
    let method = request.method().clone();
    let wa = &state.whatsapp;
    let route = path.trim_end_matches('/').strip_prefix("/api/whatsapp").unwrap_or_default().to_owned();
    match (method, route.as_str()) {
        (Method::GET, "") => overview(wa).await,
        (Method::POST, "/install") => match body(request).await {
            Ok(body) => start_install(wa, &body),
            Err(response) => response,
        },
        (Method::POST, "/update") => start_update(wa),
        (Method::POST, "/pair") => match body(request).await {
            Ok(body) => pair(wa, &body).await,
            Err(response) => response,
        },
        (Method::POST, "/logout") => forward(wa, Method::POST, "/v1/logout", &[], None).await,
        (Method::POST, "/wipe") => match body(request).await {
            Ok(body) => wipe(wa, &body).await,
            Err(response) => response,
        },
        (Method::POST, "/uninstall") => match body(request).await {
            Ok(body) => uninstall(wa, &body).await,
            Err(response) => response,
        },
        (Method::GET, "/chats") => forward(wa, Method::GET, "/v1/chats", &pick(&query, CHATS), None).await,
        (Method::GET, "/messages") => forward(wa, Method::GET, "/v1/messages", &pick(&query, MESSAGES), None).await,
        (Method::GET, "/search") => forward(wa, Method::GET, "/v1/search", &pick(&query, SEARCH), None).await,
        (Method::POST, "/media/request") => match body(request).await {
            Ok(body) => media_request(wa, &body).await,
            Err(response) => response,
        },
        _ => refused(StatusCode::NOT_FOUND, "whatsapp_not_found", "rota do WhatsApp desconhecida"),
    }
}

/// `GET /api/whatsapp`: instalação, aceite, serviço, tarefa e o `status` do C2 com o QR em SVG.
async fn overview(wa: &WhatsApp) -> Response {
    let paths = &wa.ctx.paths;
    let installed = install::current(paths).is_some();
    let down = if installed { "stopped" } else { "absent" };
    // De pé só com o status assinado. Sem conexão (pid do `control.json` reaproveitado, porta fechada) é
    // parado; algo que responde sem a assinatura continua "running", com o código em `status_error`.
    let (service, status, status_error) = match wa.client.status_within(crate::client::OVERVIEW_STATUS_TIMEOUT).await {
        Ok(status) => ("running", status, Value::Null),
        Err(CallError::Off(_)) => (down, Value::Null, Value::Null),
        Err(error) => {
            let (_, code, _) = call_failure(&error);
            ("running", Value::Null, json!(code))
        }
    };
    let qr_svg = status["qr"].as_str().filter(|q| !q.is_empty()).and_then(|q| wa.qr_svg(q));
    json_response(StatusCode::OK, json!({
        "installed": installed, "accepted_at": install::accepted_at(paths), "service": service,
        "job": wa.installer.job(), "status": status, "qr_svg": qr_svg, "status_error": status_error,
    }))
}

fn start_install(wa: &WhatsApp, body: &Value) -> Response {
    if body["accept"] != true {
        return refused(StatusCode::BAD_REQUEST, "whatsapp_accept_required", "ligar o WhatsApp exige aceitar o aviso de risco");
    }
    // Recusa de ocupado antes de gravar: um 409 não registra aceite.
    if wa.installer.busy() {
        return busy();
    }
    if let Err(failure) = install::accept(&wa.ctx.paths) {
        tracing::warn!(code = failure.code, detail = %failure.detail, "whatsapp: aceite não gravado");
        return refused(StatusCode::INTERNAL_SERVER_ERROR, failure.code, "não consegui gravar o aceite");
    }
    started(wa, install::spawn(&wa.installer, "install", wa.ctx.clone()))
}

fn start_update(wa: &WhatsApp) -> Response {
    if install::current(&wa.ctx.paths).is_none() {
        return refused(StatusCode::CONFLICT, "whatsapp_not_installed", "o WhatsApp não está instalado");
    }
    started(wa, install::spawn(&wa.installer, "update", wa.ctx.clone()))
}

fn started(wa: &WhatsApp, result: Result<(), Busy>) -> Response {
    match result {
        Ok(()) => json_response(StatusCode::ACCEPTED, json!({"job": wa.installer.job()})),
        Err(Busy) => busy(),
    }
}

async fn pair(wa: &WhatsApp, body: &Value) -> Response {
    let out = match &body["phone"] {
        Value::Null => None,
        Value::String(phone) => Some(json!({"phone": phone})),
        _ => return refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "phone deve ser texto"),
    };
    forward(wa, Method::POST, "/v1/pair", &[], out.as_ref()).await
}

/// Com o serviço de pé, o binário apaga e desvincula; parado, o Hangar apaga e a tela pede que a pessoa
/// tire o aparelho no celular.
async fn wipe(wa: &WhatsApp, body: &Value) -> Response {
    if body["confirm"] != true {
        return refused(StatusCode::BAD_REQUEST, "whatsapp_confirm_required", "apagar exige {\"confirm\":true}");
    }
    // De pé é o serviço respondendo: só a falta de conexão leva ao ramo do disco, com as guardas dele.
    match wa.client.call(reqwest::Method::POST, "/v1/wipe", &[], Some(&json!({"confirm": true}))).await {
        Ok(reply) => return relay(reply),
        Err(CallError::Off(_)) => {}
        Err(error) => return call_error(&error),
    }
    let Ok(_guard) = wa.installer.inline() else { return busy() };
    // Subindo, caindo ou esperando religar, o processo pode abrir os bancos sem ter escrito o `control.json`.
    // No Windows não há como perguntar, mas lá o arquivo aberto não se apaga e a resposta diz o que já saiu.
    if install::current(&wa.ctx.paths).is_some()
        && hangar_managed_binary::service::process_running(&wa.ctx.paths.home, &install::SERVICE).await {
        return refused(StatusCode::CONFLICT, "whatsapp_service_starting", "o serviço do WhatsApp está subindo; tente de novo em alguns segundos");
    }
    let data = wa.ctx.paths.data.clone();
    match tokio::task::spawn_blocking(move || wipe_data(&data)).await {
        Ok(Ok(())) => json_response(StatusCode::OK, json!({"logout": "not_done"})),
        Ok(Err(partial)) => {
            tracing::warn!(detail = %partial.detail, removed = ?partial.removed, "whatsapp: apagar os dados falhou");
            let (code, msg) = if partial.removed.is_empty() {
                ("whatsapp_wipe_failed", "não consegui apagar os dados do WhatsApp")
            } else {
                ("whatsapp_wipe_partial", "apaguei só parte dos dados do WhatsApp")
            };
            detail(StatusCode::INTERNAL_SERVER_ERROR, code, msg, json!({"removed": partial.removed, "failed": partial.failed}))
        }
        Err(_) => refused(StatusCode::INTERNAL_SERVER_ERROR, "whatsapp_wipe_failed", "a limpeza parou no meio"),
    }
}

async fn uninstall(wa: &WhatsApp, body: &Value) -> Response {
    if body["confirm"] != true {
        return refused(StatusCode::BAD_REQUEST, "whatsapp_confirm_required", "remover exige {\"confirm\":true}");
    }
    let logout = match &body["logout"] {
        Value::Null => false,
        Value::Bool(logout) => *logout,
        _ => return refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "logout deve ser true ou false"),
    };
    let Ok(_guard) = wa.installer.inline() else { return busy() };
    let unlinked = if logout {
        match wa.client.call(reqwest::Method::POST, "/v1/logout", &[], None).await {
            Ok(Reply { status: 204, .. }) => "done",
            Ok(reply) if reply.status == 409 && c2_code(&reply.body).as_deref() == Some("not_paired") => "not_paired",
            Ok(reply) => return relay(reply),
            Err(error) => return call_error(&error),
        }
    } else {
        "not_requested"
    };
    if let Err(failure) = install::uninstall(&wa.ctx).await {
        tracing::warn!(code = failure.code, detail = %failure.detail, "whatsapp: remover falhou");
        return detail(StatusCode::INTERNAL_SERVER_ERROR, failure.code, "não consegui remover o serviço do WhatsApp",
            json!({"detail": failure.detail, "logout": unlinked}));
    }
    wa.installer.forget();
    json_response(StatusCode::OK, json!({"uninstalled": true, "logout": unlinked}))
}

async fn media_request(wa: &WhatsApp, body: &Value) -> Response {
    let mut out = Map::new();
    for field in ["chat", "id", "since"] {
        match &body[field] {
            Value::Null => {}
            Value::String(text) => { out.insert(field.into(), json!(text)); }
            _ => return refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "chat, id e since são texto"),
        }
    }
    forward(wa, Method::POST, "/v1/media/request", &[], Some(&Value::Object(out))).await
}

async fn forward(wa: &WhatsApp, method: Method, path: &str, query: &[(String, String)], body: Option<&Value>) -> Response {
    let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    match wa.client.call(method, path, &query, body).await {
        Ok(reply) => relay(reply),
        Err(error) => call_error(&error),
    }
}

/// O que o C2 respondeu, no formato da casa: sucesso como veio; erro com o código prefixado `whatsapp_`.
fn relay(reply: Reply) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::NO_CONTENT {
        return StatusCode::NO_CONTENT.into_response();
    }
    if status.is_success() {
        // 202 sem corpo (pareamento) sai como `{}`: os clientes leem JSON de todo sucesso que não é 204.
        if reply.body.iter().all(u8::is_ascii_whitespace) {
            return json_response(status, json!({}));
        }
        // Só se confere que é JSON em UTF-8 (o `IgnoredAny` não confere o UTF-8 dos textos): os bytes assinados
        // passam como vieram.
        let json = std::str::from_utf8(&reply.body).is_ok_and(|text| serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok());
        return if json {
            (status, [(header::CONTENT_TYPE, "application/json")], reply.body).into_response()
        } else {
            refused(StatusCode::BAD_GATEWAY, "whatsapp_bad_reply", "o serviço do WhatsApp respondeu algo que não é JSON")
        };
    }
    let parsed: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    let code = c2_code(&reply.body).map_or_else(|| "whatsapp_error".to_owned(), |code| format!("whatsapp_{code}"));
    let msg = parsed["detail"]["msg"].as_str().unwrap_or("o serviço do WhatsApp recusou o pedido");
    // 401 do binário é a assinatura do Hangar recusada (chave ou relógio), não o token de quem pediu.
    let status = if status == StatusCode::UNAUTHORIZED { StatusCode::BAD_GATEWAY } else { status };
    detail(status, &code, msg, json!({}))
}

/// O `detail.code` do C2, só com letras minúsculas e `_`.
fn c2_code(body: &[u8]) -> Option<String> {
    let parsed: Value = serde_json::from_slice(body).ok()?;
    parsed["detail"]["code"].as_str()
        .filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        .map(str::to_owned)
}

fn call_error(error: &CallError) -> Response {
    let (status, code, msg) = call_failure(error);
    if let CallError::Untrusted(detail) | CallError::Transport(detail) = error {
        tracing::warn!(code, %detail, "whatsapp: pedido ao serviço falhou");
    }
    refused(status, code, msg)
}

fn call_failure(error: &CallError) -> (StatusCode, &'static str, &'static str) {
    match error {
        CallError::Off(_) => (StatusCode::CONFLICT, "whatsapp_off", OFF_MSG),
        CallError::Untrusted(_) => (StatusCode::BAD_GATEWAY, "whatsapp_untrusted", "a resposta do serviço do WhatsApp não tem a assinatura dele"),
        CallError::Transport(_) => (StatusCode::BAD_GATEWAY, "whatsapp_unreachable", "o serviço do WhatsApp não respondeu"),
    }
}

fn busy() -> Response {
    refused(StatusCode::CONFLICT, "whatsapp_busy", "outra instalação ou remoção do WhatsApp está em curso")
}

fn refused(status: StatusCode, code: &str, msg: &str) -> Response {
    detail(status, code, msg, json!({}))
}

/// Corpo JSON de objeto; vazio vale `{}`.
async fn body(request: Request) -> Result<Value, Response> {
    let raw = axum::body::to_bytes(request.into_body(), MAX_BODY).await
        .map_err(|_| refused(StatusCode::PAYLOAD_TOO_LARGE, "whatsapp_bad_request", "corpo do pedido grande demais"))?;
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(&raw) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "corpo do pedido deve ser um objeto JSON")),
    }
}

/// Só os parâmetros que a leitura aceita, sem os vazios.
fn pick(query: &str, allowed: &[&str]) -> Vec<(String, String)> {
    form_urlencoded::parse(query.as_bytes())
        .filter(|(k, v)| allowed.contains(&k.as_ref()) && !v.is_empty())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Apaga bancos (com o WAL e o journal do SQLite) e mídia; `control.*` fica, para o binário subir igual.
/// Parar no meio (no Windows, arquivo aberto) devolve o que já saiu.
fn wipe_data(data: &Path) -> Result<(), Partial> {
    let mut removed = Vec::new();
    let stop = |name: String, e: std::io::Error, removed: Vec<String>| Partial { detail: format!("{name}: {e}"), failed: name, removed };
    for db in ["messages.db", "session.db"] {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let name = format!("{db}{suffix}");
            match std::fs::remove_file(data.join(&name)) {
                Ok(()) => removed.push(name),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(stop(name, e, removed)),
            }
        }
    }
    match std::fs::remove_dir_all(data.join("media")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(stop("media".into(), e, removed)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
