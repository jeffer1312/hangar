//! Contrato terminal privado: autentica antes de consumir o corpo.
use std::{collections::HashMap, net::SocketAddr, sync::{Arc, LazyLock, Mutex}, time::{Duration, Instant}};
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::{IntoResponse, Response}};
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;
use crate::{routes::AppState, terminal_control::CaptureRequest};

pub const MAX_BODY: usize = 16 * 1024 * 1024;
const WARN_INTERVAL: Duration = Duration::from_secs(60);
const MAX_WARNINGS: usize = 256;

#[derive(Default)]
struct WarningLimiter {
    recent: HashMap<(Option<String>, &'static str), Instant>,
}
impl WarningLimiter {
    fn allow(&mut self, session: Option<&str>, code: &'static str, now: Instant) -> bool {
        self.recent.retain(|_, last| now.duration_since(*last) < WARN_INTERVAL);
        let key = (session.map(String::from), code);
        if self.recent.contains_key(&key) || self.recent.len() >= MAX_WARNINGS { return false; }
        self.recent.insert(key, now);
        true
    }
}

fn warn_terminal(session: Option<&str>, code: &'static str) {
    static WARNINGS: LazyLock<Mutex<WarningLimiter>> = LazyLock::new(|| Mutex::new(WarningLimiter::default()));
    if !WARNINGS.lock().unwrap().allow(session, code, Instant::now()) { return; }
    if code == "invalid terminal request" {
        tracing::warn!(code, "observação terminal recusada");
    } else if let Some(session) = session {
        tracing::warn!(session, code, "observação terminal usa reserva Python");
    } else {
        tracing::warn!(code, "observação terminal usa reserva Python");
    }
}

fn json(value: Value) -> Response {
    ([("content-type", "application/json")], value.to_string()).into_response()
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Acquire(CaptureRequest),
    Capture(CaptureRequest),
    Release { consumer: String },
}

impl Operation {
    fn safe_session(&self) -> Option<&str> {
        let name = match self { Self::Acquire(r) | Self::Capture(r) => &r.name, Self::Release { .. } => return None };
        (!name.is_empty() && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))).then_some(name)
    }
}

pub async fn terminal(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let supplied = req.headers().get("x-hangar-internal").map(|v| v.as_bytes()).unwrap_or_default();
    let external = req.headers().get_all("x-forwarded-for").iter().any(|header|
        header.to_str().map_or(true, |v| v.split(',').any(|ip|
            ip.trim().parse::<std::net::IpAddr>().map_or(true, |ip| !ip.is_loopback()))));
    if !peer.ip().is_loopback() || external || st.cfg.internal_secret.is_empty()
        || !bool::from(supplied.ct_eq(st.cfg.internal_secret.as_bytes())) {
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
    let Ok(op) = serde_json::from_slice::<Operation>(&bytes) else { return invalid(); };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_expire_and_remain_bounded_by_session_and_code() {
        let mut limiter = WarningLimiter::default();
        let now = Instant::now();
        assert!(limiter.allow(Some("a"), "invalid terminal target", now));
        assert!(!limiter.allow(Some("a"), "invalid terminal target", now));
        assert!(limiter.allow(Some("b"), "invalid terminal target", now));
        assert!(limiter.allow(Some("a"), "invalid capture request", now));
        assert!(limiter.allow(None, "invalid terminal request", now));
        assert!(!limiter.allow(None, "invalid terminal request", now));
        for n in 0..MAX_WARNINGS { limiter.allow(Some(&format!("s-{n}")), "terminal observer EOF", now); }
        assert_eq!(limiter.recent.len(), MAX_WARNINGS);
        assert!(!limiter.allow(Some("overflow"), "terminal observer EOF", now));
        assert!(limiter.allow(Some("a"), "invalid terminal target", now + WARN_INTERVAL));
        assert_eq!(limiter.recent.len(), 1);
    }
}
