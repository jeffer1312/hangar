use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub executable_path: String,
    pub model_path: String,
    pub language: String,
    pub converter_path: String,
}

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct ConfigSnapshot {
    pub providers: Vec<ProviderConfig>,
    pub legacy: Option<ProviderConfig>,
    pub vocabulary: String,
    pub state_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transcription {
    pub text: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aviso: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptionError {
    pub status: u16,
    pub code: String,
    pub detail: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Dictation,
    File,
    Video,
}

impl Profile {
    pub fn limits(self) -> (u64, u64) {
        match self {
            Self::Dictation => (60, 150),
            Self::File => (120, 240),
            Self::Video => (60, 120),
        }
    }
}
