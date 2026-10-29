//! Uma decisão de autenticação por transação, inclusive entre processos.
mod ffi;
mod shared;

use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
pub use shared::{AuthConfig, SharedLimiter};

pub const AUTH_ABI: u32 = 1;
pub const AUTH_STORE: u32 = 1;
pub const MAX_FAILS: usize = 8;
pub const WINDOW: f64 = 30.0;
pub const MAX_ORIGINS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AuthError { StoreUnavailable = 1, InvalidInput = 2, Panic = 3, AbiMismatch = 4, EpochMismatch = 5 }
impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::StoreUnavailable => "auth_store_unavailable", Self::InvalidInput => "auth_store_invalid",
            Self::Panic => "auth_operation_failed", Self::AbiMismatch => "auth_abi_mismatch", Self::EpochMismatch => "auth_epoch_mismatch" })
    }
}
impl std::error::Error for AuthError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "u32", into = "u32")]
#[repr(u32)]
pub enum Profile { RequireAuthHttp, BrowserWebSocket, VoiceWebSocket, TerminalWebSocket, LegacyOwnerProbe }
impl TryFrom<u32> for Profile {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> { match v {
        0 => Ok(Self::RequireAuthHttp), 1 => Ok(Self::BrowserWebSocket), 2 => Ok(Self::VoiceWebSocket),
        3 => Ok(Self::TerminalWebSocket), 4 => Ok(Self::LegacyOwnerProbe), _ => Err("invalid profile") } }
}
impl From<Profile> for u32 { fn from(p: Profile) -> Self { p as u32 } }

// Credenciais só existem nestes tipos em memória; eles não implementam Debug.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthAttempt {
    pub profile: Profile,
    pub origin: String,
    pub presented_token: Option<Vec<u8>>,
    pub verified_guest: bool,
    pub login_guest: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AuthDecision {
    pub status: u32,
    pub failure_recorded: u32,
    pub became_blocked: u32,
}
impl AuthDecision {
    pub const OWNER: Self = Self { status: 0, failure_recorded: 0, became_blocked: 0 };
    pub const GUEST: Self = Self { status: 1, ..Self::OWNER };
    pub const UNAUTHORIZED: Self = Self { status: 2, ..Self::OWNER };
    pub const BLOCKED: Self = Self { status: 3, ..Self::OWNER };
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Origins(pub Vec<(String, Vec<f64>)>);
impl Origins {
    fn blocked(&mut self, origin: &str, now: f64) -> bool {
        let Some(i) = self.0.iter().position(|(ip, _)| ip == origin) else { return false };
        self.0[i].1.retain(|t| now - t < WINDOW);
        if self.0[i].1.is_empty() { self.0.remove(i); return false; }
        self.0[i].1.len() >= MAX_FAILS
    }
    fn clear(&mut self, origin: &str) { self.0.retain(|(ip, _)| ip != origin); }
    fn record(&mut self, origin: &str, now: f64) -> AuthDecision {
        if is_loopback(origin) { return AuthDecision::UNAUTHORIZED; }
        let i = match self.0.iter().position(|(ip, _)| ip == origin) {
            Some(i) => i,
            None => { self.0.push((origin.to_owned(), Vec::new())); self.0.len() - 1 }
        };
        let hits = &mut self.0[i].1;
        if hits.len() >= MAX_FAILS { return AuthDecision::UNAUTHORIZED; }
        hits.push(now);
        let became_blocked = u32::from(hits.len() == MAX_FAILS);
        if self.0.len() > MAX_ORIGINS {
            self.0.retain(|(_, hits)| hits.last().is_some_and(|t| now - t < WINDOW));
            while self.0.len() > MAX_ORIGINS {
                let mut oldest = 0;
                for j in 1..self.0.len() {
                    if self.0[j].1.last() < self.0[oldest].1.last() { oldest = j; }
                }
                self.0.remove(oldest);
            }
        }
        AuthDecision { failure_recorded: 1, became_blocked, ..AuthDecision::UNAUTHORIZED }
    }
    pub fn evaluate(&mut self, expected: &[u8], attempt: &AuthAttempt, now: f64) -> AuthDecision {
        // HTTP dispensa convidados antes do limite; WS conserva o bloqueio da origem.
        if attempt.profile == Profile::RequireAuthHttp && (attempt.verified_guest || attempt.login_guest) {
            return AuthDecision::GUEST;
        }
        let local = is_loopback(&attempt.origin);
        if !local && self.blocked(&attempt.origin, now) { return AuthDecision::BLOCKED; }
        if attempt.profile == Profile::VoiceWebSocket && (attempt.verified_guest || attempt.login_guest) { return AuthDecision::UNAUTHORIZED; }
        if attempt.verified_guest { return AuthDecision::GUEST; }
        let presented = attempt.presented_token.as_deref().unwrap_or_default();
        let matches = bool::from(presented.ct_eq(expected)) && match attempt.profile {
            Profile::RequireAuthHttp => true,
            Profile::LegacyOwnerProbe => attempt.presented_token.is_some(),
            _ => !expected.is_empty(),
        };
        if matches {
            if attempt.profile == Profile::LegacyOwnerProbe
                || (attempt.profile == Profile::RequireAuthHttp && attempt.origin != "192.0.2.1") {
                self.clear(&attempt.origin);
            }
            return AuthDecision::OWNER;
        }
        if attempt.profile == Profile::TerminalWebSocket && attempt.login_guest { return AuthDecision::GUEST; }
        if attempt.profile == Profile::LegacyOwnerProbe { return AuthDecision::UNAUTHORIZED; }
        if !local && !(attempt.origin == "192.0.2.1" && presented.is_empty()
            && matches!(attempt.profile, Profile::RequireAuthHttp | Profile::VoiceWebSocket)) {
            return self.record(&attempt.origin, now);
        }
        AuthDecision::UNAUTHORIZED
    }
    pub fn record_failure(&mut self, origin: &str, now: f64) -> AuthDecision { self.record(origin, now) }
    pub fn is_blocked(&mut self, origin: &str, now: f64) -> bool { !is_loopback(origin) && self.blocked(origin, now) }
    pub fn clear_origin(&mut self, origin: &str) { self.clear(origin); }
}
pub fn is_loopback(ip: &str) -> bool { matches!(ip, "127.0.0.1" | "::1" | "localhost") }

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptContext {
    pub connection_kind: String,
    pub profile: Profile,
    pub origin: String,
    pub method: String,
    pub raw_target: String,
    pub credential_source: String,
    pub presented_token: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests;
