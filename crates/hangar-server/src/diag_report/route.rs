//! `POST /api/diag/report` (C1): só o dono; monta o pacote, manda ao worker (C2) e, com a senha da
//! resposta, o diário inteiro (C2b). Nada vai ao Python.
use super::{ClientInput, TEXT_MAX, build};
use crate::session_write::{detail, json_response};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// Onde o worker atende em produção; o teste troca pelo `AppState::diag_report_base`.
pub const WORKER_BASE: &str = "https://hangar.dev.br";
const BODY_IN_MAX: usize = 64 * 1024;
const FAILURES_MAX: usize = 50;
const CLIENT_MAX: usize = 200;
const ROUTE_MAX: usize = 200;
const DIARY_TIMEOUT: Duration = Duration::from_secs(60);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    crate::http_client().timeout(Duration::from_secs(20)).build().unwrap_or_default()
});

fn code_ok(code: &str) -> bool {
    (1..=64).contains(&code.len()) && code.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b':' | b'-'))
}

fn valid(input: &ClientInput) -> bool {
    input.text.as_deref().is_none_or(|t| t.chars().count() <= TEXT_MAX)
        && input.client.chars().count() <= CLIENT_MAX
        && input.failures.len() <= FAILURES_MAX
        && input.failures.iter().all(|f| f.route.chars().count() <= ROUTE_MAX && f.code.as_deref().is_none_or(code_ok))
        && input.group.is_some() == input.group_token.is_some()
        && input.group.as_deref().is_none_or(group_ok)
        && input.group_token.as_deref().is_none_or(token_ok)
}

/// O código de um relatório do worker (`[a-z2-7]{8}`).
fn group_ok(group: &str) -> bool { group.len() == 8 && group.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7')) }

/// A assinatura do grupo do worker (`<base36>.<base32 de 26>`): vai num cabeçalho, então só esses caracteres.
fn token_ok(token: &str) -> bool {
    token.split_once('.').is_some_and(|(issued, mac)| (1..=10).contains(&issued.len()) && issued.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
        && mac.len() == 26 && mac.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7')))
}

/// O que o worker devolveu: o código, a senha do diário e o grupo (confirmado, ou a assinatura de um novo).
struct Sent { code: String, upload: Option<String>, group: Option<String>, group_token: Option<String> }

fn stamp() -> String {
    let version = crate::update::running_package().map_or(env!("CARGO_PKG_VERSION"), |p| p.version.as_str());
    format!("hangar-server/{version}")
}

/// Falha do envio: o código do C1 e o que vai nos `params`.
struct Failed { status: StatusCode, code: &'static str, msg: &'static str, params: Value }

impl Failed {
    fn unreachable() -> Self {
        Failed { status: StatusCode::BAD_GATEWAY, code: "diag_report_unreachable", msg: "o servidor de relatórios não respondeu", params: Value::Null }
    }
    fn refused(status: u16) -> Self {
        Failed { status: StatusCode::BAD_GATEWAY, code: "diag_report_refused", msg: "o servidor de relatórios recusou o envio", params: json!({"status": status}) }
    }
}

/// O relatório (C2); devolve o código e a senha do diário.
async fn send_report(base: &str, stamp: &str, group: Option<(&str, &str)>, body: Vec<u8>) -> Result<Sent, Failed> {
    let mut req = CLIENT.post(format!("{base}/api/relatorio")).header("content-type", "application/json").header("x-hangar-stamp", stamp);
    if let Some((group, token)) = group { req = req.header("x-hangar-group", group).header("x-hangar-group-token", token); }
    let resp = req.body(body).send().await.map_err(|_| Failed::unreachable())?;
    let status = resp.status().as_u16();
    if status != 201 {
        let refusal = resp.json::<Value>().await.ok().and_then(|v| v["erro"].as_str().map(str::to_owned));
        return Err(match (status, refusal.as_deref()) {
            (429, Some("grupo-cheio")) => Failed { status: StatusCode::CONFLICT, code: "diag_report_group_full",
                msg: "o envio já tem o máximo de máquinas", params: Value::Null },
            (400, Some("grupo")) => Failed { status: StatusCode::CONFLICT, code: "diag_report_group_unknown",
                msg: "o servidor de relatórios não achou o primeiro relatório deste envio", params: Value::Null },
            (429, _) => Failed { status: StatusCode::TOO_MANY_REQUESTS, code: "diag_report_rate_limited",
                msg: "muitos relatórios em pouco tempo; tente de novo em um minuto", params: Value::Null },
            (other, _) => Failed::refused(other),
        });
    }
    let reply: Value = resp.json().await.map_err(|_| Failed::refused(201))?;
    let code = reply["code"].as_str().filter(|c| (1..=16).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric()))
        .ok_or_else(|| Failed::refused(201))?.to_owned();
    let text = |field: &str, ok: fn(&str) -> bool| reply[field].as_str().filter(|v| ok(v)).map(str::to_owned);
    Ok(Sent { code, upload: reply["upload"].as_str().map(str::to_owned), group: text("group", group_ok), group_token: text("group_token", token_ok) })
}

/// O diário inteiro (C2b). A senha vale uma vez: falha aqui não tem nova tentativa.
async fn send_diary(base: &str, stamp: &str, code: &str, upload: &str, gz: Vec<u8>) -> bool {
    let sent = CLIENT.put(format!("{base}/api/relatorio/diario")).timeout(DIARY_TIMEOUT)
        .header("x-hangar-stamp", stamp).header("x-hangar-code", code).header("x-hangar-upload", upload)
        .header("content-type", "application/gzip").body(gz).send().await;
    match sent {
        Ok(r) if r.status() == StatusCode::CREATED => true,
        Ok(r) => { tracing::warn!(status = r.status().as_u16(), "diag_report: diário recusado pelo worker"); false }
        Err(e) => { tracing::warn!("diag_report: diário não enviado: {e}"); false }
    }
}

pub async fn report_route(
    axum::extract::State(st): axum::extract::State<Arc<crate::routes::AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let headers = req.headers().clone();
    let mut resp = 'r: {
        if crate::routes::on_guest_port(&req) || headers.contains_key("x-hangar-guest") {
            break 'r detail(StatusCode::FORBIDDEN, "diag_report_forbidden", "só o dono envia o relatório", Value::Null);
        }
        let (fwd, owner) = crate::routes::gate(&st, peer, &req);
        if !owner {
            st.auth.record_fail(&fwd.client_ip);
            break 'r detail(StatusCode::UNAUTHORIZED, "unauthorized", "token do dono ausente ou errado", Value::Null);
        }
        let input = match axum::body::to_bytes(req.into_body(), BODY_IN_MAX).await {
            Ok(bytes) => serde_json::from_slice::<ClientInput>(&bytes).ok().filter(valid),
            Err(_) => None,
        };
        let Some(input) = input else {
            break 'r detail(StatusCode::UNPROCESSABLE_ENTITY, "diag_report_bad_request", "corpo do relatório inválido", Value::Null);
        };
        let group = input.group.clone().zip(input.group_token.clone());
        let bundle = build(&st, input).await;
        let stamp = stamp();
        match send_report(&st.diag_report_base, &stamp, group.as_ref().map(|(g, t)| (g.as_str(), t.as_str())), bundle.body).await {
            Ok(sent) => {
                let diary = match sent.upload {
                    Some(upload) if !bundle.diary_gz.is_empty() => send_diary(&st.diag_report_base, &stamp, &sent.code, &upload, bundle.diary_gz).await,
                    _ => false,
                };
                // O cliente confere `group`: worker antigo grava a máquina avulsa e não o devolve.
                json_response(StatusCode::CREATED, json!({"code": sent.code, "diary": if diary { "sent" } else { "failed" },
                    "group": sent.group, "group_token": sent.group_token}))
            }
            Err(f) => {
                st.diag.report("rust.diag_report", "", f.code, "relatório de diagnóstico não enviado");
                detail(f.status, f.code, f.msg, f.params)
            }
        }
    };
    crate::routes::cors(&headers, resp.headers_mut());
    resp
}
