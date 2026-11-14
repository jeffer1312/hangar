//! `GET /api/sessions/{name}/pane`: o texto do pane do agente para o espelho do terminal, do dono e do
//! convidado da sessão. Sem sessão no multiplexador é 404; multiplexador calado e captura que falha são
//! erro com código, nunca o texto vazio de um pane parado.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::capture::{MuxProcess, TIMEOUT};
use crate::routes::{AppState, cors, who};
use crate::session_write::{detail_body, json_response, session_name};

/// `lines` do FastAPI (`int`, padrão 200) preso em 50..5000: o espelho pede mais ao rolar, e o teto
/// segura o tamanho de cada leitura do poll.
pub fn lines(query: Option<&str>) -> Result<u32, Value> {
    crate::archive_list::int_param(query, "lines", 200).map(|n| n.clamp(50, 5000) as u32)
}

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (_, who) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let Some(name) = session_name(req.uri().path()).filter(|n| who.as_ref().is_some_and(|c| c.can(n, req.method()))) else {
        return crate::session_write::stranger(&st, peer, req, who.as_ref()).await;
    };
    let (status, body) = match lines(req.uri().query()) {
        Err(error) => (StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [error]})),
        Ok(lines) => read(&st, &name, lines).await,
    };
    let mut response = json_response(status, body);
    cors(req.headers(), response.headers_mut());
    response
}

async fn read(st: &AppState, name: &str, lines: u32) -> (StatusCode, Value) {
    let missing = || (StatusCode::NOT_FOUND, detail_body("erro_sessao_inexistente", "sessao nao encontrada", json!({})));
    let mux = MuxProcess::new(st.list.env().capture_program.clone(), TIMEOUT);
    match mux.has_session(name).await {
        None => return failed(st, name, "capture_mux_no_answer"),
        Some(false) => return missing(),
        Some(true) => {}
    }
    // O pane do agente, não o ativo: uma janela nova do usuário não troca o que o espelho mostra.
    let target = st.state.agent_target(name).await;
    match mux.capture_checked_lines(name, &target, lines).await {
        Ok(text) => (StatusCode::OK, json!({"text": text, "scrollback": scrollback(&mux, name, &target).await})),
        Err("capture_session_missing") => missing(),
        Err(code) => failed(st, name, code),
    }
}

fn failed(st: &AppState, name: &str, code: &str) -> (StatusCode, Value) {
    if crate::warn_limit::allow(Some(name), code) {
        tracing::warn!(session = name, code, "espelho: o pane da sessão não pôde ser lido");
        st.diag.report("rust.pane_failed", name, code, "o pane da sessão não pôde ser lido para o espelho");
    }
    (StatusCode::SERVICE_UNAVAILABLE, detail_body(code, "não consegui ler o terminal da sessão; tente de novo", json!({})))
}

/// Linhas de histórico que pedir mais traria: 0 na tela alternada (o Claude), onde subir é do próprio TUI.
/// Leitura que falha também é 0, como no Python: só some o "mais histórico", e a falha vai ao log.
async fn scrollback(mux: &MuxProcess, name: &str, target: &str) -> u64 {
    match mux.run(&["display", "-p", "-t", target, "#{?alternate_on,0,#{history_size}}"]).await {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0),
        _ => {
            if crate::warn_limit::allow(Some(name), "pane_scrollback_failed") {
                tracing::warn!(session = name, code = "pane_scrollback_failed", "espelho: histórico do pane não lido; vale 0");
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lines;
    use proptest::prelude::*;

    fn query(raw: &str) -> String { format!("lines={}", percent_encoding::utf8_percent_encode(raw, percent_encoding::NON_ALPHANUMERIC)) }

    proptest! {
        /// R3-02: qualquer inteiro, com os enfeites que o `int` do pydantic aceita, vira o mesmo número preso.
        #[test]
        fn an_integer_is_clamped(n in -1_000_000_000i64..1_000_000_000, plus in any::<bool>(), zeros in 0usize..3, pad in 0usize..3) {
            let sign = if n >= 0 && plus { "+" } else { "" };
            let raw = format!("{}{sign}{n}{}{}", " ".repeat(pad), if zeros > 0 { format!(".{}", "0".repeat(zeros - 1)) } else { String::new() }, " ".repeat(pad));
            prop_assert_eq!(lines(Some(&query(&raw))), Ok(n.clamp(50, 5000) as u32));
        }

        /// R3-03: texto que não é inteiro é recusado com o valor que veio, nunca vira o padrão.
        #[test]
        fn a_non_integer_is_refused(raw in "[0-9]*[a-zA-Z.,e][0-9a-zA-Z]*") {
            prop_assume!(!decimal_zeros(&raw));
            let error = lines(Some(&query(&raw))).unwrap_err();
            prop_assert_eq!(&error["type"], "int_parsing");
            prop_assert_eq!(&error["loc"], &serde_json::json!(["query", "lines"]));
            prop_assert_eq!(&error["input"], raw.as_str());
        }
    }

    /// `5.`, `5.00`: inteiros para o pydantic, fora do caso recusado.
    fn decimal_zeros(raw: &str) -> bool { raw.split_once('.').is_some_and(|(a, b)| !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c == b'0')) }

    #[test]
    fn absent_is_200_and_the_last_value_wins() {
        assert_eq!(lines(None), Ok(200));
        assert_eq!(lines(Some("x=1")), Ok(200));
        assert_eq!(lines(Some("lines=10&lines=900")), Ok(900), "o Starlette lê o último");
        assert_eq!(lines(Some("lines=1_000")), Ok(1000));
        assert_eq!(lines(Some("lines=-5")), Ok(50));
        assert_eq!(lines(Some(&format!("lines={}", "9".repeat(40)))), Ok(5000));
        for raw in ["", " ", "1e3", "0x10", "1__0", "_1", "1.5", "abc"] {
            assert!(lines(Some(&query(raw))).is_err(), "{raw:?}");
        }
    }
}
