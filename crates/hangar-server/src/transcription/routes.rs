use axum::body::{Body, to_bytes};
use crate::routes::AppState;
use super::model::{ConfigSnapshot, Profile, TranscriptionError};

pub(crate) async fn configuration(state: &AppState) -> Result<ConfigSnapshot, TranscriptionError> {
    let failed = || TranscriptionError { status: 503, code: "transcription_config_unavailable".into(),
        detail: "A configuração de transcrição não está disponível.".into() };
    let request = axum::http::Request::get(format!("http://{}/internal/transcription/config", state.cfg.upstream))
        .header("x-hangar-internal", &state.cfg.internal_secret).body(Body::empty()).map_err(|_| failed())?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let response = state.http.request(request).await.map_err(|_| failed())?;
        if !response.status().is_success() { return Err(failed()); }
        let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024).await.map_err(|_| failed())?;
        serde_json::from_slice(&bytes).map_err(|_| failed())
    }).await.map_err(|_| failed())?
}

/// Transcrição pedida dentro do próprio servidor (ditado, anexo, vídeo), sem passar pelo Python.
pub(crate) async fn transcribe_here(state: &AppState, content: bytes::Bytes, filename: Option<String>, profile: Profile)
    -> Result<super::model::Transcription, TranscriptionError> {
    let result = match state.transcription_slots.clone().try_acquire_owned() {
        Err(_) => Err(TranscriptionError { status: 503, code: "transcription_busy".into(), detail: "As vagas de transcrição estão ocupadas. Tente novamente.".into() }),
        Ok(_slot) => match configuration(state).await {
            Ok(snapshot) => {
                state.transcription.configure(snapshot).await;
                state.transcription.transcribe(content, filename, profile).await
            }
            Err(error) => Err(error),
        },
    };
    if let Err(error) = &result {
        state.diag.report("transcription.failed", "", &error.code, "A transcrição falhou.");
    }
    result
}
