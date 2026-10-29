//! Leitura em voz alta, narração e os consumidores do cliente externo de texto.
pub mod config;

#[derive(Debug, Clone)]
pub struct SpeechError {
    pub status: u16,
    pub detail: String,
}

impl SpeechError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into() }
    }
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.detail)
    }
}

impl std::error::Error for SpeechError {}
