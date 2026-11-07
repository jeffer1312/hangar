//! Aviso repetido sai no máximo uma vez por minuto por (sessão, código): falha em laço não enche o log.
use std::{collections::HashMap, sync::{LazyLock, Mutex}, time::{Duration, Instant}};

pub(crate) const WARN_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const MAX_WARNINGS: usize = 256;

#[derive(Default)]
pub(crate) struct WarningLimiter {
    pub(crate) recent: HashMap<(Option<String>, String), Instant>,
}

impl WarningLimiter {
    pub(crate) fn allow(&mut self, session: Option<&str>, code: &str, now: Instant) -> bool {
        self.recent.retain(|_, last| now.duration_since(*last) < WARN_INTERVAL);
        let key = (session.map(String::from), code.to_owned());
        if self.recent.contains_key(&key) { return false; }
        // Cheio: avisa sem guardar. Perder o motivo de uma falha nova é pior que repetir a linha.
        if self.recent.len() < MAX_WARNINGS { self.recent.insert(key, now); }
        true
    }
}

/// true = pode avisar agora.
pub fn allow(session: Option<&str>, code: &str) -> bool {
    static WARNINGS: LazyLock<Mutex<WarningLimiter>> = LazyLock::new(|| Mutex::new(WarningLimiter::default()));
    WARNINGS.lock().unwrap_or_else(|e| e.into_inner()).allow(session, code, Instant::now())
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
        assert!(limiter.allow(Some("overflow"), "terminal observer EOF", now));
        assert_eq!(limiter.recent.len(), MAX_WARNINGS);
        assert!(limiter.allow(Some("a"), "invalid terminal target", now + WARN_INTERVAL));
        assert_eq!(limiter.recent.len(), 1);
    }
}
