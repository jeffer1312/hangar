use super::model::{ConfigSnapshot, Profile, Transcription, TranscriptionError};
use super::{cloud::{self, AttemptFailure}, quota::{self, Quota, Verdict}};
use bytes::Bytes;
use tokio::sync::{RwLock, Mutex};
use std::time::Duration;

pub struct TranscriptionService {
    snapshot: RwLock<ConfigSnapshot>,
    client: reqwest::Client,
    quota: Mutex<Quota>,
}

impl Default for TranscriptionService {
    fn default() -> Self {
        Self { snapshot: RwLock::new(ConfigSnapshot::default()), quota: Mutex::new(Quota::default()),
            client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
                .user_agent("hangar/1.0").connect_timeout(Duration::from_secs(10))
                .build().expect("Cliente HTTP de transcrição com parâmetros válidos") }
    }
}

impl TranscriptionService {
    pub async fn configure(&self, snapshot: ConfigSnapshot) {
        self.quota.lock().await.load(&snapshot.state_path);
        *self.snapshot.write().await = snapshot;
    }

    pub async fn transcribe(&self, content: Bytes, filename: Option<String>, profile: Profile)
        -> Result<Transcription, TranscriptionError> {
        let snapshot = self.snapshot.read().await.clone();
        self.run(snapshot, content, filename, profile).await
    }

    pub async fn transcribe_provider(&self, id: &str, content: Bytes, filename: Option<String>)
        -> Result<Transcription, TranscriptionError> {
        let mut snapshot = self.snapshot.read().await.clone();
        snapshot.providers.retain(|p| p.id == id);
        snapshot.legacy = None;
        if snapshot.providers.is_empty() {
            return Err(TranscriptionError { status: 404, code: "transcription_provider_missing".into(),
                detail: "Serviço de transcrição não encontrado.".into() });
        }
        self.run(snapshot, content, filename, Profile::Dictation).await
    }

    async fn run(&self, snapshot: ConfigSnapshot, content: Bytes, filename: Option<String>, profile: Profile)
        -> Result<Transcription, TranscriptionError> {
        if content.is_empty() {
            return Err(TranscriptionError { status: 400, code: "transcription_audio_empty".into(),
                detail: "A gravação não contém áudio.".into() });
        }
        let (per_provider, budget) = profile.limits();
        let legacy = snapshot.providers.is_empty();
        let providers = if legacy { snapshot.legacy.into_iter().collect::<Vec<_>>() } else { snapshot.providers };
        if providers.is_empty() {
            return Err(TranscriptionError { status: 503, code: "transcription_not_configured".into(),
                detail: "Nenhum serviço de transcrição está configurado.".into() });
        }
        let free: Vec<_> = {
            let quota = self.quota.lock().await;
            providers.iter().filter(|p| quota.wait(&p.id).is_none()).cloned().collect()
        };
        let queue = if free.is_empty() { providers } else { free };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(if legacy { 120 } else { budget });
        let mut first: Option<TranscriptionError> = None;
        let mut reasons = Vec::new();
        for provider in queue {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining < Duration::from_secs(5) { break; }
            let timeout = remaining.min(Duration::from_secs(if legacy { 120 } else { per_provider }));
            let name = cloud::display_name(&provider);
            let result = if matches!(provider.kind.as_str(), "openai" | "elevenlabs") {
                cloud::transcribe(&self.client, &provider, content.clone(), filename.as_deref(), &snapshot.vocabulary, timeout).await
            } else {
                Err(AttemptFailure::unavailable("transcription_local_unavailable", "O serviço local de transcrição não está disponível."))
            };
            match result {
                Ok(text) => {
                    self.quota.lock().await.update(&provider.id, None, None);
                    let aviso = if reasons.is_empty() { None } else {
                        Some(format!("Transcrito pelo {name}: {}", reasons.join("; ")))
                    };
                    return Ok(Transcription { text, provider: name, aviso });
                }
                Err(failure) => {
                    if first.is_none() {
                        first = Some(TranscriptionError { detail: format!("{name}: {}", failure.error.detail), ..failure.error.clone() });
                    }
                    let (verdict, until) = quota::classify(&provider.kind, &failure);
                    if matches!(verdict, Verdict::Quota) {
                        self.quota.lock().await.update(&provider.id, until,
                            Some(format!("sem cota ({})", failure.status.unwrap_or_default())));
                    }
                    reasons.push(quota::reason(&name, &verdict, &failure));
                }
            }
        }
        let mut error = first.unwrap_or_else(|| TranscriptionError { status: 504,
            code: "transcription_timeout".into(), detail: "Nenhum serviço de transcrição respondeu a tempo.".into() });
        if reasons.len() > 1 { error.detail.push_str(&format!(" (depois: {})", reasons[1..].join("; "))); }
        Err(error)
    }

    pub async fn status(&self) -> Vec<serde_json::Value> {
        let snapshot = self.snapshot.read().await;
        let quota = self.quota.lock().await;
        snapshot.providers.iter().map(|p| {
            let wait = quota.wait(&p.id);
            serde_json::json!({"id":p.id,"kind":p.kind,"name":cloud::display_name(p),
                "waiting_until":wait.as_ref().map(|w| w.until), "reason":wait.and_then(|w| w.reason)})
        }).collect()
    }

    pub async fn shutdown(&self) {
        // O processo local passa a ser encerrado aqui quando configurado.
    }
}
