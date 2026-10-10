use super::model::{ProviderConfig, TranscriptionError};
use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const OPENAI_BASE: &str = "https://api.groq.com/openai/v1";
pub(crate) const OPENAI_MODEL: &str = "whisper-large-v3";
const ELEVENLABS_URL: &str = "https://api.elevenlabs.io/v1/speech-to-text";
const RESPONSE_LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) struct AttemptFailure {
    pub error: TranscriptionError,
    pub status: Option<u16>,
    pub provider_code: String,
    pub retry_after: Option<f64>,
    pub empty: bool,
}

impl AttemptFailure {
    pub fn unavailable(code: &str, detail: &str) -> Self {
        Self { error: TranscriptionError { status: 503, code: code.into(), detail: detail.into() },
            status: None, provider_code: String::new(), retry_after: None, empty: false }
    }
}

pub(crate) fn filename(name: Option<&str>) -> String {
    let extension = name.and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "wav".into());
    format!("audio.{extension}")
}

pub(crate) fn display_name(provider: &ProviderConfig) -> String {
    if !provider.name.trim().is_empty() { return provider.name.trim().into(); }
    if provider.kind == "elevenlabs" { return "ElevenLabs".into(); }
    if provider.kind == "whisper_cpp" { return "whisper.cpp".into(); }
    let base = if provider.base_url.trim().is_empty() { OPENAI_BASE } else { provider.base_url.trim() };
    let host = reqwest::Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| base.into());
    let model = if provider.model.trim().is_empty() { OPENAI_MODEL } else { provider.model.trim() };
    format!("{host} · {model}")
}

fn keyterms(vocabulary: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in vocabulary.split(',').map(str::trim) {
        if !term.is_empty() && term.chars().count() < 50 && term.split_whitespace().count() <= 5
            && !term.chars().any(|c| "<>{}[]\\".contains(c)) && !terms.iter().any(|t| t == term) {
            terms.push(term.to_owned());
        }
    }
    terms
}

pub(crate) async fn transcribe(
    client: &reqwest::Client, provider: &ProviderConfig, content: bytes::Bytes,
    name: Option<&str>, vocabulary: &str, timeout: Duration,
) -> Result<String, AttemptFailure> {
    let url = if provider.kind == "elevenlabs" { ELEVENLABS_URL.into() } else {
        let base = if provider.base_url.trim().is_empty() { OPENAI_BASE } else { provider.base_url.trim() };
        format!("{}/audio/transcriptions", base.trim_end_matches('/'))
    };
    transcribe_to(client, provider, &url, content, name, vocabulary, timeout).await
}

fn retry_after(value: Option<&str>) -> Option<f64> {
    let value = value?.trim();
    if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
        return value.parse::<u64>().ok().map(|seconds| now + seconds as f64);
    }
    chrono::DateTime::parse_from_rfc2822(value).ok().map(|d| d.timestamp() as f64)
}

async fn response_bytes(response: reqwest::Response) -> Result<Vec<u8>, ()> {
    if response.content_length().is_some_and(|n| n > RESPONSE_LIMIT as u64) { return Err(()); }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT { return Err(()); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) async fn transcribe_to(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    url: &str,
    content: bytes::Bytes,
    name: Option<&str>,
    vocabulary: &str,
    timeout: Duration,
) -> Result<String, AttemptFailure> {
    let mut form = Form::new();
    let language = if provider.language.trim().is_empty() { "pt" } else { provider.language.trim() };
    let model = provider.model.trim();
    if provider.kind == "elevenlabs" {
        form = form.text("model_id", if model.is_empty() { "scribe_v2" } else { model }.to_owned())
            .text("language_code", language.to_owned()).text("tag_audio_events", "false");
        for term in keyterms(vocabulary) { form = form.text("keyterms", term); }
    } else {
        form = form.text("model", if model.is_empty() { OPENAI_MODEL } else { model }.to_owned())
            .text("response_format", "json").text("language", language.to_owned());
        // O endpoint roteado ignora esse campo; não fingir suporte ao vocabulário.
        let openrouter = reqwest::Url::parse(url).ok().is_some_and(|u| u.host_str() == Some("openrouter.ai"));
        if !vocabulary.is_empty() && !openrouter { form = form.text("prompt", vocabulary.to_owned()); }
    }
    let length = content.len() as u64;
    form = form.part("file", Part::stream_with_length(reqwest::Body::from(content), length).file_name(filename(name)));
    let mut request = client.post(url).timeout(timeout).multipart(form);
    if !provider.api_key.trim().is_empty() {
        request = if provider.kind == "elevenlabs" { request.header("xi-api-key", provider.api_key.trim()) }
            else { request.bearer_auth(provider.api_key.trim()) };
    }
    let response = request.send().await.map_err(|error| {
        let mut failure = AttemptFailure::unavailable("transcription_network_failed", "Não foi possível contatar o serviço de transcrição.");
        failure.error.status = if error.is_timeout() { 504 } else { 502 };
        failure
    })?;
    let status = response.status();
    let wait = retry_after(response.headers().get("retry-after").and_then(|v| v.to_str().ok()));
    let raw = response_bytes(response).await.map_err(|_| {
        let mut failure = AttemptFailure::unavailable("transcription_response_invalid", "Resposta de transcrição incompleta ou grande demais.");
        failure.error.status = 502;
        failure
    })?;
    let value = serde_json::from_slice::<serde_json::Value>(&raw).ok();
    if !status.is_success() {
        let code = value.as_ref().and_then(|v| v.get("detail"))
            .and_then(|v| v.get("code").or_else(|| v.get("status")))
            .and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
        return Err(AttemptFailure {
            error: TranscriptionError { status: 502, code: "transcription_provider_failed".into(),
                detail: format!("Serviço de transcrição recusou o pedido (HTTP {}).", status.as_u16()) },
            status: Some(status.as_u16()), provider_code: code, retry_after: wait, empty: false,
        });
    }
    let text = value.as_ref().and_then(|v| v.get("text")).and_then(serde_json::Value::as_str)
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    if text.is_empty() {
        return Err(AttemptFailure { error: TranscriptionError { status: 502, code: "transcription_empty".into(),
            detail: "O serviço de transcrição respondeu sem texto.".into() }, status: Some(200),
            provider_code: String::new(), retry_after: None, empty: true });
    }
    Ok(text)
}
