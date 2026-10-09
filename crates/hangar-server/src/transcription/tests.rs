use super::{cloud, model::ProviderConfig};
use axum::{Router, body::Body, http::{Request, StatusCode}, response::IntoResponse, routing::post};
use bytes::Bytes;
use std::time::Duration;
use super::{model::{ConfigSnapshot, Profile}, service::TranscriptionService};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

async fn openai_endpoint(req: Request<Body>) -> axum::response::Response {
    if req.headers().contains_key("authorization") {
        return (StatusCode::BAD_REQUEST, "chave vazia enviada").into_response();
    }
    let body = axum::body::to_bytes(req.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    if !text.contains("name=\"response_format\"\r\n\r\njson")
        || !text.contains("name=\"language\"\r\n\r\npt")
        || !text.contains("name=\"model\"\r\n\r\nmodelo-do-usuario")
        || !text.contains("hangar-send")
        || !text.contains("audio.wav")
        || !text.contains("bytes-do-audio") {
        return (StatusCode::BAD_REQUEST, "contrato de transcrição incompatível").into_response();
    }
    ([("content-type", "application/json")], "{\"text\":\"Transcrição em português.\"}").into_response()
}

async fn endpoint(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn openai_upload_is_json_and_key_is_optional() {
    let (url, task) = endpoint(Router::new().route("/audio/transcriptions", post(openai_endpoint))).await;
    let provider = ProviderConfig { id: "local".into(), kind: "openai".into(),
        model: "modelo-do-usuario".into(), ..Default::default() };
    let result = cloud::transcribe_to(&reqwest::Client::new(), &provider,
        &format!("{url}/audio/transcriptions"), Bytes::from_static(b"bytes-do-audio"),
        Some("../../malicioso.wav"), "hangar-send", Duration::from_secs(2)).await;
    task.abort();
    assert_eq!(result.unwrap(), "Transcrição em português.");
}

#[tokio::test]
async fn empty_json_is_a_failure_instead_of_a_message() {
    let (url, task) = endpoint(Router::new().route("/", post(|| async {
        ([("content-type", "application/json")], "{\"text\":\"  \"}")
    }))).await;
    let result = cloud::transcribe_to(&reqwest::Client::new(), &ProviderConfig::default(),
        &url, Bytes::from_static(b"audio"), None, "", Duration::from_secs(2)).await;
    task.abort();
    let error = result.unwrap_err();
    assert!(error.empty);
    assert_eq!(error.error.status, 502);
}

#[tokio::test]
async fn fallback_is_explicit_and_quota_survives_service_restart() {
    let first_calls = Arc::new(AtomicUsize::new(0));
    let calls = first_calls.clone();
    let (first, first_task) = endpoint(Router::new().route("/audio/transcriptions", post(move || {
        let calls = calls.clone();
        async move { calls.fetch_add(1, Ordering::SeqCst);
            (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "3600")], "{}") }
    }))).await;
    let (second, second_task) = endpoint(Router::new().route("/audio/transcriptions", post(|| async {
        "{\"text\":\"Texto pelo serviço configurado.\"}"
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let snapshot = ConfigSnapshot {
        providers: vec![
            ProviderConfig { id: "first".into(), name: "Principal".into(), kind: "openai".into(), base_url: first, ..Default::default() },
            ProviderConfig { id: "second".into(), name: "Reserva".into(), kind: "openai".into(), base_url: second, ..Default::default() },
        ], state_path: temp.path().join("wait.json").to_string_lossy().into_owned(), ..Default::default()
    };
    let service = TranscriptionService::default();
    service.configure(snapshot.clone()).await;
    let result = service.transcribe(Bytes::from_static(b"audio"), None, Profile::Dictation).await.unwrap();
    assert_eq!(result.text, "Texto pelo serviço configurado.");
    assert_eq!(result.provider, "Reserva");
    assert!(result.aviso.unwrap().contains("sem cota"));
    let restarted = TranscriptionService::default();
    restarted.configure(snapshot).await;
    let result = restarted.transcribe(Bytes::from_static(b"audio"), None, Profile::Dictation).await.unwrap();
    first_task.abort(); second_task.abort();
    assert!(result.aviso.is_none());
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failure_does_not_create_an_unconfigured_external_fallback() {
    let (url, task) = endpoint(Router::new().route("/audio/transcriptions", post(|| async { StatusCode::UNAUTHORIZED }))).await;
    let service = TranscriptionService::default();
    service.configure(ConfigSnapshot { providers: vec![ProviderConfig { id: "only".into(),
        name: "Configurado".into(), kind: "openai".into(), base_url: url, ..Default::default() }],
        ..Default::default() }).await;
    let failure = service.transcribe(Bytes::from_static(b"audio"), None, Profile::Dictation).await.unwrap_err();
    task.abort();
    assert_eq!(failure.status, 502);
    assert!(failure.detail.contains("Configurado"));
    assert!(failure.detail.contains("401"));
}

const WAV: &[u8] = b"RIFF\x26\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x80\x3e\x00\x00\x00\x7d\x00\x00\x02\x00\x10\x00data\x02\x00\x00\x00\x00\x00";

async fn installed_whisper() -> (tempfile::TempDir, ProviderConfig) {
    tokio::task::spawn_blocking(|| {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("Whisper instalado");
        std::fs::create_dir(&directory).unwrap();
        let program = directory.join(if cfg!(windows) { "whisper-server.exe" } else { "whisper-server" });
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/whisper_server.rs");
        assert!(std::process::Command::new("rustc").args(["--edition=2024", "-o"])
            .arg(&program).arg(source).status().unwrap().success());
        let model = directory.join("ggml-model.bin");
        std::fs::write(&model, b"fixture").unwrap();
        let provider = ProviderConfig { id: "whisper".into(), kind: "whisper_cpp".into(), language: "pt".into(),
            executable_path: program.to_string_lossy().into_owned(), model_path: model.to_string_lossy().into_owned(),
            ..Default::default() };
        (temp, provider)
    }).await.unwrap()
}

#[tokio::test]
async fn local_process_is_reused_and_stopped_by_its_owner() {
    let (temp, provider) = installed_whisper().await;
    let service = TranscriptionService::default();
    service.configure(ConfigSnapshot { providers: vec![provider.clone()],
        state_path: temp.path().join("wait.json").to_string_lossy().into_owned(), ..Default::default() }).await;
    let (first, second) = tokio::join!(
        service.transcribe(Bytes::from_static(WAV), Some("fala.wav".into()), Profile::Dictation),
        service.transcribe(Bytes::from_static(WAV), Some("fala.wav".into()), Profile::Dictation),
    );
    assert_eq!(first.unwrap().text, "Transcrição local em português.");
    assert_eq!(second.unwrap().text, "Transcrição local em português.");
    let starts = std::fs::read_to_string(std::path::Path::new(&provider.model_path).with_extension("starts")).unwrap();
    assert_eq!(starts.lines().count(), 1);
    service.shutdown().await;
    assert!(!temp.path().join("transcription-local.json").exists());
}

#[tokio::test]
async fn missing_local_model_is_a_visible_failure() {
    let (temp, mut provider) = installed_whisper().await;
    provider.model_path = temp.path().join("modelo-ausente.bin").to_string_lossy().into_owned();
    let service = TranscriptionService::default();
    service.configure(ConfigSnapshot { providers: vec![provider], ..Default::default() }).await;
    let error = service.transcribe(Bytes::from_static(WAV), None, Profile::Dictation).await.unwrap_err();
    assert_eq!(error.code, "whisper_model_missing");
}

#[tokio::test]
async fn missing_converter_does_not_forward_local_audio_to_cloud() {
    let (_temp, mut provider) = installed_whisper().await;
    provider.converter_path = "conversor-que-nao-existe".into();
    let service = TranscriptionService::default();
    service.configure(ConfigSnapshot { providers: vec![provider], ..Default::default() }).await;
    let error = service.transcribe(Bytes::from_static(b"audio-webm"), Some("fala.webm".into()), Profile::Dictation).await.unwrap_err();
    assert_eq!(error.code, "audio_converter_missing");
}

#[tokio::test]
async fn health_port_must_belong_to_the_started_process() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(super::process::owns_port(std::process::id(), port).await.unwrap());
    assert!(!super::process::owns_port(0, port).await.unwrap());
}
