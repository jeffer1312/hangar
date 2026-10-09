use super::cloud::AttemptFailure;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::Path, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Wait {
    pub until: f64,
    pub reason: Option<String>,
}

#[derive(Default)]
pub(crate) struct Quota {
    path: String,
    waits: BTreeMap<String, Wait>,
}

pub(crate) fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

pub(crate) enum Verdict { Transient, Auth, Quota, Other }

pub(crate) fn classify(kind: &str, failure: &AttemptFailure) -> (Verdict, Option<f64>) {
    let status = failure.status;
    if status.is_none() || status.is_some_and(|s| s >= 500) { return (Verdict::Transient, None); }
    if kind == "elevenlabs" && matches!(failure.provider_code.as_str(), "concurrent_limit_exceeded" | "system_busy") {
        return (Verdict::Transient, None);
    }
    if matches!(status, Some(402 | 429)) || kind == "elevenlabs"
        && matches!(failure.provider_code.as_str(), "quota_exceeded" | "insufficient_credits") {
        let fallback = if status == Some(429) { 3600. } else { 86400. };
        return (Verdict::Quota, Some(failure.retry_after.unwrap_or_else(|| now() + fallback)));
    }
    if matches!(status, Some(401 | 403)) { return (Verdict::Auth, None); }
    (Verdict::Other, None)
}

pub(crate) fn reason(name: &str, verdict: &Verdict, failure: &AttemptFailure) -> String {
    let status = failure.status.map(|s| s.to_string()).unwrap_or_default();
    match verdict {
        Verdict::Auth => format!("{name} recusou a chave ({status}); confira a chave desse serviço"),
        Verdict::Quota => format!("{name} sem cota ({status}), em espera"),
        _ if failure.empty => format!("{name} deu resposta sem texto"),
        _ if failure.status.is_none() => format!("{name} não respondeu"),
        _ => format!("{name} falhou ({status})"),
    }
}

impl Quota {
    pub fn load(&mut self, path: &str) {
        if self.path == path { return; }
        self.path = path.into();
        self.waits = if path.is_empty() { BTreeMap::new() } else {
            std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
        };
    }

    pub fn wait(&self, id: &str) -> Option<Wait> {
        self.waits.get(id).filter(|w| w.until.is_finite() && w.until > now()).cloned()
    }

    pub fn update(&mut self, id: &str, until: Option<f64>, reason: Option<String>) {
        self.waits.retain(|_, w| w.until.is_finite() && w.until > now());
        if let Some(until) = until { self.waits.insert(id.into(), Wait { until, reason }); }
        else { self.waits.remove(id); }
        if self.path.is_empty() { return; }
        if self.persist().is_err() {
            tracing::warn!(code = "transcription_quota_persist_failed", "Não foi possível guardar a espera por cota.");
        }
    }

    fn persist(&self) -> std::io::Result<()> {
        let path = Path::new(&self.path);
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut file, &self.waits)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}
