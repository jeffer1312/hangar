use serde_json::{Map, Value, json};
use std::{fs, io, path::Path};

pub const TTL: f64 = 300.0;
pub const RATE_LIMIT_WAIT: f64 = 600.0;

#[derive(Default)]
pub struct QuotaCache {
    entries: Map<String, Value>,
}

impl QuotaCache {
    pub fn load(path: &Path) -> Self {
        let entries = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        Self { entries }
    }

    pub fn needs_refresh(&self, id: &str, now: f64, force: bool) -> bool {
        let Some(entry) = self.entries.get(id) else {
            return true;
        };
        let Some(at) = entry["gravado_em"].as_f64() else {
            return true;
        };
        if !at.is_finite() || at > now + RATE_LIMIT_WAIT {
            return true;
        }
        let retry = entry["retry_at"].as_f64().or_else(|| {
            // O cache antigo representava o 429 com o carimbo 300 s no futuro.
            (!entry
                .as_object()
                .is_some_and(|item| item.contains_key("retry_at"))
                && (at > now
                    || entry["cota"]["motivo"] == "http-429"
                    || entry["cota"]["ts"].as_f64().is_some_and(|ts| ts < at)))
            .then_some(at + TTL)
        });
        if retry
            .is_some_and(|until| until.is_finite() && until > now && until <= now + RATE_LIMIT_WAIT)
        {
            return false;
        }
        force || now - at >= TTL || !entry["cota"].is_object()
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.entries
            .get(id)
            .and_then(|entry| entry.get("cota"))
            .filter(|value| value.is_object())
            .cloned()
    }

    pub fn update(&mut self, id: &str, reading: Value, now: f64) {
        let limited = reading["motivo"] == "http-429";
        let value = if reading["estado"] == "indisponivel" {
            self.get(id)
                .filter(|old| old["estado"] == "lida")
                .unwrap_or(reading)
        } else {
            reading
        };
        self.entries.insert(
            id.into(),
            json!({"gravado_em": now, "cota": value,
            "retry_at": if limited { Some(now + RATE_LIMIT_WAIT) } else { None }}),
        );
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }

    pub fn credential_changed(&self, id: &str, signature: &Value) -> bool {
        self.entries.get(id).is_some_and(|entry| {
            if entry
                .as_object()
                .is_some_and(|e| e.contains_key("credential_signature"))
            {
                &entry["credential_signature"] != signature
            } else {
                signature.is_null() && entry["cota"]["estado"] == "lida"
            }
        })
    }

    pub fn set_credential(&mut self, id: &str, signature: Value) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry["credential_signature"] = signature;
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&self.entries)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Value {
        json!({"estado":"lida","ts":1000,"janelas":[{"pct":42}]})
    }

    #[test]
    fn forced_429_survives_restart_and_preserves_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut cache = QuotaCache::default();
        cache.update("kimi:test", good(), 1000.0);
        cache.update("claude:test", good(), 1000.0);
        cache.update(
            "claude:test",
            json!({"estado":"indisponivel","motivo":"http-429"}),
            1000.0,
        );
        cache.save(&path).unwrap();
        let loaded = QuotaCache::load(&path);
        assert!(!loaded.needs_refresh("claude:test", 1599.0, true));
        assert!(loaded.needs_refresh("claude:test", 1600.0, true));
        assert_eq!(loaded.get("claude:test"), Some(good()));
        assert_eq!(loaded.get("kimi:test"), Some(good()));
    }

    #[test]
    fn legacy_future_stamp_retains_rate_limit_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        fs::write(
            &path,
            json!({"codex:test":{"gravado_em":1300,"cota":good()}}).to_string(),
        )
        .unwrap();
        let cache = QuotaCache::load(&path);
        assert!(!cache.needs_refresh("codex:test", 1100.0, true));
        assert!(!cache.needs_refresh("codex:test", 1599.0, false));
        assert!(!cache.needs_refresh("codex:test", 1599.0, true));
        assert!(cache.needs_refresh("codex:test", 1600.0, false));
    }

    #[test]
    fn ttl_force_logout_and_network_failure_have_distinct_effects() {
        let mut cache = QuotaCache::default();
        cache.update("test", good(), 1000.0);
        assert!(!cache.needs_refresh("test", 1299.0, false));
        assert!(cache.needs_refresh("test", 1300.0, false));
        assert!(cache.needs_refresh("test", 1100.0, true));
        cache.update(
            "test",
            json!({"estado":"indisponivel","motivo":"sem-resposta"}),
            1100.0,
        );
        assert_eq!(cache.get("test"), Some(good()));
        cache.update(
            "test",
            json!({"estado":"sem_credencial","janelas":[]}),
            1200.0,
        );
        assert_eq!(cache.get("test").unwrap()["estado"], "sem_credencial");
    }

    #[test]
    fn implausible_future_stamp_does_not_freeze_refresh() {
        let mut cache = QuotaCache::default();
        cache.update("test", good(), 10000.0);
        assert!(cache.needs_refresh("test", 1000.0, false));
        assert!(cache.needs_refresh("test", 1000.0, true));
    }
}
