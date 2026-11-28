//! `POST /api/sessions/{name}/child-answer`: o `child_answer` pelo `hangar-send --responder`, para o pai que não tem a
//! tool no catálogo (sessão aberta antes dela). Mesmas recusas, a mesma marca do pai e a mesma `/answer` da tool.
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
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
    let options = row.options.clone().unwrap_or_default();
    let answers = match hangar_delegation::answer::answers_from_replies(row.ask_form.as_ref(), &options, &body.replies) {
        Ok(answers) => answers,
        Err(why) => return refused(StatusCode::UNPROCESSABLE_ENTITY, "answer_reply_invalid", &why),
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
        .unwrap_or("answer_refused");
    let status = match code {
        "erro_nao_ligada" | "sessao_desconhecida" => StatusCode::FORBIDDEN,
        "link_returns_write_failed" | "child_answer_list_failed" | "groups_unavailable" | "group_file_unreadable" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    };
    // `child_answer_failed` é a tarefa da `/answer` que caiu depois de sair: pode ter chegado.
    respond(status, detail_body(code, refusal, json!({})), code == "child_answer_failed")
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("groups_unavailable: grupos indisponíveis", StatusCode::SERVICE_UNAVAILABLE, "groups_unavailable", false),
            ("group_file_unreadable: arquivo de grupo de 'f' ilegível", StatusCode::SERVICE_UNAVAILABLE, "group_file_unreadable", false),
            ("sem código: frase", StatusCode::CONFLICT, "answer_refused", false),
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
