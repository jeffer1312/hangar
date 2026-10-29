use super::{MAX_AUDIO_BYTES, SpeechConfig, SpeechError, valid_audio};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) const API: &str = "https://api.elevenlabs.io/v1";

pub(super) async fn request(
    config: &SpeechConfig,
    api: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Vec<u8>, SpeechError> {
    let key = config.string("elevenlabs_api_key");
    let key = key.trim();
    if key.is_empty() {
        return Err(SpeechError::new(
            503,
            "Chave da ElevenLabs não configurada — abra Configurações do servidor.",
        ));
    }
    let client = crate::http_client()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .user_agent("hangar/1.0")
        .build()
        .map_err(|_| SpeechError::new(502, "Não foi possível iniciar o cliente da ElevenLabs."))?;
    let url = format!("{}{path}", api.trim_end_matches('/'));
    let mut request = if body.is_some() {
        client.post(url)
    } else {
        client.get(url)
    };
    request = request
        .header("xi-api-key", key)
        .header("content-type", "application/json");
    if let Some(body) = body {
        request = request.json(&body);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| SpeechError::new(502, "Falha ao contatar a ElevenLabs."))?;
    let status = response.status();
    let limit = if status.is_success() {
        MAX_AUDIO_BYTES
    } else {
        16 * 1024
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SpeechError::new(502, "Falha ao ler a resposta da ElevenLabs."))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            if !status.is_success() {
                bytes.extend_from_slice(&chunk[..limit - bytes.len()]);
                break;
            }
            return Err(SpeechError::new(
                502,
                "A ElevenLabs devolveu áudio acima do limite de segurança.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let detail: String = String::from_utf8_lossy(&bytes)
            .replace(key, "[segredo removido]")
            .chars()
            .take(300)
            .collect();
        return Err(SpeechError::new(
            502,
            format!(
                "ElevenLabs {}: {}",
                status.as_u16(),
                if detail.is_empty() {
                    "(sem corpo)"
                } else {
                    &detail
                }
            ),
        ));
    }
    Ok(bytes)
}

pub(super) async fn synthesize(
    config: &SpeechConfig,
    api: &str,
    text: &str,
    voice: &str,
    adjustments: Value,
) -> Result<Vec<u8>, SpeechError> {
    let mut body =
        json!({"text":text,"model_id":"eleven_multilingual_v2","apply_text_normalization":"on"});
    if adjustments
        .as_object()
        .is_some_and(|values| !values.is_empty())
    {
        body["voice_settings"] = adjustments;
    }
    let voice = percent_encoding::utf8_percent_encode(voice, percent_encoding::NON_ALPHANUMERIC);
    let audio = request(
        config,
        api,
        &format!("/text-to-speech/{voice}?output_format=mp3_44100_128"),
        Some(body),
    )
    .await?;
    if audio.is_empty() {
        return Err(SpeechError::new(502, "ElevenLabs devolveu áudio vazio."));
    }
    if !valid_audio(&audio) {
        return Err(SpeechError::new(
            502,
            "ElevenLabs devolveu um formato de áudio não reconhecido (nem WAV nem MP3).",
        ));
    }
    Ok(audio)
}

pub(super) async fn voices(config: &SpeechConfig, api: &str) -> Result<Value, SpeechError> {
    let bytes = request(config, api, "/voices", None).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| SpeechError::new(502, "A ElevenLabs devolveu uma lista de vozes inválida."))?;
    let object = value.as_object().ok_or_else(|| {
        SpeechError::new(502, "A ElevenLabs devolveu uma lista de vozes inválida.")
    })?;
    let Some(voices) = object.get("voices") else {
        return Ok(json!([]));
    };
    let voices = voices.as_array().ok_or_else(|| {
        SpeechError::new(502, "A ElevenLabs devolveu uma lista de vozes inválida.")
    })?;
    let mut result = Vec::new();
    for voice in voices {
        if !voice.is_object() {
            return Err(SpeechError::new(
                502,
                "A ElevenLabs devolveu uma voz inválida.",
            ));
        }
        result.push(json!({"id":voice.get("voice_id").unwrap_or(&Value::Null),"nome":voice.get("name").unwrap_or(&Value::Null)}));
    }
    Ok(Value::Array(result))
}

pub(super) async fn balance(config: &SpeechConfig, api: &str) -> Result<Value, SpeechError> {
    let bytes = request(config, api, "/user/subscription", None).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| SpeechError::new(502, "A ElevenLabs devolveu um saldo inválido."))?;
    if !value.is_object() {
        return Err(SpeechError::new(
            502,
            "A ElevenLabs devolveu um saldo inválido.",
        ));
    }
    Ok(
        json!({"usados":value.get("character_count").unwrap_or(&Value::Null),"limite":value.get("character_limit").unwrap_or(&Value::Null)}),
    )
}
