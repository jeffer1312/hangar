//! `POST /api/sessions/{name}/child-answer`: o `child_answer` pelo `hangar-send --responder`, para o pai que não tem a
//! tool no catálogo (sessão aberta antes dela). Mesmas recusas, a mesma marca do pai e a mesma `/answer` da tool.
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use hangar_api::ask::AskQuestion;
use hangar_api::session::SessionRow;
use serde::Deserialize;
use serde_json::{Value, json};

use super::tools::{Tools, answer_unconfirmed, unconfirmed};
use crate::groups::link_return;
use crate::session_write::{detail_body, json_response};

pub(super) fn router(tools: Arc<Tools>) -> Router {
    Router::new().route("/api/sessions/{name}/child-answer", axum::routing::post(
        move |Path(name): Path<String>, headers: HeaderMap, body: Bytes| answer(tools.clone(), name, headers, body)))
}

/// Quem responde não vem no corpo: é a sessão dos cabeçalhos de identidade, como na tool.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body { interaction_id: String, replies: Vec<String> }

/// Toda resposta leva `maybe_sent`: `false` só quando nada chegou à pergunta, e o `hangar-send` só aí diz que pode repetir.
async fn answer(tools: Arc<Tools>, child: String, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(body) = serde_json::from_slice::<Body>(&body) else {
        return refused(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo esperado: {interaction_id, replies}");
    };
    let (me, voice) = match tools.caller(&headers).await { Ok(caller) => caller, Err(refusal) => return refusal_response(&refusal) };
    if let Err(refusal) = tools.answerable_child(&me, voice, &child).await { return refusal_response(&refusal); }
    if let Some(refusal) = tools.kind_refusal(&child, &body.interaction_id).await { return refusal_response(&refusal); }
    let row = match tools.child_row(&child).await { Ok(row) => row, Err(refusal) => return refusal_response(&refusal) };
    let Some(row) = row.filter(|row| link_return::open_interaction(row).as_deref() == Some(body.interaction_id.as_str())) else {
        return refused(StatusCode::CONFLICT, "erro_interacao_desatualizada",
            &format!("a pergunta de '{child}' mudou ou já fechou; responda com o interaction_id do aviso mais recente"));
    };
    let answers = match answers_from_replies(&row, &body.replies) {
        Ok(answers) => answers,
        Err(why) => return refused(StatusCode::UNPROCESSABLE_ENTITY, "erro_resposta_invalida", &why),
    };
    match tools.answer_child(&me, &child, &body.interaction_id, answers).await {
        Err(refusal) => refusal_response(&refusal),
        Ok((status, value)) if unconfirmed(status, &value) => respond(StatusCode::OK, answer_unconfirmed(), true),
        Ok((status, value)) => {
            let sent = link_return::answer_maybe_sent(status, &value);
            respond(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), value, sent)
        }
    }
}

fn respond(status: StatusCode, value: Value, maybe_sent: bool) -> Response {
    let mut body = match value { Value::Object(map) => map, other => serde_json::Map::from_iter([("detail".to_owned(), other)]) };
    body.insert("maybe_sent".into(), Value::Bool(maybe_sent));
    json_response(status, Value::Object(body))
}

/// Recusa de antes de qualquer envio.
fn refused(status: StatusCode, code: &str, msg: &str) -> Response { respond(status, detail_body(code, msg, json!({})), false) }

/// As recusas da tool são `código: frase`; a rota leva o código ao `detail`.
fn refusal_response(refusal: &str) -> Response {
    let code = refusal.split_once(": ").map(|(code, _)| code)
        .filter(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .unwrap_or("erro_resposta_recusada");
    let status = match code {
        "erro_nao_ligada" | "sessao_desconhecida" => StatusCode::FORBIDDEN,
        "link_returns_write_failed" | "child_answer_list_failed" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    };
    // `child_answer_failed` é a tarefa da `/answer` que caiu depois de sair: pode ter chegado.
    respond(status, detail_body(code, refusal, json!({})), code == "child_answer_failed")
}

/// Uma resposta por item do formulário, na ordem: `N` ou `N,M` escolhe opções (contadas de 1); `texto:…` e o resto são
/// texto livre, no campo de texto que vem depois das opções, como no repasse ao pai.
fn answers_from_replies(row: &SessionRow, replies: &[String]) -> Result<Vec<Value>, String> {
    let form = row.ask_form.clone().and_then(|form| serde_json::from_value::<AskQuestion>(form).ok()).filter(|form| !form.questions.is_empty());
    let items: Vec<(Vec<String>, bool)> = match form {
        Some(form) => form.questions.into_iter().map(|q| (q.options.into_iter().map(|o| o.label).collect(), q.multi_select)).collect(),
        None => vec![(row.options.clone().unwrap_or_default(), false)],
    };
    if replies.len() != items.len() {
        return Err(format!("a pergunta tem {} item(ns) e vieram {} resposta(s): mande uma por item, na ordem", items.len(), replies.len()));
    }
    items.iter().zip(replies).enumerate().map(|(n, ((labels, multi), reply))| answer_item(n + 1, labels, *multi, reply)).collect()
}

fn answer_item(item: usize, labels: &[String], multi: bool, reply: &str) -> Result<Value, String> {
    let reply = reply.trim();
    if let Some(text) = reply.strip_prefix(link_return::ANSWER_TEXT_PREFIX).map(str::trim) {
        if text.is_empty() { return Err(format!("resposta {item} vazia")); }
        return Ok(json!({"kind": "text", "value": text, "type_index": labels.len()}));
    }
    if reply.is_empty() { return Err(format!("resposta {item} vazia")); }
    let picked: Option<Vec<usize>> = reply.split(',').map(|n| n.trim().parse::<usize>().ok()).collect();
    // Item sem opções só aceita texto: um número ali é a resposta escrita.
    let Some(mut picked) = picked.filter(|_| !labels.is_empty()) else {
        return Ok(json!({"kind": "text", "value": reply, "type_index": labels.len()}));
    };
    if let Some(bad) = picked.iter().find(|n| **n == 0 || **n > labels.len()) {
        return Err(format!("item {item}: a opção {bad} não existe (1 a {})", labels.len()));
    }
    picked.sort_unstable();
    picked.dedup();
    if picked.len() > 1 && !multi { return Err(format!("item {item} aceita uma opção só")); }
    let chosen: Vec<&String> = picked.iter().map(|n| &labels[n - 1]).collect();
    let indices: Vec<usize> = picked.iter().map(|n| n - 1).collect();
    Ok(json!({"kind": "option", "indices": indices, "multi": multi, "labels": chosen}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(form: Value) -> SessionRow {
        serde_json::from_value(json!({"name": "f", "provider": "claude", "state": "awaiting_input", "ask_form": form})).unwrap()
    }

    fn form(items: &[(&[&str], bool)]) -> Value {
        let questions: Vec<Value> = items.iter().map(|(options, multi)| json!({"header": "h", "question": "q", "multiSelect": multi,
            "options": options.iter().map(|label| json!({"label": label})).collect::<Vec<_>>()})).collect();
        json!({"interaction_id": "q1", "questions": questions})
    }

    fn replies(items: &[&str]) -> Vec<String> { items.iter().map(|s| (*s).to_owned()).collect() }

    #[test]
    fn a_number_picks_the_option_counted_from_one_and_text_goes_after_the_options() {
        let asked = row(form(&[(&["src", "docs"], false), (&["a", "b", "c"], true)]));
        assert_eq!(answers_from_replies(&asked, &replies(&["2", "3,1"])).unwrap(), vec![
            json!({"kind": "option", "indices": [1], "multi": false, "labels": ["docs"]}),
            json!({"kind": "option", "indices": [0, 2], "multi": true, "labels": ["a", "c"]}),
        ]);
        assert_eq!(answers_from_replies(&asked, &replies(&["outra pasta", "b"])).unwrap()[0],
            json!({"kind": "text", "value": "outra pasta", "type_index": 2}));
    }

    #[test]
    fn the_text_prefix_sends_a_number_as_free_text() {
        let asked = row(form(&[(&["src", "docs"], false)]));
        assert_eq!(answers_from_replies(&asked, &replies(&["texto: 2"])).unwrap(), vec![json!({"kind": "text", "value": "2", "type_index": 2})]);
        assert!(answers_from_replies(&asked, &replies(&["texto:"])).is_err(), "texto vazio");
    }

    #[test]
    fn a_reply_that_does_not_fit_the_form_is_refused_before_any_answer() {
        let asked = row(form(&[(&["src", "docs"], false)]));
        for bad in [replies(&["3"]), replies(&["0"]), replies(&["1,2"]), replies(&[" "]), replies(&["1", "2"]), replies(&[])] {
            assert!(answers_from_replies(&asked, &bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn without_a_form_the_line_options_answer_and_a_question_without_options_takes_text() {
        let mut line = row(Value::Null);
        line.options = Some(vec!["sim".into(), "não".into()]);
        assert_eq!(answers_from_replies(&line, &replies(&["1"])).unwrap(), vec![json!({"kind": "option", "indices": [0], "multi": false, "labels": ["sim"]})]);
        line.options = None;
        assert_eq!(answers_from_replies(&line, &replies(&["42"])).unwrap(), vec![json!({"kind": "text", "value": "42", "type_index": 0})]);
    }

    async fn body_of(response: Response) -> Value {
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
    }

    /// Recusa da tool mantém o código e diz que nada saiu; só a tarefa da `/answer` caída diz que pode ter saído.
    #[tokio::test]
    async fn the_route_keeps_the_code_of_the_tool_refusal_and_says_whether_anything_left() {
        for (refusal, status, code, sent) in [
            ("erro_nao_ligada: 'f' não é filha ligada a você", StatusCode::FORBIDDEN, "erro_nao_ligada", false),
            ("sessao_desconhecida: nenhum cabeçalho", StatusCode::FORBIDDEN, "sessao_desconhecida", false),
            ("link_returns_write_failed: não gravei", StatusCode::SERVICE_UNAVAILABLE, "link_returns_write_failed", false),
            ("erro_permissao_do_dono: menu", StatusCode::CONFLICT, "erro_permissao_do_dono", false),
            ("recusado: 'f' é esta sessão", StatusCode::CONFLICT, "recusado", false),
            ("child_answer_failed: a resposta não terminou", StatusCode::CONFLICT, "child_answer_failed", true)] {
            let response = refusal_response(refusal);
            assert_eq!(response.status(), status, "{refusal}");
            let body = body_of(response).await;
            assert_eq!((body["detail"]["code"].as_str(), &body["maybe_sent"]), (Some(code), &json!(sent)), "{refusal}");
        }
        let passed = body_of(respond(StatusCode::INTERNAL_SERVER_ERROR, json!("x"), true)).await;
        assert_eq!(passed, json!({"detail": "x", "maybe_sent": true}));
    }
}
