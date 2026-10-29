//! Síntese, preparo e cache de áudio, sem dependência do backend Python.
use super::{SpeechError, config::SpeechConfig};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

mod audio;
mod cache;
mod local;
mod network;
mod text;

pub const CHARACTER_LIMIT: usize = 10_000;
type Lease = tokio::sync::OwnedMutexGuard<()>;

async fn acquire(
    flight: Arc<tokio::sync::Mutex<()>>,
    wait: std::time::Duration,
) -> Result<Lease, SpeechError> {
    tokio::time::timeout(wait, flight.lock_owned())
        .await
        .map_err(|_| {
            SpeechError::new(
                503,
                "Uma síntese deste áudio ainda está em andamento. Tente novamente.",
            )
        })
}

pub struct Service {
    api: String,
    flights: Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            api: network::API.to_owned(),
            flights: Mutex::default(),
        }
    }
}

#[derive(Debug)]
pub struct Synthesis {
    pub hash: String,
    pub cached: bool,
    pub provider: String,
}

pub fn prepare(text: &str) -> String {
    text::prepare(text)
}

/// O chamador valida os 64 caracteres hexadecimais antes de resolver o arquivo público.
pub fn cache_path(config: &SpeechConfig, hash: &str) -> PathBuf {
    cache::path(&config.cache_dir, hash)
}
pub async fn voices(config: &SpeechConfig) -> Result<Value, SpeechError> {
    Service::default().voices(config).await
}
pub async fn balance(config: &SpeechConfig) -> Result<Value, SpeechError> {
    Service::default().balance(config).await
}

fn adjustments(config: &SpeechConfig) -> Value {
    let mut values = serde_json::Map::new();
    for (key, target, default, minimum, maximum) in [
        ("tts_stability", "stability", 50, 0, 100),
        ("tts_similarity_boost", "similarity_boost", 75, 0, 100),
        ("tts_style", "style", 0, 0, 100),
        ("tts_speed", "speed", 100, 70, 120),
    ] {
        let value = config.integer(key, default).clamp(minimum, maximum);
        if value != default {
            values.insert(target.to_owned(), json!(value as f64 / 100.0));
        }
    }
    Value::Object(values)
}

fn hash_value(
    text: &str,
    voice: &str,
    provider: &str,
    adjustments: &Value,
    instruction: &str,
) -> String {
    let mut value = format!(
        "{provider}\0{voice}\0{}\0{text}",
        crate::orchestration::codec::dumps(adjustments, true)
    );
    if !instruction.is_empty() {
        value.push('\0');
        value.push_str(instruction);
    }
    ring::digest::digest(&ring::digest::SHA256, value.as_bytes())
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}

fn valid_audio(audio: &[u8]) -> bool {
    audio.starts_with(b"RIFF")
        || audio.starts_with(b"ID3")
        || audio.len() >= 2 && audio[0] == 0xff && audio[1] & 0xe0 == 0xe0
}

impl Service {
    #[cfg(test)]
    pub(crate) fn at(api: String) -> Self {
        Self {
            api,
            flights: Mutex::default(),
        }
    }

    pub async fn voices(&self, config: &SpeechConfig) -> Result<Value, SpeechError> {
        network::voices(config, &self.api).await
    }

    pub async fn balance(&self, config: &SpeechConfig) -> Result<Value, SpeechError> {
        network::balance(config, &self.api).await
    }

    pub async fn synthesize(
        &self,
        config: &SpeechConfig,
        text: &str,
        voice: &str,
        provider: &str,
        instruction: &str,
    ) -> Result<Synthesis, SpeechError> {
        if !matches!(provider, "elevenlabs" | "local") {
            return Err(SpeechError::new(
                400,
                format!("Provedor desconhecido: {provider}"),
            ));
        }
        if text.chars().count() > CHARACTER_LIMIT {
            return Err(SpeechError::new(
                413,
                format!(
                    "O texto passa do teto de {CHARACTER_LIMIT} caracteres — selecione um trecho menor."
                ),
            ));
        }
        let provider = if provider == "elevenlabs"
            && config.string("elevenlabs_api_key").trim().is_empty()
            && !config.string("tts_local_cmd").trim().is_empty()
        {
            "local"
        } else {
            provider
        };
        let voice = if provider == "local" || !voice.is_empty() {
            voice.to_owned()
        } else {
            let configured = config.string("elevenlabs_voice_id");
            if configured.trim().is_empty() {
                "ORgG8rwdAiMYRug8RJwR".to_owned()
            } else {
                configured.trim().to_owned()
            }
        };
        let adjustments = if provider == "local" {
            json!({})
        } else {
            adjustments(config)
        };
        let hash = hash_value(text, &voice, provider, &adjustments, instruction);
        let identity = config.cache_dir.join(&hash);
        let flight = {
            let mut flights = self
                .flights
                .lock()
                .map_err(|_| SpeechError::new(503, "O controle de síntese está indisponível."))?;
            flights.retain(|_, flight| flight.strong_count() > 0);
            let flight = flights
                .get(&identity)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
            flights.insert(identity, Arc::downgrade(&flight));
            flight
        };
        // Só um produtor paga e publica o mesmo áudio; quem esperou relê o cache já pronto.
        let guard = acquire(flight, std::time::Duration::from_secs(30)).await?;
        let directory = config.cache_dir.clone();
        let key = hash.clone();
        let (cached, guard) = tokio::task::spawn_blocking(move || {
            cache::prepare(&directory)?;
            cache::lookup(&directory, &key).map(|cached| (cached, guard))
        })
        .await
        .map_err(|_| SpeechError::new(500, "Não foi possível conferir o cache de áudio."))??;
        if !cached {
            let audio = if provider == "local" {
                local::run(config, text, guard).await?
            } else {
                network::synthesize(config, &self.api, text, &voice, adjustments, guard).await?
            };
            let directory = config.cache_dir.clone();
            let key = hash.clone();
            tokio::task::spawn_blocking(move || cache::store(&directory, &key, audio))
                .await
                .map_err(|_| {
                    SpeechError::new(
                        500,
                        "Não foi possível concluir a gravação do áudio em cache.",
                    )
                })??;
        }
        Ok(Synthesis {
            hash,
            cached,
            provider: provider.to_owned(),
        })
    }
}

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod tests;
