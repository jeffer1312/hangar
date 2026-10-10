use super::{cloud::{self, AttemptFailure}, model::ProviderConfig, process::{self, ManagedChild}};
use bytes::Bytes;

const MAX_AUDIO: u64 = 100 * 1024 * 1024;

fn pcm_wav(content: &[u8]) -> bool {
    content.len() >= 44 && &content[..4] == b"RIFF" && &content[8..16] == b"WAVEfmt "
        && &content[20..24] == b"\x01\x00\x01\x00" && &content[24..28] == b"\x80\x3e\x00\x00"
        && &content[34..36] == b"\x10\x00"
}

pub(crate) async fn normalize(provider: &ProviderConfig, content: Bytes, name: Option<&str>) -> Result<Bytes, AttemptFailure> {
    if pcm_wav(&content) { return Ok(content); }
    let directory = tempfile::Builder::new().prefix("hangar-transcription-").tempdir()
        .map_err(|_| AttemptFailure::unavailable("audio_scratch_failed", "Não foi possível preparar o áudio para conversão."))?;
    let input = directory.path().join(cloud::filename(name));
    let output = directory.path().join("converted.wav");
    tokio::fs::write(&input, content).await
        .map_err(|_| AttemptFailure::unavailable("audio_scratch_failed", "Não foi possível guardar o áudio temporário."))?;
    let program = if provider.converter_path.trim().is_empty() { "ffmpeg" } else { provider.converter_path.trim() };
    let mut command = process::command(&process::expand_path(program));
    command.args(["-nostdin", "-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(&input).args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", "-fs"])
        .arg(MAX_AUDIO.to_string()).arg(&output);
    let mut process = ManagedChild::spawn(command, "audio_conversion_failed",
        "Não foi possível executar o FFmpeg. Instale o conversor ou confira seu caminho no servidor.").await?;
    let status = process.child.wait().await
        .map_err(|_| AttemptFailure::unavailable("audio_conversion_failed", "A conversão do áudio não terminou corretamente."))?;
    process.stop().await;
    if !status.success() {
        return Err(AttemptFailure::unavailable("audio_conversion_failed", "O FFmpeg não conseguiu converter esta gravação."));
    }
    let size = tokio::fs::metadata(&output).await.map(|m| m.len()).unwrap_or(0);
    if size == 0 || size >= MAX_AUDIO {
        return Err(AttemptFailure::unavailable("audio_conversion_failed", "O áudio convertido está vazio ou excede o limite de 100 MiB."));
    }
    let result = tokio::fs::read(&output).await
        .map_err(|_| AttemptFailure::unavailable("audio_conversion_failed", "Não foi possível ler o áudio convertido."))?;
    if !pcm_wav(&result) {
        return Err(AttemptFailure::unavailable("audio_conversion_failed", "O conversor não produziu WAV PCM compatível."));
    }
    Ok(Bytes::from(result))
}
