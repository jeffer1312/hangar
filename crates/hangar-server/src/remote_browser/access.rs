//! Credenciais HTTP e WebSocket têm políticas diferentes de contagem de falhas.
use std::time::Instant;

use axum::http::{HeaderMap, Method};
use crate::auth::{Auth, presented_token, query_param};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError { Unauthorized, Blocked, Unavailable(crate::auth::AuthError) }

fn decision(value: Result<crate::auth::AuthDecision, crate::auth::AuthError>) -> Result<(), AccessError> {
    match value {
        Ok(d) if d.status <= 1 => Ok(()),
        Ok(d) if d.status == 3 => Err(AccessError::Blocked),
        Ok(_) => Err(AccessError::Unauthorized),
        Err(e) => Err(AccessError::Unavailable(e)),
    }
}

pub async fn authorize_http(auth: &Auth, ip: &str, headers: &HeaderMap, query: Option<&str>, https: bool) -> Result<(), AccessError> {
    decision(auth.evaluate(crate::auth::AuthAttempt { profile: crate::auth::Profile::RequireAuthHttp,
        origin: ip.into(), presented_token: presented_token(headers, query, &Method::GET, https),
        verified_guest: false, login_guest: false }).await)
}

pub async fn authorize_websocket(auth: &Auth, ip: &str, query: Option<&str>, verified_guest: bool) -> Result<(), AccessError> {
    decision(auth.evaluate(crate::auth::AuthAttempt { profile: crate::auth::Profile::BrowserWebSocket,
        origin: ip.into(), presented_token: query_param(query, "token").filter(|t| !t.is_empty()).map(String::into_bytes),
        verified_guest, login_guest: false }).await)
}

pub fn http(auth: &Auth, ip: &str, headers: &HeaderMap, query: Option<&str>, https: bool,
            now: Instant) -> Result<(), AccessError> {
    decision(Ok(auth.evaluate_at_local(crate::auth::AuthAttempt { profile: crate::auth::Profile::RequireAuthHttp,
        origin: ip.into(), presented_token: presented_token(headers, query, &Method::GET, https),
        verified_guest: false, login_guest: false }, now)))
}

/// `verified_guest` vem apenas do porteiro de convites, depois de conferir sessão e nível.
pub fn websocket(auth: &Auth, ip: &str, query: Option<&str>, verified_guest: bool,
                 now: Instant) -> Result<(), AccessError> {
    decision(Ok(auth.evaluate_at_local(crate::auth::AuthAttempt { profile: crate::auth::Profile::BrowserWebSocket,
        origin: ip.into(), presented_token: query_param(query, "token").filter(|t| !t.is_empty()).map(String::into_bytes),
        verified_guest, login_guest: false }, now)))
}
