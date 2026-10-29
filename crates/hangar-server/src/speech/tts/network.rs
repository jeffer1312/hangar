use super::{
    Lease, SpeechConfig, SpeechError,
    audio::{AudioFile, Transfer},
    valid_audio,
};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) const API: &str = "https://api.elevenlabs.io/v1";

async fn response(
    config: &SpeechConfig,
    api: &str,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::Response, SpeechError> {
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
    let is_audio = body.is_some();
    let mut request = if is_audio {
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
    if status.is_success() {
        return Ok(response);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SpeechError::new(502, "Falha ao ler a resposta da ElevenLabs."))?
    {
        if bytes.len().saturating_add(chunk.len()) > 16 * 1024 {
            bytes.extend_from_slice(&chunk[..16 * 1024 - bytes.len()]);
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let detail: String = String::from_utf8_lossy(&bytes)
        .replace(key, "[segredo removido]")
        .chars()
        .take(300)
        .collect();
    Err(SpeechError::new(
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
    ))
}

async fn metadata(config: &SpeechConfig, api: &str, path: &str) -> Result<Vec<u8>, SpeechError> {
    let mut response = response(config, api, path, None).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SpeechError::new(502, "Falha ao ler a resposta da ElevenLabs."))?
    {
        if bytes.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
            return Err(SpeechError::new(
                502,
                "A ElevenLabs devolveu metadados acima do limite de segurança.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) async fn synthesize(
    config: &SpeechConfig,
    api: &str,
    text: &str,
    voice: &str,
    adjustments: Value,
    lease: Lease,
) -> Result<AudioFile, SpeechError> {
    let operation = async {
        let mut body = json!({"text":text,"model_id":"eleven_multilingual_v2","apply_text_normalization":"on"});
        if adjustments
            .as_object()
            .is_some_and(|values| !values.is_empty())
        {
            body["voice_settings"] = adjustments;
        }
        let voice =
            percent_encoding::utf8_percent_encode(voice, percent_encoding::NON_ALPHANUMERIC);
        let mut response = response(
            config,
            api,
            &format!("/text-to-speech/{voice}?output_format=mp3_44100_128"),
            Some(body),
        )
        .await?;
        // O tamanho do WAV não representa a duração do texto: o disco recebe o fluxo sem teto artificial.
        let mut transfer = Transfer::new(&config.cache_dir, lease)?;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| SpeechError::new(502, "Falha ao ler a resposta da ElevenLabs."))?
        {
            if transfer.write(&chunk).await.is_err() {
                // Recolher o escritor recupera a causa de disco, sem transformá-la em erro de rede.
                let _ = transfer.finish().await?;
                return Err(SpeechError::new(500, "Falha ao gravar o áudio em cache."));
            }
        }
        let audio = transfer.finish().await?;
        if audio.is_empty() {
            return Err(SpeechError::new(502, "ElevenLabs devolveu áudio vazio."));
        }
        if !valid_audio(audio.prefix()) {
            return Err(SpeechError::new(
                502,
                "ElevenLabs devolveu um formato de áudio não reconhecido (nem WAV nem MP3).",
            ));
        }
        Ok(audio)
    };
    // O prazo também cobre a espera pelo disco, sem abandonar o escritor na expiração.
    tokio::time::timeout(Duration::from_secs(120), operation)
        .await
        .unwrap_or_else(|_| {
            Err(SpeechError::new(
                502,
                "A síntese da ElevenLabs passou do prazo de 120 s.",
            ))
        })
}

pub(super) async fn voices(config: &SpeechConfig, api: &str) -> Result<Value, SpeechError> {
    let bytes = metadata(config, api, "/voices").await?;
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
    let bytes = metadata(config, api, "/user/subscription").await?;
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
