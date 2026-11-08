//! Resposta ao retrato identificado do AskUserQuestion. "Conversar" é um único controle serial
//! do dono: conferir a pergunta, fechar e entregar o texto, com um recibo só.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::control::{expected, interaction_failure, interaction_result, panel_open_refusal};
use crate::interactions::{ExpectedInteraction, InteractionError, InteractionKind};
use super::input::{MSG_CONTROL_DEFERRED, MSG_CONTROL_UNCONFIRMED, answer as respond, json_content_type, runtime_failed};
use super::{Ctx, WriteRoute, admit, detail_body, invalid_body};
use crate::mods::state::random_hex;
use crate::routes::AppState;
use crate::runtime::gateway::EntryHandle;
use crate::runtime::protocol::{Disposition, OperationKind, RuntimeCommand, RuntimeError, RuntimeReply};
use crate::runtime::terminal::TerminalHandle;

type Answer = (StatusCode, Value);

const MSG_CHAT_UNCONFIRMED: &str = "a pergunta foi fechada, mas a resposta por texto não foi confirmada — confira na sessão antes de responder de novo";
const MSG_HEADLESS_INVALID: &str = "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.";
const MSG_HEADLESS_SEND: &str = "Não foi possível enviar a resposta.";
const MSG_FALLBACK_NOT_DELIVERED: &str = "não consegui responder por aqui — a pergunta segue aberta, responda no terminal";
/// Sem terminal não há texto digitado: a resposta vai pelo canal da CLI, que não confirmou.
const MSG_HEADLESS_UNCONFIRMED: &str = "A CLI não confirmou a resposta; confira a conversa antes de responder de novo.";
const MSG_CODEX_SEND: &str = "Não foi possível confirmar o envio da resposta ao Codex.";
/// Código com que o ator sem terminal recusa uma resposta inválida (`claude.rs::error`).
const ACTOR_REFUSAL: &str = "claude_command";

const FIELDS: [&str; 8] = ["kind", "question_id", "indices", "multi", "value", "labels", "type_index", "chat_index"];

/// Um item de `answers`, só nos tipos que o FastAPI aceita sem converter nada.
#[derive(Clone, Debug)]
pub struct Item {
    kind: String,
    question_id: Option<String>,
    indices: Option<Vec<i64>>,
    multi: bool,
    value: Option<String>,
    labels: Vec<String>,
    type_index: Option<i64>,
    chat_index: Option<i64>,
}

impl Item {
    /// O formato que o ator lê; `indices` ausente vira lista vazia, como no Python.
    fn to_json(&self) -> Value {
        json!({"kind": self.kind, "question_id": self.question_id, "indices": self.indices.clone().unwrap_or_default(),
            "multi": self.multi, "value": self.value, "labels": self.labels, "type_index": self.type_index, "chat_index": self.chat_index})
    }
}

pub struct Body {
    answers: Vec<Item>,
    /// `null`, texto ou inteiro: o `int | str | None` do `AnswerBody`.
    request_id: Value,
    interaction_id: Option<String>,
    steer: bool,
}

impl Body {
    pub fn answers_json(&self) -> Vec<Value> { self.answers.iter().map(Item::to_json).collect() }
}

fn optional<T>(value: Option<&Value>, read: impl Fn(&Value) -> Option<T>) -> Option<Option<T>> {
    match value { None | Some(Value::Null) => Some(None), Some(v) => read(v).map(Some) }
}

fn parse_item(value: &Value) -> Option<Item> {
    let object = value.as_object()?;
    if object.keys().any(|k| !FIELDS.contains(&k.as_str())) { return None; }
    let strings = |v: &Value| v.as_array()?.iter().map(|s| s.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>();
    Some(Item {
        kind: object.get("kind")?.as_str()?.to_owned(),
        question_id: optional(object.get("question_id"), |v| v.as_str().map(str::to_owned))?,
        indices: optional(object.get("indices"), |v| v.as_array()?.iter().map(Value::as_i64).collect::<Option<Vec<_>>>())?,
        multi: match object.get("multi") { None => false, Some(Value::Bool(b)) => *b, Some(_) => return None },
        value: optional(object.get("value"), |v| v.as_str().map(str::to_owned))?,
        labels: match object.get("labels") { None => Vec::new(), Some(v) => strings(v)? },
        type_index: optional(object.get("type_index"), Value::as_i64)?,
        chat_index: optional(object.get("chat_index"), Value::as_i64)?,
    })
}

/// `AnswerBody` estrito. Tipo que o pydantic converte (`"1"`, `1.0`, `"true"`) ou recusa (campo a mais)
/// não passa daqui: a rota recusa com `erro_corpo_invalido`, sem converter nem repassar.
pub fn parse_body(bytes: &Bytes) -> Option<Body> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object()?;
    if object.keys().any(|k| !matches!(k.as_str(), "answers" | "request_id" | "interaction_id" | "steer")) { return None; }
    let answers = object.get("answers")?.as_array()?.iter().map(parse_item).collect::<Option<Vec<_>>>()?;
    let request_id = match object.get("request_id") {
        None | Some(Value::Null) => Value::Null,
        Some(v @ Value::String(_)) => v.clone(),
        Some(v) if v.as_i64().is_some() => v.clone(),
        Some(_) => return None,
    };
    let interaction_id = optional(object.get("interaction_id"), |v| v.as_str().map(str::to_owned))?;
    let steer = match object.get("steer") { None => false, Some(value) => value.as_bool()? };
    Some(Body { answers, request_id, interaction_id, steer })
}

fn refused(msg: &str) -> Answer { (StatusCode::CONFLICT, detail_body("erro_sem_resposta", msg, json!({}))) }

fn success() -> Answer { (StatusCode::OK, json!({"ok": true, "fallback": false})) }

/// `repr()` de texto do Python, para a mensagem de tipo desconhecido.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => { out.push('\\'); out.push(c); }
            c if c.is_control() && (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `terminal_input._validate`: tudo checado antes de tocar no pane; a primeira falha é a mensagem.
pub fn validate(items: &[Item]) -> Result<(), String> {
    for a in items {
        match a.kind.as_str() {
            "text" => {
                let Some(value) = &a.value else { return Err("value required for text kind".into()) };
                if value.chars().any(|c| (c as u32) < 32 && c != '\t') { return Err("control characters not allowed".into()); }
                if a.type_index.is_none() { return Err("type_index required for text kind".into()); }
            }
            "chat" if a.chat_index.is_none() => return Err("chat_index required for chat kind".into()),
            "chat" => {}
            "option" => {
                let indices = a.indices.as_deref().unwrap_or_default();
                if indices.is_empty() { return Err("indices required for option kind".into()); }
                if indices.iter().any(|i| *i < 0) { return Err("indices must be >= 0".into()); }
            }
            other => return Err(format!("unknown answer kind: {}", py_repr(other))),
        }
    }
    Ok(())
}

/// `runtime_terminal._validate_text`: texto de terminal não é branco nem tem caractere de controle.
fn text_ok(text: &str) -> bool { !text.trim().is_empty() && !text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')) }

/// `runtime_terminal._validate_control` para `answer_questions`, depois de `validate`.
fn validate_control(items: &[Item], request_id: &Value) -> Result<(), String> {
    if items.is_empty() { return Err("resposta terminal vazia".into()); }
    let in_range = |i: &i64| (0..100).contains(i);
    for a in items {
        match a.kind.as_str() {
            "option" => {
                let indices = a.indices.as_deref().unwrap_or_default();
                if a.labels.is_empty() || !indices.iter().all(in_range) || (!a.multi && indices.len() != 1) { return Err("opção fora da pergunta".into()); }
            }
            "text" | "chat" => {
                let position = if a.kind == "text" { a.type_index } else { a.chat_index };
                if !position.as_ref().is_some_and(in_range) { return Err("posição fora da pergunta".into()); }
                if let Some(value) = a.value.as_deref().filter(|_| a.kind == "text") {
                    if !text_ok(value) { return Err("texto inválido para terminal".into()); }
                    if value.contains('\n') { return Err("resposta terminal com quebra de linha".into()); }
                }
            }
            _ => {}
        }
    }
    if !request_id.is_null() && !request_id.is_string() { return Err("pedido terminal sem identidade válida".into()); }
    Ok(())
}

pub enum Plan {
    /// O controle `answer_questions` com este corpo.
    Control(Value),
    /// "Conversar sobre isso" sem pergunta segurada: as respostas viram texto.
    Chat,
}

pub fn terminal_plan(body: &Body, expected: &ExpectedInteraction, panel_open: bool) -> Result<Plan, Answer> {
    if body.interaction_id.as_deref().is_none_or(str::is_empty) { return Err(interaction_failure(InteractionError::Missing)); }
    if body.interaction_id.as_deref() != Some(expected.interaction_id.as_str()) || expected.target.kind != InteractionKind::Question {
        return Err(interaction_failure(InteractionError::Changed));
    }
    let plugin = expected.target.request_key.starts_with("plugin:");
    if !plugin && panel_open { return Err(panel_open_refusal()); }
    validate(&body.answers).map_err(|msg| refused(&msg))?;
    if !body.request_id.is_null() {
        let same = if let Some(id) = expected.target.request_key.strip_prefix("plugin:") { body.request_id.as_str() == Some(id) }
            else { expected.native_id(Some(&body.request_id)).is_ok() };
        if !same { return Err(interaction_failure(InteractionError::Changed)); }
    }
    let mut payload = json!({"answers": body.answers_json(), "interaction_id": body.interaction_id});
    if !body.request_id.is_null() { payload["request_id"] = body.request_id.clone(); }
    validate_control(&body.answers, &body.request_id).map_err(|msg| refused(&msg))?;
    if !plugin && body.answers.iter().any(|a| a.kind == "chat") { return Ok(Plan::Chat); }
    Ok(Plan::Control(payload))
}

/// Porta de `_askq_conversar_text`: as respostas já dadas viram mensagem, porque a TUI descarta o
/// que foi respondido ao escolher "Chat about this". Sem nada a preservar devolve vazio.
pub fn chat_text(answers: &[Value], questions: &[String]) -> String {
    let question = |i: usize| questions.get(i).map(String::as_str).filter(|q| !q.is_empty());
    let (mut answered, mut chatting) = (Vec::new(), Vec::new());
    for (i, a) in answers.iter().enumerate() {
        if a["kind"] == "chat" {
            if let Some(q) = question(i) { chatting.push(q); }
            continue;
        }
        let response = if a["kind"] == "text" { a["value"].as_str().unwrap_or_default().to_owned() }
            else { a["labels"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default() };
        if response.is_empty() { continue; }
        answered.push(match question(i) { Some(q) => format!("- {q} → {response}"), None => format!("- {response}") });
    }
    if answered.is_empty() { return String::new(); }
    let mut lines = match chatting.len() {
        0 => vec!["Sobre uma das perguntas prefiro conversar antes de responder.".to_owned()],
        1 => vec![format!("Sobre «{}» prefiro conversar antes de responder.", chatting[0])],
        _ => {
            let mut lines = vec!["Sobre estas perguntas prefiro conversar antes de responder:".to_owned()];
            lines.extend(chatting.iter().map(|q| format!("- {q}")));
            lines
        }
    };
    lines.push("Já respondi:".into());
    lines.extend(answered);
    lines.join("\n")
}

/// Porta de `_askq_fallback_text`: as respostas viram "pergunta → resposta" quando as teclas falham.
/// "Conversar" não vira linha (o Esc da reserva já leva ao chat). Sem nada a mandar devolve vazio.
pub fn fallback_text(answers: &[Value], questions: &[String]) -> String {
    let lines: Vec<String> = answers.iter().enumerate().filter_map(|(i, a)| {
        let response = match a["kind"].as_str() {
            Some("option") => a["labels"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
            Some("text") => a["value"].as_str().unwrap_or_default().to_owned(),
            _ => return None,
        };
        if response.is_empty() { return None; }
        Some(match questions.get(i).filter(|q| !q.is_empty()) { Some(q) => format!("- {q} → {response}"), None => format!("- {response}") })
    }).collect();
    if lines.is_empty() { return String::new(); }
    format!("Respondendo as perguntas (o seletor de opções falhou, vai por texto):\n{}", lines.join("\n"))
}

pub fn control_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    if let Some(failed) = interaction_result(sent) { return failed; }
    let reply = match sent {
        Ok(reply) => reply,
        Err(error) => return runtime_failed(error),
    };
    // Reserva por texto (as teclas falharam): só a mensagem entregue diz que respondeu.
    match reply.payload["code"].as_str() {
        Some("answer_fallback_sent") if reply.disposition == Disposition::Accepted => return (StatusCode::OK, json!({"ok": true, "fallback": true})),
        Some("answer_fallback_not_delivered") => return (StatusCode::CONFLICT, detail_body("erro_resposta_nao_entregue", MSG_FALLBACK_NOT_DELIVERED, json!({}))),
        Some("answer_fallback_unproved") => return (StatusCode::CONFLICT, detail_body("erro_drive_fallback_falhou",
            "não deu pra confirmar a resposta por texto depois das teclas", json!({"erro": "answer_fallback_unproved"}))),
        _ => {}
    }
    match reply.disposition {
        Disposition::Accepted => success(),
        Disposition::Deferred => refused(MSG_CONTROL_DEFERRED),
        Disposition::Unknown => (StatusCode::CONFLICT, detail_body("erro_sem_confirmacao_resposta", MSG_CHAT_UNCONFIRMED, json!({}))),
        Disposition::Rejected => refused(MSG_CONTROL_UNCONFIRMED),
    }
}

/// "Conversar" com respostas: o texto que não se confirma é 502 `erro_envio_falhou`, não 409. O app
/// trata 409 como "nada digitado" e reenviar duplicaria a mensagem.
pub fn chat_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    match sent {
        // O texto ficou na fila do Rust, que o entrega quando o composer voltar: aceito, como no Python.
        Ok(reply) if reply.disposition == Disposition::Deferred && reply.payload["queued"] == true =>
            (StatusCode::OK, json!({"ok": true, "fallback": false, "queued": true})),
        Ok(reply) if reply.disposition == Disposition::Unknown
            && matches!(reply.payload["code"].as_str(), Some("erro_sem_confirmacao_resposta" | "answer_chat_close_unproved")) =>
            (StatusCode::BAD_GATEWAY, detail_body("erro_envio_falhou", MSG_CHAT_UNCONFIRMED, json!({}))),
        _ => control_answer(sent),
    }
}

/// O corpo do `answer_questions` sem terminal: o ator confere a pergunta pendente e monta a resposta.
pub fn headless_command(body: &Body) -> Value {
    let mut command = json!({"request_id": body.request_id, "answers": body.answers_json(), "interaction_id": body.interaction_id});
    if body.steer { command["steer"] = json!(true); }
    command
}

/// No Codex a recusa do ator (`codex_command`) é falha de envio, como no Python, com a frase do Codex.
pub fn headless_answer(codex: bool, sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    if let Some(failed) = interaction_result(sent) { return failed; }
    let invalid = || (StatusCode::CONFLICT, detail_body("erro_codex_resposta_invalida", MSG_HEADLESS_INVALID, json!({})));
    let unsent = || (StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_codex_resposta_envio",
        if codex { MSG_CODEX_SEND } else { MSG_HEADLESS_SEND }, json!({})));
    match sent {
        Err(error) if error.code == ACTOR_REFUSAL && !codex => invalid(),
        // Resposta que não serve à pergunta é erro de quem respondeu, não do envio (o `ValueError` do Python).
        Err(error) if codex && error.code == crate::runtime::codex::INVALID_ANSWER => invalid(),
        Err(_) => unsent(),
        Ok(reply) => match reply.disposition {
            Disposition::Accepted => success(),
            Disposition::Rejected | Disposition::Deferred => invalid(),
            Disposition::Unknown => (StatusCode::CONFLICT, detail_body("erro_sem_confirmacao_resposta", if codex { MSG_CHAT_UNCONFIRMED } else { MSG_HEADLESS_UNCONFIRMED }, json!({}))),
        },
    }
}

async fn terminal(ctx: &Ctx, body: &Body, expected: &ExpectedInteraction, handle: &TerminalHandle) -> Answer {
    match terminal_plan(body, expected, ctx.st.term.is_active(&ctx.name)) {
        Err(refusal) => refusal,
        Ok(Plan::Control(payload)) => control_answer(&handle.control(random_hex(16), "answer_questions".into(), payload).await),
        Ok(Plan::Chat) => {
            let mut payload = json!({"answers":body.answers_json(), "interaction_id":body.interaction_id});
            if !body.request_id.is_null() { payload["request_id"] = body.request_id.clone(); }
            chat_answer(&handle.control(random_hex(16), "answer_chat".into(), payload).await)
        }
    }
}

pub async fn answer(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Answer).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some(body) = parse_body(&bytes).filter(|_| json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let expected = match expected(&ctx, body.interaction_id.as_deref()).await { Ok(expected) => expected, Err(failed) => return respond(&ctx, failed) };
    if matches!(ctx.target.handle, EntryHandle::OtherTerminal { .. }) && st.term.is_active(&ctx.name) {
        return respond(&ctx, panel_open_refusal());
    }
    let result = match &ctx.target.handle {
        EntryHandle::Terminal { handle, .. } => terminal(&ctx, &body, &expected, handle).await,
        headless => {
            let sent = headless.command(RuntimeCommand { operation_id: random_hex(16), kind: OperationKind::AnswerQuestions, payload: headless_command(&body) }).await;
            let codex = ctx.target.provider == "codex";
            if codex { super::codex::log_outcome(&ctx.name, "answer", &sent, "resposta da pergunta do Codex não aceita"); }
            headless_answer(codex, &sent)
        }
    };
    respond(&ctx, result)
}
