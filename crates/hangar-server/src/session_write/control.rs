//! `/interrupt`, `/select`, `/select/submit`, `/keys`, `/term-input` e as rotas da fila (`DELETE`/`PATCH
//! …/queue/{id}`, `…/queue/reorder`, `…/queue/{id}/steer`) de sessão Claude e Codex, com a mesma resposta
//! do Python (`api.py`) onde ela existia. Daqui para baixo o Rust é o dono: falha é
//! erro com código, nunca repasse. A pergunta que o plugin segura é do Rust: o ator do terminal a
//! confere e a fecha no próprio Esc.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};

use super::input::{
    Diary, MSG_CODEX_CONTROL, MSG_CONTROL_DEFERRED, MSG_CONTROL_UNCONFIRMED, MSG_STEER_REFUSED, MSG_STEER_UNKNOWN, answer, codex_control,
    control_failed, diary_code, json_content_type, payload_code, runtime_failed, sem_turno,
};
use super::{Ctx, WriteRoute, admit, detail_body, invalid_body};
use crate::mods::state::random_hex;
use crate::interactions::{ExpectedInteraction, InteractionError};
use crate::routes::AppState;
use crate::runtime::gateway::EntryHandle;
use crate::runtime::protocol::{Disposition, OperationKind, RuntimeCommand, RuntimeError, RuntimeReply};
use crate::runtime::queue::{Action, queue_text_ok};

type Answer = (StatusCode, Value);

const MSG_NO_TURN: &str = "Não há turno ativo para interromper.";
const MSG_CODEX_NO_TURN: &str = "Não há turno Codex ativo para interromper.";
const MSG_PERM_OPTION: &str = "opção fora do pedido de permissão";
const MSG_PANEL_OPEN: &str = "Terminal aberto nesta sessao. Feche o painel pra responder por aqui.";
const MSG_NOT_SENT: &str = "não consegui responder pelo terminal — opção NÃO enviada";
pub(super) const MSG_UNCONFIRMED_ANSWER: &str = "resposta enviada, mas nao deu pra confirmar a tempo — confira na sessao antes de responder de novo";
const MSG_NOT_MARKED: &str = "não consegui marcar essa opção no terminal — tente de novo";
const MSG_NOT_SUBMITTED: &str = "não consegui enviar as opções marcadas — tente de novo";
const MSG_PLAN_GONE: &str = "o terminal não mostra mais a aprovação do plano — nada foi enviado";
const MSG_NO_PERMISSION: &str = "nenhum pedido de permissão pendente";
const MSG_NOT_IN_QUEUE: &str = "entrada não está na fila";
const GIVE_UP_COMPOSER_BUSY_CODE: &str = "erro_fila_campo_ocupado";
const MSG_COMPOSER_BUSY: &str = "Há texto no campo do terminal: apague ou envie para a fila andar";
const MSG_COMPOSER_UNREAD: &str = "Não deu para ler o campo do terminal agora; tente de novo";

fn ok() -> Answer { (StatusCode::OK, json!({"ok": true})) }

pub(super) fn interaction_failure(error: InteractionError) -> Answer {
    (error.status(), detail_body(error.code(), error.message(), json!({})))
}

pub(super) fn interaction_result(sent: &Result<RuntimeReply, RuntimeError>) -> Option<Answer> {
    let code = match sent {
        Err(error) => error.code.as_str(),
        Ok(reply) if reply.disposition == Disposition::Rejected => payload_code(reply)?,
        _ => return None,
    };
    InteractionError::from_code(code).map(interaction_failure)
}

pub(super) async fn expected(ctx: &Ctx, supplied: Option<&str>) -> Result<ExpectedInteraction, Answer> {
    if supplied.is_none_or(str::is_empty) { return Err(interaction_failure(InteractionError::Missing)); }
    let runtime = ctx.st.state.runtime.get().ok_or_else(|| interaction_failure(InteractionError::Unavailable))?;
    let binding = runtime.interaction_binding(&ctx.name).await.map_err(|_| interaction_failure(InteractionError::Unavailable))?;
    if binding.key != ctx.target.key || binding.generation != ctx.target.generation { return Err(interaction_failure(InteractionError::Changed)); }
    ctx.st.interactions.expected(&binding, supplied).map_err(interaction_failure)
}

fn failure(code: &str, msg: &str, params: Value) -> Answer { (StatusCode::CONFLICT, detail_body(code, msg, params)) }

/// O painel de terminal anexado trunca o pane: quem conta linha nele escolheria errado.
pub fn panel_open_refusal() -> Answer { failure("erro_terminal_aberto", MSG_PANEL_OPEN, json!({})) }

// ── /select ─────────────────────────────────────────────────────────────────────────────────────

pub fn select_plan(expected: &ExpectedInteraction, option: u64, panel_open: bool) -> Result<Value, Answer> {
    if expected.target.kind == crate::interactions::InteractionKind::Question { return Err(interaction_failure(InteractionError::Changed)); }
    let permission = expected.target.request_key.starts_with("plugin:perm:");
    if permission && !matches!(option, 1 | 2) { return Err(failure("erro_opcao_nao_convergiu", MSG_PERM_OPTION, json!({}))); }
    if !permission && panel_open { return Err(panel_open_refusal()); }
    Ok(json!({"option": option, "interaction_id": expected.interaction_id}))
}

pub fn select_terminal_answer(sent: &Result<RuntimeReply, RuntimeError>) -> (Answer, Diary) {
    if let Some(failed) = interaction_result(sent) { return (failed, None); }
    let reply = match sent {
        Ok(reply) => reply,
        // Antes da entrega: nada chegou ao pane.
        Err(error) => return ((StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_opcao_nao_convergiu", MSG_NOT_SENT, json!({"detalhe": error.to_string()}))), None),
    };
    match reply.disposition {
        Disposition::Accepted => (ok(), None),
        Disposition::Deferred => (control_failed(MSG_CONTROL_DEFERRED), None),
        Disposition::Unknown => (failure("erro_sem_confirmacao_resposta", MSG_UNCONFIRMED_ANSWER, json!({})), None),
        Disposition::Rejected if payload_code(reply) == Some("plan_dialog_missing") => (failure("erro_plano_fora_da_tela", MSG_PLAN_GONE, json!({})), None),
        Disposition::Rejected => {
            let code = payload_code(reply).unwrap_or("rejected");
            (failure("erro_opcao_nao_convergiu", MSG_NOT_MARKED, json!({"detalhe": code})), Some(("opcao.nao_convergiu", diary_code(code))))
        }
    }
}

/// Sem terminal a opção responde ao pedido de permissão em aberto (1 permite, 2 nega).
pub fn select_headless_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    if let Some(failed) = interaction_result(sent) { return failed; }
    let not_answered = |msg: &str| (StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_opcao_nao_convergiu", &format!("não consegui responder: {msg}"), json!({})));
    match sent {
        // O ator recusa a opção sem permissão em aberto com este código (não como resposta).
        Err(error) if error.code == "no_pending_permission" => failure("erro_opcao_nao_convergiu", MSG_NO_PERMISSION, json!({})),
        // Opção que o cartão do Codex não tem: erro de quem respondeu, não falha de envio.
        Err(error) if error.code == crate::runtime::codex::INVALID_ANSWER => failure("erro_codex_resposta_invalida", &error.message, json!({})),
        Err(error) => not_answered(&error.to_string()),
        Ok(reply) => match reply.disposition {
            Disposition::Accepted => ok(),
            Disposition::Unknown => failure("erro_sem_confirmacao_resposta", MSG_UNCONFIRMED_ANSWER, json!({})),
            Disposition::Rejected => not_answered(reply.payload["error"].as_str().filter(|e| !e.is_empty()).unwrap_or(MSG_STEER_REFUSED)),
            Disposition::Deferred => not_answered(MSG_STEER_REFUSED),
        },
    }
}

/// Formas normalizáveis e inválidas seguem intactas ao FastAPI, inclusive o ID recebido.
/// `text` vazio vale como ausente; com texto, só a terceira opção do plano (o retorno a Claude), numa linha.
fn parse_option(bytes: &Bytes) -> Option<(u64, Option<String>, Option<String>)> {
    let object = serde_json::from_slice::<Value>(bytes).ok()?;
    let object = object.as_object()?;
    if object.keys().any(|key| key != "option" && key != "interaction_id" && key != "text") { return None; }
    let field = |name: &str| match object.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        _ => None,
    };
    let (supplied, text) = (field("interaction_id")?, field("text")?.filter(|t| !t.trim().is_empty()));
    let option = object.get("option")?.as_u64().filter(|n| (1..=50).contains(n))?;
    if text.as_deref().is_some_and(|t| option != 3 || t.chars().any(char::is_control)) { return None; }
    Some((option, supplied, text))
}

/// Diálogo do `ExitPlanMode`: o cartão vem do aviso do plugin, a resposta são as teclas do diálogo.
pub fn plan_interaction(expected: &ExpectedInteraction) -> bool { expected.target.request_key.starts_with("plugin:plan:") }

fn parse_submit(bytes: &Bytes) -> Option<Option<String>> {
    let object = if bytes.is_empty() { json!({}) } else { serde_json::from_slice::<Value>(bytes).ok()? };
    let object = object.as_object()?;
    if object.keys().any(|key| key != "interaction_id") { return None; }
    match object.get("interaction_id") {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(id)) => Some(Some(id.clone())),
        _ => None,
    }
}

// ── /select/submit ──────────────────────────────────────────────────────────────────────────────

pub fn submit_answer(sent: &Result<RuntimeReply, RuntimeError>) -> (Answer, Diary) {
    if let Some(failed) = interaction_result(sent) { return (failed, None); }
    let reply = match sent {
        Ok(reply) => reply,
        Err(error) => return (runtime_failed(error), None),
    };
    match reply.disposition {
        Disposition::Accepted => (ok(), None),
        Disposition::Deferred => (control_failed(MSG_CONTROL_DEFERRED), None),
        Disposition::Unknown => (failure("erro_sem_confirmacao_resposta", MSG_UNCONFIRMED_ANSWER, json!({})), None),
        Disposition::Rejected => {
            let code = payload_code(reply).unwrap_or("rejected");
            (failure("erro_opcao_nao_convergiu", MSG_NOT_SUBMITTED, json!({"detalhe": code})), Some(("opcao.envio_falhou", diary_code(code))))
        }
    }
}

// ── /interrupt, /keys, /term-input ──────────────────────────────────────────────────────────────

/// Um controle de terminal que só responde ok ou recusa; a tecla ou o texto que o ator recusa é a
/// validação do Python (`ValueError` → 400 com texto puro).
pub fn control_step_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Result<(), Answer> {
    let reply = sent.as_ref().map_err(runtime_failed)?;
    match reply.disposition {
        Disposition::Accepted => Ok(()),
        Disposition::Deferred => Err(control_failed(MSG_CONTROL_DEFERRED)),
        Disposition::Rejected if payload_code(reply) == Some("key_not_allowed") => Err((StatusCode::BAD_REQUEST, json!({"detail": "tecla não permitida"}))),
        Disposition::Rejected if payload_code(reply) == Some("invalid_text") => Err((StatusCode::BAD_REQUEST, json!({"detail": "texto inválido para terminal"}))),
        _ => Err(control_failed(MSG_CONTROL_UNCONFIRMED)),
    }
}

/// O desfecho do escritor de pane do Codex com terminal, com as mesmas respostas do `control_step_answer`.
pub fn pane_step_answer(result: &crate::terminal_input::DeliveryResult) -> Result<(), Answer> {
    use crate::terminal_input::Disposition as Pane;
    match (result.disposition, result.code.as_str()) {
        (Pane::Accepted, _) => Ok(()),
        (Pane::Rejected, "key_not_allowed") => Err((StatusCode::BAD_REQUEST, json!({"detail": "tecla não permitida"}))),
        (Pane::Rejected, "invalid_text") => Err((StatusCode::BAD_REQUEST, json!({"detail": "texto inválido para terminal"}))),
        (Pane::Deferred, _) => Err(control_failed(MSG_CONTROL_DEFERRED)),
        _ => Err(control_failed(MSG_CONTROL_UNCONFIRMED)),
    }
}

pub fn interrupt_terminal_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    control_step_answer(sent).map_or_else(|refused| refused, |()| ok())
}

/// Sem turno em voo não há o que interromper; responder ok seria fingir. O Codex responde com o
/// código e a frase dele (`api.interrupt`).
pub fn interrupt_headless_answer(codex: bool, sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    if codex {
        return match sent {
            Ok(reply) if reply.disposition == Disposition::Accepted && reply.payload["interrupted"] == false => codex_control(MSG_CODEX_NO_TURN),
            Ok(reply) if reply.disposition == Disposition::Accepted => ok(),
            _ => codex_control(MSG_CODEX_CONTROL),
        };
    }
    let reply = match sent {
        Ok(reply) => reply,
        Err(error) => return sem_turno(&error.to_string()),
    };
    match reply.disposition {
        Disposition::Accepted if reply.payload["interrupted"] == false => sem_turno(MSG_NO_TURN),
        Disposition::Accepted => ok(),
        Disposition::Unknown => sem_turno(MSG_STEER_UNKNOWN),
        _ => sem_turno(reply.payload["error"].as_str().filter(|e| !e.is_empty()).unwrap_or(MSG_STEER_REFUSED)),
    }
}

/// Texto do terminal interativo: qualquer coisa, menos caractere de controle (fora `\n` e `\t`).
fn terminal_text_ok(text: &str) -> bool { !text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')) }

/// Os controles do `/term-input` na ordem do Python: texto, depois tecla. Texto inválido recusa antes
/// de enviar qualquer coisa.
pub fn term_input_steps(text: Option<&str>, key: Option<&str>) -> Result<Vec<(&'static str, Value)>, Answer> {
    let mut steps = Vec::new();
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        if !terminal_text_ok(text) { return Err((StatusCode::BAD_REQUEST, json!({"detail": "texto inválido para terminal"}))); }
        steps.push(("terminal_input", json!({"text": text})));
    }
    if let Some(key) = key.filter(|k| !k.is_empty()) { steps.push(("interactive_key", json!({"key": key}))); }
    Ok(steps)
}

/// `clear` como o FastAPI lê um booleano de query; `None` = valor que ele recusa (422 dele).
fn parse_clear(query: Option<&str>) -> Option<bool> {
    let mut clear = Some(false);
    for pair in query.unwrap_or_default().split('&') {
        let Some(("clear", value)) = pair.split_once('=').or(Some((pair, ""))) else { continue };
        clear = match percent_decode_str(value).decode_utf8().ok()?.to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
            "0" | "false" | "f" | "no" | "n" | "off" => Some(false),
            _ => None,
        };
    }
    clear
}

/// `{"key": texto}` do `KeyBody` estrito.
fn parse_key(bytes: &Bytes) -> Option<String> {
    let object = serde_json::from_slice::<Value>(bytes).ok()?;
    let object = object.as_object()?;
    if object.len() != 1 { return None; }
    object.get("key")?.as_str().map(str::to_owned)
}

/// `{"text": texto|null, "key": texto|null}` do `TermInputBody` estrito, os dois opcionais.
fn parse_term_input(bytes: &Bytes) -> Option<(Option<String>, Option<String>)> {
    let object = serde_json::from_slice::<Value>(bytes).ok()?;
    let object = object.as_object()?;
    if object.keys().any(|k| k != "text" && k != "key") { return None; }
    let field = |name: &str| match object.get(name) { None | Some(Value::Null) => Some(None), Some(Value::String(s)) => Some(Some(s.clone())), Some(_) => None };
    Some((field("text")?, field("key")?))
}

// ── fila: DELETE, PATCH, reorder e steer ────────────────────────────────────────────────────────

const MSG_QUEUE_GONE: &str = "A mensagem já saiu da fila.";
const MSG_QUEUE_TEXT: &str = "A mensagem da fila precisa de texto e não pode começar com /.";
const MSG_STEER_QUESTION: &str = "Responda a permissão ou pergunta pendente antes de orientar.";
const MSG_STEER_NOT_DELIVERED: &str = "A sessão não aceitou agora; a mensagem continua na fila.";
const MSG_STEER_UNCERTAIN: &str = "Não deu para confirmar se a mensagem chegou; confira a conversa antes de repetir.";

fn queue_gone() -> Answer { (StatusCode::CONFLICT, detail_body("queue_entry_gone", MSG_QUEUE_GONE, json!({}))) }
fn queue_refused(code: &str, msg: &str) -> Answer { (StatusCode::CONFLICT, detail_body(code, msg, json!({}))) }
fn steered(value: bool) -> Answer { (StatusCode::OK, json!({"ok": true, "steered": value})) }

/// Rota do painel da fila em `tail` (o que vem depois de `/api/sessions/{n}/`), com o id ainda escapado.
/// `reorder` só é caminho fixo no `POST`: no `DELETE` e no `PATCH` vale como o id de mesmo nome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueRoute<'a> { Remove(&'a str), Edit(&'a str), Reorder, Steer(&'a str) }

pub(crate) fn queue_route<'a>(method: &Method, tail: &'a str) -> Option<QueueRoute<'a>> {
    let rest = tail.strip_prefix("queue/")?;
    let id = |id: &'a str| (!id.is_empty() && !id.contains('/')).then_some(id);
    match *method {
        Method::DELETE => id(rest).map(QueueRoute::Remove),
        Method::PATCH => id(rest).map(QueueRoute::Edit),
        Method::POST if rest == "reorder" => Some(QueueRoute::Reorder),
        Method::POST => rest.strip_suffix("/steer").and_then(id).map(QueueRoute::Steer),
        _ => None,
    }
}

/// O handler da rota da fila, para as pontes que já a reconheceram.
pub(crate) async fn queue_dispatch(route: QueueRoute<'_>, st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let (s, c) = (State(st), ConnectInfo(peer));
    match route {
        QueueRoute::Remove(_) => queue_remove(s, c, req).await,
        QueueRoute::Edit(_) => queue_edit(s, c, req).await,
        QueueRoute::Reorder => queue_reorder(s, c, req).await,
        QueueRoute::Steer(_) => queue_steer(s, c, req).await,
    }
}

/// Id da entrada no caminho do pedido (`…/queue/{id}` ou `…/queue/{id}/steer`), sem o escape.
fn queue_entry(parts: &Parts) -> Option<String> {
    let tail = parts.uri.path().strip_prefix("/api/sessions/")?.split_once('/')?.1;
    let (QueueRoute::Remove(raw) | QueueRoute::Edit(raw) | QueueRoute::Steer(raw)) = queue_route(&parts.method, tail)? else { return None };
    percent_decode_str(raw).decode_utf8().ok().map(|id| id.into_owned())
}

/// `{"text": texto}`, só esse campo.
pub(crate) fn parse_queue_text(bytes: &Bytes) -> Option<String> {
    let value = serde_json::from_slice::<Value>(&super::input::scrub_lone_surrogates(bytes)).ok()?;
    let object = value.as_object().filter(|o| o.len() == 1)?;
    object.get("text")?.as_str().map(str::to_owned)
}

/// `{"id": texto, "before_id": texto|null}`, os dois presentes; `null` é o fim da fila.
pub(crate) fn parse_queue_reorder(bytes: &Bytes) -> Option<(String, Option<String>)> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    let object = value.as_object().filter(|o| o.len() == 2)?;
    let id = object.get("id")?.as_str().filter(|id| !id.is_empty())?.to_owned();
    let before = match object.get("before_id")? {
        Value::Null => None,
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => return None,
    };
    Some((id, before))
}

/// Resultado da ação da fila: `done` é o sucesso; o resto (`absent`, `not_waiting`) é a mesma frase.
pub(crate) fn queue_change_answer(outcome: &Result<Value, RuntimeError>, done: &str) -> Answer {
    match outcome {
        Ok(value) if value == done => ok(),
        Ok(_) => queue_gone(),
        Err(error) => runtime_failed(error),
    }
}

/// Resposta do controle `steer_entry` do ator do terminal. O adiamento não tem código estável: todo
/// `Deferred` sem código conhecido é "não entregue agora".
pub(crate) fn queue_steer_terminal(reply: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let reply = match reply { Ok(reply) => reply, Err(error) => return runtime_failed(error) };
    match (reply.disposition, payload_code(reply)) {
        // O código (`queue_steer_hint_missing`) vai junto: é ele que faz o app dizer que a mensagem espera a pausa.
        (Disposition::Accepted, code) => {
            let (status, mut body) = steered(reply.payload["steered"] == true);
            if let Some(code) = code { body["code"] = json!(code); }
            (status, body)
        }
        (_, Some("queue_entry_gone")) => queue_gone(),
        (_, Some("queue_steer_question_open")) => queue_refused("queue_steer_question_open", MSG_STEER_QUESTION),
        (Disposition::Unknown, _) => queue_refused("queue_steer_uncertain", MSG_STEER_UNCERTAIN),
        _ => queue_refused("queue_steer_not_delivered", MSG_STEER_NOT_DELIVERED),
    }
}

pub fn queue_answer(removed: &Result<Value, RuntimeError>) -> Answer {
    match removed {
        Ok(done) if *done == json!(true) => ok(),
        Ok(_) => (StatusCode::NOT_FOUND, detail_body("erro_fila_entrada_nao_encontrada", MSG_NOT_IN_QUEUE, json!({}))),
        Err(error) if error.code == crate::runtime::terminal::GIVE_UP_COMPOSER_BUSY =>
            (StatusCode::CONFLICT, detail_body(GIVE_UP_COMPOSER_BUSY_CODE, MSG_COMPOSER_BUSY, json!({}))),
        Err(error) if error.code == crate::runtime::terminal::GIVE_UP_COMPOSER_UNREAD =>
            (StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_fila_campo_ilegivel", MSG_COMPOSER_UNREAD, json!({}))),
        Err(error) => runtime_failed(error),
    }
}

// ── rotas ───────────────────────────────────────────────────────────────────────────────────────

fn report(ctx: &Ctx, diary: Diary) {
    let Some((event, code)) = diary else { return };
    let reason = if event == "opcao.nao_convergiu" { "o terminal não confirmou a opção marcada" } else { "o terminal não aceitou o envio das opções marcadas" };
    ctx.st.diag.report(event, &ctx.name, &code, reason);
}

pub async fn select(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Select).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some((option, supplied, text)) = parse_option(&bytes).filter(|_| json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let expected = match expected(&ctx, supplied.as_deref()).await { Ok(expected) => expected, Err(failed) => return answer(&ctx, failed) };
    if text.is_some() && !plan_interaction(&expected) { return invalid_body(ctx.headers()); }
    let operation_id = random_hex(16);
    let result = match &ctx.target.handle {
        EntryHandle::Terminal { handle, .. } => match select_plan(&expected, option, st.term.is_active(&ctx.name)) {
            Err(refused) => refused,
            Ok(mut payload) => {
                if let Some(text) = text { payload["text"] = json!(text); }
                let (answered, diary) = select_terminal_answer(&handle.control(operation_id, "select".into(), payload).await);
                report(&ctx, diary);
                answered
            }
        },
        headless => select_headless_answer(&headless.command(RuntimeCommand { operation_id, kind: OperationKind::Select,
            payload: json!({"option": option, "interaction_id": supplied}) }).await),
    };
    answer(&ctx, result)
}

pub async fn select_submit(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::SelectSubmit).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some(supplied) = parse_submit(&bytes).filter(|_| bytes.is_empty() || json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let expected = match expected(&ctx, supplied.as_deref()).await { Ok(expected) => expected, Err(failed) => return answer(&ctx, failed) };
    // A TUI do Codex não tem a aba Submit do Claude: a pergunta dele se responde pelo `/answer`.
    if matches!(ctx.target.handle, EntryHandle::CodexTerminal { .. }) { return answer(&ctx, interaction_failure(InteractionError::Changed)); }
    let EntryHandle::Terminal { handle, .. } = &ctx.target.handle else { unreachable!("a tabela manda o sem terminal ao Python") };
    if expected.target.kind != crate::interactions::InteractionKind::Menu { return answer(&ctx, interaction_failure(InteractionError::Changed)); }
    let result = if st.term.is_active(&ctx.name) { panel_open_refusal() } else {
        let (answered, diary) = submit_answer(&handle.control(random_hex(16), "submit_selected".into(), json!({"interaction_id": supplied})).await);
        report(&ctx, diary);
        answered
    };
    answer(&ctx, result)
}

pub async fn interrupt(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let query = req.uri().query().map(str::to_owned);
    let (ctx, _) = match admit(&st, peer, req, WriteRoute::Interrupt).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some(clear) = parse_clear(query.as_deref()) else { return invalid_body(ctx.headers()) };
    let operation_id = random_hex(16);
    let result = match &ctx.target.handle {
        // O ator lê a pergunta do plugin antes do Esc e a fecha só se o Esc saiu.
        EntryHandle::Terminal { handle, .. } => interrupt_terminal_answer(&handle.control(operation_id, "interrupt".into(), json!({"clear": clear})).await),
        headless => {
            let sent = headless.command(RuntimeCommand { operation_id, kind: OperationKind::Interrupt, payload: json!({}) }).await;
            let codex = ctx.target.provider == "codex";
            if codex { super::codex::log_outcome(&ctx.name, "interrupt", &sent, "interrupção do Codex não confirmada; a rota responde 409 erro_codex_controle"); }
            interrupt_headless_answer(codex, &sent)
        }
    };
    answer(&ctx, result)
}

pub async fn keys(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::Keys).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some(key) = parse_key(&bytes).filter(|_| json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let result = match &ctx.target.handle {
        EntryHandle::Terminal { handle, .. } => control_step_answer(&handle.control(random_hex(16), "navigation_key".into(), json!({"key": key})).await),
        // Codex com terminal: o pane é do Rust também, pelo mesmo escritor de teclas.
        EntryHandle::CodexTerminal { target, cell, .. } => pane_step_answer(&crate::runtime::codex_terminal::pane_driver(&target.terminal, cell.pane_serial(), None).key(&key, false).await),
        EntryHandle::Headless(_) => unreachable!("a tabela manda o sem terminal ao Python"),
    };
    answer(&ctx, result.map_or_else(|refused| refused, |()| ok()))
}

pub async fn term_input(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::TermInput).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some((text, key)) = parse_term_input(&bytes).filter(|_| json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let result = match term_input_steps(text.as_deref(), key.as_deref()) {
        Err(refused) => refused,
        Ok(steps) => {
            let mut done = ok();
            for (control, payload) in steps {
                let step = match &ctx.target.handle {
                    EntryHandle::Terminal { handle, .. } => control_step_answer(&handle.control(random_hex(16), control.into(), payload).await),
                    EntryHandle::CodexTerminal { target, cell, .. } => {
                        let driver = crate::runtime::codex_terminal::pane_driver(&target.terminal, cell.pane_serial(), None);
                        pane_step_answer(&if control == "terminal_input" { driver.text(payload["text"].as_str().unwrap_or("")).await }
                            else { driver.key(payload["key"].as_str().unwrap_or(""), true).await })
                    }
                    EntryHandle::Headless(_) => unreachable!("a tabela manda o sem terminal ao Python"),
                };
                if let Err(refused) = step { done = refused; break; }
            }
            done
        }
    };
    answer(&ctx, result)
}

pub async fn queue_remove(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, _) = match admit(&st, peer, req, WriteRoute::QueueRemove).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some(entry_id) = queue_entry(&ctx.parts) else { return invalid_body(ctx.headers()) };
    // O X do painel só tira a que espera: entrega incerta nunca vira desistência por ele.
    let only_waiting = match crate::auth::query_param(ctx.parts.uri.query(), "only_waiting").as_deref() {
        None => false,
        Some("true" | "1") => true,
        Some(_) => return invalid_body(ctx.headers()),
    };
    let cancelled = ctx.target.handle.queue(random_hex(16), Action::Cancel { entry_id: entry_id.clone() }).await;
    if !only_waiting && let Err(failure) = &cancelled {
        tracing::warn!(session = %ctx.name, code = %failure.code, "descarte: o cancelamento falhou; tenta tirar a linha");
    }
    if only_waiting || matches!(cancelled, Ok(ref done) if *done == json!("cancelled")) {
        return answer(&ctx, queue_change_answer(&cancelled, "cancelled"));
    }
    let mut removed = ctx.target.handle.queue(random_hex(16), Action::Remove { entry_id: entry_id.clone() }).await;
    // A entrega incerta não sai por `Remove` (segura a escrita): descartar é desistir dela e então tirá-la.
    if matches!(removed, Ok(ref done) if *done != json!(true)) {
        let given = ctx.target.handle.queue(random_hex(16), Action::GiveUp { entry_id: entry_id.clone() }).await;
        removed = match given {
            // A desistência já soltou a fila; a linha "não chegou" que sobrar sai num segundo descarte.
            Ok(done) if done == json!(true) => match ctx.target.handle.queue(random_hex(16), Action::Remove { entry_id }).await {
                Ok(_) => Ok(json!(true)),
                Err(failure) => {
                    tracing::warn!(session = %ctx.name, code = %failure.code, "descarte: entrega desistida, mas a linha não saiu da fila");
                    Ok(json!(true))
                }
            },
            other => other,
        };
    }
    answer(&ctx, queue_answer(&removed))
}

pub async fn queue_edit(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::QueueEdit).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let (Some(entry_id), Some(text)) = (queue_entry(&ctx.parts), parse_queue_text(&bytes).filter(|_| json_content_type(ctx.headers())))
        else { return invalid_body(ctx.headers()) };
    if !queue_text_ok(&text) {
        return answer(&ctx, (StatusCode::UNPROCESSABLE_ENTITY, detail_body("queue_text_invalid", MSG_QUEUE_TEXT, json!({}))));
    }
    let edited = ctx.target.handle.queue(random_hex(16), Action::Edit { entry_id, text }).await;
    answer(&ctx, queue_change_answer(&edited, "edited"))
}

pub async fn queue_reorder(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::QueueReorder).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some((entry_id, before_id)) = parse_queue_reorder(&bytes).filter(|_| json_content_type(ctx.headers())) else { return invalid_body(ctx.headers()) };
    let moved = ctx.target.handle.queue(random_hex(16), Action::Reorder { entry_id, before_id }).await;
    answer(&ctx, queue_change_answer(&moved, "moved"))
}

pub async fn queue_steer(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit(&st, peer, req, WriteRoute::QueueSteer).await {
        Ok(admitted) => admitted,
        Err(response) => return response,
    };
    let Some(entry_id) = queue_entry(&ctx.parts) else { return invalid_body(ctx.headers()) };
    // Corpo vazio ou `{}`: nada além do id do caminho.
    if !bytes.is_empty() && serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v.as_object().map(|o| o.is_empty())) != Some(true) {
        return invalid_body(ctx.headers());
    }
    let result = match &ctx.target.handle {
        // Serial com a drenagem dentro do ator: subir e entregar acontecem sem outra mensagem no meio.
        EntryHandle::Terminal { handle, .. } =>
            queue_steer_terminal(&handle.control(random_hex(16), "steer_entry".into(), json!({"entry_id": entry_id})).await),
        handle => queue_steer_runtime(handle, &entry_id).await,
    };
    answer(&ctx, result)
}

/// Claude e Codex sem terminal, e Codex com terminal: o turno é do processo. Com turno, `SteerQueue` só desta
/// entrada; sem turno (ou ele acabou no meio), ela vai para a frente e sai na entrega comum.
async fn queue_steer_runtime(handle: &EntryHandle, entry_id: &str) -> Answer {
    let mut returned = false;
    if handle.view().await.is_ok_and(|view| view["in_progress"] == true) {
        let sent = handle.command(RuntimeCommand { operation_id: random_hex(16), kind: OperationKind::SteerQueue,
            payload: json!({"entry_id": entry_id}) }).await;
        match steer_in_turn(&sent, entry_id) {
            Ok(answer) => return answer,
            Err(back) => returned = back,
        }
    }
    let moved = handle.queue(random_hex(16), Action::Promote { entry_id: entry_id.into() }).await;
    let after = match steer_after_promote(&moved, returned) { Ok(after) => after, Err(answer) => return answer };
    // Sem entregar agora (pergunta aberta, processo subindo), ela fica na frente: a próxima rodada a leva.
    if let Err(error) = handle.drain().await {
        tracing::warn!(code = %error.code, "orientar sem turno: a drenagem não rodou; a entrada ficou na frente");
    }
    after
}

/// A resposta do `SteerQueue` no meio do turno: `Ok` encerra a rota; `Err` segue ao `Promote`, com `true` quando a
/// orientação foi aceita mas a entrada voltou à fila (fora de `ids`) e `false` quando não havia turno.
pub fn steer_in_turn(sent: &Result<RuntimeReply, RuntimeError>, entry_id: &str) -> Result<Answer, bool> {
    match sent {
        // Fora de `ids`: ou ela já não esperava, ou a orientação dela voltou à fila; o `Promote` separa os dois.
        Ok(reply) if reply.disposition == Disposition::Accepted =>
            if reply.payload["ids"].as_array().is_some_and(|ids| ids.iter().any(|id| id == entry_id)) { Ok(steered(true)) } else { Err(true) },
        Ok(reply) if payload_code(reply) == Some("steer_question_open") => Ok(queue_refused("queue_steer_question_open", MSG_STEER_QUESTION)),
        Ok(reply) if payload_code(reply) == Some("steer_no_turn") => Err(false),
        Ok(reply) if reply.disposition == Disposition::Unknown => Ok(queue_refused("queue_steer_uncertain", MSG_STEER_UNCERTAIN)),
        Ok(_) => Ok(queue_refused("queue_steer_not_delivered", MSG_STEER_NOT_DELIVERED)),
        Err(error) => Ok(runtime_failed(error)),
    }
}

/// Depois do `Promote`: `Err` responde já (a entrada sumiu, ou a fila falhou); `Ok` é a resposta depois da drenagem.
/// A orientação aceita que voltou à fila foi para a frente, mas não entrou no turno: `queue_steer_not_delivered`.
pub fn steer_after_promote(moved: &Result<Value, RuntimeError>, returned: bool) -> Result<Answer, Answer> {
    match moved {
        Ok(moved) if moved == "moved" => Ok(if returned { queue_refused("queue_steer_not_delivered", MSG_STEER_NOT_DELIVERED) } else { steered(false) }),
        Ok(_) => Err(queue_gone()),
        Err(error) => Err(runtime_failed(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_reads_like_a_fastapi_bool() {
        assert_eq!(parse_clear(None), Some(false));
        assert_eq!(parse_clear(Some("clear=true")), Some(true));
        assert_eq!(parse_clear(Some("x=1&clear=No")), Some(false));
        assert_eq!(parse_clear(Some("clear=0&clear=on")), Some(true), "o último vale");
        assert_eq!(parse_clear(Some("clear=talvez")), None);
        assert_eq!(parse_clear(Some("clear=")), None);
        assert_eq!(parse_clear(Some("clear")), None);
    }

    #[test]
    fn queue_bodies_are_strict() {
        let b = |s: &'static str| Bytes::from_static(s.as_bytes());
        assert_eq!(parse_queue_text(&b(r#"{"text":"oi"}"#)), Some("oi".into()));
        assert_eq!(parse_queue_text(&b(r#"{"text":"oi","x":1}"#)), None);
        assert_eq!(parse_queue_reorder(&b(r#"{"id":"a","before_id":null}"#)), Some(("a".into(), None)));
        assert_eq!(parse_queue_reorder(&b(r#"{"id":"a","before_id":"b"}"#)), Some(("a".into(), Some("b".into()))));
        assert_eq!(parse_queue_reorder(&b(r#"{"id":"a"}"#)), None);
        assert_eq!(parse_queue_reorder(&b(r#"{"id":"","before_id":null}"#)), None);
    }

    #[test]
    fn queue_entry_comes_from_the_path() {
        let parts = |method: Method, path: &str| Request::builder().method(method).uri(path).body(()).unwrap().into_parts().0;
        assert_eq!(queue_entry(&parts(Method::DELETE, "/api/sessions/s/queue/a%2Fb")), Some("a/b".into()));
        assert_eq!(queue_entry(&parts(Method::POST, "/api/sessions/s/queue/a/steer")), Some("a".into()));
        assert_eq!(queue_entry(&parts(Method::PATCH, "/api/sessions/s/queue/a/steer")), None);
        assert_eq!(queue_entry(&parts(Method::DELETE, "/api/sessions/s/queue/")), None);
        assert_eq!(queue_entry(&parts(Method::POST, "/api/sessions/s/queue/reorder")), None);
    }

    #[test]
    fn queue_route_is_strict() {
        use QueueRoute::*;
        assert_eq!(queue_route(&Method::DELETE, "queue/e1"), Some(Remove("e1")));
        assert_eq!(queue_route(&Method::DELETE, "queue/reorder"), Some(Remove("reorder")));
        assert_eq!(queue_route(&Method::PATCH, "queue/e1"), Some(Edit("e1")));
        assert_eq!(queue_route(&Method::POST, "queue/reorder"), Some(Reorder));
        assert_eq!(queue_route(&Method::POST, "queue/e1/steer"), Some(Steer("e1")));
        for (method, tail) in [(Method::POST, "queue/e1"), (Method::DELETE, "queue/a/b"), (Method::PATCH, "queue/"),
            (Method::POST, "queue//steer"), (Method::POST, "queue/a/b/steer"), (Method::GET, "queue/e1"), (Method::DELETE, "input")] {
            assert_eq!(queue_route(&method, tail), None, "{method} {tail}");
        }
    }

    #[test]
    fn queue_change_maps_outcomes() {
        assert_eq!(queue_change_answer(&Ok(json!("edited")), "edited").0, StatusCode::OK);
        let gone = queue_change_answer(&Ok(json!("not_waiting")), "edited");
        assert_eq!((gone.0, gone.1["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("queue_entry_gone")));
        assert_eq!(queue_change_answer(&Ok(json!("absent")), "moved").0, StatusCode::CONFLICT);
    }

    #[test]
    fn terminal_steer_answers() {
        let r = |disposition, payload| -> Result<RuntimeReply, RuntimeError> { Ok(RuntimeReply { operation_id: "x".into(), disposition, payload }) };
        assert_eq!(queue_steer_terminal(&r(Disposition::Accepted, json!({"steered":true}))).1["steered"], true);
        let late = queue_steer_terminal(&r(Disposition::Accepted, json!({"steered":false,"code":"queue_steer_hint_missing"}))).1;
        assert_eq!((late["steered"].clone(), late["code"].clone()), (json!(false), json!("queue_steer_hint_missing")), "o código chega ao app");
        assert_eq!(queue_steer_terminal(&r(Disposition::Rejected, json!({"code":"queue_entry_gone"}))).1["detail"]["code"], "queue_entry_gone");
        assert_eq!(queue_steer_terminal(&r(Disposition::Rejected, json!({"code":"queue_steer_question_open"}))).1["detail"]["code"], "queue_steer_question_open");
        assert_eq!(queue_steer_terminal(&r(Disposition::Unknown, json!({"code":"queue_steer_uncertain"}))).1["detail"]["code"], "queue_steer_uncertain");
        assert_eq!(queue_steer_terminal(&r(Disposition::Deferred, json!({}))).1["detail"]["code"], "queue_steer_not_delivered");
    }

    #[test]
    fn terminal_text_refuses_control_characters_but_not_newline_or_tab() {
        assert!(terminal_text_ok("ls\n\tx"));
        assert!(!terminal_text_ok("a\u{1}b"));
    }
}
