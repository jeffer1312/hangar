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
use hangar_managed_binary::service::{Error as ServiceError, ProcessState};
use serde_json::{Map, Value, json};

use crate::client::{CallError, Client, Reply};
use crate::install::{self, Busy, Context, Installed, Installer, UpdateCheck};
use crate::outbox::{self, Outbox};

const MAX_BODY: usize = 64 * 1024;
const OWNER_ONLY: (&str, &str) = ("whatsapp_owner_only", "só o dono usa o WhatsApp do Hangar");
const OFF_MSG: &str = "O WhatsApp está desligado. Ligue em Configurações > WhatsApp.";
/// Parâmetros que cada leitura repassa ao C2; o resto (o `token` do dono, por exemplo) fica aqui.
const CHATS: &[&str] = &["q", "limit"];
const MESSAGES: &[&str] = &["chat", "since", "until", "before_id", "limit"];
const SEARCH: &[&str] = &["q", "chat", "limit"];
const OUTBOX: &[&str] = &["state"];
/// Código e frase da recusa ao binário que não tem a rota de envio, ou a do envio editado.
const SEND_UNSUPPORTED: (&str, &str) = ("whatsapp_send_unsupported", "o hangar-whatsapp instalado não envia mensagens; atualize o hangar-whatsapp");
const EDIT_UNSUPPORTED: (&str, &str) = ("whatsapp_edit_unsupported",
    "o hangar-whatsapp instalado não envia texto editado; atualize o hangar-whatsapp. Nada foi enviado");
const OUTDATED_MSG: &str = "o WhatsApp recusou a versão do hangar-whatsapp; atualize o hangar-whatsapp";
/// Frase do envio recusado porque uma troca de versão, do dono ou do relógio, segura a vez.
const UPDATING_MSG: &str = "o WhatsApp está atualizando; tente de novo em instantes";
/// A tarefa da passada do relógio: as telas reagem só a `update`, que é a do dono.
pub const AUTO_UPDATE_KIND: &str = "auto_update";

/// O WhatsApp de uma máquina: instalação, aceite e o cliente do serviço.
pub struct WhatsApp {
    ctx: Context,
    installer: Installer,
    client: Client,
    /// O último texto do QR e o desenho dele: o resumo relido com o mesmo QR não o desenha de novo.
    qr: Mutex<Option<(String, Option<String>)>>,
    outbox: Outbox,
    /// Um envio por vez segura a vez do instalador: dois envios seguidos não se recusam como "atualizando".
    sending: tokio::sync::Mutex<()>,
    /// Avisa quando um envio fica esperando outro: a prova da fila não depende de relógio.
    #[cfg(test)]
    send_waiting: tokio::sync::Notify,
}

/// Uma passada da atualização automática: `from` é a tag instalada antes; `to`, a instalada depois quando trocou
/// (`Updated`), senão a `latest` da última procura (ou `from`, sem procura gravada).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoUpdate {
    pub from: String,
    pub to: String,
    pub result: Result<install::Outcome, install::Failure>,
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
        Self::with_outbox(ctx, Outbox::default())
    }

    /// `outbox` com a subida desta vida já marcada ([`Outbox::started_at`]).
    pub fn with_outbox(ctx: Context, outbox: Outbox) -> Arc<Self> {
        let client = Client::new(ctx.paths.data.clone());
        Arc::new(Self {
            ctx, installer: Installer::default(), client, qr: Mutex::default(), outbox, sending: tokio::sync::Mutex::default(),
            #[cfg(test)]
            send_waiting: tokio::sync::Notify::new(),
        })
    }

    /// A atualização do relógio ([`install::Mode::Auto`]) na vez do instalador, como tarefa que o `GET` mostra.
    /// `None`: WhatsApp não instalado, ou outra tarefa, operação ou envio segurando a vez.
    pub async fn auto_update(&self) -> Option<AutoUpdate> {
        let from = install::current(&self.ctx.paths)?.tag;
        let reservation = self.installer.reserve(AUTO_UPDATE_KIND, install::Step::Download).ok()?;
        Some(self.run_auto_update(reservation, from).await)
    }

    /// A passada na vez já guardada; `from` é a tag instalada antes dela.
    async fn run_auto_update(&self, reservation: install::Reservation, from: String) -> AutoUpdate {
        let (sent, got) = tokio::sync::oneshot::channel();
        let ctx = self.ctx.clone();
        reservation.start(move |progress| async move {
            let result = install::update(&ctx, install::Mode::Auto, &*progress).await;
            let _ = sent.send(result.clone());
            result
        }).await;
        // Sem resultado, a tarefa caiu no meio: o `Jobs` já gravou a queda para a tela.
        let result = got.await.unwrap_or_else(|_| Err(install::Failure::new("whatsapp_job_crashed", "a atualização automática parou no meio")));
        let to = match &result {
            Ok(install::Outcome::Updated) => install::current(&self.ctx.paths).map(|c| c.tag),
            _ => install::read_update_check(&self.ctx.paths).map(|c| c.latest),
        };
        AutoUpdate { to: to.unwrap_or_else(|| from.clone()), from, result }
    }

    /// Os rascunhos pendentes (evento `whatsapp_outbox`).
    pub fn outbox(&self) -> &Outbox {
        &self.outbox
    }

    /// A lista dos rascunhos pendentes, para sempre: a cada 30 s e a cada rascunho criado ou respondido.
    pub async fn watch_outbox(&self, hooks: &dyn outbox::Hooks) {
        self.outbox.run(&self.client, hooks).await;
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
        (Method::POST, "/update") => match body(request).await {
            Ok(body) => start_update(wa, &body),
            Err(response) => response,
        },
        (Method::POST, "/pair") => match body(request).await {
            Ok(body) => pair(wa, &body).await,
            Err(response) => response,
        },
        (Method::POST, "/logout") => forward(wa, Method::POST, "/v1/logout", &[], None).await,
        (Method::POST, "/wipe") => match body(request).await {
            Ok(body) => {
                let response = wipe(wa, &body).await;
                // Os rascunhos somem com os dados: a lista relê, e a espera da tool acorda e vê o rascunho sumido.
                wa.outbox.poke();
                response
            }
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
        (Method::POST, "/outbox") => match body(request).await {
            Ok(body) => create_draft(wa, &body).await,
            Err(response) => response,
        },
        (Method::GET, "/outbox") => list_drafts(wa, &query).await,
        (method, route) if route.starts_with("/outbox/") => draft(wa, method, &route["/outbox/".len()..], request).await,
        _ => refused(StatusCode::NOT_FOUND, "whatsapp_not_found", "rota do WhatsApp desconhecida"),
    }
}

/// `GET /api/whatsapp`: instalação, aceite, serviço, tarefa, o `status` do C2 com o QR em SVG, a tag instalada e a
/// versão nova da última procura.
async fn overview(wa: &WhatsApp) -> Response {
    let paths = &wa.ctx.paths;
    let current = install::current(paths);
    let installed = current.is_some();
    let down = if installed { "stopped" } else { "absent" };
    // De pé só com o status assinado. Sem conexão (pid do `control.json` reaproveitado, porta fechada) é
    // parado; algo que responde sem a assinatura continua "running", com o código em `status_error`.
    let (service, status, status_error) = match wa.client.status_within(crate::client::OVERVIEW_STATUS_TIMEOUT).await {
        Ok(status) => ("running", status, Value::Null),
        Err(CallError::Off(_)) => match &current {
            Some(current) => {
                let process = hangar_managed_binary::service::process_state(&paths.home, &install::SERVICE, &current.binary).await;
                (down, Value::Null, stopped_error(process))
            }
            None => (down, Value::Null, Value::Null),
        },
        Err(error) => {
            let (_, code, _) = call_failure(&error);
            ("running", Value::Null, json!(code))
        }
    };
    let qr_svg = status["qr"].as_str().filter(|q| !q.is_empty()).and_then(|q| wa.qr_svg(q));
    json_response(StatusCode::OK, json!({
        "installed": installed, "accepted_at": install::accepted_at(paths), "service": service,
        "job": wa.installer.job(), "status": status, "qr_svg": qr_svg, "status_error": status_error,
        "installed_tag": current.as_ref().map(|c| c.tag.as_str()),
        "update": available_update(current.as_ref(), install::read_update_check(paths)),
    }))
}

/// A última procura (contrato C-D) só vale como versão nova quando a `latest` dela passa da instalada.
fn available_update(current: Option<&Installed>, check: Option<UpdateCheck>) -> Value {
    match (current, check) {
        (Some(current), Some(check)) if crate::release::newer(&check.latest, &current.tag) => {
            serde_json::to_value(check).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// Com o serviço parado, `whatsapp_schema_newer` quando o último processo saiu porque os dados são de uma
/// versão mais nova do binário: religar não adianta, só atualizar. Gerenciador que não responde deixa o
/// parado de sempre e vai ao log.
fn stopped_error(process: Result<ProcessState, ServiceError>) -> Value {
    match process {
        Ok(process) if process.last_exit == Some(i32::from(install::SCHEMA_NEWER_EXIT)) => json!("whatsapp_schema_newer"),
        Ok(_) => Value::Null,
        Err(error) => {
            tracing::warn!(step = %error.step, detail = %error.detail, "whatsapp: estado do serviço ilegível");
            Value::Null
        }
    }
}

/// Apagar do disco só com o serviço confirmado parado: subindo ou sem resposta do gerenciador, recusa.
fn wipe_guard(running: Result<bool, ServiceError>) -> Result<(), Response> {
    match running {
        Ok(false) => Ok(()),
        Ok(true) => Err(refused(StatusCode::CONFLICT, "whatsapp_service_starting",
            "o serviço do WhatsApp está subindo; tente de novo em alguns segundos")),
        Err(error) => {
            tracing::warn!(step = %error.step, detail = %error.detail, "whatsapp: estado do serviço ilegível; não apaguei");
            Err(refused(StatusCode::CONFLICT, "whatsapp_service_state_unknown",
                "não consegui confirmar que o serviço do WhatsApp está parado; tente de novo"))
        }
    }
}

fn start_install(wa: &WhatsApp, body: &Value) -> Response {
    if body["accept"] != true {
        return refused(StatusCode::BAD_REQUEST, "whatsapp_accept_required", "ligar o WhatsApp exige aceitar o aviso de risco");
    }
    // A vez é guardada antes de gravar: um 409 não registra aceite, e o aceite gravado sempre começa a tarefa.
    let Ok(reservation) = wa.installer.reserve("install", install::Step::Download) else { return busy() };
    if let Err(failure) = install::accept(&wa.ctx.paths) {
        tracing::warn!(code = failure.code, detail = %failure.detail, "whatsapp: aceite não gravado");
        return refused(StatusCode::INTERNAL_SERVER_ERROR, failure.code, "não consegui gravar o aceite");
    }
    install::begin(reservation, wa.ctx.clone(), install::Mode::Manual);
    started(wa, Ok(()))
}

fn start_update(wa: &WhatsApp, body: &Value) -> Response {
    let mode = match update_mode(body) {
        Ok(mode) => mode,
        Err(response) => return response,
    };
    if install::current(&wa.ctx.paths).is_none() {
        return refused(StatusCode::CONFLICT, "whatsapp_not_installed", "o WhatsApp não está instalado");
    }
    started(wa, install::spawn(&wa.installer, "update", wa.ctx.clone(), mode))
}

/// `confirm_migration` ausente ou `false` é o botão de sempre; `true`, o dono confirmou a mudança de formato dos dados.
fn update_mode(body: &Value) -> Result<install::Mode, Response> {
    match &body["confirm_migration"] {
        Value::Null | Value::Bool(false) => Ok(install::Mode::Manual),
        Value::Bool(true) => Ok(install::Mode::Confirmed),
        _ => Err(refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "confirm_migration deve ser true ou false")),
    }
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
    // Atualização em curso troca o processo no meio do pedido: apagar espera a vez, de pé ou parado.
    let Ok(_guard) = wa.installer.inline() else { return busy() };
    // De pé é o serviço respondendo: só a falta de conexão leva ao ramo do disco, com as guardas dele.
    match wa.client.call(reqwest::Method::POST, "/v1/wipe", &[], Some(&json!({"confirm": true}))).await {
        Ok(reply) => return relay(reply),
        Err(CallError::Off(_)) => {}
        Err(error) => return call_error(&error),
    }
    // Subindo, caindo ou esperando religar, o processo pode abrir os bancos sem ter escrito o `control.json`:
    // só o gerenciador confirmando a parada (unit parada, LaunchAgent descarregado, tarefa pronta) libera.
    if install::current(&wa.ctx.paths).is_some()
        && let Err(response) = wipe_guard(hangar_managed_binary::service::process_running(&wa.ctx.paths.home, &install::SERVICE).await) {
        return response;
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

/// `POST /api/whatsapp/outbox`: cria o rascunho no binário. Só os campos do C5 seguem.
async fn create_draft(wa: &WhatsApp, body: &Value) -> Response {
    let mut out = Map::new();
    for (field, required) in [("chat", true), ("text", true), ("origin", true), ("file", false), ("reply_to", false), ("origin_cwd", false)] {
        match &body[field] {
            Value::Null if !required => {}
            Value::String(text) => { out.insert(field.into(), json!(text)); }
            _ => return refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request",
                "chat, text e origin são texto; file, reply_to e origin_cwd, texto ou ausentes"),
        }
    }
    if let Err(response) = api_at_least(wa, outbox::SEND_API, SEND_UNSUPPORTED) {
        return response;
    }
    let response = forward(wa, Method::POST, "/v1/outbox", &[], Some(&Value::Object(out))).await;
    wa.outbox.poke();
    response
}

/// `GET /api/whatsapp/outbox`: os pendentes, salvo outro `state` pedido.
async fn list_drafts(wa: &WhatsApp, query: &str) -> Response {
    if let Err(response) = api_at_least(wa, outbox::SEND_API, SEND_UNSUPPORTED) {
        return response;
    }
    let mut params = pick(query, OUTBOX);
    if params.is_empty() {
        params.push(("state".into(), "pending".into()));
    }
    forward(wa, Method::GET, "/v1/outbox", &params, None).await
}

/// `GET /api/whatsapp/outbox/{id}`, `POST …/{id}/send` (`{"via":"desktop"|"mobile"}`), `POST …/{id}/send-edited`
/// (`{"via", "text"}`, o texto que o dono editou no cartão) e `POST …/{id}/decline`.
async fn draft(wa: &WhatsApp, method: Method, rest: &str, request: Request) -> Response {
    let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
    if !outbox::id_ok(id) {
        return refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "id de rascunho inválido");
    }
    if let Err(response) = api_at_least(wa, outbox::SEND_API, SEND_UNSUPPORTED) {
        return response;
    }
    let path = format!("/v1/outbox/{id}");
    match (method, action) {
        (Method::GET, "") => forward(wa, Method::GET, &path, &[], None).await,
        (Method::POST, action @ ("send" | "send-edited")) => {
            let edited = action == "send-edited";
            if edited && let Err(response) = api_at_least(wa, outbox::EDIT_API, EDIT_UNSUPPORTED) {
                return response;
            }
            let body = match body(request).await {
                Ok(body) => body,
                Err(response) => return response,
            };
            let via = match parse_via(&body) {
                Ok(via) => via,
                Err(response) => return response,
            };
            let mut out = json!({"via": via});
            if edited {
                // Normalizar e validar o texto (tamanho, vazio sem anexo) é do binário, que responde 400 e deixa o rascunho pendente.
                let Some(text) = body["text"].as_str() else {
                    return refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "text é obrigatório e é texto");
                };
                out["text"] = json!(text);
            }
            // Uma troca de versão no meio cortaria o envio (fica `unconfirmed`): ele segura a vez do instalador.
            let _turn = match wa.sending.try_lock() {
                Ok(turn) => turn,
                Err(_) => {
                    #[cfg(test)]
                    wa.send_waiting.notify_one();
                    wa.sending.lock().await
                }
            };
            let Ok(_guard) = wa.installer.inline() else { return send_busy(wa) };
            let response = match wa.client.call(Method::POST, &format!("{path}/{action}"), &[], Some(&out)).await {
                // O 404 que não é de rascunho é a rota que não existe (binário antigo que se anuncia novo): vira a mesma
                // recusa do `api` antigo, nunca sucesso.
                Ok(reply) if edited && reply.status == 404 && c2_code(&reply.body).as_deref() != Some("not_found") => {
                    refused(StatusCode::CONFLICT, EDIT_UNSUPPORTED.0, EDIT_UNSUPPORTED.1)
                }
                Ok(reply) => relay(reply),
                Err(error) => call_error(&error),
            };
            wa.outbox.poke();
            response
        }
        (Method::POST, "decline") => {
            let response = forward(wa, Method::POST, &format!("{path}/decline"), &[], None).await;
            wa.outbox.poke();
            response
        }
        _ => refused(StatusCode::NOT_FOUND, "whatsapp_not_found", "rota do WhatsApp desconhecida"),
    }
}

fn parse_via(body: &Value) -> Result<&str, Response> {
    match body["via"].as_str() {
        Some(via @ ("desktop" | "mobile")) => Ok(via),
        _ => Err(refused(StatusCode::BAD_REQUEST, "whatsapp_bad_request", "via é \"desktop\" ou \"mobile\"")),
    }
}

/// Binário que se anuncia abaixo de `min` não tem a rota: a recusa sai daqui, sem pedido ao serviço. Sem `control.json`
/// segue: o pedido responde `whatsapp_off`.
fn api_at_least(wa: &WhatsApp, min: u32, (code, msg): (&str, &str)) -> Result<(), Response> {
    match wa.client.claimed() {
        Some(control) if control.api < min => Err(refused(StatusCode::CONFLICT, code, msg)),
        _ => Ok(()),
    }
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
    // `outdated`: quem resolve é atualizar o binário, e a frase diz isso.
    let msg = if code == "whatsapp_outdated" { OUTDATED_MSG } else { parsed["detail"]["msg"].as_str().unwrap_or("o serviço do WhatsApp recusou o pedido") };
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

pub(crate) fn call_failure(error: &CallError) -> (StatusCode, &'static str, &'static str) {
    match error {
        CallError::Off(_) => (StatusCode::CONFLICT, "whatsapp_off", OFF_MSG),
        CallError::Untrusted(_) => (StatusCode::BAD_GATEWAY, "whatsapp_untrusted", "a resposta do serviço do WhatsApp não tem a assinatura dele"),
        CallError::Transport(_) => (StatusCode::BAD_GATEWAY, "whatsapp_unreachable", "o serviço do WhatsApp não respondeu"),
    }
}

/// Envio recusado com a vez ocupada: "atualizando" só com uma troca de versão em curso; o resto é a operação de sempre.
fn send_busy(wa: &WhatsApp) -> Response {
    let job = wa.installer.job();
    let updating = job["done"] == false && matches!(job["kind"].as_str(), Some(kind) if kind == "update" || kind == AUTO_UPDATE_KIND);
    if updating { refused(StatusCode::CONFLICT, "whatsapp_busy", UPDATING_MSG) } else { busy() }
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
