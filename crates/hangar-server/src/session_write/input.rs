//! `/input` e `/steer` de sessão Claude, com a mesma resposta do Python (`api.py`: `input_prompt`,
//! `steer_session`, `_send_managed`). Daqui para baixo o Rust é o dono: falha é erro com código,
//! nunca repasse. Divergência deliberada: `/steer` recusado pelo ator sem terminal é 409
//! `erro_sem_turno` (o Python deixava o ValueError virar 500; a rota dele foi corrigida junto).
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{Ctx, Write, WriteRoute, admit, busy_body, detail_body, json_response, relay, route_write, session_name};
use crate::mods::state::random_hex;
use crate::proxy::Forward;
use crate::routes::{AppState, cors, pass};
use crate::runtime::gateway::WriteTarget;
use crate::runtime::protocol::{Disposition, OperationKind, RuntimeCommand, RuntimeError, RuntimeReply};

const MSG_UNKNOWN: &str = "resultado incerto; entrada conservada sem reenvio";
const MSG_REJECTED: &str = "entrada recusada pelo runtime";
pub(super) const MSG_STEER_UNKNOWN: &str = "resultado incerto; a operação foi conservada sem reenvio";
pub(super) const MSG_STEER_REFUSED: &str = "operação recusada pelo runtime";
pub(super) const MSG_CONTROL_DEFERRED: &str = "O controle não foi executado; confira a sessão.";
pub(super) const MSG_CONTROL_UNCONFIRMED: &str = "Não foi possível confirmar o controle; confira a sessão antes de repetir.";
/// Só o envelope de erro interessa na resposta do Python ao aviso.
const RELAY_REPLY_LIMIT: usize = 1 << 20;

pub struct Params<'a> {
    pub text: &'a str,
    pub steer: bool,
    pub terminal: bool,
    pub operation_id: &'a str,
}

pub enum Sent {
    /// `entry`: o recado tem linha na fila (texto sem `/`) e o pedido quer orientação imediata.
    Done { delivered: bool, native: bool, entry: bool },
    Refused { code: &'static str, msg: String, params: Value },
}

/// Evento e código do diário do Python (`runtime.send_*`), só código e nome da sessão.
pub type Diary = Option<(&'static str, String)>;

fn queued(text: &str) -> bool { !text.trim_start().starts_with('/') }

/// O diário só aceita `[a-z0-9_]{1,64}`; o motivo do terminal pode trazer outra coisa.
pub(super) fn diary_code(raw: &str) -> String {
    let code: String = raw.chars().take(60).map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' }).collect();
    if code.is_empty() { "unknown".into() } else { code }
}

pub(super) fn payload_code(reply: &RuntimeReply) -> Option<&str> { reply.payload["code"].as_str().filter(|c| !c.is_empty()) }

fn failed(code: String, msg: String) -> (Sent, Diary) {
    let refused = Sent::Refused { code: "erro_envio_falhou", params: json!({"erro": msg}), msg };
    (refused, Some(("runtime.send_failed", diary_code(&code))))
}

/// A resposta do ator ao `submit` → o que o `_send_managed` devolveria.
pub fn classify(p: &Params, sent: &Result<RuntimeReply, RuntimeError>) -> (Sent, Diary) {
    let reply = match sent {
        Ok(reply) => reply,
        Err(error) => return failed(error.code.clone(), error.to_string()),
    };
    let (queued, entry) = (queued(p.text), p.steer && queued(p.text));
    let unknown = reply.disposition == Disposition::Unknown;
    // Com terminal a entrega incerta é confirmada depois pelo transcript, uma vez, sem reenvio.
    let proved_later = unknown && queued && p.terminal;
    if unknown && (proved_later || reply.payload["transport_lost"] == true) {
        let code = payload_code(reply).unwrap_or(if proved_later { "terminal_delivery_unknown" } else { "transport_lost" });
        return (Sent::Done { delivered: false, native: false, entry }, Some(("runtime.send_uncertain", diary_code(code))));
    }
    match reply.disposition {
        Disposition::Unknown => failed("unknown".into(), MSG_UNKNOWN.into()),
        Disposition::Rejected => failed("rejected".into(), MSG_REJECTED.into()),
        Disposition::Deferred if !queued && p.terminal => {
            // Comando de barra não tem linha na fila: adiado, ele não roda depois sozinho.
            let motivo = payload_code(reply).unwrap_or("deferred");
            let comando = p.text.split_whitespace().next().unwrap_or_default();
            let msg = format!("{comando} não foi executado: o terminal não aceitou agora ({motivo}). Mande de novo.");
            let refused = Sent::Refused { code: "erro_comando_nao_executado", msg, params: json!({"comando": comando, "motivo": motivo}) };
            (refused, Some(("runtime.command_deferred", diary_code(motivo))))
        }
        disposition => (Sent::Done { delivered: disposition == Disposition::Accepted, native: reply.payload["native"] == true, entry }, None),
    }
}

/// Só sem terminal: com ele o Claude não promove (só o Kimi), então não há `steer_queue`.
pub fn wants_steer_queue(p: &Params, sent: &Sent) -> bool {
    !p.terminal && p.steer && matches!(sent, Sent::Done { entry: true, delivered: false, .. })
}

pub fn was_steered(operation_id: &str, promoted: &Result<RuntimeReply, RuntimeError>) -> bool {
    matches!(promoted, Ok(reply) if reply.disposition == Disposition::Accepted
        && reply.payload["ids"].as_array().is_some_and(|ids| ids.iter().any(|id| id == operation_id)))
}

pub fn input_answer(sent: &Sent, steered: bool) -> (StatusCode, Value) {
    match sent {
        // A orientação confirmada também conta como entrega.
        Sent::Done { delivered, native, .. } => (StatusCode::OK, json!({"ok": true, "delivered": *delivered || steered, "steered": steered, "native": native})),
        Sent::Refused { code, msg, params } => (StatusCode::BAD_REQUEST, detail_body(code, msg, params.clone())),
    }
}

pub(super) fn sem_turno(msg: &str) -> (StatusCode, Value) { (StatusCode::CONFLICT, detail_body("erro_sem_turno", msg, json!({}))) }

pub(super) const MSG_CODEX_CONTROL: &str = "O Codex não aceitou a alteração; atualize a sessão e tente novamente.";
/// Recusa de controle do Codex sem terminal: código e frase próprios, iguais aos do Python.
pub(super) fn codex_control(msg: &str) -> (StatusCode, Value) { (StatusCode::CONFLICT, detail_body("erro_codex_controle", msg, json!({}))) }

/// `/steer` sem terminal: com texto vai ao turno em voo; sem texto promove a fila. No Codex toda
/// recusa é `erro_codex_controle`.
pub fn steer_headless(with_text: bool, codex: bool, control: &Result<RuntimeReply, RuntimeError>) -> (StatusCode, Value) {
    let answer = steer_headless_claude(with_text, control);
    if codex && answer.0 != StatusCode::OK { return codex_control(MSG_CODEX_CONTROL); }
    answer
}

fn steer_headless_claude(with_text: bool, control: &Result<RuntimeReply, RuntimeError>) -> (StatusCode, Value) {
    let reply = match control {
        Ok(reply) => reply,
        Err(error) => return sem_turno(&error.to_string()),
    };
    match reply.disposition {
        Disposition::Accepted if with_text => (StatusCode::OK, json!({"ok": true, "promoted": false})),
        Disposition::Accepted => {
            let ids: Vec<&str> = reply.payload["ids"].as_array().map(|ids| ids.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let queued: Vec<String> = ids.iter().map(|id| format!("queued-{id}")).collect();
            (StatusCode::OK, json!({"ok": true, "promoted": false, "confirmed": ids.len(), "queued_ids": queued}))
        }
        Disposition::Unknown => sem_turno(MSG_STEER_UNKNOWN),
        _ => sem_turno(reply.payload["error"].as_str().filter(|e| !e.is_empty()).unwrap_or(MSG_STEER_REFUSED)),
    }
}

pub(super) fn control_failed(msg: &str) -> (StatusCode, Value) { (StatusCode::CONFLICT, detail_body("erro_opcao_nao_convergiu", msg, json!({}))) }

/// Falha do runtime no `/steer` com terminal: o Python deixa virar 500; aqui tem código.
pub(super) fn runtime_failed(error: &RuntimeError) -> (StatusCode, Value) {
    (StatusCode::BAD_GATEWAY, detail_body("erro_envio_falhou", &error.to_string(), json!({"erro": error.to_string()})))
}

/// `/steer` com terminal, 1ª metade: `Ok(promoted)` quando o controle foi aceito.
pub fn steer_terminal_control(control: &Result<RuntimeReply, RuntimeError>) -> Result<bool, (StatusCode, Value)> {
    let reply = control.as_ref().map_err(runtime_failed)?;
    match reply.disposition {
        Disposition::Accepted => Ok(match reply.payload.get("promoted") { None => true, Some(Value::Bool(b)) => *b, Some(Value::Null) => false, Some(_) => true }),
        Disposition::Deferred => Err(control_failed(MSG_CONTROL_DEFERRED)),
        _ => Err(control_failed(MSG_CONTROL_UNCONFIRMED)),
    }
}

/// 2ª metade: a contagem de entregas que o `confirm` baixou da fila.
pub fn steer_terminal_done(promoted: bool, confirm: &Result<Value, RuntimeError>) -> (StatusCode, Value) {
    match confirm {
        Ok(done) => (StatusCode::OK, json!({"ok": true, "promoted": promoted, "confirmed": done["confirmed"].as_u64().unwrap_or(0)})),
        Err(error) => runtime_failed(error),
    }
}

/// O `/compact` do Codex é controle (`thread/compact/start`), não texto: quem decide é o `_send_managed` do Python.
pub fn relays_codex_input(provider: &str, text: &str) -> bool { provider == "codex" && text.split_whitespace().next() == Some("/compact") }

/// Corpo do `InputBody` do FastAPI: só `text` (texto) e `steer` (booleano); o resto é 422 dele.
fn parse_body(bytes: &Bytes) -> Option<(String, bool)> {
    let object = serde_json::from_slice::<Value>(bytes).ok()?;
    let object = object.as_object()?;
    if object.keys().any(|k| k != "text" && k != "steer") { return None; }
    let steer = match object.get("steer") { None => false, Some(Value::Bool(b)) => *b, Some(_) => return None };
    Some((object.get("text")?.as_str()?.to_owned(), steer))
}

/// O FastAPI só lê o corpo como JSON sem `Content-Type` ou com `application/json`; outro tipo é 422 dele.
pub(crate) fn json_content_type(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::CONTENT_TYPE) else { return true };
    let Ok(text) = value.to_str() else { return false };
    let mime = text.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    mime == "application/json" || (mime.starts_with("application/") && mime.ends_with("+json"))
}

fn is_clear(text: &str) -> bool { text.split_whitespace().next() == Some("/clear") }

fn first_token_is_clear(bytes: &Bytes) -> bool {
    serde_json::from_slice::<Value>(bytes).ok().is_some_and(|v| v["text"].as_str().is_some_and(is_clear))
}

pub(super) fn answer(ctx: &Ctx, (status, body): (StatusCode, Value)) -> Response {
    let mut response = json_response(status, body);
    cors(ctx.headers(), response.headers_mut());
    response
}

fn report(st: &AppState, name: &str, diary: Diary) {
    let Some((event, code)) = diary else { return };
    let reason = match event {
        "runtime.send_failed" => "o runtime não entregou o recado",
        "runtime.send_uncertain" => "entrega sem prova; o recado fica na fila até o transcript confirmar",
        _ => "o terminal não aceitou o comando de barra agora",
    };
    st.diag.report(event, name, &code, reason);
}

/// O miolo do `/input` depois da admissão: envia ao ator, orienta se pedido e responde como o Python.
async fn send(st: &AppState, name: &str, target: &WriteTarget, text: &str, steer: bool) -> (StatusCode, Value) {
    let operation_id = random_hex(16);
    let params = Params { text, steer, terminal: target.terminal, operation_id: &operation_id };
    let sent = target.handle.command(RuntimeCommand { operation_id: operation_id.clone(), kind: OperationKind::Input,
        payload: json!({"text": text, "pre_transcript": false}) }).await;
    let (sent, diary) = classify(&params, &sent);
    report(st, name, diary);
    let mut steered = false;
    if wants_steer_queue(&params, &sent) {
        let promoted = target.handle.command(RuntimeCommand { operation_id: random_hex(16), kind: OperationKind::SteerQueue,
            payload: json!({"entry_id": operation_id}) }).await;
        // O recado já está na fila: falha de orientação nunca desfaz nem repete o envio.
        if let Err(error) = &promoted { tracing::warn!(session = %name, code = %error.code, "orientação do recado falhou; ele segue na fila"); }
        steered = was_steered(&operation_id, &promoted);
    }
    input_answer(&sent, steered)
}

pub async fn input(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    // O /clear com terminal esvazia a fila sob `freeze` no Python: segue a ele antes da porta.
    let terminal = match (st.state.runtime.get(), session_name(req.uri().path())) {
        (Some(runtime), Some(name)) => runtime.is_terminal(&name).await,
        _ => false,
    };
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Input, |body| terminal && first_token_is_clear(body)).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some((text, steer)) = parse_body(&bytes).filter(|_| json_content_type(ctx.headers())) else { return relay(ctx, bytes).await };
    if relays_codex_input(&ctx.target.provider, &text) { return relay(ctx, bytes).await; }
    let result = send(&ctx.st, &ctx.name, &ctx.target, &text, steer).await;
    answer(&ctx, result)
}

/// Recado de fora da rota (aviso de grupo): o mesmo `/input` com `steer: false` que o `_enviar` do
/// Python faz. `Err` = o `detail` da resposta, o envelope `{code, params, msg}`.
pub async fn deliver_text(st: &AppState, name: &str, text: &str) -> Result<(), Value> {
    let body = json!({"text": text, "steer": false}).to_string();
    let route = match st.state.runtime.get() {
        // Mesma ordem do handler: o /clear com terminal segue ao Python antes da porta.
        Some(runtime) if !(runtime.is_terminal(name).await && is_clear(text)) =>
            route_write(runtime, name, WriteRoute::Input, st.write_gate_wait).await,
        _ => Write::Python,
    };
    let (status, reply) = match route {
        // O /compact do Codex o handler também repassa ao Python, com o passe já solto.
        Write::Rust(held, target) if relays_codex_input(&target.provider, text) => {
            drop(held);
            relay_as_owner(st, name, body).await
        }
        Write::Rust(held, target) => {
            let result = send(st, name, &target, text, false).await;
            drop(held);
            result
        }
        Write::Busy => (StatusCode::CONFLICT, busy_body()),
        Write::Python => relay_as_owner(st, name, body).await,
    };
    if status.is_success() { return Ok(()); }
    Err(match reply.get("detail") {
        Some(detail) if !detail.is_null() => detail.clone(),
        // O mesmo genérico do `_deliver` do Python para resposta sem o envelope.
        _ => json!({"code": "erro_envio_falhou_desconhecida", "params": {}, "msg": "falha desconhecida no envio"}),
    })
}

/// O `/input` do Python, pelo mesmo repasse do handler, como o dono local.
async fn relay_as_owner(st: &AppState, name: &str, body: String) -> (StatusCode, Value) {
    // Os não reservados da URL ficam crus: o caminho repassado é o mesmo que o app mandaria.
    const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');
    let path = format!("/api/sessions/{}/input", utf8_percent_encode(name, SEGMENT));
    let Ok(req) = Request::post(path).header(header::AUTHORIZATION, format!("Bearer {}", st.cfg.auth_token))
        .header(header::CONTENT_TYPE, "application/json").body(Body::from(body)) else { return (StatusCode::BAD_REQUEST, Value::Null) };
    let fwd = Forward { client_ip: "127.0.0.1".into(), https: false };
    let response = pass(st, req, &fwd).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), RELAY_REPLY_LIMIT).await.unwrap_or_default();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

pub async fn steer(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Steer, |_| false).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    // Sem corpo = promover a fila; com corpo, o `InputBody` estrito do FastAPI.
    let body = if bytes.is_empty() { Some(None) } else { parse_body(&bytes).map(|(text, _)| Some(text)) };
    let Some(text) = body.filter(|_| bytes.is_empty() || json_content_type(ctx.headers())) else { return relay(ctx, bytes).await };
    let operation_id = random_hex(16);
    let result = match &ctx.target.handle {
        crate::runtime::gateway::EntryHandle::Terminal { handle, .. } => {
            // Só o Kimi promove com texto; no Claude com terminal o corpo é ignorado e promove a fila.
            let control = handle.control(operation_id, "steer".into(), json!({})).await;
            match steer_terminal_control(&control) {
                Err(refused) => refused,
                Ok(promoted) => steer_terminal_done(promoted, &handle.confirm().await),
            }
        }
        headless => {
            let (kind, payload) = match &text {
                Some(text) => (OperationKind::Steer, json!({"text": text, "turn_id": null})),
                None => (OperationKind::SteerQueue, json!({"entry_id": null})),
            };
            let sent = headless.command(RuntimeCommand { operation_id, kind, payload }).await;
            let codex = ctx.target.provider == "codex";
            let result = steer_headless(text.is_some(), codex, &sent);
            if codex && result.0 != StatusCode::OK { super::codex::log_outcome(&ctx.name, "steer", &sent, "orientação do Codex não aceita; a rota responde 409 erro_codex_controle"); }
            result
        }
    };
    answer(&ctx, result)
}
