use serde_json::{Map, Value, json};
use std::{fs, io, path::Path};

pub const TTL: f64 = 300.0;
pub const RATE_LIMIT_WAIT: f64 = 600.0;
/// Espera de uma fonte que recusou a conexão, pela vez seguida: cada recusa trava a leitura (no
/// Windows, a pilha TCP tenta de novo antes de desistir).
pub const UNREACHABLE_WAITS: [f64; 3] = [600.0, 1800.0, 3600.0];

#[derive(Default)]
pub struct QuotaCache {
    entries: Map<String, Value>,
    /// Só grava o que mudou: cada gravação sincroniza arquivo e pasta, e a consulta comum não muda nada.
    dirty: bool,
}

impl QuotaCache {
    pub fn load(path: &Path) -> Self {
        let entries = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        Self {
            entries,
            dirty: false,
        }
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
        // O botão atualizar fura esta espera; a do 429 não.
        if !force
            && entry["unreachable_until"].as_f64().is_some_and(|until| {
                until.is_finite() && until > now && until <= now + UNREACHABLE_WAITS[2]
            })
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
        let good = reading["estado"] == "lida";
        // A última leitura boa fica, mas com o motivo da falha nova: números velhos sem aviso
        // pareciam certos.
        let value = match reading["estado"].as_str() {
            Some("indisponivel") => match self.get(id).filter(|old| old["estado"] == "lida") {
                Some(mut old) => {
                    old["motivo"] = reading["motivo"].clone();
                    old
                }
                None => reading,
            },
            // Token vencido não zera o uso: a abertura ainda precisa saber que a conta estava a 100%.
            // O estado fica `expirada` (as telas pedem login) e `ts` é o da leitura guardada, nunca agora.
            Some("expirada") => match self.get(id).filter(|old| old["ts"].is_number() && old["janelas"].as_array().is_some_and(|w| !w.is_empty())) {
                Some(old) => {
                    let mut kept = reading;
                    kept["janelas"] = old["janelas"].clone();
                    kept["ts"] = old["ts"].clone();
                    kept
                }
                None => reading,
            },
            _ => reading,
        };
        let mut entry = json!({"gravado_em": now, "cota": value,
            "retry_at": if limited { Some(now + RATE_LIMIT_WAIT) } else { None }});
        if let Some(previous) = self.entries.get(id) {
            if let Some(endpoint) = previous.get("endpoint_sha") {
                entry["endpoint_sha"] = endpoint.clone();
            }
            // Só leitura boa zera a espera de recusas; contagem e prazo andam juntos.
            if !good {
                for field in ["unreachable_count", "unreachable_until"] {
                    if let Some(value) = previous.get(field) {
                        entry[field] = value.clone();
                    }
                }
            }
        }
        self.entries.insert(id.into(), entry);
        self.dirty = true;
    }

    pub fn remove(&mut self, id: &str) {
        self.dirty |= self.entries.remove(id).is_some();
    }

    /// Endereço e credencial da fonte (`Reader::fingerprint`). Trocado qualquer um, a cota guardada
    /// era de outra chave e a espera de outro endereço: a entrada sai inteira. Grava só o hash, que
    /// leva a chave junto: `base_url` editada à mão pode ter senha ou token, e sem a chave o hash de
    /// uma senha curta se quebraria offline.
    pub fn set_endpoint(&mut self, id: &str, fingerprint: &str) {
        let Some(entry) = self.entries.get_mut(id).and_then(Value::as_object_mut) else {
            return;
        };
        let digest = ring::digest::digest(&ring::digest::SHA256, fingerprint.as_bytes());
        let sha: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        // O campo antigo guardava a URL em claro: sai na primeira passada.
        let legacy = entry.remove("endpoint").is_some();
        match entry.get("endpoint_sha").and_then(Value::as_str) {
            Some(known) if known == sha => self.dirty |= legacy,
            Some(_) => {
                self.entries.remove(id);
                self.dirty = true;
            }
            None => {
                entry.remove("unreachable_count");
                entry.remove("unreachable_until");
                entry.insert("endpoint_sha".into(), json!(sha));
                self.dirty = true;
            }
        }
    }

    /// A fonte respondeu (qualquer status HTTP): está no ar, a contagem de recusas zera.
    pub fn clear_unreachable(&mut self, id: &str) {
        let Some(entry) = self.entries.get_mut(id).and_then(Value::as_object_mut) else {
            return;
        };
        let count = entry.remove("unreachable_count").is_some();
        self.dirty |= entry.remove("unreachable_until").is_some() || count;
    }

    /// Recusa registrada; devolve a tentativa em que a escada está. O botão atualizar não sobe
    /// degrau: três cliques seguidos não podem levar a fonte a 1 h de espera.
    pub fn mark_unreachable(&mut self, id: &str, now: f64, forced: bool) -> u64 {
        let Some(entry) = self.entries.get_mut(id).and_then(Value::as_object_mut) else {
            return 0;
        };
        // O arquivo é editável à mão: o valor lido não passa do último degrau.
        let steps = UNREACHABLE_WAITS.len() as u64;
        let previous = entry.get("unreachable_count").and_then(Value::as_u64).unwrap_or(0).min(steps);
        let count = if forced { previous.max(1) } else { (previous + 1).min(steps) };
        let wait = UNREACHABLE_WAITS[(count - 1) as usize];
        entry.insert("unreachable_count".into(), json!(count));
        entry.insert("unreachable_until".into(), json!(now + wait));
        self.dirty = true;
        count
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
        // Ausente e nulo não são iguais: a chave gravada distingue o cache antigo.
        if let Some(entry) = self.entries.get_mut(id)
            && entry.get("credential_signature") != Some(&signature)
        {
            entry["credential_signature"] = signature;
            self.dirty = true;
        }
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn save(&mut self, path: &Path) -> io::Result<()> {
        // No Windows a pasta do cache não é a das travas de conta e pode ainda não existir.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&self.entries)?)?;
        self.dirty = false;
        Ok(())
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
        let mut kept = good();
        kept["motivo"] = json!("http-429");
        assert_eq!(loaded.get("claude:test"), Some(kept));
        assert_eq!(loaded.get("kimi:test"), Some(good()));
    }

    #[test]
    fn first_save_creates_the_cache_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AppData/Local/hangar/cotas-cache.json");
        let mut cache = QuotaCache::default();
        cache.update("kimi:test", good(), 1000.0);
        cache.save(&path).unwrap();
        assert_eq!(QuotaCache::load(&path).get("kimi:test"), Some(good()));
    }

    #[test]
    fn unreadable_cache_is_ignored_and_rewritten_by_the_next_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        for garbage in [&b"{\"kimi:test\":"[..], b"[1,2,3]", b"\xff\xfe", b""] {
            fs::write(&path, garbage).unwrap();
            let mut cache = QuotaCache::load(&path);
            assert!(cache.needs_refresh("kimi:test", 1000.0, false));
            assert!(!cache.dirty(), "ler não pode regravar o arquivo");
            cache.update("kimi:test", good(), 1000.0);
            cache.save(&path).unwrap();
            assert_eq!(QuotaCache::load(&path).get("kimi:test"), Some(good()));
        }
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
    fn failure_after_good_read_keeps_numbers_and_shows_the_new_reason() {
        let mut cache = QuotaCache::default();
        cache.update("test", good(), 1000.0);
        cache.update(
            "test",
            json!({"estado":"indisponivel","janelas":[],"motivo":"painel-sem-numeros"}),
            1100.0,
        );
        let kept = cache.get("test").unwrap();
        assert_eq!((&kept["estado"], &kept["janelas"], &kept["ts"]), (&json!("lida"), &json!([{"pct":42}]), &json!(1000)));
        assert_eq!(kept["motivo"], "painel-sem-numeros");
        cache.update("test", good(), 1200.0);
        assert_eq!(cache.get("test").unwrap()["motivo"], Value::Null);
    }

    #[test]
    fn expired_token_keeps_the_last_usage_with_its_own_age() {
        let mut cache = QuotaCache::default();
        cache.update("claude:x", json!({"estado":"lida","ts":1000,"janelas":[{"pct":100.0}]}), 1000.0);
        cache.update("claude:x", json!({"estado":"expirada","ts":null,"janelas":[],"motivo":"token-expirado"}), 1500.0);
        let kept = cache.get("claude:x").unwrap();
        assert_eq!((&kept["estado"], &kept["motivo"]), (&json!("expirada"), &json!("token-expirado")), "as telas seguem pedindo login");
        assert_eq!((&kept["janelas"], &kept["ts"]), (&json!([{"pct":100.0}]), &json!(1000)), "o uso e a idade são os da leitura boa");
        cache.update("claude:x", json!({"estado":"expirada","ts":null,"janelas":[],"motivo":"login-necessario"}), 1600.0);
        assert_eq!(cache.get("claude:x").unwrap()["janelas"], json!([{"pct":100.0}]), "a segunda falha não apaga");
        let mut fresh = QuotaCache::default();
        fresh.update("claude:y", json!({"estado":"expirada","ts":null,"janelas":[]}), 1000.0);
        assert_eq!(fresh.get("claude:y").unwrap()["janelas"], json!([]), "sem leitura boa não inventa uso");
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
        let mut kept = good();
        kept["motivo"] = json!("sem-resposta");
        assert_eq!(cache.get("test"), Some(kept));
        cache.update(
            "test",
            json!({"estado":"sem_credencial","janelas":[]}),
            1200.0,
        );
        assert_eq!(cache.get("test").unwrap()["estado"], "sem_credencial");
    }

    #[test]
    fn refused_connection_waits_longer_each_time_until_a_good_read_or_a_new_address() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let refused = || json!({"estado":"indisponivel","motivo":"sem-resposta","janelas":[]});
        let mut cache = QuotaCache::default();
        let mut at = 1000.0;
        for wait in [600.0, 1800.0, 3600.0, 3600.0] {
            cache.update("chave:x", refused(), at);
            cache.set_endpoint("chave:x", "http://127.0.0.1:8317/usages");
            cache.mark_unreachable("chave:x", at, false);
            cache.save(&path).unwrap();
            cache = QuotaCache::load(&path);
            assert!(!cache.needs_refresh("chave:x", at + wait - 1.0, false), "espera {wait}");
            assert!(cache.needs_refresh("chave:x", at + 1.0, true), "o botão atualizar fura a espera");
            assert!(cache.needs_refresh("chave:x", at + wait, false));
            at += wait;
        }
        cache.update("chave:x", good(), at);
        cache.update("chave:x", refused(), at + 300.0);
        cache.mark_unreachable("chave:x", at + 300.0, false);
        assert!(!cache.needs_refresh("chave:x", at + 300.0 + 599.0, false), "leitura boa zera a contagem");
        assert!(cache.needs_refresh("chave:x", at + 300.0 + 600.0, false));
        cache.set_endpoint("chave:x", "http://127.0.0.1:9999/usages");
        cache.mark_unreachable("chave:x", at + 900.0, false);
        assert!(cache.needs_refresh("chave:x", at + 900.0 + 600.0, false), "endereço novo recomeça em 10 min");
        cache.set_endpoint("chave:x", "http://outro/usages");
        assert!(cache.needs_refresh("chave:x", at + 1300.0, false), "endereço novo não herda a espera");
        cache.update("chave:x", json!({"estado":"indisponivel","motivo":"http-404"}), at + 1300.0);
        assert!(cache.needs_refresh("chave:x", at + 1300.0 + 300.0, false), "resposta HTTP não espera além do TTL");
    }

    #[test]
    fn endpoint_is_stored_as_a_hash_and_the_old_plain_field_goes_away() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let url = "https://usuario:senha@host/usages?token=abc";
        fs::write(&path, json!({"chave:x":{"gravado_em":1000,"cota":good(),"endpoint":url,
            "unreachable_count":2,"unreachable_until":5000}}).to_string()).unwrap();
        let mut cache = QuotaCache::load(&path);
        cache.set_endpoint("chave:x", url);
        cache.update("chave:x", good(), 1000.0);
        cache.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("senha") && !text.contains("token"), "{text}");
        let mut cache = QuotaCache::load(&path);
        cache.set_endpoint("chave:x", url);
        assert!(!cache.dirty(), "mesmo endereço não regrava");
    }

    #[test]
    fn counter_from_a_hand_edited_cache_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        fs::write(&path, json!({"chave:x":{"gravado_em":1000,"cota":good(),
            "unreachable_count":u64::MAX,"unreachable_until":5000}}).to_string()).unwrap();
        let mut cache = QuotaCache::load(&path);
        assert_eq!(cache.mark_unreachable("chave:x", 1000.0, false), 3);
        assert_eq!(cache.mark_unreachable("chave:x", 1000.0, true), 3);
    }

    #[test]
    fn any_http_answer_resets_and_forced_attempts_do_not_climb() {
        let refused = || json!({"estado":"indisponivel","motivo":"sem-resposta","janelas":[]});
        let mut cache = QuotaCache::default();
        cache.update("chave:x", refused(), 1000.0);
        assert_eq!(cache.mark_unreachable("chave:x", 1000.0, false), 1);
        assert_eq!(cache.mark_unreachable("chave:x", 1000.0, true), 1, "atualizar não sobe degrau");
        assert_eq!(cache.mark_unreachable("chave:x", 1000.0, true), 1);
        assert_eq!(cache.mark_unreachable("chave:x", 1000.0, false), 2);
        cache.update("chave:x", json!({"estado":"indisponivel","motivo":"http-404"}), 1100.0);
        cache.clear_unreachable("chave:x");
        assert!(cache.needs_refresh("chave:x", 1400.0, false), "resposta HTTP tira a espera");
        assert_eq!(cache.mark_unreachable("chave:x", 1400.0, false), 1, "e recomeça a escada");
    }

    #[test]
    fn silent_failure_after_refusal_keeps_the_wait_with_its_step() {
        let refused = || json!({"estado":"indisponivel","motivo":"sem-resposta","janelas":[]});
        let mut cache = QuotaCache::default();
        cache.update("chave:x", refused(), 1000.0);
        cache.mark_unreachable("chave:x", 1000.0, false);
        // Prazo esgotado logo depois (sem contato): nem some a espera, nem a contagem fica sem ela.
        cache.update("chave:x", refused(), 1100.0);
        assert!(!cache.needs_refresh("chave:x", 1500.0, false), "a espera de 10 min segue");
        assert!(cache.needs_refresh("chave:x", 1600.0, false));
    }

    #[test]
    fn new_key_on_the_same_address_drops_the_old_reading() {
        let mut cache = QuotaCache::default();
        cache.update("chave:x", good(), 1000.0);
        cache.set_endpoint("chave:x", "https://host/usages\0chave-1");
        cache.set_endpoint("chave:x", "https://host/usages\0chave-1");
        assert_eq!(cache.get("chave:x"), Some(good()), "mesma chave conserva");
        cache.set_endpoint("chave:x", "https://host/usages\0chave-2");
        assert_eq!(cache.get("chave:x"), None, "chave nova não herda a cota da antiga");
        assert!(cache.needs_refresh("chave:x", 1001.0, false));
    }

    #[test]
    fn implausible_future_stamp_does_not_freeze_refresh() {
        let mut cache = QuotaCache::default();
        cache.update("test", good(), 10000.0);
        assert!(cache.needs_refresh("test", 1000.0, false));
        assert!(cache.needs_refresh("test", 1000.0, true));
    }
}
