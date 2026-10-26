//! `POST /model-effort` do Claude sem conta ChatGPT fixa. Com terminal, o ator digita o seletor do
//! `/model` (`model_picker`); sem terminal, o ator troca pelo pedido de controle. As respostas são as da
//! rota Python que esta substitui, inclusive as recusas do seletor em texto cru.
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{busy_body, detail_body, enter_then_find, invalid_body, json_response};
use crate::routes::{AppState, cors};
use crate::runtime::gateway::{EntryHandle, RuntimeRegistry};
use crate::runtime::protocol::{Disposition, OperationKind, RuntimeCommand, RuntimeReply};

/// O `wait_for` da rota Python em volta da troca sem terminal.
const HEADLESS_WAIT: Duration = Duration::from_secs(40);
const PANEL_MSG: &str = "Terminal aberto nesta sessao. Feche o painel pra responder por aqui.";

#[derive(Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Scope { #[default] Session, Default }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body { #[serde(default)] model: Option<String>, #[serde(default)] effort: Option<String>, #[serde(default)] scope: Scope }

fn answer(headers: &HeaderMap, status: StatusCode, body: Value) -> Response {
    let mut response = json_response(status, body);
    cors(headers, response.headers_mut());
    response
}

/// O `HTTPException(status, texto)` do Python: `detail` é a frase, sem código.
fn plain(status: StatusCode, text: &str) -> (StatusCode, Value) { (status, json!({"detail": text})) }

/// `model_args.validar("claude", …)`, que a rota sem terminal aplica ao corpo como veio.
fn headless_invalid(model: Option<&str>, effort: Option<&str>) -> Option<&'static str> {
    if model.is_some_and(|m| !crate::session_open::resolve::valid_model(m)) {
        return Some("model: use letras, números e . _ : / - ~ [ ] (até 128 caracteres, sem começar com -)");
    }
    if effort.is_some_and(|e| !["low", "medium", "high", "xhigh", "max"].contains(&e)) {
        return Some("effort: use um de low, medium, high, xhigh, max");
    }
    None
}

/// `None`: a sessão não é um Claude do Rust (Pi, omp, Kimi, Codex sem ator, nenhuma) e o pedido segue ao Python
/// como veio.
pub(super) async fn serve(st: &Arc<AppState>, runtime: Arc<RuntimeRegistry>, name: String, headers: &HeaderMap, bytes: &[u8]) -> Option<Response> {
    // Claude com terminal ainda sem ator aqui: o Rust o adota agora.
    let found = match enter_then_find(&runtime, &name, st.write_gate_wait).await {
        Ok(None) => match runtime.adopt_now(&name).await {
            Some(Ok(())) => enter_then_find(&runtime, &name, st.write_gate_wait).await,
            // Codex abrindo a conversa: esta rota é do Claude, e segue como antes.
            Some(Err(error)) if error.code == crate::runtime::codex_relaunch::STARTING => Ok(None),
            Some(Err(error)) => return Some(answer(headers, StatusCode::CONFLICT, detail_body(&error.code, &error.message, json!({})))),
            None => Ok(None),
        },
        other => other,
    };
    let Ok(found) = found else { return Some(answer(headers, StatusCode::CONFLICT, busy_body())) };
    let (held, target) = found.filter(|(_, target)| target.provider == "claude")?;
    // Só objeto, como o modelo do pydantic: o serde aceitaria a lista `[]` como struct vazio.
    let body = serde_json::from_slice::<Value>(bytes).ok().filter(Value::is_object).and_then(|v| serde_json::from_value::<Body>(v).ok());
    let Some(body) = body else { return Some(invalid_body(headers)) };
    let id = crate::mods::state::random_hex(16);
    let (status, value) = match (held, target) {
        (held, target) if !target.terminal => {
            if let Some(text) = headless_invalid(body.model.as_deref(), body.effort.as_deref()) {
                plain(StatusCode::UNPROCESSABLE_ENTITY, text)
            } else {
                let EntryHandle::Headless(handle) = &target.handle else { return Some(answer(headers, StatusCode::CONFLICT, busy_body())) };
                let command = RuntimeCommand { operation_id: id, kind: OperationKind::SetModel, payload: json!({"model": body.model, "effort": body.effort}) };
                let sent = tokio::time::timeout(HEADLESS_WAIT, handle.command(command)).await;
                drop(held);
                headless_answer(sent)
            }
        }
        (held, target) => {
            match terminal_checks(st, &name, &body) {
                Err(refusal) => refusal,
                Ok((model, effort)) => {
                    let EntryHandle::Terminal { handle, .. } = &target.handle else { return Some(answer(headers, StatusCode::CONFLICT, busy_body())) };
                    let payload = json!({"model": model, "effort": effort, "scope": if body.scope == Scope::Default { "default" } else { "session" }});
                    let reply = handle.control(id, "set_model_effort".into(), payload).await;
                    drop(held);
                    terminal_answer(reply)
                }
            }
        }
    };
    Some(answer(headers, status, value))
}

/// Painel aberto e a validação de antes da trava, na ordem do Python.
fn terminal_checks(st: &AppState, name: &str, body: &Body) -> Result<(Option<String>, Option<String>), (StatusCode, Value)> {
    if st.term.is_active(name) {
        return Err((StatusCode::CONFLICT, detail_body("erro_terminal_aberto", PANEL_MSG, json!({}))));
    }
    crate::model_picker::normalize(body.model.as_deref(), body.effort.as_deref()).map_err(|text| plain(StatusCode::UNPROCESSABLE_ENTITY, &text))
}

/// `adapter.control` do Python em cima do ator: adiado também vale; incerto e recusado são 409.
fn headless_answer(sent: Result<Result<RuntimeReply, crate::runtime::protocol::RuntimeError>, tokio::time::error::Elapsed>) -> (StatusCode, Value) {
    let failed = |why: &str| (StatusCode::CONFLICT, detail_body("erro_modelo_indisponivel", &format!("não consegui trocar: {why}"), json!({})));
    match sent {
        Ok(Ok(reply)) if reply.disposition == Disposition::Accepted => (StatusCode::OK, json!({"ok": true, "scope": "session", "result": null})),
        // Adiada não foi aplicada: um "ok" faria o app mostrar o modelo novo que a sessão ainda não usa.
        Ok(Ok(reply)) if reply.disposition == Disposition::Deferred => (StatusCode::CONFLICT, detail_body("erro_modelo_adiado",
            "a troca de modelo não foi aplicada agora; confira o modelo da sessão antes de tentar de novo", json!({}))),
        Ok(Ok(reply)) if reply.disposition == Disposition::Unknown => failed("resultado incerto; a operação foi conservada sem reenvio"),
        Ok(Ok(reply)) => failed(reply.payload["error"].as_str().unwrap_or("operação recusada pelo runtime")),
        Ok(Err(error)) => failed(&error.message),
        // O comando pode ainda chegar depois do prazo: o código diz que o resultado é desconhecido.
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, detail_body("erro_modelo_sem_resposta",
            "o Claude não respondeu à troca de modelo a tempo; confira a sessão antes de tentar de novo", json!({}))),
    }
}

fn terminal_answer(reply: Result<RuntimeReply, crate::runtime::protocol::RuntimeError>) -> (StatusCode, Value) {
    match reply {
        Ok(reply) if reply.disposition == Disposition::Accepted => (StatusCode::OK, reply.payload),
        Ok(reply) => match reply.payload["error_code"].as_str() {
            // Recusa do seletor: o texto cru, como o `HTTPException(e.status, e.detail)` do Python.
            None => plain(StatusCode::CONFLICT, reply.payload["detail"].as_str().unwrap_or("model picker failed")),
            Some(code) => (StatusCode::CONFLICT, detail_body(public_code(code), reply.payload["detail"].as_str().unwrap_or(""), json!({}))),
        },
        Err(error) => (StatusCode::CONFLICT, detail_body(public_code(&error.code), &error.message, json!({}))),
    }
}

fn public_code(code: &str) -> &str {
    match code { "terminal_panel_active" | "terminal_panel_open" => "erro_terminal_aberto", _ => code }
}

#[cfg(test)]
mod timeout_tests {
    /// O prazo da troca sem terminal vencido sai com código e frase: a troca pode ter acontecido depois.
    #[tokio::test]
    async fn a_headless_switch_past_its_deadline_answers_a_code() {
        let elapsed = tokio::time::timeout(std::time::Duration::ZERO, std::future::pending::<()>()).await.unwrap_err();
        let (status, body) = super::headless_answer(Err(elapsed));
        assert_eq!((status.as_u16(), body["detail"]["code"].as_str()), (504, Some("erro_modelo_sem_resposta")));
        assert!(!body["detail"]["msg"].as_str().unwrap_or("").is_empty());
    }

    /// Troca adiada não é troca feita: sai com código, nunca `ok`.
    #[test]
    fn a_deferred_switch_is_not_ok() {
        let reply = crate::runtime::protocol::RuntimeReply { operation_id: "x".into(), disposition: super::Disposition::Deferred, payload: serde_json::json!({}) };
        let (status, body) = super::headless_answer(Ok(Ok(reply)));
        assert_eq!((status.as_u16(), body["detail"]["code"].as_str()), (409, Some("erro_modelo_adiado")));
    }
}
