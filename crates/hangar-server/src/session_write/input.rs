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

use super::{Ctx, Write, WriteRoute, admit, busy_body, detail_body, invalid_body, json_response, route_write};
use crate::mods::state::random_hex;
use crate::proxy::Forward;
use crate::routes::{AppState, cors, pass};
use crate::runtime::gateway::WriteTarget;
use crate::runtime::protocol::{Disposition, OperationKind, RuntimeCommand, RuntimeError, RuntimeReply};

const MSG_UNKNOWN: &str = "resultado incerto; entrada conservada sem reenvio";
const MSG_REJECTED: &str = "entrada recusada pelo runtime";
const MSG_CLEAR_WAIT: &str = "Espere o /clear terminar e envie de novo.";
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
    /// No Codex a barra também tem linha na fila (o ator anexa toda entrada), como no `_send_one_codex_locked`.
    pub codex: bool,
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
        // Texto durante a trava do `/clear`: fora da fila (a troca de conversa a esvaziaria calada), o app o guarda.
        Err(error) if error.code == "runtime_clear_barrier" => {
            let refused = Sent::Refused { code: "erro_aguarde_clear", msg: MSG_CLEAR_WAIT.into(), params: json!({}) };
            return (refused, Some(("runtime.send_failed", diary_code(&error.code))));
        }
        Err(error) => return failed(error.code.clone(), error.to_string()),
    };
    let queued = queued(p.text) || p.codex;
    let entry = p.steer && queued;
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
            // Mensagens esperando no Hangar: o comando não passa à frente delas (nem as apaga num `/clear`).
            let msg = if motivo == "queue_waiting" { format!("{comando} não foi executado: há mensagens na fila. Oriente ou apague antes de mandar o comando.") }
                else { format!("{comando} não foi executado: o terminal não aceitou agora ({motivo}). Mande de novo.") };
            let refused = Sent::Refused { code: "erro_comando_nao_executado", msg, params: json!({"comando": comando, "motivo": motivo}) };
            (refused, Some(("runtime.command_deferred", diary_code(motivo))))
        }
        disposition => (Sent::Done { delivered: disposition == Disposition::Accepted, native: reply.payload["native"] == true, entry }, None),
    }
}

/// A orientação afeta só a entrada criada por este pedido, em qualquer modo do Claude e Codex.
pub fn wants_steer_queue(p: &Params, sent: &Sent) -> bool {
    p.steer && matches!(sent, Sent::Done { entry: true, delivered: false, .. })
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

/// `/steer` sem corpo que orientou uma entrada do Hangar (`steer_first` com `entry_id`): a resposta é a do
/// Orientar, mais os campos que o chip de app antigo lê. Nunca `promoted: true`: o app antigo tiraria todas as
/// bolhas da fila. `confirmed` conta a que saiu agora (orientada ou entregue com o Claude parado); `None` é o
/// caminho de antes, sem entrada esperando.
pub fn steer_first_answer(control: &Result<RuntimeReply, RuntimeError>) -> Option<(StatusCode, Value)> {
    let entry = control.as_ref().ok()?.payload.get("entry_id")?.clone();
    let (status, mut body) = super::control::queue_steer_terminal(control);
    if status == StatusCode::OK {
        let payload = &control.as_ref().ok()?.payload;
        let sent = payload["steered"] == true || payload["drained"].as_u64().is_some_and(|n| n > 0);
        body["promoted"] = json!(false);
        body["confirmed"] = json!(u64::from(sent));
        body["queued_ids"] = json!([entry]);
    }
    Some((status, body))
}

/// 2ª metade: a contagem de entregas que o `confirm` baixou da fila.
pub fn steer_terminal_done(promoted: bool, confirm: &Result<Value, RuntimeError>) -> (StatusCode, Value) {
    match confirm {
        Ok(done) => (StatusCode::OK, json!({"ok": true, "promoted": promoted, "confirmed": done["confirmed"].as_u64().unwrap_or(0)})),
        Err(error) => runtime_failed(error),
    }
}

/// O `/compact` do Codex é controle do motor (`thread/compact/start`), não texto, com e sem terminal.
pub fn is_codex_compact(provider: &str, text: &str) -> bool { provider == "codex" && text.split_whitespace().next() == Some("/compact") }

pub fn is_codex_clear(provider: &str, text: &str) -> bool { provider == "codex" && text.split_whitespace().next() == Some("/clear") }

/// Emoji cortado no ditado ou na colagem chega como `\ud83d` solto, que o serde recusa: vira U+FFFD e a
/// mensagem sai, como no Python. Par completo e `\\u` literal ficam como estão.
pub(crate) fn scrub_lone_surrogates(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let escape = |at: usize| -> Option<u16> {
        let hex = bytes.get(at..at + 6)?;
        if hex[0] != b'\\' || hex[1] != b'u' { return None; }
        u16::from_str_radix(std::str::from_utf8(&hex[2..]).ok()?, 16).ok()
    };
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' { if let Some(out) = out.as_mut() { out.push(bytes[i]); } i += 1; continue; }
        let step = match escape(i) {
            Some(0xD800..=0xDBFF) if escape(i + 6).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) => 12,
            Some(0xD800..=0xDFFF) => {
                let out = out.get_or_insert_with(|| bytes[..i].to_vec());
                out.extend_from_slice(&[b'\\', b'u', b'f', b'f', b'f', b'd']);
                i += 6; continue;
            }
            Some(_) => 6,
            None => 2.min(bytes.len() - i),
        };
        if let Some(out) = out.as_mut() { out.extend_from_slice(&bytes[i..i + step]); }
        i += step;
    }
    out.map_or(std::borrow::Cow::Borrowed(bytes), std::borrow::Cow::Owned)
}

/// `jev_task` e `jev_message` pedem a decisão automática; não se combinam com orientação explícita.
fn parse_input(bytes: &Bytes) -> Option<(String, bool, Option<(String, String)>)> {
    let object = serde_json::from_slice::<Value>(&scrub_lone_surrogates(bytes)).ok()?;
    let object = object.as_object()?;
    if object.keys().any(|k| !matches!(k.as_str(), "text" | "steer" | "jev_task" | "jev_message")) { return None; }
    let steer = match object.get("steer") { None => false, Some(Value::Bool(b)) => *b, Some(_) => return None };
    let task = match object.get("jev_task") {
        None if !object.contains_key("jev_message") => None,
        Some(Value::String(task)) if task.chars().count() <= 1000 && !steer => {
            let message = object.get("jev_message")?.as_str()?;
            if message.chars().count() > 2000 { return None; }
            Some((task.clone(), message.to_owned()))
        }
        _ => return None,
    };
    Some((object.get("text")?.as_str()?.to_owned(), steer, task))
}

fn parse_body(bytes: &Bytes) -> Option<(String, bool)> {
    let (text, steer, task) = parse_input(bytes)?;
    task.is_none().then_some((text, steer))
}

/// O FastAPI só lê o corpo como JSON sem `Content-Type` ou com `application/json`; outro tipo é 422 dele.
pub(crate) fn json_content_type(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::CONTENT_TYPE) else { return true };
    let Ok(text) = value.to_str() else { return false };
    let mime = text.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    mime == "application/json" || (mime.starts_with("application/") && mime.ends_with("+json"))
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
async fn send(st: &AppState, name: &str, target: &WriteTarget, text: &str, steer: bool, expected_turn: Option<Value>) -> (StatusCode, Value) {
    let operation_id = random_hex(16);
    let params = Params { text, steer, terminal: target.terminal, codex: target.provider == "codex", operation_id: &operation_id };
    let sent = target.handle.command(RuntimeCommand { operation_id: operation_id.clone(), kind: OperationKind::Input,
        payload: json!({"text": text, "pre_transcript": false}) }).await;
    let (sent, diary) = classify(&params, &sent);
    report(st, name, diary);
    let mut steered = false;
    if wants_steer_queue(&params, &sent) {
        let mut payload = json!({"entry_id": operation_id});
        if let Some(expected) = expected_turn { payload["expected_turn"] = expected; }
        if let crate::runtime::gateway::EntryHandle::Terminal { handle, .. } = &target.handle {
            let result = handle.control(random_hex(16), "steer_entry".into(), payload).await;
            let (status, reply) = super::control::queue_steer_terminal(&result);
            return input_answer(&sent, status.is_success() && reply["steered"] == true);
        }
        let promoted = target.handle.command(RuntimeCommand { operation_id: random_hex(16), kind: OperationKind::SteerQueue,
            payload }).await;
        // O recado já está na fila: falha de orientação nunca desfaz nem repete o envio.
        if let Err(error) = &promoted { tracing::warn!(session = %name, code = %error.code, "orientação do recado falhou; ele segue na fila"); }
        steered = was_steered(&operation_id, &promoted);
    }
    input_answer(&sent, steered)
}

/// No Claude, o ator terminal conduz o `/clear`; no Codex, ele é controle de conversa nova.
pub async fn input(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Input).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some((text, steer, task)) = parse_input(&bytes).filter(|_| json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    if is_codex_clear(&ctx.target.provider, &text) {
        return answer(&ctx, super::codex::clear(&ctx.target, &text, Some(ctx.st.term.clone())).await);
    }
    if is_codex_compact(&ctx.target.provider, &text) {
        let result = super::codex::compact(&ctx.target, &text).await;
        return answer(&ctx, result);
    }
    let (steer, note, expected_turn) = if let Some((task, message)) = task.filter(|_| !text.trim_start().starts_with('/')) {
        let before = ctx.target.handle.snapshot().await.ok();
        let (mut direct, mut note) = super::delivery_timing::decide(&ctx.st, &message, &task).await;
        if direct {
            let after = ctx.target.handle.snapshot().await.ok();
            if !super::delivery_timing::same_work(before.as_ref(), after.as_ref()) { direct = false; note = "work_changed"; }
        }
        let expected = direct.then(|| before.as_ref().and_then(|snapshot| crate::runtime::protocol::turn_marker(&snapshot["view"]))).flatten();
        (direct, Some(note), expected)
    } else { (steer, None, None) };
    let mut result = send(&ctx.st, &ctx.name, &ctx.target, &text, steer, expected_turn).await;
    if let Some(note) = note {
        let outcome = if steer && result.1["delivered"] != true { "refused" }
            else if !steer && result.1["delivered"] == true { "delivered" } else { note };
        tracing::info!(target: "delivery_timing", session=%ctx.name, outcome, "decisão de entrega aplicada");
        result.1["timing_note"] = json!(outcome);
    }
    answer(&ctx, result)
}

/// Texto para o Codex com terminal que ainda abre a conversa: fica na fila da chave dela e a resposta é a
/// do envio enfileirado, na hora. Sem turno, orientar vira só fila; `/clear` e `/compact` esperam a conversa.
async fn queue_opening(st: &AppState, name: &str, key: &str, text: &str, steer: bool) -> (StatusCode, Value) {
    if is_codex_clear("codex", text) || is_codex_compact("codex", text) {
        let error = crate::runtime::codex_relaunch::starting();
        return (StatusCode::CONFLICT, detail_body(&error.code, &error.message, json!({})));
    }
    let Some(runtime) = st.state.runtime.get() else { return (StatusCode::SERVICE_UNAVAILABLE, detail_body("runtime_watcher", "o servidor ainda não abriu as sessões", json!({}))) };
    match runtime.queue_codex_opening(name, key, text).await {
        Ok(()) => (StatusCode::OK, json!({"ok": true, "delivered": false, "steered": false, "native": false})),
        // A fila já tem dono (adotada nesse meio-tempo, ou o ator segura a trava): o texto vai pela entrada
        // dele, uma vez só; vida antiga gravada na fila sem ator nenhum fica no erro.
        Err(error) if error.code == crate::runtime::codex_terminal::ADOPTED_MEANWHILE || error.code == "runtime_lease" => {
            match route_write(runtime, name, WriteRoute::Input, st.write_gate_wait).await {
                Write::Rust(held, target) => {
                    let result = send(st, name, &target, text, steer, None).await;
                    drop(held);
                    result
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, detail_body(&error.code, &error.message, json!({}))),
            }
        }
        Err(error) => (super::failed_status(&error), detail_body(&error.code, &error.message, json!({}))),
    }
}

/// `/input` do dono (ou pela ponte) para o Codex com terminal que ainda abre a conversa.
pub(super) async fn opening_response(st: &Arc<AppState>, name: &str, key: &str, headers: &HeaderMap, bytes: &Bytes) -> Response {
    let Some((text, steer, _)) = parse_input(bytes).filter(|_| json_content_type(headers)) else { return invalid_body(headers) };
    let (status, body) = queue_opening(st, name, key, &text, steer).await;
    let mut response = json_response(status, body);
    cors(headers, response.headers_mut());
    response
}

/// Recado de fora da rota (aviso de grupo): o mesmo `/input` com `steer: false` que o `_enviar` do
/// Python faz. `Err` = o `detail` da resposta, o envelope `{code, params, msg}`.
pub async fn deliver_text(st: &AppState, name: &str, text: &str) -> Result<(), Value> {
    let (status, reply) = deliver_reply(st, name, text).await;
    if status.is_success() { return Ok(()); }
    Err(match reply.get("detail") {
        Some(detail) if !detail.is_null() => detail.clone(),
        // O mesmo genérico do `_deliver` do Python para resposta sem o envelope.
        _ => json!({"code": "erro_envio_falhou_desconhecida", "params": {}, "msg": "falha desconhecida no envio"}),
    })
}

/// O mesmo recado, com a resposta inteira do `/input` (o broadcast a devolve por sessão).
pub async fn deliver_reply(st: &AppState, name: &str, text: &str) -> (StatusCode, Value) {
    write_input(st, name, text, false).await
}

/// O `/input` do dono feito por dentro, com a resposta inteira (o recado do par externo devolve
/// o que a entrega respondeu).
pub async fn write_input(st: &AppState, name: &str, text: &str, steer: bool) -> (StatusCode, Value) {
    let body = json!({"text": text, "steer": steer}).to_string();
    let route = match st.state.runtime.get() {
        Some(runtime) => route_write(runtime, name, WriteRoute::Input, st.write_gate_wait).await,
        None => Write::Python,
    };
    match route {
        Write::Rust(held, target) if is_codex_clear(&target.provider, text) => {
            let result = super::codex::clear(&target, text, Some(st.term.clone())).await;
            drop(held);
            result
        }
        // O /compact do Codex é do motor, como no handler.
        Write::Rust(held, target) if is_codex_compact(&target.provider, text) => {
            let result = super::codex::compact(&target, text).await;
            drop(held);
            result
        }
        Write::Rust(held, target) => {
            let result = send(st, name, &target, text, steer, None).await;
            drop(held);
            result
        }
        Write::Busy => (StatusCode::CONFLICT, busy_body()),
        Write::Opening(key) => queue_opening(st, name, &key, text, steer).await,
        // Pela ponte do Python: ele mesmo entrega, depois de a ponte responder (sem ida e volta).
        Write::Python => match crate::groups::bridge::current() {
            Some(ctx) => { ctx.defer_delivery(name, text, steer); (StatusCode::OK, json!({"ok": true, "deferred": true})) }
            None => relay_as_owner(st, name, body).await,
        },
        Write::Failed(error) => (super::failed_status(&error), detail_body(&error.code, &error.message, json!({}))),
    }
}

/// Corpo do `BroadcastBody` do FastAPI (estrito): `names` não vazio de textos e `text`.
fn parse_broadcast(bytes: &Bytes) -> Option<(Vec<String>, String)> {
    let object = serde_json::from_slice::<Value>(bytes).ok()?;
    let object = object.as_object()?;
    if object.keys().any(|k| k != "names" && k != "text") { return None; }
    let names: Vec<String> = object.get("names")?.as_array()?.iter().map(|n| n.as_str().map(str::to_owned)).collect::<Option<_>>()?;
    let text = object.get("text")?.as_str()?.to_owned();
    (!names.is_empty()).then_some((names, text))
}

/// `POST /api/broadcast` do dono: o alvo com entrada no Rust (Claude ou Codex) ou Codex com terminal que dá
/// para adotar recebe aqui, pela entrega do `/input`, e nunca passa pelo Python; os demais seguem ao Python
/// num pedido só com os nomes deles, e os resultados se juntam na ordem pedida. Sem alvo do Rust, o pedido
/// vai ao Python como chegou.
pub async fn broadcast(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    let (Some(runtime), true) = (st.state.runtime.get().cloned(), owner) else { return pass(&st, req, &fwd).await };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, super::BODY_LIMIT).await else { return super::too_large(&parts.headers) };
    // Barra e corpo torto: a recusa é a do Python, com o corpo intacto.
    let parsed = parse_broadcast(&bytes).filter(|(_, text)| json_content_type(&parts.headers) && !text.trim_start().starts_with('/'));
    let Some((names, text)) = parsed else { return super::forward_whole(&st, parts, bytes, &fwd).await };
    let mut ours = std::collections::BTreeSet::new();
    // Codex com terminal do Rust que não deu para adotar: o código é daqui e o nome nunca vai ao Python.
    let mut refused = serde_json::Map::new();
    for name in &names {
        if runtime.has_entry(None, name).await { ours.insert(name.clone()); continue; }
        match runtime.adopt_now(name).await {
            Some(Ok(())) => { ours.insert(name.clone()); }
            // Conversa ainda abrindo: a entrega do `/input` guarda o texto na fila dela.
            Some(Err(error)) if error.code == crate::runtime::codex_relaunch::STARTING => { ours.insert(name.clone()); }
            Some(Err(error)) => {
                let detail = detail_body(&error.code, &error.message, json!({}))["detail"].clone();
                refused.insert(name.clone(), json!({"ok": false, "error": detail, "delivered": false}));
            }
            None => {}
        }
    }
    if ours.is_empty() && refused.is_empty() { return super::forward_whole(&st, parts, bytes, &fwd).await; }
    let rest: Vec<&String> = names.iter().filter(|n| !ours.contains(*n) && !refused.contains_key(*n)).collect();
    let mut python = serde_json::Map::new();
    if !rest.is_empty() {
        let body = json!({"names": rest, "text": text}).to_string();
        let Ok(forwarded) = Request::post("/api/broadcast").header(header::AUTHORIZATION, format!("Bearer {}", st.cfg.auth_token))
            .header(header::CONTENT_TYPE, "application/json").body(Body::from(body)) else { return super::forward_whole(&st, parts, bytes, &fwd).await };
        let response = pass(&st, forwarded, &Forward { client_ip: "127.0.0.1".into(), https: false, guest: None }).await;
        if !response.status().is_success() { return response; }
        let reply = to_bytes(response.into_body(), RELAY_REPLY_LIMIT).await.unwrap_or_default();
        if let Some(results) = serde_json::from_slice::<Value>(&reply).ok().and_then(|v| v["results"].as_object().cloned()) { python = results; }
    }
    // Resposta sem corpo legível nunca vira `error: null`: o envio pode ter falhado sem dizer por quê.
    let unreadable = || json!({"ok": false, "delivered": false, "error": detail_body("broadcast_reply_unreadable",
        "A resposta do envio veio ilegível; confira a sessão antes de reenviar.", json!({}))["detail"].clone()});
    let mut results = serde_json::Map::new();
    for name in &names {
        let result = if let Some(refusal) = refused.get(name) { refusal.clone() } else if ours.contains(name) {
            match deliver_reply(&st, name, &text).await {
                (status, reply) if status.is_success() => reply,
                (_, reply) => match reply.get("detail").filter(|detail| !detail.is_null()) {
                    Some(detail) => json!({"ok": false, "error": detail, "delivered": false}),
                    None => unreadable(),
                },
            }
        } else {
            python.remove(name).unwrap_or_else(unreadable)
        };
        results.insert(name.clone(), result);
    }
    let mut response = json_response(StatusCode::OK, json!({"results": results}));
    crate::routes::cors(&parts.headers, response.headers_mut());
    response
}

/// O `/input` do Python, pelo mesmo repasse do handler, como o dono local.
async fn relay_as_owner(st: &AppState, name: &str, body: String) -> (StatusCode, Value) {
    // Os não reservados da URL ficam crus: o caminho repassado é o mesmo que o app mandaria.
    const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');
    let path = format!("/api/sessions/{}/input", utf8_percent_encode(name, SEGMENT));
    let Ok(req) = Request::post(path).header(header::AUTHORIZATION, format!("Bearer {}", st.cfg.auth_token))
        .header(header::CONTENT_TYPE, "application/json").body(Body::from(body)) else { return (StatusCode::BAD_REQUEST, Value::Null) };
    let fwd = Forward { client_ip: "127.0.0.1".into(), https: false, guest: None };
    let response = pass(st, req, &fwd).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), RELAY_REPLY_LIMIT).await.unwrap_or_default();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

pub async fn steer(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Steer).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    // Sem corpo = promover a fila; com corpo, o `InputBody` estrito do FastAPI.
    let body = if bytes.is_empty() { Some(None) } else { parse_body(&bytes).map(|(text, _)| Some(text)) };
    let Some(text) = body.filter(|_| bytes.is_empty() || json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let operation_id = random_hex(16);
    let result = match &ctx.target.handle {
        crate::runtime::gateway::EntryHandle::Terminal { handle, .. } => {
            // Só o Kimi promove com texto; no Claude com terminal o corpo é ignorado. Com mensagem esperando no
            // Hangar, ela é orientada (a primeira); sem nenhuma, promove a fila do Claude Code.
            let control = handle.control(operation_id, "steer_first".into(), json!({})).await;
            match steer_first_answer(&control) {
                Some(answered) => answered,
                None => match steer_terminal_control(&control) {
                    Err(refused) => refused,
                    Ok(promoted) => steer_terminal_done(promoted, &handle.confirm().await),
                },
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

#[cfg(test)]
mod broadcast_tests {
    use super::*;

    #[test]
    fn automatic_delivery_requires_bounded_context_and_cannot_override_explicit_steering() {
        let b = |s: &str| Bytes::copy_from_slice(s.as_bytes());
        assert_eq!(parse_input(&b(r#"{"text":"oi","jev_task":"Criar a tela","jev_message":"Use azul"}"#)),
            Some(("oi".into(), false, Some(("Criar a tela".into(), "Use azul".into())))));
        for refused in [r#"{"text":"oi","jev_task":"Criar a tela"}"#,
            r#"{"text":"oi","jev_message":"Use azul"}"#,
            r#"{"text":"oi","steer":true,"jev_task":"t","jev_message":"m"}"#,
            r#"{"text":"oi","jev_task":true,"jev_message":"m"}"#] {
            assert!(parse_input(&b(refused)).is_none());
        }
        assert!(parse_input(&b(&json!({"text":"oi","jev_task":"t","jev_message":"á".repeat(2001)}).to_string())).is_none());
        assert!(parse_input(&b(&json!({"text":"oi","jev_task":"á".repeat(1001),"jev_message":"m"}).to_string())).is_none());
        assert!(parse_body(&b(r#"{"text":"oi","jev_task":"t","jev_message":"m"}"#)).is_none(), "Orientar explícito não aceita contexto automático");
    }

    #[test]
    fn broadcast_body_is_as_strict_as_the_fastapi_model() {
        let b = |s: &'static str| Bytes::from_static(s.as_bytes());
        assert_eq!(parse_broadcast(&b(r#"{"names":["a","b"],"text":"oi"}"#)), Some((vec!["a".into(), "b".into()], "oi".into())));
        for refused in [r#"{"names":[],"text":"oi"}"#, r#"{"names":["a"],"text":"oi","x":1}"#, r#"{"names":["a",1],"text":"oi"}"#,
            r#"{"names":"a","text":"oi"}"#, r#"{"names":["a"]}"#, "[]", "oi"] {
            assert_eq!(parse_broadcast(&b(refused)), None, "{refused}");
        }
    }

    #[test]
    fn cut_emoji_is_replaced_and_the_text_still_goes() {
        let b = |s: &'static str| Bytes::from_static(s.as_bytes());
        assert_eq!(parse_body(&b(r#"{"text":"oi \ud83d"}"#)), Some(("oi \u{FFFD}".into(), false)));
        // Par completo escapado (alto + baixo) continua sendo o emoji.
        let pair = format!("{{\"text\":\"{}udc00 e {}ud83d{}ude00 e {}{}ud83d\"}}", '\\', '\\', '\\', '\\', '\\');
        assert_eq!(parse_body(&Bytes::from(pair)), Some(("\u{FFFD} e \u{1F600} e \\ud83d".into(), false)));
        assert!(crate::session_write::table::body_ok(crate::session_write::table::WriteRoute::Steer, &b(r#"{"text":"\ud83d"}"#)));
    }
}
