//! Contrato terminal privado: autentica antes de consumir o corpo.
use std::{net::SocketAddr, sync::Arc, time::Duration};
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}};
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;
use crate::{routes::AppState, terminal_control::CaptureRequest};

pub const MAX_BODY: usize = 16 * 1024 * 1024;

fn warn_terminal(session: Option<&str>, code: &'static str) {
    if !crate::warn_limit::allow(session, code) { return; }
    if code == "invalid terminal request" {
        tracing::warn!(code, "observação terminal recusada");
    } else if let Some(session) = session {
        tracing::warn!(session, code, "observação terminal falhou");
    } else {
        tracing::warn!(code, "observação terminal falhou");
    }
}

fn json(value: Value) -> Response {
    ([("content-type", "application/json")], value.to_string()).into_response()
}

enum Operation {
    Acquire(CaptureRequest),
    Capture(CaptureRequest),
    Release { consumer: String },
}

impl Operation {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let mut fields: crate::json_envelope::Fields = serde_json::from_slice(bytes).ok()?;
        let operation: String = fields.take("op").ok()?;
        match operation.as_str() {
            "acquire" => fields.decode().ok().map(Self::Acquire),
            "capture" => fields.decode().ok().map(Self::Capture),
            "release" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Release { consumer: String }
                fields.decode::<Release>().ok().map(|body| Self::Release { consumer: body.consumer })
            }
            _ => None,
        }
    }
    fn safe_session(&self) -> Option<&str> {
        let name = match self { Self::Acquire(r) | Self::Capture(r) => &r.name, Self::Release { .. } => return None };
        (!name.is_empty() && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))).then_some(name)
    }
}

/// Pedido do Python pela porta privada: loopback, sem repasse de fora e com o segredo interno.
pub(crate) fn trusted_internal(st: &AppState, peer: SocketAddr, headers: &HeaderMap) -> bool {
    let supplied = headers.get("x-hangar-internal").map(|v| v.as_bytes()).unwrap_or_default();
    let external = headers.get_all("x-forwarded-for").iter().any(|header|
        header.to_str().map_or(true, |v| v.split(',').any(|ip|
            ip.trim().parse::<std::net::IpAddr>().map_or(true, |ip| !ip.is_loopback()))));
    peer.ip().is_loopback() && !external && !st.cfg.internal_secret.is_empty()
        && bool::from(supplied.ct_eq(st.cfg.internal_secret.as_bytes()))
}

pub async fn terminal(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !trusted_internal(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let invalid = || {
        warn_terminal(None, "invalid terminal request");
        (StatusCode::BAD_REQUEST, "invalid terminal request").into_response()
    };
    let bytes = match tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY)).await {
        Ok(Ok(bytes)) => bytes,
        _ => return invalid(),
    };
    let Some(op) = Operation::parse(&bytes) else { return invalid(); };
    let session = op.safe_session().map(String::from);
    let result = match op {
        Operation::Acquire(request) => st.terminal.acquire(request).await.map(|_| serde_json::json!({})),
        Operation::Capture(request) => st.terminal.capture(request).await.and_then(|r| serde_json::to_value(r)
            .map_err(|_| crate::terminal_control::TerminalError("invalid terminal result"))),
        Operation::Release { consumer } => {
            if consumer.is_empty() || consumer.len() > 128 { return invalid(); }
            st.terminal.release(&consumer).await.map(|_| serde_json::json!({}))
        }

    };
    match result {
        Ok(value) => json(value),
        Err(e) => {
            warn_terminal(session.as_deref(), e.0);
            if matches!(e.0, "invalid capture request" | "invalid terminal target") {
                (StatusCode::BAD_REQUEST, "invalid terminal request").into_response()
            } else {
                (StatusCode::SERVICE_UNAVAILABLE, "terminal observer unavailable").into_response()
            }
        }
    }
}
