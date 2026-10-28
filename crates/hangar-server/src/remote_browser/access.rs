//! Credenciais HTTP e WebSocket têm políticas diferentes de contagem de falhas.
use std::time::Instant;

use axum::http::{HeaderMap, Method};
use crate::auth::{Auth, is_loopback, presented_token, query_param};

const CONNECT_PEER: &str = "192.0.2.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError { Unauthorized, Blocked }

pub fn http(auth: &Auth, ip: &str, headers: &HeaderMap, query: Option<&str>, https: bool,
            now: Instant) -> Result<(), AccessError> {
    if !is_loopback(ip) && auth.is_blocked_at(ip, now) {
        return Err(AccessError::Blocked);
    }
    let token = presented_token(headers, query, &Method::GET, https);
    if !auth.token_matches(token.as_deref()) {
        // O Connect divide uma origem: visita sem senha não pode bloquear o dono.
        if ip != CONNECT_PEER || token.as_ref().is_some_and(|t| !t.is_empty()) {
            auth.record_fail_at(ip, now);
        }
        return Err(AccessError::Unauthorized);
    }
    // Acertos frequentes do dono no Connect não concedem novos palpites a terceiros.
    if ip != CONNECT_PEER { auth.clear_failures(ip); }
    Ok(())
}

/// `verified_guest` vem apenas do porteiro de convites, depois de conferir sessão e nível.
pub fn websocket(auth: &Auth, ip: &str, query: Option<&str>, verified_guest: bool,
                 now: Instant) -> Result<(), AccessError> {
    if auth.is_blocked_at(ip, now) { return Err(AccessError::Blocked); }
    if verified_guest { return Ok(()); }
    let token = query_param(query, "token").filter(|t| !t.is_empty());
    if !auth.token_matches(token.as_deref().map(str::as_bytes)) {
        auth.record_fail_at(ip, now);
        return Err(AccessError::Unauthorized);
    }
    Ok(())
}
